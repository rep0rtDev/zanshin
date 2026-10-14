//! Audit log of admin API calls.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::Result;

use crate::map_sqlx;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AuditEntry {
    pub id: Uuid,
    pub at: DateTime<Utc>,
    pub actor_key_id: Option<Uuid>,
    pub actor_name: String,
    pub actor_kind: String,
    pub method: String,
    pub path: String,
    pub status: i32,
    pub request_id: Option<String>,
    pub ip: Option<String>,
    pub body: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct NewAuditEntry {
    pub actor_key_id: Option<Uuid>,
    pub actor_name: String,
    pub actor_kind: String,
    pub method: String,
    pub path: String,
    pub status: i32,
    pub request_id: Option<String>,
    pub ip: Option<String>,
    pub body: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    pub after: Option<Uuid>,
    pub limit: i64,
    pub actor: Option<String>,
    pub method: Option<String>,
    pub path_prefix: Option<String>,
    pub since: Option<DateTime<Utc>>,
}

const COLS: &str =
    "id, at, actor_key_id, actor_name, actor_kind, method, path, status, request_id, ip, body";

#[derive(Clone)]
pub struct AuditRepo {
    pg: PgPool,
}

impl AuditRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn insert(&self, e: &NewAuditEntry) -> Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO audit_log (id, actor_key_id, actor_name, actor_kind, method, path, status, request_id, ip, body) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(id)
        .bind(e.actor_key_id)
        .bind(&e.actor_name)
        .bind(&e.actor_kind)
        .bind(&e.method)
        .bind(&e.path)
        .bind(e.status)
        .bind(&e.request_id)
        .bind(&e.ip)
        .bind(&e.body)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(id)
    }

    /// Newest first; `after` is the id of the last row of the previous page (ids are UUIDv7, time ordered).
    pub async fn list(&self, f: &AuditFilter) -> Result<Vec<AuditEntry>> {
        let path_like = f.path_prefix.as_ref().map(|p| format!("{p}%"));
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM audit_log \
             WHERE ($1::uuid IS NULL OR id < $1) \
               AND ($2::text IS NULL OR actor_name = $2) \
               AND ($3::text IS NULL OR method = $3) \
               AND ($4::text IS NULL OR path LIKE $4) \
               AND ($5::timestamptz IS NULL OR at >= $5) \
             ORDER BY id DESC LIMIT $6"
        ))
        .bind(f.after)
        .bind(&f.actor)
        .bind(&f.method)
        .bind(path_like)
        .bind(f.since)
        .bind(f.limit.clamp(1, 500))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn prune(&self, before: DateTime<Utc>) -> Result<u64> {
        let r = sqlx::query("DELETE FROM audit_log WHERE at < $1")
            .bind(before)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(r.rows_affected())
    }
}
