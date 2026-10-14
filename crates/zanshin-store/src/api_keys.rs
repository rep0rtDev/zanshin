use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
    apikey::{ApiKeyKind, Role},
    permissions::Permissions,
    Error, Result,
};

use crate::map_sqlx;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ApiKeyRow {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub prefix: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub read_only: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub rate_limit_per_minute: Option<i32>,
    pub role: String,
    /// Admin keys: explicit grants; `None` = derived from `role`.
    pub permissions: Option<serde_json::Value>,
    pub rotated_from: Option<Uuid>,
    pub rotated_to: Option<Uuid>,
}

impl ApiKeyRow {
    pub fn kind(&self) -> ApiKeyKind {
        ApiKeyKind::parse(&self.kind).unwrap_or(ApiKeyKind::Client)
    }

    pub fn role(&self) -> Role {
        Role::parse(&self.role).unwrap_or(Role::Admin)
    }
    pub fn permissions(&self) -> Permissions {
        self.permissions
            .clone()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_else(|| Permissions::from_role(self.role()))
    }
}

const COLS: &str =
    "id, name, kind, prefix, created_at, last_used_at, revoked_at, read_only, expires_at, rate_limit_per_minute, role, permissions, rotated_from, rotated_to";

/// Optional restrictions applied to a new key.
#[derive(Debug, Clone, Default)]
pub struct KeyOptions {
    /// Admin keys only: may read everything under `/admin/v1` but not mutate.
    pub read_only: bool,
    pub expires_at: Option<DateTime<Utc>>,
    /// Per-key request budget overriding the global scope limit.
    pub rate_limit_per_minute: Option<i32>,
    /// Admin keys only: console/admin role. `None` = admin (or viewer when `read_only`).
    pub role: Option<Role>,
    /// Admin keys only: explicit grants (take precedence over `role` for authorization).
    pub permissions: Option<Permissions>,
}

impl KeyOptions {
    pub fn effective_role(&self) -> Role {
        match self.role {
            Some(r) => r,
            None if self.read_only => Role::Viewer,
            None => Role::Admin,
        }
    }
}

#[derive(Clone)]
pub struct ApiKeyRepo {
    pg: PgPool,
}

impl ApiKeyRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn create(
        &self,
        name: &str,
        kind: ApiKeyKind,
        prefix: &str,
        hash: &[u8],
        opts: &KeyOptions,
    ) -> Result<ApiKeyRow> {
        sqlx::query_as(&format!(
            "INSERT INTO api_keys (id, name, kind, prefix, key_hash, read_only, expires_at, rate_limit_per_minute, role, permissions) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING {COLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(name)
        .bind(kind.as_str())
        .bind(prefix)
        .bind(hash)
        .bind(opts.read_only)
        .bind(opts.expires_at)
        .bind(opts.rate_limit_per_minute)
        .bind(opts.effective_role().as_str())
        .bind(opts.permissions.as_ref().map(|p| serde_json::to_value(p).unwrap_or_default()))
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn get(&self, id: Uuid) -> Result<ApiKeyRow> {
        sqlx::query_as(&format!("SELECT {COLS} FROM api_keys WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pg)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::not_found("api key not found"))
    }

    /// Mint a successor with the same properties; the old key keeps working until `grace` elapses
    /// (or its own expiry, whichever is sooner) so callers can roll over without downtime.
    pub async fn rotate(
        &self,
        old_id: Uuid,
        prefix: &str,
        hash: &[u8],
        grace: chrono::Duration,
    ) -> Result<ApiKeyRow> {
        let mut tx = self.pg.begin().await.map_err(map_sqlx)?;
        let old: Option<ApiKeyRow> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM api_keys WHERE id = $1 AND revoked_at IS NULL FOR UPDATE"
        ))
        .bind(old_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        let old = old.ok_or_else(|| Error::not_found("api key not found or revoked"))?;
        if old.rotated_to.is_some() {
            return Err(Error::conflict("api key was already rotated"));
        }
        let new_id = Uuid::now_v7();
        let new: ApiKeyRow = sqlx::query_as(&format!(
            "INSERT INTO api_keys (id, name, kind, prefix, key_hash, read_only, expires_at, rate_limit_per_minute, role, permissions, rotated_from) \
             VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8, $9, $10) RETURNING {COLS}"
        ))
        .bind(new_id)
        .bind(&old.name)
        .bind(&old.kind)
        .bind(prefix)
        .bind(hash)
        .bind(old.read_only)
        .bind(old.rate_limit_per_minute)
        .bind(&old.role)
        .bind(&old.permissions)
        .bind(old_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        let grace_until = Utc::now() + grace;
        sqlx::query(
            "UPDATE api_keys SET rotated_to = $2, expires_at = LEAST(COALESCE(expires_at, $3), $3) WHERE id = $1",
        )
        .bind(old_id)
        .bind(new_id)
        .bind(grace_until)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(new)
    }

    pub async fn find_active_by_hash(&self, hash: &[u8]) -> Result<Option<ApiKeyRow>> {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM api_keys WHERE key_hash = $1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())"
        ))
        .bind(hash)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn touch(&self, id: Uuid) -> Result<()> {
        sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<ApiKeyRow>> {
        sqlx::query_as(&format!("SELECT {COLS} FROM api_keys ORDER BY created_at"))
            .fetch_all(&self.pg)
            .await
            .map_err(map_sqlx)
    }

    pub async fn revoke(&self, id: Uuid) -> Result<ApiKeyRow> {
        sqlx::query_as(&format!("UPDATE api_keys SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL RETURNING {COLS}"))
            .bind(id)
            .fetch_optional(&self.pg)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::not_found("api key not found or already revoked"))
    }
}
