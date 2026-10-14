use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
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
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const COLS: &str = "id, key, name, kind, config, enabled, version, created_at, updated_at";

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
