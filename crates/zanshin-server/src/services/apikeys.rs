//! API key verification with a short Redis cache in front of PostgreSQL.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zanshin_core::{
    apikey::{self, ApiKeyKind, Role},
    permissions::{Action, Permissions},
    Error, Result,
};
use zanshin_store::{
    api_keys::{ApiKeyRepo, ApiKeyRow, KeyOptions},
    cache::Cache,
};

const CACHE_TTL: Duration = Duration::from_secs(30);

/// The authenticated caller behind an API key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub key_id: Option<Uuid>,
    pub name: String,
    pub kind: ApiKeyKind,
    /// Admin keys only: may read but not mutate.
    #[serde(default)]
    pub read_only: bool,
    /// Per-key request budget overriding the scope default.
    #[serde(default)]
    pub rate_limit_per_minute: Option<u32>,
    /// Admin principals only: what the console / admin API lets them do.
    #[serde(default)]
    pub role: Role,
    /// Explicit grants; `None` = derived from `role` (older keys, OIDC roles).
    #[serde(default)]
    pub permissions: Option<Permissions>,
    /// Console user behind this principal (password / TOTP login), if any.
    #[serde(default)]
    pub user_id: Option<Uuid>,
}

impl Principal {
    pub fn is_admin(&self) -> bool {
        self.kind == ApiKeyKind::Admin
    }
    pub fn permissions(&self) -> Permissions {
        self.permissions
            .clone()
            .unwrap_or_else(|| Permissions::from_role(self.role))
    }
    pub fn allows(&self, resource: &str, action: Action) -> bool {
        self.permissions().allows(resource, action)
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
                    read_only: false,
                    rate_limit_per_minute: None,
                    role: Role::Admin,
                    permissions: None,
                    user_id: None,
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
            read_only: row.role() == Role::Viewer,
            rate_limit_per_minute: row.rate_limit_per_minute.map(|n| n as u32),
            role: row.role(),
            permissions: row.permissions.as_ref().map(|_| row.permissions()),
            user_id: None,
        };
        let _ = self.cache.set_json(&cache_key, &principal, CACHE_TTL).await;
        let repo = self.repo.clone();
        tokio::spawn(async move {
            let _ = repo.touch(row.id).await;
        });
        Ok(principal)
    }

    /// Mint a successor key with identical properties; the old one expires after `grace`.
    pub async fn rotate(&self, id: Uuid, grace: chrono::Duration) -> Result<(ApiKeyRow, String)> {
        let old = self.repo.get(id).await?;
        let generated = apikey::generate(old.kind());
        let row = self
            .repo
            .rotate(id, &generated.prefix, &generated.hash, grace)
            .await?;
        Ok((row, generated.plaintext))
    }

    pub async fn create(
        &self,
        name: &str,
        kind: ApiKeyKind,
        opts: KeyOptions,
    ) -> Result<(ApiKeyRow, String)> {
        if name.trim().is_empty() || name.len() > 64 {
            return Err(Error::invalid("name must be 1..64 characters"));
        }
        if (opts.read_only || opts.role.is_some()) && kind != ApiKeyKind::Admin {
            return Err(Error::invalid("read_only / role apply to admin keys only"));
        }
        if opts.read_only && matches!(opts.role, Some(r) if r != Role::Viewer) {
            return Err(Error::invalid("read_only conflicts with a non-viewer role"));
        }
        if let Some(exp) = opts.expires_at {
            if exp <= chrono::Utc::now() {
                return Err(Error::invalid("expires_at must be in the future"));
            }
        }
        if matches!(opts.rate_limit_per_minute, Some(n) if n <= 0) {
            return Err(Error::invalid("rate_limit_per_minute must be positive"));
        }
        let g = apikey::generate(kind);
        let row = self
            .repo
            .create(name.trim(), kind, &g.prefix, &g.hash, &opts)
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
