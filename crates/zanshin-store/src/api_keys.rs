use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{apikey::ApiKeyKind, Error, Result};

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
}

impl ApiKeyRow {
    pub fn kind(&self) -> ApiKeyKind {
        ApiKeyKind::parse(&self.kind).unwrap_or(ApiKeyKind::Client)
    }
}

const COLS: &str =
    "id, name, kind, prefix, created_at, last_used_at, revoked_at, read_only, expires_at, rate_limit_per_minute";

/// Optional restrictions applied to a new key.
#[derive(Debug, Clone, Default)]
pub struct KeyOptions {
    /// Admin keys only: may read everything under `/admin/v1` but not mutate.
    pub read_only: bool,
    pub expires_at: Option<DateTime<Utc>>,
    /// Per-key request budget overriding the global scope limit.
    pub rate_limit_per_minute: Option<i32>,
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
            "INSERT INTO api_keys (id, name, kind, prefix, key_hash, read_only, expires_at, rate_limit_per_minute) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {COLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(name)
        .bind(kind.as_str())
        .bind(prefix)
        .bind(hash)
        .bind(opts.read_only)
        .bind(opts.expires_at)
        .bind(opts.rate_limit_per_minute)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
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
