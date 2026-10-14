use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
    integrations::IntegrationHealth,
    integrations::{Integration, IntegrationKind},
    Error, Result,
};

use crate::map_sqlx;

#[derive(FromRow)]
struct Row {
    id: Uuid,
    key: String,
    name: String,
    kind: String,
    config: Value,
    enabled: bool,
    last_status: Option<String>,
    last_error: Option<String>,
    last_at: Option<DateTime<Utc>>,
    sent_count: i64,
    failed_count: i64,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<Row> for Integration {
    type Error = Error;
    fn try_from(r: Row) -> Result<Self> {
        Ok(Integration {
            id: r.id,
            key: r.key,
            name: r.name,
            kind: r.kind.parse::<IntegrationKind>()?,
            config: r.config,
            enabled: r.enabled,
            health: IntegrationHealth {
                last_status: r.last_status,
                last_error: r.last_error,
                last_at: r.last_at,
                sent_count: r.sent_count,
                failed_count: r.failed_count,
            },
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const COLS: &str = "id, key, name, kind, config, enabled, last_status, last_error, last_at, sent_count, failed_count, version, created_at, updated_at";

#[derive(Clone)]
pub struct IntegrationRepo {
    pg: PgPool,
}

impl IntegrationRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<Integration>> {
        let rows: Vec<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM integrations ORDER BY key"))
                .fetch_all(&self.pg)
                .await
                .map_err(map_sqlx)?;
        rows.into_iter().map(Integration::try_from).collect()
    }

    pub async fn get(&self, key: &str) -> Result<Integration> {
        let row: Option<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM integrations WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(Integration::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("integrations {key:?} not found")))
    }

    pub async fn create(&self, m: &Integration) -> Result<Integration> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO integrations (id, key, name, kind, config, enabled) VALUES ($1, $2, $3, $4, $5, $6) RETURNING {COLS}"
        ))
        .bind(m.id)
        .bind(&m.key)
        .bind(&m.name)
        .bind(m.kind.as_str())
        .bind(&m.config)
        .bind(m.enabled)
        .fetch_one(&self.pg)
        .await
        .map_err(|err| match map_sqlx(err) {
            Error::Conflict(_) => Error::conflict(format!("integrations {:?} already exists", m.key)),
            other => other,
        })?;
        Integration::try_from(row)
    }

    pub async fn update(&self, m: &Integration) -> Result<Integration> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "UPDATE integrations SET name = $2, kind = $3, config = $4, enabled = $5, version = version + 1, updated_at = now() WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&m.key)
        .bind(&m.name)
        .bind(m.kind.as_str())
        .bind(&m.config)
        .bind(m.enabled)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Integration::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("integrations {:?} not found", m.key)))
    }

    /// Record the outcome of a delivery attempt. `failed` replaces `last_error`; a success keeps
    /// the previous error visible so operators can still see what went wrong last time.
    pub async fn record_outcome(&self, key: &str, status: &str, detail: &str) -> Result<()> {
        let failed = status == "failed";
        sqlx::query(
            "UPDATE integrations SET last_status = $2, last_at = now(),
                    last_error = CASE WHEN $3 THEN $4 ELSE last_error END,
                    sent_count = sent_count + CASE WHEN $2 = 'sent' THEN 1 ELSE 0 END,
                    failed_count = failed_count + CASE WHEN $3 THEN 1 ELSE 0 END
             WHERE key = $1",
        )
        .bind(key)
        .bind(status)
        .bind(failed)
        .bind(detail.chars().take(500).collect::<String>())
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM integrations WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!("integrations {key:?} not found")));
        }
        Ok(())
    }
}
