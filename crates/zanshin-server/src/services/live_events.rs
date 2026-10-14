//! Live event definitions: catalog + admin writes + per-identity view.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use zanshin_core::{
    flags::validate_key,
    live_events::{ActiveEvent, JoinMode, LiveEvent},
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
    #[serde(default)]
    pub join_mode: Option<JoinMode>,
    #[serde(default)]
    pub sticky: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    pub participation_secs: Option<Option<i64>>,
    #[serde(default)]
    pub flag_overrides: Option<serde_json::Map<String, Value>>,
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
        self.for_participant(
            flat,
            audiences,
            names,
            active_only,
            chrono::Utc::now(),
            &HashMap::new(),
        )
        .await
    }

    /// Events for a player, taking their joins (event id → joined_at) and the evaluation clock
    /// into account. Sticky participants keep events they joined even if targeting no longer
    /// matches.
    pub async fn for_participant(
        &self,
        flat: &PropertyMap,
        audiences: &HashSet<String>,
        names: &[String],
        active_only: bool,
        now: chrono::DateTime<chrono::Utc>,
        joins: &HashMap<Uuid, chrono::DateTime<chrono::Utc>>,
    ) -> Vec<ActiveEvent> {
        let member = |k: &str| audiences.contains(k);
        self.catalog
            .list()
            .await
            .iter()
            .filter(|e| names.is_empty() || names.contains(&e.key))
            .filter(|e| e.targets(flat, &member) || (e.sticky && joins.contains_key(&e.id)))
            .map(|e| e.view_for(now, joins.get(&e.id).copied()))
            .filter(|v| !active_only || v.active)
            .collect()
    }

    /// Flag overrides contributed by the events currently active for the player.
    pub fn flag_pins(
        &self,
        active: &[ActiveEvent],
        catalog: &[Arc<LiveEvent>],
    ) -> HashMap<String, (String, Value)> {
        let mut out = HashMap::new();
        for ev in catalog {
            if ev.flag_overrides.is_empty() || !active.iter().any(|a| a.active && a.key == ev.key) {
                continue;
            }
            for (flag, value) in &ev.flag_overrides {
                out.entry(flag.clone())
                    .or_insert_with(|| (ev.key.clone(), value.clone()));
            }
        }
        out
    }

    /// Join an event (explicit or auto). Requires the event to be open now unless `sticky`
    /// participants may pre-register during any window; returns the player's view afterwards.
    pub async fn join(
        &self,
        identity_id: Uuid,
        key: &str,
        flat: &PropertyMap,
        audiences: &HashSet<String>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ActiveEvent> {
        let ev = self.get(key).await?;
        if !ev.enabled {
            return Err(Error::invalid(format!("live event {key:?} is disabled")));
        }
        if !ev.targets(flat, &|k: &str| audiences.contains(k)) {
            return Err(Error::denied(format!(
                "identity is not targeted by live event {key:?}"
            )));
        }
        if ev.current(now).is_none() {
            return Err(Error::invalid(format!(
                "live event {key:?} is not open right now"
            )));
        }
        let joined_at = self.repo.join(ev.id, identity_id).await?;
        metrics::counter!("zanshin_live_event_joins_total").increment(1);
        Ok(ev.view_for(now, Some(joined_at)))
    }

    pub async fn leave(&self, identity_id: Uuid, key: &str) -> Result<bool> {
        let ev = self.get(key).await?;
        self.repo.leave(ev.id, identity_id).await
    }

    pub async fn joins_for(
        &self,
        identity_id: Uuid,
    ) -> HashMap<Uuid, chrono::DateTime<chrono::Utc>> {
        self.repo.joins_for(identity_id).await.unwrap_or_default()
    }

    pub async fn participants(
        &self,
        key: &str,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<zanshin_store::live_events::Participant>> {
        let ev = self.get(key).await?;
        self.repo
            .participants(ev.id, after, limit.clamp(1, 1000))
            .await
    }

    /// Timeline: past + upcoming occurrences in a range and joins per day.
    pub async fn timeline(
        &self,
        key: &str,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Value> {
        let ev = self.get(key).await?;
        let mut occurrences = Vec::new();
        let mut cursor = from;
        while let Some(o) = ev.current(cursor).or_else(|| ev.next(cursor)) {
            if o.start > to || occurrences.len() >= 500 {
                break;
            }
            cursor = o.end + chrono::Duration::seconds(1);
            occurrences.push(o);
        }
        let joins = self.repo.join_timeline(ev.id).await?;
        let (total, active) = self.repo.participant_count(ev.id).await?;
        Ok(serde_json::json!({
            "occurrences": occurrences,
            "joins": joins.iter().map(|(d, n)| serde_json::json!({"day": d, "count": n})).collect::<Vec<_>>(),
            "participants": {"total": total, "active": active},
        }))
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
            join_mode: p.join_mode.unwrap_or_default(),
            sticky: p.sticky.unwrap_or(false),
            participation_secs: p.participation_secs.unwrap_or(None),
            flag_overrides: p.flag_overrides.unwrap_or_default(),
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
        if let Some(v) = p.join_mode {
            e.join_mode = v;
        }
        if let Some(v) = p.sticky {
            e.sticky = v;
        }
        if let Some(v) = p.participation_secs {
            e.participation_secs = v;
        }
        if let Some(v) = p.flag_overrides {
            e.flag_overrides = v;
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
