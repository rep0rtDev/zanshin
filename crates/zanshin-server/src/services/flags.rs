//! Feature flag catalog: in-memory model of every flag, kept fresh through a Redis change stream,
//! evaluated locally per identity.

use std::{collections::HashMap, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;
use zanshin_core::{
    flags::{validate_key, Evaluation, Flag, Rule, ValueType},
    properties::PropertyMap,
    Error, Identity, PropertySet, Result,
};
use zanshin_store::{
    cache::Cache,
    flags::{FlagInput, FlagRepo},
    identities::IdentityRepo,
};

pub const BUS_CHANNEL: &str = "flags";
const FULL_RELOAD_EVERY: Duration = Duration::from_secs(60);

/// Change notification shared between replicas (Redis) and with connected clients (broadcast).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlagChange {
    pub key: String,
    pub version: i64,
    #[serde(default)]
    pub deleted: bool,
    /// Set when only one identity's evaluation changed (override added / removed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<Uuid>,
    /// Replica that produced the change; used to ignore our own messages echoed back by Redis.
    #[serde(default)]
    pub origin: Uuid,
}

#[derive(Default)]
struct Catalog {
    flags: HashMap<String, Arc<Flag>>,
}

#[derive(Clone)]
pub struct FlagService {
    repo: FlagRepo,
    identities: IdentityRepo,
    cache: Cache,
    redis_url: String,
    catalog: Arc<RwLock<Catalog>>,
    changes: broadcast::Sender<FlagChange>,
    node_id: Uuid,
}

impl FlagService {
    pub fn new(repo: FlagRepo, identities: IdentityRepo, cache: Cache, redis_url: String) -> Self {
        let (changes, _) = broadcast::channel(1024);
        Self {
            repo,
            identities,
            cache,
            redis_url,
            catalog: Arc::new(RwLock::new(Catalog::default())),
            changes,
            node_id: Uuid::new_v4(),
        }
    }

    /// Load every flag from PostgreSQL into memory.
    pub async fn load_all(&self) -> Result<usize> {
        let flags = self.repo.list().await?;
        let n = flags.len();
        let mut cat = self.catalog.write().await;
        cat.flags = flags
            .into_iter()
            .map(|f| (f.key.clone(), Arc::new(f)))
            .collect();
        metrics::gauge!("zanshin_flags_total").set(n as f64);
        Ok(n)
    }

    /// Follow the Redis change stream and periodically reconcile. Runs until the process exits.
    pub fn spawn_listener(&self) {
        let svc = self.clone();
        tokio::spawn(async move {
            loop {
                match svc.cache.subscribe(&svc.redis_url, &[BUS_CHANNEL]).await {
                    Ok(mut sub) => {
                        tracing::info!("flag change stream connected");
                        let mut tick = tokio::time::interval(FULL_RELOAD_EVERY);
                        tick.tick().await;
                        loop {
                            tokio::select! {
                                msg = sub.next::<FlagChange>() => match msg {
                                    Some((_, change)) => {
                                        if change.origin != svc.node_id {
                                            svc.apply_remote(change).await;
                                        }
                                    }
                                    None => { tracing::warn!("flag change stream dropped; reconnecting"); break; }
                                },
                                _ = tick.tick() => {
                                    if let Err(e) = svc.reconcile().await {
                                        tracing::warn!(error = %e, "flag reconcile failed");
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "flag change stream unavailable; retrying")
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    async fn apply_remote(&self, change: FlagChange) {
        if change.identity_id.is_none() {
            if change.deleted {
                self.catalog.write().await.flags.remove(&change.key);
            } else {
                match self.repo.get(&change.key).await {
                    Ok(f) => {
                        self.catalog
                            .write()
                            .await
                            .flags
                            .insert(f.key.clone(), Arc::new(f));
                    }
                    Err(Error::NotFound(_)) => {
                        self.catalog.write().await.flags.remove(&change.key);
                    }
                    Err(e) => tracing::warn!(key = %change.key, error = %e, "flag reload failed"),
                }
            }
        }
        let _ = self.changes.send(change);
    }

    /// Full reload; emits change events for anything whose version moved.
    async fn reconcile(&self) -> Result<()> {
        let fresh = self.repo.list().await?;
        let mut cat = self.catalog.write().await;
        let mut events = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for f in fresh {
            seen.insert(f.key.clone());
            if cat.flags.get(&f.key).map(|old| old.version) != Some(f.version) {
                events.push(FlagChange {
                    origin: self.node_id,
                    key: f.key.clone(),
                    version: f.version,
                    deleted: false,
                    identity_id: None,
                });
            }
            cat.flags.insert(f.key.clone(), Arc::new(f));
        }
        for gone in cat
            .flags
            .keys()
            .filter(|k| !seen.contains(*k))
            .cloned()
            .collect::<Vec<_>>()
        {
            cat.flags.remove(&gone);
            events.push(FlagChange {
                origin: self.node_id,
                key: gone,
                version: 0,
                deleted: true,
                identity_id: None,
            });
        }
        metrics::gauge!("zanshin_flags_total").set(cat.flags.len() as f64);
        drop(cat);
        for e in events {
            let _ = self.changes.send(e);
        }
        Ok(())
    }

    async fn publish(&self, change: FlagChange) {
        // Apply locally first so this replica is consistent even if Redis is down.
        self.apply_remote(change.clone()).await;
        if let Err(e) = self.cache.publish(BUS_CHANNEL, &change).await {
            tracing::warn!(error = %e, "flag change not published to bus");
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<FlagChange> {
        self.changes.subscribe()
    }

    // ---- reads -------------------------------------------------------------------------------

    pub async fn list(&self) -> Vec<Arc<Flag>> {
        let cat = self.catalog.read().await;
        let mut v: Vec<_> = cat.flags.values().cloned().collect();
        v.sort_by(|a, b| a.key.cmp(&b.key));
        v
    }

    pub async fn get(&self, key: &str) -> Result<Arc<Flag>> {
        self.catalog
            .read()
            .await
            .flags
            .get(key)
            .cloned()
            .ok_or_else(|| Error::not_found(format!("flag {key:?} not found")))
    }

    /// Evaluate flags for an identity. `names` empty = every flag.
    pub async fn evaluate(&self, identity: &Identity, names: &[String]) -> Result<Vec<Evaluation>> {
        let overrides = self
            .repo
            .overrides_for(identity.id)
            .await
            .unwrap_or_default();
        Ok(self
            .evaluate_with(identity, &identity.properties, names, &overrides)
            .await)
    }

    /// Evaluate with explicit properties (admin dry-run, or a client supplying context).
    pub async fn evaluate_with(
        &self,
        identity: &Identity,
        props: &PropertySet,
        names: &[String],
        overrides: &HashMap<String, Value>,
    ) -> Vec<Evaluation> {
        let flat = flatten_with_identity(identity, props);
        let audiences = |_: &str| false;
        let cat = self.catalog.read().await;
        let mut out = Vec::new();
        if names.is_empty() {
            let mut keys: Vec<_> = cat.flags.keys().collect();
            keys.sort();
            for k in keys {
                out.push(cat.flags[k].evaluate(
                    &identity.external_id,
                    &flat,
                    &audiences,
                    overrides.get(k),
                ));
            }
        } else {
            for k in names {
                if let Some(f) = cat.flags.get(k) {
                    out.push(f.evaluate(
                        &identity.external_id,
                        &flat,
                        &audiences,
                        overrides.get(k),
                    ));
                }
            }
        }
        metrics::counter!("zanshin_flag_evaluations_total").increment(out.len() as u64);
        out
    }

    pub async fn evaluate_one(&self, identity: &Identity, key: &str) -> Result<Evaluation> {
        self.get(key).await?;
        let mut v = self
            .evaluate(identity, std::slice::from_ref(&key.to_string()))
            .await?;
        v.pop()
            .ok_or_else(|| Error::not_found(format!("flag {key:?} not found")))
    }

    pub async fn evaluate_by_external_id(
        &self,
        external_id: &str,
        names: &[String],
    ) -> Result<Vec<Evaluation>> {
        let identity = self.identities.get_by_external_id(external_id).await?;
        self.evaluate(&identity, names).await
    }

    // ---- admin writes ------------------------------------------------------------------------

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
            version: 1,
            created_at: now,
            updated_at: now,
        };
        flag.validate()?;
        let created = self.repo.create(&flag).await?;
        self.publish(FlagChange {
            origin: self.node_id,
            key: created.key.clone(),
            version: created.version,
            deleted: false,
            identity_id: None,
        })
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
        flag.validate()?;
        let updated = self.repo.update(&flag).await?;
        self.publish(FlagChange {
            origin: self.node_id,
            key: updated.key.clone(),
            version: updated.version,
            deleted: false,
            identity_id: None,
        })
        .await;
        metrics::counter!("zanshin_flag_writes_total", "op" => "update").increment(1);
        Ok(updated)
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.repo.delete(key).await?;
        self.publish(FlagChange {
            origin: self.node_id,
            key: key.to_string(),
            version: 0,
            deleted: true,
            identity_id: None,
        })
        .await;
        metrics::counter!("zanshin_flag_writes_total", "op" => "delete").increment(1);
        Ok(())
    }

    pub async fn set_override(&self, key: &str, identity_id: Uuid, value: Value) -> Result<()> {
        let flag = self.get(key).await?;
        flag.value_type.check(&value)?;
        self.identities.get(identity_id).await?;
        self.repo.set_override(flag.id, identity_id, &value).await?;
        self.publish(FlagChange {
            origin: self.node_id,
            key: key.to_string(),
            version: flag.version,
            deleted: false,
            identity_id: Some(identity_id),
        })
        .await;
        Ok(())
    }

    pub async fn delete_override(&self, key: &str, identity_id: Uuid) -> Result<()> {
        let flag = self.get(key).await?;
        self.repo.delete_override(flag.id, identity_id).await?;
        self.publish(FlagChange {
            origin: self.node_id,
            key: key.to_string(),
            version: flag.version,
            deleted: false,
            identity_id: Some(identity_id),
        })
        .await;
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

fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>> {
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

/// Admin-facing partial input; mirrors `FlagInput` with serde.
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
        }
    }
}
