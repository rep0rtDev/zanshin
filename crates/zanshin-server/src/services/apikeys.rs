//! API key verification with a short Redis cache in front of PostgreSQL.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zanshin_core::{
    apikey::{self, ApiKeyKind},
    Error, Result,
};
use zanshin_store::{
    api_keys::{ApiKeyRepo, ApiKeyRow},
    cache::Cache,
};

const CACHE_TTL: Duration = Duration::from_secs(30);

/// The authenticated caller behind an API key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub key_id: Option<Uuid>,
    pub name: String,
    pub kind: ApiKeyKind,
}

impl Principal {
    pub fn is_admin(&self) -> bool {
        self.kind == ApiKeyKind::Admin
    }
    pub fn is_server(&self) -> bool {
        matches!(self.kind, ApiKeyKind::Server | ApiKeyKind::Admin)
    }
}

#[derive(Clone)]
pub struct ApiKeyService {
    repo: ApiKeyRepo,
    cache: Cache,
    bootstrap_hash: Option<Vec<u8>>,
}

impl ApiKeyService {
    pub fn new(repo: ApiKeyRepo, cache: Cache, bootstrap_key: Option<String>) -> Self {
        Self {
            repo,
            cache,
            bootstrap_hash: bootstrap_key.map(|k| apikey::hash(&k)),
        }
    }

    /// Resolve a plaintext key to a principal. Unknown or revoked keys are `Unauthenticated`.
    pub async fn authenticate(&self, plaintext: &str) -> Result<Principal> {
        let hash = apikey::hash(plaintext);
        if let Some(b) = &self.bootstrap_hash {
            if constant_time_eq(b, &hash) {
                return Ok(Principal {
                    key_id: None,
                    name: "bootstrap".into(),
                    kind: ApiKeyKind::Admin,
                });
            }
        }
        if !apikey::looks_like_key(plaintext) {
            return Err(Error::unauthenticated("invalid api key"));
        }
        let cache_key = self.cache.key(&["apikey", &hex(&hash)]);
        if let Ok(Some(p)) = self.cache.get_json::<Principal>(&cache_key).await {
            return Ok(p);
        }
        let row = self
            .repo
            .find_active_by_hash(&hash)
            .await?
            .ok_or_else(|| Error::unauthenticated("invalid api key"))?;
        let principal = Principal {
            key_id: Some(row.id),
            name: row.name.clone(),
            kind: row.kind(),
        };
        let _ = self.cache.set_json(&cache_key, &principal, CACHE_TTL).await;
        let repo = self.repo.clone();
        tokio::spawn(async move {
            let _ = repo.touch(row.id).await;
        });
        Ok(principal)
    }

    pub async fn create(&self, name: &str, kind: ApiKeyKind) -> Result<(ApiKeyRow, String)> {
        if name.trim().is_empty() || name.len() > 64 {
            return Err(Error::invalid("name must be 1..64 characters"));
        }
        let g = apikey::generate(kind);
        let row = self
            .repo
            .create(name.trim(), kind, &g.prefix, &g.hash)
            .await?;
        Ok((row, g.plaintext))
    }

    pub async fn list(&self) -> Result<Vec<ApiKeyRow>> {
        self.repo.list().await
    }

    pub async fn revoke(&self, id: Uuid) -> Result<ApiKeyRow> {
        let row = self.repo.revoke(id).await?;
        // The cache key is derived from the hash which we no longer have; entries age out in CACHE_TTL.
        Ok(row)
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
