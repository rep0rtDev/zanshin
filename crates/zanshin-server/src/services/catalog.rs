//! Generic in-memory catalog of keyed, versioned definitions backed by a repository.

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use tokio::sync::RwLock;
use zanshin_core::{Error, Result};

use super::bus::{CatalogSync, Change, ChangeKind};

pub trait CatalogItem: Send + Sync + 'static {
    fn key(&self) -> &str;
    fn version(&self) -> i64;
}

#[async_trait]
pub trait CatalogSource<T>: Send + Sync {
    async fn list(&self) -> Result<Vec<T>>;
    async fn get(&self, key: &str) -> Result<T>;
}

pub struct Catalog<T> {
    kind: ChangeKind,
    label: &'static str,
    items: RwLock<HashMap<String, Arc<T>>>,
    source: Arc<dyn CatalogSource<T>>,
}

impl<T: CatalogItem> Catalog<T> {
    pub fn new(
        kind: ChangeKind,
        label: &'static str,
        source: Arc<dyn CatalogSource<T>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            kind,
            label,
            items: RwLock::new(HashMap::new()),
            source,
        })
    }

    pub async fn load_all(&self) -> Result<usize> {
        let items = self.source.list().await?;
        let n = items.len();
        *self.items.write().await = items
            .into_iter()
            .map(|i| (i.key().to_string(), Arc::new(i)))
            .collect();
        metrics::gauge!("zanshin_catalog_items", "kind" => self.label).set(n as f64);
        Ok(n)
    }

    pub async fn list(&self) -> Vec<Arc<T>> {
        let mut v: Vec<_> = self.items.read().await.values().cloned().collect();
        v.sort_by(|a, b| a.key().cmp(b.key()));
        v
    }

    pub async fn get(&self, key: &str) -> Result<Arc<T>> {
        self.items
            .read()
            .await
            .get(key)
            .cloned()
            .ok_or_else(|| Error::not_found(format!("{} {key:?} not found", self.label)))
    }

    pub async fn snapshot(&self) -> HashMap<String, Arc<T>> {
        self.items.read().await.clone()
    }

    pub async fn put(&self, item: T) {
        self.items
            .write()
            .await
            .insert(item.key().to_string(), Arc::new(item));
    }

    pub async fn remove(&self, key: &str) {
        self.items.write().await.remove(key);
    }
}

#[async_trait]
impl<T: CatalogItem> CatalogSync for Catalog<T> {
    async fn apply(&self, change: &Change) {
        if change.deleted {
            self.remove(&change.key).await;
            return;
        }
        match self.source.get(&change.key).await {
            Ok(item) => self.put(item).await,
            Err(Error::NotFound(_)) => self.remove(&change.key).await,
            Err(e) => {
                tracing::warn!(kind = self.label, key = %change.key, error = %e, "catalog reload failed")
            }
        }
    }

    async fn reconcile(&self) -> Result<Vec<Change>> {
        let fresh = self.source.list().await?;
        let mut items = self.items.write().await;
        let mut changes = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for f in fresh {
            seen.insert(f.key().to_string());
            if items.get(f.key()).map(|old| old.version()) != Some(f.version()) {
                changes.push(Change {
                    kind: self.kind,
                    key: f.key().to_string(),
                    version: f.version(),
                    deleted: false,
                    identity_id: None,
                    origin: uuid::Uuid::nil(),
                });
            }
            items.insert(f.key().to_string(), Arc::new(f));
        }
        for gone in items
            .keys()
            .filter(|k| !seen.contains(*k))
            .cloned()
            .collect::<Vec<_>>()
        {
            items.remove(&gone);
            changes.push(Change {
                kind: self.kind,
                key: gone,
                version: 0,
                deleted: true,
                identity_id: None,
                origin: uuid::Uuid::nil(),
            });
        }
        metrics::gauge!("zanshin_catalog_items", "kind" => self.label).set(items.len() as f64);
        Ok(changes)
    }
}
