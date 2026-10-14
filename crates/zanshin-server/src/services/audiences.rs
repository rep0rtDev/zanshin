//! Audience definitions: catalog + admin writes + membership evaluation.

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use uuid::Uuid;
use zanshin_core::{
    audiences::{Audience, AudienceKind, Recompute},
    flags::validate_key,
    properties::PropertyMap,
    rules::Condition,
    Error, Identity, Result,
};
use zanshin_store::{
    audiences::{AudienceRepo, IdentityLists, ListEntry, Member, UsageRef},
    identities::IdentityRepo,
};

use super::{
    bus::{ChangeBus, ChangeKind},
    catalog::{Catalog, CatalogItem, CatalogSource},
    eval::flatten_with_identity,
    flags::normalize_tags,
};

impl CatalogItem for Audience {
    fn key(&self) -> &str {
        &self.key
    }
    fn version(&self) -> i64 {
        self.version
    }
}

#[async_trait]
impl CatalogSource<Audience> for AudienceRepo {
    async fn list(&self) -> Result<Vec<Audience>> {
        AudienceRepo::list(self).await
    }
    async fn get(&self, key: &str) -> Result<Audience> {
        AudienceRepo::get(self, key).await
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AudiencePatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub condition: Option<Condition>,
    #[serde(default)]
    pub percentage: Option<f64>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub salt: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub kind: Option<AudienceKind>,
    #[serde(default)]
    pub recompute: Option<Recompute>,
    #[serde(default)]
    pub schedule_cron: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    /// Alternative to `condition`: an expression in the targeting language.
    #[serde(default)]
    pub expression: Option<String>,
}

impl AudiencePatch {
    fn resolve_condition(&mut self) -> Result<()> {
        if let Some(expr) = self.expression.take() {
            self.condition = Some(zanshin_core::expr::parse(&expr)?);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct AudienceService {
    pub repo: AudienceRepo,
    identities: IdentityRepo,
    bus: ChangeBus,
    pub catalog: Arc<Catalog<Audience>>,
}

impl AudienceService {
    pub fn new(repo: AudienceRepo, identities: IdentityRepo, bus: ChangeBus) -> Self {
        let catalog = Catalog::new(ChangeKind::Audience, "audience", Arc::new(repo.clone()));
        Self {
            repo,
            identities,
            bus,
            catalog,
        }
    }

    pub async fn list(&self) -> Vec<Arc<Audience>> {
        self.catalog.list().await
    }

    pub async fn get(&self, key: &str) -> Result<Arc<Audience>> {
        self.catalog.get(key).await
    }

    /// Keys of every audience the identity belongs to. Resolution order per audience:
    /// exclude list → include list → stored membership (`managed` / `scheduled`) → live rule.
    /// Live rule results are mirrored into `audience_members` in the background so member lists
    /// and join timestamps exist for every audience.
    pub async fn memberships(&self, identity: &Identity, flat: &PropertyMap) -> HashSet<String> {
        let all = self.catalog.list().await;
        let lists = self
            .repo
            .lists_for_identity(identity.id)
            .await
            .unwrap_or_default();
        let mut keys = HashSet::new();
        let mut join = Vec::new();
        let mut leave = Vec::new();
        for a in all.iter() {
            let member = Self::resolve(a, identity, flat, &lists);
            if member {
                keys.insert(a.key.clone());
            }
            if a.kind == AudienceKind::Dynamic && a.recompute == Recompute::Live && a.enabled {
                let stored = lists.stored.contains(&a.id);
                let rule = a.contains(&identity.external_id, flat);
                if rule && !stored {
                    join.push(a.id);
                } else if !rule && stored && !lists.included.contains(&a.id) {
                    leave.push(a.id);
                }
            }
        }
        if !join.is_empty() || !leave.is_empty() {
            let repo = self.repo.clone();
            let id = identity.id;
            tokio::spawn(async move {
                for aid in join {
                    repo.add_members(aid, &[id], "rule").await.ok();
                }
                for aid in leave {
                    repo.remove_members(aid, &[id]).await.ok();
                }
            });
        }
        keys
    }

    fn resolve(
        a: &Audience,
        identity: &Identity,
        flat: &PropertyMap,
        lists: &IdentityLists,
    ) -> bool {
        if !a.enabled || lists.excluded.contains(&a.id) {
            return false;
        }
        if lists.included.contains(&a.id) {
            return true;
        }
        match (a.kind, a.recompute) {
            (AudienceKind::Managed, _) | (AudienceKind::Dynamic, Recompute::Scheduled) => {
                lists.stored.contains(&a.id)
            }
            (AudienceKind::Dynamic, Recompute::Live) => a.contains(&identity.external_id, flat),
        }
    }

    /// Re-evaluate the rule for every identity and store the result. Returns the member count.
    pub async fn recompute(&self, key: &str) -> Result<i64> {
        let a = self.repo.get(key).await?;
        if a.kind == AudienceKind::Managed {
            return self.repo.mark_computed(a.id).await;
        }
        let mut after = None;
        loop {
            let page = self.identities.list(after, 1000).await?;
            if page.is_empty() {
                break;
            }
            after = page.last().map(|i| i.id);
            let evaluated: Vec<Uuid> = page.iter().map(|i| i.id).collect();
            let matching: Vec<Uuid> = page
                .iter()
                .filter(|i| {
                    let flat = flatten_with_identity(i, &i.properties);
                    a.contains(&i.external_id, &flat)
                })
                .map(|i| i.id)
                .collect();
            self.repo.sync_members(a.id, &matching, &evaluated).await?;
        }
        let n = self.repo.mark_computed(a.id).await?;
        self.bus
            .publish(self.bus.change(ChangeKind::Audience, &a.key, a.version))
            .await;
        Ok(n)
    }

    /// Scheduled audiences whose cron fired since their last computation.
    pub async fn due_for_recompute(&self) -> Vec<String> {
        let now = chrono::Utc::now();
        self.catalog
            .list()
            .await
            .iter()
            .filter(|a| {
                a.enabled && a.kind == AudienceKind::Dynamic && a.recompute == Recompute::Scheduled
            })
            .filter(|a| {
                let Some(last) = a.last_computed_at else {
                    return true;
                };
                match a.schedule_cron.parse::<cron::Schedule>() {
                    Ok(s) => s.after(&last).next().is_some_and(|t| t <= now),
                    Err(_) => now - last > chrono::Duration::hours(24),
                }
            })
            .map(|a| a.key.clone())
            .collect()
    }

    pub async fn members(&self, key: &str, after: Option<Uuid>, limit: i64) -> Result<Vec<Member>> {
        let a = self.repo.get(key).await?;
        self.repo.members(a.id, after, limit.clamp(1, 1000)).await
    }

    /// Managed membership by external id (server / admin). Unknown ids are skipped and returned.
    pub async fn set_managed(
        &self,
        key: &str,
        external_ids: &[String],
        add: bool,
    ) -> Result<(u64, Vec<String>)> {
        let a = self.repo.get(key).await?;
        if a.kind != AudienceKind::Managed {
            return Err(Error::invalid(format!(
                "audience {key:?} is not managed; use include/exclude lists for dynamic audiences"
            )));
        }
        let mut ids = Vec::new();
        let mut missing = Vec::new();
        for ext in external_ids {
            match self.identities.get_by_external_id(ext).await {
                Ok(i) => ids.push(i.id),
                Err(_) => missing.push(ext.clone()),
            }
        }
        let n = if add {
            self.repo.add_members(a.id, &ids, "managed").await?
        } else {
            self.repo.remove_members(a.id, &ids).await?
        };
        self.repo.mark_computed(a.id).await?;
        Ok((n, missing))
    }

    pub async fn lists(&self, key: &str) -> Result<Vec<ListEntry>> {
        let a = self.repo.get(key).await?;
        self.repo.lists(a.id).await
    }

    pub async fn set_list(
        &self,
        key: &str,
        external_id: &str,
        mode: &str,
        until: Option<chrono::DateTime<chrono::Utc>>,
        reason: &str,
    ) -> Result<()> {
        if mode != "include" && mode != "exclude" {
            return Err(Error::invalid("mode must be include or exclude"));
        }
        let a = self.repo.get(key).await?;
        let i = self.identities.get_by_external_id(external_id).await?;
        self.repo.set_list(a.id, i.id, mode, until, reason).await
    }

    pub async fn unset_list(&self, key: &str, external_id: &str) -> Result<()> {
        let a = self.repo.get(key).await?;
        let i = self.identities.get_by_external_id(external_id).await?;
        self.repo.unset_list(a.id, i.id).await
    }

    pub async fn usage(&self, key: &str) -> Result<Vec<UsageRef>> {
        self.repo.get(key).await?;
        self.repo.usage(key).await
    }

    /// Duplicate an audience (rule, sampling, tags) under a new key; membership is not copied.
    pub async fn copy(&self, key: &str, new_key: &str, name: Option<String>) -> Result<Audience> {
        let src = self.repo.get(key).await?;
        let patch = AudiencePatch {
            name: Some(name.unwrap_or_else(|| format!("{} (copy)", src.name))),
            description: Some(src.description.clone()),
            condition: Some(src.condition.clone()),
            percentage: Some(src.percentage),
            enabled: Some(false),
            salt: Some(src.salt.clone()),
            tags: Some(src.tags.clone()),
            kind: Some(src.kind),
            recompute: Some(src.recompute),
            schedule_cron: Some(src.schedule_cron.clone()),
            source: Some(src.source.clone()),
            expression: None,
        };
        let mut a = self.build(new_key, patch)?;
        a.copied_from = Some(src.key.clone());
        self.insert(a).await
    }

    fn build(&self, key: &str, mut p: AudiencePatch) -> Result<Audience> {
        validate_key(key)?;
        p.resolve_condition()?;
        let now = chrono::Utc::now();
        let kind = p.kind.unwrap_or_default();
        let a = Audience {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            description: p.description.unwrap_or_default(),
            condition: match p.condition {
                Some(c) => c,
                None if kind == AudienceKind::Managed => Condition::always(),
                None => return Err(Error::invalid("condition or expression is required")),
            },
            percentage: p.percentage.unwrap_or(100.0),
            enabled: p.enabled.unwrap_or(true),
            salt: p.salt.unwrap_or_default(),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            kind,
            recompute: p.recompute.unwrap_or_default(),
            schedule_cron: p.schedule_cron.unwrap_or_default(),
            source: p.source.unwrap_or_default(),
            copied_from: None,
            last_computed_at: None,
            member_count: 0,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        a.validate()?;
        Ok(a)
    }

    async fn insert(&self, a: Audience) -> Result<Audience> {
        let created = self.repo.create(&a).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::Audience, &created.key, created.version),
            )
            .await;
        Ok(created)
    }

    pub async fn create(&self, key: &str, p: AudiencePatch) -> Result<Audience> {
        let a = self.build(key, p)?;
        self.insert(a).await
    }

    pub async fn update(&self, key: &str, mut p: AudiencePatch) -> Result<Audience> {
        p.resolve_condition()?;
        let mut a = self.repo.get(key).await?;
        if let Some(v) = p.name {
            a.name = v;
        }
        if let Some(v) = p.description {
            a.description = v;
        }
        if let Some(v) = p.condition {
            a.condition = v;
        }
        if let Some(v) = p.percentage {
            a.percentage = v;
        }
        if let Some(v) = p.enabled {
            a.enabled = v;
        }
        if let Some(v) = p.salt {
            a.salt = v;
        }
        if let Some(v) = p.tags {
            a.tags = normalize_tags(v)?;
        }
        if let Some(v) = p.kind {
            a.kind = v;
        }
        if let Some(v) = p.recompute {
            a.recompute = v;
        }
        if let Some(v) = p.schedule_cron {
            a.schedule_cron = v;
        }
        if let Some(v) = p.source {
            a.source = v;
        }
        a.validate()?;
        let updated = self.repo.update(&a).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::Audience, &updated.key, updated.version),
            )
            .await;
        Ok(updated)
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.repo.delete(key).await?;
        let mut c = self.bus.change(ChangeKind::Audience, key, 0);
        c.deleted = true;
        self.bus.publish(c).await;
        Ok(())
    }
}
