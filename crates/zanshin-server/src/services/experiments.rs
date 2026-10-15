//! Experiments: catalog, sticky assignment, exposure tracking, admin lifecycle.

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use uuid::Uuid;
use zanshin_core::{
    experiments::{
        default_split_key, Assignment, BanditConfig, Experiment, ExperimentVariant, Guardrail,
        Holdout, Status,
    },
    flags::validate_key,
    properties::PropertyMap,
    rules::Condition,
    Error, Identity, Result,
};
use zanshin_store::{
    experiments::{AssignmentRow, ExperimentRepo, VariantStats},
    holdouts::HoldoutRepo,
};

use super::{
    bus::{ChangeBus, ChangeKind},
    catalog::{Catalog, CatalogItem, CatalogSource},
    flags::normalize_tags,
};

impl CatalogItem for Experiment {
    fn key(&self) -> &str {
        &self.key
    }
    fn version(&self) -> i64 {
        self.version
    }
}

#[async_trait]
impl CatalogSource<Experiment> for ExperimentRepo {
    async fn list(&self) -> Result<Vec<Experiment>> {
        ExperimentRepo::list(self).await
    }
    async fn get(&self, key: &str) -> Result<Experiment> {
        ExperimentRepo::get(self, key).await
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HoldoutPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub percentage: Option<f64>,
    pub salt: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExperimentPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub hypothesis: Option<String>,
    #[serde(default)]
    pub status: Option<Status>,
    #[serde(default)]
    pub condition: Option<Condition>,
    #[serde(default)]
    pub audiences: Option<Vec<String>>,
    #[serde(default)]
    pub percentage: Option<f64>,
    #[serde(default)]
    pub variants: Option<Vec<ExperimentVariant>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub control: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub flag_key: Option<Option<String>>,
    #[serde(default)]
    pub salt: Option<String>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub starts_at: Option<Option<chrono::DateTime<chrono::Utc>>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub ends_at: Option<Option<chrono::DateTime<chrono::Utc>>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default)]
    pub split_key: Option<String>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub max_participants: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub enrol_until: Option<Option<chrono::DateTime<chrono::Utc>>>,
    #[serde(default)]
    pub locked: Option<bool>,
    #[serde(default)]
    pub metrics: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub layer: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub holdout: Option<Option<String>>,
    pub guardrails: Option<Vec<Guardrail>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub bandit: Option<Option<BanditConfig>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub phase_of: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub winner: Option<Option<String>>,
}

/// `missing` → None (keep), `null` → Some(None) (clear), value → Some(Some(v)).
fn deserialize_double_option<'de, T, D>(d: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// An identity's live enrolment.
pub struct Enrolment {
    pub experiment: Arc<Experiment>,
    pub variant: ExperimentVariant,
}

impl Enrolment {
    pub fn to_assignment(&self) -> Assignment {
        Assignment {
            key: self.experiment.key.clone(),
            name: self.experiment.name.clone(),
            variant: self.variant.key.clone(),
            value: self.variant.value.clone(),
            version: self.experiment.version,
        }
    }
}

#[derive(Clone)]
pub struct ExperimentService {
    repo: ExperimentRepo,
    bus: ChangeBus,
    events: super::events::EventService,
    pub catalog: Arc<Catalog<Experiment>>,
    pub holdouts: HoldoutRepo,
}

impl ExperimentService {
    pub fn new(
        repo: ExperimentRepo,
        holdouts: HoldoutRepo,
        bus: ChangeBus,
        events: super::events::EventService,
    ) -> Self {
        let catalog = Catalog::new(ChangeKind::Experiment, "experiment", Arc::new(repo.clone()));
        Self {
            repo,
            bus,
            events,
            catalog,
            holdouts,
        }
    }

    /// Start (0..100) of `exp`'s range inside its layer: the summed percentages of the other
    /// non-stopped experiments in that layer created before it.
    pub fn layer_offset(exp: &Experiment, all: &[Arc<Experiment>]) -> f64 {
        let Some(layer) = exp.layer.as_deref() else {
            return 0.0;
        };
        all.iter()
            .filter(|o| {
                o.layer.as_deref() == Some(layer)
                    && o.key != exp.key
                    && o.status != Status::Stopped
                    && (o.created_at, &o.key) < (exp.created_at, &exp.key)
            })
            .map(|o| o.percentage)
            .sum()
    }

    /// Reject a layer whose non-stopped experiments would exceed 100 % together.
    async fn check_layer(&self, e: &Experiment) -> Result<()> {
        let Some(layer) = e.layer.as_deref() else {
            return Ok(());
        };
        if e.status == Status::Stopped {
            return Ok(());
        }
        // Read storage, not the catalog: the catalog refreshes asynchronously and would miss an
        // experiment created a moment ago.
        let others: f64 = self
            .repo
            .list()
            .await?
            .iter()
            .filter(|o| {
                o.layer.as_deref() == Some(layer) && o.key != e.key && o.status != Status::Stopped
            })
            .map(|o| o.percentage)
            .sum();
        if others + e.percentage > 100.0 + 1e-9 {
            return Err(Error::invalid(format!(
                "layer {layer:?} is over-allocated: other experiments already take {others}%, this one asks for {}%",
                e.percentage
            )));
        }
        if let Some(h) = &e.holdout {
            self.holdouts.get(h).await?;
        }
        Ok(())
    }

    /// Is this player kept out of `exp` by its holdout group?
    async fn held_out(&self, exp: &Experiment, subject: &str) -> bool {
        let Some(h) = exp.holdout.as_deref() else {
            return false;
        };
        match self.holdouts.get(h).await {
            Ok(h) => h.holds(subject),
            Err(_) => false,
        }
    }

    // ---- holdouts -----------------------------------------------------------------------------

    pub async fn list_holdouts(&self) -> Result<Vec<Holdout>> {
        self.holdouts.list().await
    }

    pub async fn get_holdout(&self, key: &str) -> Result<Holdout> {
        self.holdouts.get(key).await
    }

    pub async fn create_holdout(&self, key: &str, p: HoldoutPatch) -> Result<Holdout> {
        validate_key(key)?;
        let h = Holdout {
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            description: p.description.unwrap_or_default(),
            percentage: p.percentage.unwrap_or(5.0),
            salt: p
                .salt
                .unwrap_or_else(|| Uuid::now_v7().simple().to_string()),
            enabled: p.enabled.unwrap_or(true),
            version: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        h.validate()?;
        self.holdouts.create(&h).await
    }

    pub async fn update_holdout(&self, key: &str, p: HoldoutPatch) -> Result<Holdout> {
        let mut h = self.holdouts.get(key).await?;
        if let Some(v) = p.name {
            h.name = v;
        }
        if let Some(v) = p.description {
            h.description = v;
        }
        if let Some(v) = p.percentage {
            h.percentage = v;
        }
        if let Some(v) = p.salt {
            h.salt = v;
        }
        if let Some(v) = p.enabled {
            h.enabled = v;
        }
        h.validate()?;
        self.holdouts.update(&h).await
    }

    pub async fn delete_holdout(&self, key: &str) -> Result<()> {
        let used = self.holdouts.used_by(key).await?;
        if !used.is_empty() {
            return Err(Error::conflict(format!(
                "holdout {key:?} is referenced by experiments: {}",
                used.join(", ")
            )));
        }
        self.holdouts.delete(key).await
    }

    /// Experiments whose holdout group keeps this player out (for the identity page).
    pub async fn holdouts_for(&self, subject: &str) -> Result<Vec<String>> {
        Ok(self
            .holdouts
            .list()
            .await?
            .into_iter()
            .filter(|h| h.holds(subject))
            .map(|h| h.key)
            .collect())
    }

    pub async fn list(&self) -> Vec<Arc<Experiment>> {
        self.catalog.list().await
    }

    pub async fn get(&self, key: &str) -> Result<Arc<Experiment>> {
        self.catalog.get(key).await
    }

    /// Live enrolments for an identity, creating sticky assignments where the identity is newly
    /// eligible. `names` empty = every experiment.
    pub async fn enrolments(
        &self,
        identity: &Identity,
        flat: &PropertyMap,
        audiences: &HashSet<String>,
        names: &[String],
    ) -> Result<Vec<Enrolment>> {
        let now = chrono::Utc::now();
        let all = self.catalog.list().await;
        let candidates: Vec<_> = all
            .into_iter()
            .filter(|e| e.is_live(now) && (names.is_empty() || names.contains(&e.key)))
            .collect();
        if candidates.is_empty() {
            return Ok(vec![]);
        }
        let existing = self.repo.assignments_for(identity.id).await?;
        let all_for_layers = if candidates.iter().any(|e| e.layer.is_some()) {
            self.catalog.list().await
        } else {
            Vec::new()
        };
        let mut out = Vec::new();
        for exp in candidates {
            let variant_key = match existing.get(&exp.id) {
                Some(v) => Some(v.clone()),
                None if exp.is_enrolling(now)
                    && self.has_capacity(&exp).await
                    && !self.held_out(&exp, &identity.external_id).await =>
                {
                    let offset = Self::layer_offset(&exp, &all_for_layers);
                    match exp.allocate_in_layer(
                        &identity.external_id,
                        flat,
                        &|k: &str| audiences.contains(k),
                        offset,
                    ) {
                        Some(v) => {
                            let stored = self.repo.assign(exp.id, identity.id, &v.key).await?;
                            let mut meta = serde_json::Map::new();
                            meta.insert(
                                "experiment".into(),
                                serde_json::Value::String(exp.key.clone()),
                            );
                            self.events
                                .system(
                                    identity,
                                    zanshin_core::events::EV_EXPERIMENT_JOIN,
                                    stored.clone(),
                                    Some(meta),
                                    None,
                                )
                                .await;
                            metrics::counter!("zanshin_experiment_assignments_total", "experiment" => exp.key.clone()).increment(1);
                            Some(stored)
                        }
                        None => None,
                    }
                }
                None => None,
            };
            if let Some(vk) = variant_key {
                if let Some(variant) = exp.variant(&vk) {
                    out.push(Enrolment {
                        experiment: exp.clone(),
                        variant: variant.clone(),
                    });
                }
            }
        }
        Ok(out)
    }

    async fn has_capacity(&self, exp: &Experiment) -> bool {
        match exp.max_participants {
            None => true,
            Some(max) => self
                .repo
                .participant_count(exp.id)
                .await
                .map(|n| n < max)
                .unwrap_or(false),
        }
    }

    pub async fn participant_count(&self, key: &str) -> Result<i64> {
        let exp = self.get(key).await?;
        self.repo.participant_count(exp.id).await
    }

    pub async fn participants(
        &self,
        key: &str,
        variant: Option<&str>,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<zanshin_store::experiments::Participant>> {
        let exp = self.get(key).await?;
        self.repo
            .participants(exp.id, variant, after, limit.clamp(1, 1000))
            .await
    }

    pub async fn assignment_series(&self, key: &str) -> Result<Vec<(String, String, i64)>> {
        let exp = self.get(key).await?;
        self.repo.assignment_series(exp.id).await
    }

    /// Duplicate an experiment as a fresh draft (new salt, no assignments).
    /// All phases of an experiment family (the root plus every experiment chained to it through
    /// `phase_of`), oldest first.
    pub async fn phases(&self, key: &str) -> Result<Vec<Arc<Experiment>>> {
        let all = self.list().await;
        let find = |k: &str| all.iter().find(|e| e.key == k).cloned();
        let mut root = self.get(key).await?;
        let mut hops = 0;
        while let Some(p) = root.phase_of.as_deref().and_then(find) {
            root = p;
            hops += 1;
            if hops > 100 {
                break;
            }
        }
        let root_of = |mut e: Arc<Experiment>| -> String {
            let mut hops = 0;
            while let Some(p) = e.phase_of.as_deref().and_then(find) {
                e = p;
                hops += 1;
                if hops > 100 {
                    break;
                }
            }
            e.key.clone()
        };
        let mut family: Vec<Arc<Experiment>> = all
            .iter()
            .filter(|e| root_of((*e).clone()) == root.key)
            .cloned()
            .collect();
        family.sort_by_key(|e| e.created_at);
        Ok(family)
    }

    pub async fn copy(&self, key: &str, new_key: &str, as_phase: bool) -> Result<Experiment> {
        let src = self.get(key).await?;
        validate_key(new_key)?;
        let now = chrono::Utc::now();
        let mut e = (*src).clone();
        e.id = Uuid::now_v7();
        e.key = new_key.to_string();
        e.status = Status::Draft;
        e.salt = format!("{}-{}", src.salt, &new_key[..new_key.len().min(8)]);
        e.locked = false;
        e.copied_from = Some(src.key.clone());
        e.phase_of = if as_phase {
            Some(src.key.clone())
        } else {
            None
        };
        e.interrupted_at = None;
        e.interrupt_reason = String::new();
        e.winner = None;
        e.version = 1;
        e.created_at = now;
        e.updated_at = now;
        e.validate()?;
        self.check_layer(&e).await?;
        let created = self.repo.create(&e).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::Experiment, &created.key, created.version),
            )
            .await;
        Ok(created)
    }

    /// Stop early, record why and (optionally) which variant won. When a flag is linked, the
    /// winning value is written into the flag's default via the returned `(experiment, winner)`.
    pub async fn interrupt(
        &self,
        key: &str,
        reason: &str,
        winner: Option<&str>,
    ) -> Result<(Experiment, Option<ExperimentVariant>)> {
        let current = self.get(key).await?;
        if current.status == Status::Draft {
            return Err(Error::invalid("a draft experiment cannot be interrupted"));
        }
        let won = match winner {
            Some(w) => Some(
                current
                    .variant(w)
                    .cloned()
                    .ok_or_else(|| Error::invalid(format!("winner {w:?} is not a variant")))?,
            ),
            None => None,
        };
        let mut e = (*current).clone();
        e.status = Status::Stopped;
        e.interrupted_at = Some(chrono::Utc::now());
        e.interrupt_reason = reason.to_string();
        e.winner = won.as_ref().map(|v| v.key.clone());
        e.validate()?;
        let updated = self.repo.update(&e).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::Experiment, &updated.key, updated.version),
            )
            .await;
        Ok((updated, won))
    }

    pub async fn expose(&self, identity: &Identity, key: &str) -> Result<()> {
        let exp = self.get(key).await?;
        if !exp.is_live(chrono::Utc::now()) {
            return Err(Error::invalid(format!("experiment {key:?} is not live")));
        }
        if !self.repo.expose(exp.id, identity.id).await? {
            return Err(Error::not_found(
                "identity is not enrolled in this experiment",
            ));
        }
        metrics::counter!("zanshin_experiment_exposures_total", "experiment" => exp.key.clone())
            .increment(1);
        if let Ok(Some(a)) = self.repo.get_assignment(exp.id, identity.id).await {
            let mut meta = serde_json::Map::new();
            meta.insert(
                "experiment".into(),
                serde_json::Value::String(exp.key.clone()),
            );
            self.events
                .system(
                    identity,
                    zanshin_core::events::EV_EXPOSURE,
                    a.variant,
                    Some(meta),
                    None,
                )
                .await;
        }
        Ok(())
    }

    // ---- admin -------------------------------------------------------------------------------

    pub async fn create(&self, key: &str, p: ExperimentPatch) -> Result<Experiment> {
        validate_key(key)?;
        let now = chrono::Utc::now();
        let e = Experiment {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            description: p.description.unwrap_or_default(),
            hypothesis: p.hypothesis.unwrap_or_default(),
            status: p.status.unwrap_or(Status::Draft),
            condition: p.condition.unwrap_or_else(Condition::always),
            audiences: p.audiences.unwrap_or_default(),
            percentage: p.percentage.unwrap_or(100.0),
            variants: p
                .variants
                .ok_or_else(|| Error::invalid("variants are required"))?,
            control: p.control.flatten(),
            flag_key: p.flag_key.flatten(),
            salt: p.salt.unwrap_or_default(),
            starts_at: p.starts_at.flatten(),
            ends_at: p.ends_at.flatten(),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            split_key: p.split_key.unwrap_or_else(default_split_key),
            priority: p.priority.unwrap_or_default(),
            max_participants: p.max_participants.flatten(),
            enrol_until: p.enrol_until.flatten(),
            locked: p.locked.unwrap_or(false),
            metrics: p.metrics.unwrap_or_default(),
            copied_from: None,
            phase_of: p.phase_of.flatten(),
            interrupted_at: None,
            interrupt_reason: String::new(),
            winner: p.winner.flatten(),
            layer: p.layer.flatten(),
            holdout: p.holdout.flatten(),
            guardrails: p.guardrails.unwrap_or_default(),
            bandit: p.bandit.flatten(),
            version: 1,
            created_at: now,
            updated_at: now,
        };
        e.validate()?;
        self.check_layer(&e).await?;
        let created = self.repo.create(&e).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::Experiment, &created.key, created.version),
            )
            .await;
        Ok(created)
    }

    pub async fn update(&self, key: &str, p: ExperimentPatch) -> Result<Experiment> {
        let mut e = self.repo.get(key).await?;
        if e.locked && p.locked != Some(false) {
            let only_status = p.name.is_none()
                && p.description.is_none()
                && p.hypothesis.is_none()
                && p.condition.is_none()
                && p.audiences.is_none()
                && p.percentage.is_none()
                && p.variants.is_none()
                && p.control.is_none()
                && p.flag_key.is_none()
                && p.salt.is_none()
                && p.starts_at.is_none()
                && p.ends_at.is_none()
                && p.tags.is_none()
                && p.split_key.is_none()
                && p.max_participants.is_none()
                && p.enrol_until.is_none()
                && p.metrics.is_none()
                && p.layer.is_none()
                && p.holdout.is_none()
                && p.guardrails.is_none()
                && p.bandit.is_none()
                && p.phase_of.is_none();
            if !only_status {
                return Err(Error::invalid(format!(
                    "experiment {key:?} is locked; unlock it first"
                )));
            }
        }
        if let Some(v) = p.locked {
            e.locked = v;
        }
        if let Some(v) = p.split_key {
            e.split_key = v;
        }
        if let Some(v) = p.priority {
            e.priority = v;
        }
        if let Some(v) = p.max_participants {
            e.max_participants = v;
        }
        if let Some(v) = p.enrol_until {
            e.enrol_until = v;
        }
        if let Some(v) = p.metrics {
            e.metrics = v;
        }
        if let Some(v) = p.layer {
            e.layer = v;
        }
        if let Some(v) = p.holdout {
            e.holdout = v;
        }
        if let Some(v) = p.guardrails {
            e.guardrails = v;
        }
        if let Some(v) = p.bandit {
            e.bandit = v;
        }
        if let Some(v) = p.phase_of {
            e.phase_of = v;
        }
        if let Some(v) = p.winner {
            e.winner = v;
        }
        if let Some(v) = p.name {
            e.name = v;
        }
        if let Some(v) = p.description {
            e.description = v;
        }
        if let Some(v) = p.hypothesis {
            e.hypothesis = v;
        }
        if let Some(v) = p.status {
            e.status = v;
        }
        if let Some(v) = p.condition {
            e.condition = v;
        }
        if let Some(v) = p.audiences {
            e.audiences = v;
        }
        if let Some(v) = p.percentage {
            e.percentage = v;
        }
        if let Some(v) = p.variants {
            e.variants = v;
        }
        if let Some(v) = p.control {
            e.control = v;
        }
        if let Some(v) = p.flag_key {
            e.flag_key = v;
        }
        if let Some(v) = p.salt {
            e.salt = v;
        }
        if let Some(v) = p.starts_at {
            e.starts_at = v;
        }
        if let Some(v) = p.ends_at {
            e.ends_at = v;
        }
        if let Some(v) = p.tags {
            e.tags = normalize_tags(v)?;
        }
        e.validate()?;
        self.check_layer(&e).await?;
        let updated = self.repo.update(&e).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::Experiment, &updated.key, updated.version),
            )
            .await;
        Ok(updated)
    }

    pub async fn set_status(&self, key: &str, status: Status) -> Result<Experiment> {
        let current = self.repo.get(key).await?;
        let allowed = matches!(
            (current.status, status),
            (Status::Draft, Status::Running)
                | (Status::Running, Status::Paused)
                | (Status::Paused, Status::Running)
                | (Status::Running, Status::Stopped)
                | (Status::Paused, Status::Stopped)
        );
        if !allowed && current.status != status {
            return Err(Error::invalid(format!(
                "cannot move experiment from {} to {}",
                current.status.as_str(),
                status.as_str()
            )));
        }
        self.update(
            key,
            ExperimentPatch {
                status: Some(status),
                ..Default::default()
            },
        )
        .await
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.repo.delete(key).await?;
        let mut c = self.bus.change(ChangeKind::Experiment, key, 0);
        c.deleted = true;
        self.bus.publish(c).await;
        Ok(())
    }

    pub async fn results(&self, key: &str) -> Result<(Arc<Experiment>, Vec<VariantStats>)> {
        let exp = self.get(key).await?;
        let stats = self.repo.results(exp.id).await?;
        Ok((exp, stats))
    }

    pub async fn assignment(&self, key: &str, identity_id: Uuid) -> Result<Option<AssignmentRow>> {
        let exp = self.get(key).await?;
        self.repo.get_assignment(exp.id, identity_id).await
    }

    pub async fn force_assign(&self, key: &str, identity_id: Uuid, variant: &str) -> Result<()> {
        let exp = self.get(key).await?;
        if exp.variant(variant).is_none() {
            return Err(Error::invalid(format!(
                "variant {variant:?} is not part of experiment {key:?}"
            )));
        }
        self.repo.force_assign(exp.id, identity_id, variant).await?;
        let mut c = self.bus.change(ChangeKind::Experiment, key, exp.version);
        c.identity_id = Some(identity_id);
        self.bus.publish(c).await;
        Ok(())
    }

    pub async fn delete_assignment(&self, key: &str, identity_id: Uuid) -> Result<()> {
        let exp = self.get(key).await?;
        self.repo.delete_assignment(exp.id, identity_id).await?;
        let mut c = self.bus.change(ChangeKind::Experiment, key, exp.version);
        c.identity_id = Some(identity_id);
        self.bus.publish(c).await;
        Ok(())
    }

    pub async fn reset(&self, key: &str) -> Result<u64> {
        let exp = self.get(key).await?;
        let n = self.repo.reset(exp.id).await?;
        self.bus
            .publish(self.bus.change(ChangeKind::Experiment, key, exp.version))
            .await;
        Ok(n)
    }
}
