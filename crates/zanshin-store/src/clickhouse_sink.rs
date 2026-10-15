//! ClickHouse connection used as the analytics sink. Schema is created on connect.

use anyhow::Context;
use clickhouse::Client;
use zanshin_core::{Error, Result};

use crate::ClickHouseConfig;

#[derive(Clone)]
pub struct ClickHouse {
    pub client: Client,
    pub database: String,
}

const SCHEMA: &[&str] = &[r#"CREATE TABLE IF NOT EXISTS events (
        event_time   DateTime64(3, 'UTC'),
        received_at  DateTime64(3, 'UTC') DEFAULT now64(3),
        identity_id  UUID,
        external_id  String,
        name         LowCardinality(String),
        value        String,
        metadata     String CODEC(ZSTD(3)),
        session_id   UUID,
        experiments  Map(LowCardinality(String), LowCardinality(String))
    ) ENGINE = MergeTree
      PARTITION BY toYYYYMM(event_time)
      ORDER BY (name, event_time, identity_id)
      TTL toDateTime(event_time) + INTERVAL 400 DAY"#,
    "ALTER TABLE events ADD COLUMN IF NOT EXISTS experiments Map(LowCardinality(String), LowCardinality(String))",
    "ALTER TABLE events ADD COLUMN IF NOT EXISTS id UUID DEFAULT generateUUIDv4()",
    "ALTER TABLE events ADD COLUMN IF NOT EXISTS country LowCardinality(String) DEFAULT ''",
    "ALTER TABLE events ADD COLUMN IF NOT EXISTS platform LowCardinality(String) DEFAULT ''",
    "ALTER TABLE events ADD COLUMN IF NOT EXISTS app_version LowCardinality(String) DEFAULT ''"];

impl ClickHouse {
    pub async fn connect(cfg: &ClickHouseConfig) -> Result<Self> {
        let base = Client::default()
            .with_url(&cfg.url)
            .with_user(&cfg.user)
            .with_password(&cfg.password);
        base.query(&format!("CREATE DATABASE IF NOT EXISTS `{}`", cfg.database))
            .execute()
            .await
            .context("creating clickhouse database")?;
        let client = base.with_database(&cfg.database);
        for ddl in SCHEMA {
            client
                .query(ddl)
                .execute()
                .await
                .context("applying clickhouse schema")?;
        }
        let ttl = if cfg.retention_days == 0 {
            "ALTER TABLE events REMOVE TTL".to_string()
        } else {
            format!(
                "ALTER TABLE events MODIFY TTL toDateTime(event_time) + INTERVAL {} DAY",
                cfg.retention_days
            )
        };
        client
            .query(&ttl)
            .execute()
            .await
            .context("applying clickhouse retention")?;
        Ok(Self {
            client,
            database: cfg.database.clone(),
        })
    }

    pub async fn ping(&self) -> Result<()> {
        self.client
            .query("SELECT 1")
            .execute()
            .await
            .map_err(|_| Error::unavailable("clickhouse unavailable"))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub types: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub rows_read: u64,
    pub elapsed_secs: f64,
    pub truncated: bool,
}

#[derive(serde::Deserialize)]
struct JsonCompact {
    #[serde(default)]
    meta: Vec<Meta>,
    #[serde(default)]
    data: Vec<Vec<serde_json::Value>>,
    #[serde(default)]
    rows_before_limit_at_least: Option<u64>,
    #[serde(default)]
    statistics: Option<Stats>,
}
#[derive(serde::Deserialize)]
struct Meta {
    name: String,
    #[serde(rename = "type")]
    ty: String,
}
#[derive(serde::Deserialize)]
struct Stats {
    #[serde(default)]
    elapsed: f64,
    #[serde(default)]
    rows_read: u64,
}

impl ClickHouse {
    /// Runs an already-validated read-only statement with `readonly=2` (no writes or DDL; `1` would
    /// reject the row/time limits because HTTP sessions already run with readonly=2), a row cap
    /// and a time limit.
    pub async fn sql_readonly(
        &self,
        sql: &str,
        max_rows: u64,
        timeout_secs: u64,
    ) -> Result<SqlResult> {
        let mut cursor = self
            .client
            .query(sql)
            .with_option("readonly", "2")
            .with_option("max_result_rows", max_rows.to_string())
            .with_option("result_overflow_mode", "break")
            .with_option("max_execution_time", timeout_secs.to_string())
            .with_option("max_memory_usage", "2000000000")
            .with_option("output_format_json_quote_64bit_integers", "0")
            .fetch_bytes("JSONCompact")
            .map_err(|e| Error::invalid(format!("clickhouse: {e}")))?;
        let bytes = cursor
            .collect()
            .await
            .map_err(|e| Error::invalid(format!("clickhouse: {e}")))?;
        let parsed: JsonCompact = serde_json::from_slice(&bytes)
            .map_err(|e| Error::internal(anyhow::anyhow!("clickhouse json: {e}")))?;
        let n = parsed.data.len() as u64;
        Ok(SqlResult {
            columns: parsed.meta.iter().map(|m| m.name.clone()).collect(),
            types: parsed.meta.iter().map(|m| m.ty.clone()).collect(),
            truncated: n >= max_rows || parsed.rows_before_limit_at_least.is_some_and(|r| r > n),
            rows: parsed.data,
            rows_read: parsed.statistics.as_ref().map(|s| s.rows_read).unwrap_or(0),
            elapsed_secs: parsed.statistics.as_ref().map(|s| s.elapsed).unwrap_or(0.0),
        })
    }
}
