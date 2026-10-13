//! Per-identity evaluation that stitches audiences, experiments, overrides and flags together.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use zanshin_core::{
    experiments::Assignment,
    flags::{Evaluation, Pinned},
    properties::PropertyMap,
    Identity, PropertySet, Result,
};
use zanshin_store::identities::IdentityRepo;

use super::{audiences::AudienceService, experiments::ExperimentService, flags::FlagService};

#[derive(Clone)]
pub struct Evaluator {
    pub flags: FlagService,
    pub audiences: AudienceService,
    pub experiments: ExperimentService,
    identities: IdentityRepo,
}

/// Everything known about one identity for one evaluation pass.
pub struct EvalContext {
    pub flat: PropertyMap,
    pub audiences: HashSet<String>,
    pub assignments: Vec<Assignment>,
    /// flag key → (experiment key, variant key, value)
    pub flag_pins: HashMap<String, (String, String, Value)>,
    pub overrides: HashMap<String, Value>,
}

impl Evaluator {
    pub fn new(
        flags: FlagService,
        audiences: AudienceService,
        experiments: ExperimentService,
        identities: IdentityRepo,
    ) -> Self {
        Self {
            flags,
            audiences,
            experiments,
            identities,
        }
    }

    pub async fn context(
        &self,
        identity: &Identity,
        props: &PropertySet,
        persist_assignments: bool,
    ) -> Result<EvalContext> {
        let flat = flatten_with_identity(identity, props);
        let audiences = self
            .audiences
            .memberships(&identity.external_id, &flat)
            .await;
        let mut assignments = Vec::new();
        let mut flag_pins = HashMap::new();
        if persist_assignments {
            for e in self
                .experiments
                .enrolments(identity, &flat, &audiences, &[])
                .await?
            {
                if let Some(fk) = &e.experiment.flag_key {
                    flag_pins.insert(
                        fk.clone(),
                        (
                            e.experiment.key.clone(),
                            e.variant.key.clone(),
                            e.variant.value.clone(),
                        ),
                    );
                }
                assignments.push(e.to_assignment());
            }
        }
        let overrides = if persist_assignments {
            self.flags.overrides_for(identity.id).await
        } else {
            HashMap::new()
        };
        Ok(EvalContext {
            flat,
            audiences,
            assignments,
            flag_pins,
            overrides,
        })
    }

    pub async fn flags_with(
        &self,
        identity: &Identity,
        ctx: &EvalContext,
        names: &[String],
    ) -> Vec<Evaluation> {
        let member = |k: &str| ctx.audiences.contains(k);
        let snapshot = self.flags.catalog.snapshot().await;
        let mut keys: Vec<&String> = if names.is_empty() {
            snapshot.keys().collect()
        } else {
            names.iter().filter(|n| snapshot.contains_key(*n)).collect()
        };
        keys.sort();
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            let flag = &snapshot[k];
            let pinned = if let Some(v) = ctx.overrides.get(k) {
                Some(Pinned::Override(v))
            } else {
                ctx.flag_pins.get(k).map(|(e, v, val)| Pinned::Experiment {
                    experiment: e,
                    variant: v,
                    value: val,
                })
            };
            out.push(flag.evaluate(&identity.external_id, &ctx.flat, &member, pinned));
        }
        metrics::counter!("zanshin_flag_evaluations_total").increment(out.len() as u64);
        out
    }

    pub async fn flags(&self, identity: &Identity, names: &[String]) -> Result<Vec<Evaluation>> {
        let ctx = self.context(identity, &identity.properties, true).await?;
        Ok(self.flags_with(identity, &ctx, names).await)
    }

    pub async fn flag(&self, identity: &Identity, key: &str) -> Result<Evaluation> {
        self.flags.get(key).await?;
        let mut v = self
            .flags(identity, std::slice::from_ref(&key.to_string()))
            .await?;
        v.pop()
            .ok_or_else(|| zanshin_core::Error::not_found(format!("flag {key:?} not found")))
    }

    pub async fn audience_keys(&self, identity: &Identity) -> Vec<String> {
        let flat = flatten_with_identity(identity, &identity.properties);
        let mut v: Vec<_> = self
            .audiences
            .memberships(&identity.external_id, &flat)
            .await
            .into_iter()
            .collect();
        v.sort();
        v
    }

    pub async fn assignments(
        &self,
        identity: &Identity,
        names: &[String],
    ) -> Result<Vec<Assignment>> {
        let flat = flatten_with_identity(identity, &identity.properties);
        let audiences = self
            .audiences
            .memberships(&identity.external_id, &flat)
            .await;
        Ok(self
            .experiments
            .enrolments(identity, &flat, &audiences, names)
            .await?
            .iter()
            .map(|e| e.to_assignment())
            .collect())
    }

    pub async fn identity_by_external_id(&self, external_id: &str) -> Result<Identity> {
        self.identities.get_by_external_id(external_id).await
    }

    /// Admin dry run with ad-hoc properties: no assignments are written, overrides are ignored.
    pub async fn dry_run_flag(
        &self,
        subject: &str,
        props: &PropertySet,
        key: &str,
    ) -> Result<Evaluation> {
        self.flags.get(key).await?;
        let identity = synthetic_identity(subject, props);
        let ctx = self.context(&identity, props, false).await?;
        let mut v = self
            .flags_with(&identity, &ctx, std::slice::from_ref(&key.to_string()))
            .await;
        v.pop()
            .ok_or_else(|| zanshin_core::Error::not_found(format!("flag {key:?} not found")))
    }

    pub async fn dry_run_audience(
        &self,
        subject: &str,
        props: &PropertySet,
        key: &str,
    ) -> Result<bool> {
        let a = self.audiences.get(key).await?;
        let identity = synthetic_identity(subject, props);
        Ok(a.contains(subject, &flatten_with_identity(&identity, props)))
    }
}

pub fn synthetic_identity(subject: &str, props: &PropertySet) -> Identity {
    let now = chrono::Utc::now();
    Identity {
        id: uuid::Uuid::nil(),
        external_id: subject.to_string(),
        properties: props.clone(),
        created_at: now,
        updated_at: now,
        last_seen_at: now,
    }
}

/// Flattened properties plus `identity.id` / `identity.external_id` so rules can target them.
pub fn flatten_with_identity(identity: &Identity, props: &PropertySet) -> PropertyMap {
    let mut flat = props.flatten();
    flat.insert("identity.id".into(), Value::String(identity.id.to_string()));
    flat.insert(
        "identity.external_id".into(),
        Value::String(identity.external_id.clone()),
    );
    flat
}
