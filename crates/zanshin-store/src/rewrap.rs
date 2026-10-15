//! `zanshin --rewrap-secrets`: re-encrypt every stored secret under the active data key, in every
//! project schema. Idempotent; rows already sealed with the active key are left alone.

use std::fmt;

use crate::crypto;
use anyhow::Context;
use serde_json::Value;
use sqlx::{PgPool, Row};

#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub active_key: Option<String>,
    pub schemas: Vec<SchemaReport>,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct SchemaReport {
    pub schema: String,
    pub integrations: u64,
    pub export_sinks: u64,
    pub notification_channels: u64,
    pub webhooks: u64,
}

impl Report {
    pub fn total(&self) -> u64 {
        self.schemas
            .iter()
            .map(|s| s.integrations + s.export_sinks + s.notification_channels + s.webhooks)
            .sum()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.active_key {
            Some(k) => writeln!(f, "sealing with data key {k:?}")?,
            None => writeln!(f, "security.data_keys is not set — nothing to rewrap")?,
        }
        for s in &self.schemas {
            writeln!(
                f,
                "{:<24} integrations {:>4}  export sinks {:>4}  notification channels {:>4}  webhooks {:>4}",
                s.schema, s.integrations, s.export_sinks, s.notification_channels, s.webhooks
            )?;
        }
        write!(f, "{} row(s) rewrapped", self.total())
    }
}

/// Connect to `db_url` and rewrap everything (the keyring must already be installed).
pub async fn rewrap(db_url: &str) -> anyhow::Result<Report> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(db_url)
        .await
        .context("connecting to postgres")?;
    rewrap_pool(&pool).await
}

/// Rewrap the default schema and every `zanshin_<project>` schema reachable from `pool`.
pub async fn rewrap_pool(pool: &PgPool) -> anyhow::Result<Report> {
    let mut report = Report {
        active_key: crypto::active_kid(),
        schemas: Vec::new(),
    };
    if report.active_key.is_none() {
        return Ok(report);
    }
    let mut schemas = vec!["public".to_string()];
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(
        "SELECT schema_name FROM information_schema.schemata WHERE schema_name LIKE 'zanshin\\_%' ORDER BY 1",
    )
    .fetch_all(pool)
    .await?;
    schemas.extend(rows.iter().map(|r| r.get::<String, _>(0)));
    for schema in schemas {
        let mut conn = pool.acquire().await?;
        sqlx::query(&format!("SET search_path TO \"{schema}\""))
            .execute(&mut *conn)
            .await?;
        let has: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM information_schema.tables WHERE table_schema = $1 AND table_name = 'integrations'",
        )
        .bind(&schema)
        .fetch_optional(&mut *conn)
        .await?;
        if has.is_none() {
            continue;
        }
        let mut s = SchemaReport {
            schema: schema.clone(),
            ..Default::default()
        };
        s.integrations = rewrap_json(&mut conn, "integrations", "config").await?;
        s.export_sinks = rewrap_json(&mut conn, "export_sinks", "config").await?;
        s.notification_channels = rewrap_json(&mut conn, "notification_channels", "headers")
            .await?
            + rewrap_text(&mut conn, "notification_channels", "url").await?;
        s.webhooks = rewrap_text(&mut conn, "webhooks", "secret").await?;
        report.schemas.push(s);
    }
    Ok(report)
}

async fn rewrap_json(conn: &mut sqlx::PgConnection, table: &str, col: &str) -> anyhow::Result<u64> {
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!("SELECT id, {col} FROM {table}"))
        .fetch_all(&mut *conn)
        .await
        .with_context(|| format!("reading {table}.{col}"))?;
    let mut n = 0;
    for r in rows {
        let id: uuid::Uuid = r.get(0);
        let v: Value = r.get(1);
        if !crypto::value_needs_rewrap(&v) {
            continue;
        }
        let plain = crypto::open_value(&v).with_context(|| format!("{table} {id}"))?;
        sqlx::query(&format!("UPDATE {table} SET {col} = $2 WHERE id = $1"))
            .bind(id)
            .bind(crypto::seal_value(&plain))
            .execute(&mut *conn)
            .await?;
        n += 1;
    }
    Ok(n)
}

async fn rewrap_text(conn: &mut sqlx::PgConnection, table: &str, col: &str) -> anyhow::Result<u64> {
    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(&format!("SELECT id, {col} FROM {table}"))
        .fetch_all(&mut *conn)
        .await
        .with_context(|| format!("reading {table}.{col}"))?;
    let mut n = 0;
    for r in rows {
        let id: uuid::Uuid = r.get(0);
        let v: String = r.get(1);
        if !crypto::needs_rewrap(&v) {
            continue;
        }
        let plain = crypto::open_str(&v).with_context(|| format!("{table} {id}"))?;
        sqlx::query(&format!("UPDATE {table} SET {col} = $2 WHERE id = $1"))
            .bind(id)
            .bind(crypto::seal_str(&plain))
            .execute(&mut *conn)
            .await?;
        n += 1;
    }
    Ok(n)
}
