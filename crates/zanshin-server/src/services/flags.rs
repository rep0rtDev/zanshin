//! Feature flag definitions: catalog + admin writes. Evaluation lives in `eval`.

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use zanshin_core::{
    flags::{validate_key, Flag, Rule, ValueType},
    Error, Result,
};
use zanshin_store::{
    flags::{FlagInput, FlagRepo},
    identities::IdentityRepo,
};

use super::{
    bus::{ChangeBus, ChangeKind},
    catalog::{Catalog, CatalogItem, CatalogSource},
};

impl CatalogItem for Flag {
    fn key(&self) -> &str {
        &self.key
    }
    fn version(&self) -> i64 {
        self.version
    }
}

#[async_trait]
impl CatalogSource<Flag> for FlagRepo {
    async fn list(&self) -> Result<Vec<Flag>> {
        FlagRepo::list(self).await
    }
    async fn get(&self, key: &str) -> Result<Flag> {
        FlagRepo::get(self, key).await
    }
}

#[derive(Clone)]
pub struct FlagService {
    repo: FlagRepo,
    identities: IdentityRepo,
    bus: ChangeBus,
    pub catalog: Arc<Catalog<Flag>>,
}

impl FlagService {
    pub fn new(repo: FlagRepo, identities: IdentityRepo, bus: ChangeBus) -> Self {
        let catalog = Catalog::new(ChangeKind::Flag, "flag", Arc::new(repo.clone()));
        Self {
            repo,
            identities,
            bus,
            catalog,
        }
    }

    pub async fn list(&self) -> Vec<Arc<Flag>> {
        self.catalog.list().await
    }

    pub async fn get(&self, key: &str) -> Result<Arc<Flag>> {
        self.catalog.get(key).await
    }

    pub async fn overrides_for(&self, identity_id: Uuid) -> HashMap<String, Value> {
        self.repo
            .overrides_for(identity_id)
            .await
            .unwrap_or_default()
    }

    pub async fn create(&self, key: &str, input: FlagInput) -> Result<Flag> {
        validate_key(key)?;
        let now = chrono::Utc::now();
        let flag = Flag {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: input.name.unwrap_or_default(),
            description: input.description.unwrap_or_default(),
            value_type: input
                .value_type
                .ok_or_else(|| Error::invalid("value_type is required"))?,
            default_value: input
                .default_value
                .ok_or_else(|| Error::invalid("default_value is required"))?,
            enabled: input.enabled.unwrap_or(true),
            rules: input.rules.unwrap_or_default(),
            salt: input.salt.unwrap_or_default(),
            tags: normalize_tags(input.tags.unwrap_or_default())?,
            prerequisites: input.prerequisites.unwrap_or_default(),
            version: 1,
            created_at: now,
            updated_at: now,
        };
        flag.validate()?;
        self.check_prerequisites(&flag).await?;
        let created = self.repo.create(&flag).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::Flag, &created.key, created.version),
            )
            .await;
        metrics::counter!("zanshin_flag_writes_total", "op" => "create").increment(1);
        Ok(created)
    }

    pub async fn update(&self, key: &str, input: FlagInput) -> Result<Flag> {
        let mut flag = self.repo.get(key).await?;
        if let Some(v) = input.name {
            flag.name = v;
        }
        if let Some(v) = input.description {
            flag.description = v;
        }
        if let Some(v) = input.value_type {
            flag.value_type = v;
        }
        if let Some(v) = input.default_value {
            flag.default_value = v;
        }
        if let Some(v) = input.enabled {
            flag.enabled = v;
        }
        if let Some(v) = input.rules {
            flag.rules = v;
        }
        if let Some(v) = input.salt {
            flag.salt = v;
        }
        if let Some(v) = input.tags {
            flag.tags = normalize_tags(v)?;
        }
        if let Some(v) = input.prerequisites {
            flag.prerequisites = v;
        }
        flag.validate()?;
        self.check_prerequisites(&flag).await?;
        let updated = self.repo.update(&flag).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::Flag, &updated.key, updated.version),
            )
            .await;
        metrics::counter!("zanshin_flag_writes_total", "op" => "update").increment(1);
        Ok(updated)
    }

    /// Prerequisites must exist and must not form a cycle through this flag.
    async fn check_prerequisites(&self, flag: &Flag) -> Result<()> {
        if flag.prerequisites.is_empty() {
            return Ok(());
        }
        let all = self.repo.list().await?;
        let by_key: HashMap<&str, &Flag> = all.iter().map(|f| (f.key.as_str(), f)).collect();
        for p in &flag.prerequisites {
            if !by_key.contains_key(p.flag.as_str()) {
                return Err(Error::invalid(format!(
                    "prerequisite flag {:?} does not exist",
                    p.flag
                )));
            }
        }
        // DFS from each prerequisite following stored prerequisites; reaching `flag` is a cycle.
        let mut stack: Vec<&str> = flag.prerequisites.iter().map(|p| p.flag.as_str()).collect();
        let mut seen = std::collections::HashSet::new();
        while let Some(k) = stack.pop() {
            if k == flag.key {
                return Err(Error::invalid(format!(
                    "prerequisite cycle: {} depends on itself through its prerequisites",
                    flag.key
                )));
            }
            if !seen.insert(k) {
                continue;
            }
            if let Some(f) = by_key.get(k) {
                stack.extend(f.prerequisites.iter().map(|p| p.flag.as_str()));
            }
        }
        Ok(())
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.repo.delete(key).await?;
        let mut c = self.bus.change(ChangeKind::Flag, key, 0);
        c.deleted = true;
        self.bus.publish(c).await;
        metrics::counter!("zanshin_flag_writes_total", "op" => "delete").increment(1);
        Ok(())
    }

    pub async fn set_override(&self, key: &str, identity_id: Uuid, value: Value) -> Result<()> {
        let flag = self.get(key).await?;
        flag.value_type.check(&value)?;
        self.identities.get(identity_id).await?;
        self.repo.set_override(flag.id, identity_id, &value).await?;
        let mut c = self.bus.change(ChangeKind::Flag, key, flag.version);
        c.identity_id = Some(identity_id);
        self.bus.publish(c).await;
        Ok(())
    }

    pub async fn delete_override(&self, key: &str, identity_id: Uuid) -> Result<()> {
        let flag = self.get(key).await?;
        self.repo.delete_override(flag.id, identity_id).await?;
        let mut c = self.bus.change(ChangeKind::Flag, key, flag.version);
        c.identity_id = Some(identity_id);
        self.bus.publish(c).await;
        Ok(())
    }

    pub async fn list_overrides(
        &self,
        key: &str,
        limit: i64,
    ) -> Result<Vec<(Uuid, String, Value)>> {
        let flag = self.get(key).await?;
        self.repo.list_overrides(flag.id, limit).await
    }
}

pub fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>> {
    let mut out: Vec<String> = tags
        .into_iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    out.sort();
    out.dedup();
    if out.len() > 32 || out.iter().any(|t| t.len() > 64) {
        return Err(Error::invalid("at most 32 tags of up to 64 chars"));
    }
    Ok(out)
}

/// Admin-facing partial input.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FlagPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub value_type: Option<ValueType>,
    #[serde(default)]
    pub default_value: Option<Value>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub rules: Option<Vec<Rule>>,
    #[serde(default)]
    pub salt: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub prerequisites: Option<Vec<zanshin_core::flags::Prerequisite>>,
}

impl From<FlagPatch> for FlagInput {
    fn from(p: FlagPatch) -> Self {
        FlagInput {
            name: p.name,
            description: p.description,
            value_type: p.value_type,
            default_value: p.default_value,
            enabled: p.enabled,
            rules: p.rules,
            salt: p.salt,
            tags: p.tags,
            prerequisites: p.prerequisites,
        }
    }
}
