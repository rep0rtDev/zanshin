//! Audience definitions: catalog + admin writes + membership evaluation.

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use uuid::Uuid;
use zanshin_core::{
    audiences::Audience, flags::validate_key, properties::PropertyMap, rules::Condition, Error,
    Result,
};
use zanshin_store::audiences::AudienceRepo;

use super::{
    bus::{ChangeBus, ChangeKind},
    catalog::{Catalog, CatalogItem, CatalogSource},
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
}

#[derive(Clone)]
pub struct AudienceService {
    repo: AudienceRepo,
    bus: ChangeBus,
    pub catalog: Arc<Catalog<Audience>>,
}

impl AudienceService {
    pub fn new(repo: AudienceRepo, bus: ChangeBus) -> Self {
        let catalog = Catalog::new(ChangeKind::Audience, "audience", Arc::new(repo.clone()));
        Self { repo, bus, catalog }
    }

    pub async fn list(&self) -> Vec<Arc<Audience>> {
        self.catalog.list().await
    }

    pub async fn get(&self, key: &str) -> Result<Arc<Audience>> {
        self.catalog.get(key).await
    }

    /// Keys of every audience the subject belongs to.
    pub async fn memberships(&self, subject: &str, flat: &PropertyMap) -> HashSet<String> {
        let all = self.catalog.list().await;
        all.iter()
            .filter(|a| a.contains(subject, flat))
            .map(|a| a.key.clone())
            .collect()
    }

    pub async fn create(&self, key: &str, p: AudiencePatch) -> Result<Audience> {
        validate_key(key)?;
        let now = chrono::Utc::now();
        let a = Audience {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            description: p.description.unwrap_or_default(),
            condition: p
                .condition
                .ok_or_else(|| Error::invalid("condition is required"))?,
            percentage: p.percentage.unwrap_or(100.0),
            enabled: p.enabled.unwrap_or(true),
            salt: p.salt.unwrap_or_default(),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        a.validate()?;
        let created = self.repo.create(&a).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::Audience, &created.key, created.version),
            )
            .await;
        Ok(created)
    }

    pub async fn update(&self, key: &str, p: AudiencePatch) -> Result<Audience> {
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
