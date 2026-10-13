//! Live event definitions: catalog + admin writes + per-identity view.

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use zanshin_core::{
    flags::validate_key,
    live_events::{ActiveEvent, LiveEvent},
    properties::PropertyMap,
    rules::Condition,
    Error, Result,
};
use zanshin_store::live_events::LiveEventRepo;

use super::{
    bus::{ChangeBus, ChangeKind},
    catalog::{Catalog, CatalogItem, CatalogSource},
    flags::normalize_tags,
};

impl CatalogItem for LiveEvent {
    fn key(&self) -> &str {
        &self.key
    }
    fn version(&self) -> i64 {
        self.version
    }
}

#[async_trait]
impl CatalogSource<LiveEvent> for LiveEventRepo {
    async fn list(&self) -> Result<Vec<LiveEvent>> {
        LiveEventRepo::list(self).await
    }
    async fn get(&self, key: &str) -> Result<LiveEvent> {
        LiveEventRepo::get(self, key).await
    }
}

fn double_option<'de, T, D>(d: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LiveEventPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub condition: Option<Condition>,
    #[serde(default)]
    pub audiences: Option<Vec<String>>,
    #[serde(default)]
    pub starts_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, deserialize_with = "double_option")]
    pub ends_at: Option<Option<chrono::DateTime<chrono::Utc>>>,
    #[serde(default, deserialize_with = "double_option")]
    pub duration_secs: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub recurrence: Option<Option<String>>,
    #[serde(default)]
    pub payload: Option<Value>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct LiveEventService {
    repo: LiveEventRepo,
    bus: ChangeBus,
    pub catalog: Arc<Catalog<LiveEvent>>,
}

impl LiveEventService {
    pub fn new(repo: LiveEventRepo, bus: ChangeBus) -> Self {
        let catalog = Catalog::new(ChangeKind::LiveEvent, "live_event", Arc::new(repo.clone()));
        Self { repo, bus, catalog }
    }

    pub async fn list(&self) -> Vec<Arc<LiveEvent>> {
        self.catalog.list().await
    }

    pub async fn get(&self, key: &str) -> Result<Arc<LiveEvent>> {
        self.catalog.get(key).await
    }

    /// Events targeting the subject. `active_only` drops events with no current occurrence.
    pub async fn for_subject(
        &self,
        flat: &PropertyMap,
        audiences: &HashSet<String>,
        names: &[String],
        active_only: bool,
    ) -> Vec<ActiveEvent> {
        let now = chrono::Utc::now();
        let member = |k: &str| audiences.contains(k);
        self.catalog
            .list()
            .await
            .iter()
            .filter(|e| names.is_empty() || names.contains(&e.key))
            .filter(|e| e.targets(flat, &member))
            .map(|e| e.view(now))
            .filter(|v| !active_only || v.active)
            .collect()
    }

    pub async fn create(&self, key: &str, p: LiveEventPatch) -> Result<LiveEvent> {
        validate_key(key)?;
        let now = chrono::Utc::now();
        let e = LiveEvent {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            description: p.description.unwrap_or_default(),
            enabled: p.enabled.unwrap_or(true),
            condition: p.condition.unwrap_or_else(Condition::always),
            audiences: p.audiences.unwrap_or_default(),
            starts_at: p
                .starts_at
                .ok_or_else(|| Error::invalid("starts_at is required"))?,
            ends_at: p.ends_at.flatten(),
            duration_secs: p.duration_secs.flatten(),
            recurrence: p.recurrence.flatten(),
            payload: p.payload.unwrap_or(Value::Null),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        e.validate()?;
        let created = self.repo.create(&e).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::LiveEvent, &created.key, created.version),
            )
            .await;
        Ok(created)
    }

    pub async fn update(&self, key: &str, p: LiveEventPatch) -> Result<LiveEvent> {
        let mut e = self.repo.get(key).await?;
        if let Some(v) = p.name {
            e.name = v;
        }
        if let Some(v) = p.description {
            e.description = v;
        }
        if let Some(v) = p.enabled {
            e.enabled = v;
        }
        if let Some(v) = p.condition {
            e.condition = v;
        }
        if let Some(v) = p.audiences {
            e.audiences = v;
        }
        if let Some(v) = p.starts_at {
            e.starts_at = v;
        }
        if let Some(v) = p.ends_at {
            e.ends_at = v;
        }
        if let Some(v) = p.duration_secs {
            e.duration_secs = v;
        }
        if let Some(v) = p.recurrence {
            e.recurrence = v;
        }
        if let Some(v) = p.payload {
            e.payload = v;
        }
        if let Some(v) = p.tags {
            e.tags = normalize_tags(v)?;
        }
        e.validate()?;
        let updated = self.repo.update(&e).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::LiveEvent, &updated.key, updated.version),
            )
            .await;
        Ok(updated)
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.repo.delete(key).await?;
        let mut c = self.bus.change(ChangeKind::LiveEvent, key, 0);
        c.deleted = true;
        self.bus.publish(c).await;
        Ok(())
    }
}
