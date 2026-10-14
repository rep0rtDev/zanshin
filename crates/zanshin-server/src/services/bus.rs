//! Change bus: every configuration write is applied locally, broadcast to in-process listeners
//! (WebSocket / gRPC streams) and published on Redis so other replicas and Nakama modules follow.

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;
use zanshin_core::Result;
use zanshin_store::cache::Cache;

pub const BUS_CHANNEL: &str = "changes";
const RECONCILE_EVERY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Flag,
    Audience,
    Experiment,
    LiveEvent,
    Message,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub kind: ChangeKind,
    pub key: String,
    pub version: i64,
    #[serde(default)]
    pub deleted: bool,
    /// Set when only one identity's evaluation changed (override, forced assignment).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<Uuid>,
    /// Replica that produced the change; used to ignore our own messages echoed back by Redis.
    #[serde(default)]
    pub origin: Uuid,
}

/// A catalog that can apply remote changes and reconcile itself against storage.
#[async_trait]
pub trait CatalogSync: Send + Sync {
    async fn apply(&self, change: &Change);
    /// Reload everything; return a change per item whose version moved or that disappeared.
    async fn reconcile(&self) -> Result<Vec<Change>>;
}

#[derive(Clone)]
pub struct ChangeBus {
    cache: Cache,
    redis_url: String,
    tx: broadcast::Sender<Change>,
    node_id: Uuid,
    handlers: Arc<RwLock<HashMap<ChangeKind, Arc<dyn CatalogSync>>>>,
}

impl ChangeBus {
    pub fn new(cache: Cache, redis_url: String) -> Self {
        let (tx, _) = broadcast::channel(2048);
        Self {
            cache,
            redis_url,
            tx,
            node_id: Uuid::new_v4(),
            handlers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn node_id(&self) -> Uuid {
        self.node_id
    }

    pub async fn register(&self, kind: ChangeKind, handler: Arc<dyn CatalogSync>) {
        self.handlers.write().await.insert(kind, handler);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.tx.subscribe()
    }

    pub fn change(&self, kind: ChangeKind, key: impl Into<String>, version: i64) -> Change {
        Change {
            kind,
            key: key.into(),
            version,
            deleted: false,
            identity_id: None,
            origin: self.node_id,
        }
    }

    /// Apply locally, notify in-process listeners, then fan out through Redis.
    pub async fn publish(&self, change: Change) {
        self.apply_local(&change).await;
        if let Err(e) = self.cache.publish(BUS_CHANNEL, &change).await {
            tracing::warn!(error = %e, "change not published to bus");
        }
    }

    async fn apply_local(&self, change: &Change) {
        if change.identity_id.is_none() {
            if let Some(h) = self.handlers.read().await.get(&change.kind).cloned() {
                h.apply(change).await;
            }
        }
        let _ = self.tx.send(change.clone());
    }

    /// Reconcile every catalog against storage now (used after bulk DB edits such as label
    /// renames); other replicas pick the change up on their periodic reconcile.
    pub async fn reload_all(&self) {
        self.reconcile_all().await;
    }

    async fn reconcile_all(&self) {
        let handlers: Vec<_> = self.handlers.read().await.values().cloned().collect();
        for h in handlers {
            match h.reconcile().await {
                Ok(changes) => {
                    for mut c in changes {
                        c.origin = self.node_id;
                        let _ = self.tx.send(c);
                    }
                }
                Err(e) => tracing::warn!(error = %e, "catalog reconcile failed"),
            }
        }
    }

    /// Follow the Redis stream and reconcile periodically. Runs until the process exits.
    pub fn spawn_listener(&self) {
        let bus = self.clone();
        tokio::spawn(async move {
            loop {
                match bus.cache.subscribe(&bus.redis_url, &[BUS_CHANNEL]).await {
                    Ok(mut sub) => {
                        tracing::info!("change stream connected");
                        let mut tick = tokio::time::interval(RECONCILE_EVERY);
                        tick.tick().await;
                        loop {
                            tokio::select! {
                                msg = sub.next::<Change>() => match msg {
                                    Some((_, change)) => {
                                        if change.origin != bus.node_id {
                                            bus.apply_local(&change).await;
                                        }
                                    }
                                    None => { tracing::warn!("change stream dropped; reconnecting"); break; }
                                },
                                _ = tick.tick() => bus.reconcile_all().await,
                            }
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "change stream unavailable; retrying"),
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }
}
