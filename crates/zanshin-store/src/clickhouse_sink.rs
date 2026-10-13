//! ClickHouse connection used as the analytics sink. Schema is created on connect.

use anyhow::Context;
use clickhouse::Client;
use zanshin_core::{Error, Result};

use crate::ClickHouseConfig;

#[derive(Clone)]
pub struct ClickHouse {
    pub client: Client,
}

const SCHEMA: &[&str] = &[r#"CREATE TABLE IF NOT EXISTS events (
        event_time   DateTime64(3, 'UTC'),
        received_at  DateTime64(3, 'UTC') DEFAULT now64(3),
        identity_id  UUID,
        external_id  String,
        name         LowCardinality(String),
        value        String,
        metadata     String CODEC(ZSTD(3)),
        session_id   UUID
    ) ENGINE = MergeTree
      PARTITION BY toYYYYMM(event_time)
      ORDER BY (name, event_time, identity_id)
      TTL toDateTime(event_time) + INTERVAL 400 DAY"#];

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
        Ok(Self { client })
    }

    pub async fn ping(&self) -> Result<()> {
        self.client
            .query("SELECT 1")
            .execute()
            .await
            .map_err(|_| Error::unavailable("clickhouse unavailable"))
    }
}
