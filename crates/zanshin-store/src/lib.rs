//! Persistence layer. PostgreSQL is the system of record, Redis the shared cache /
//! pub-sub bus, ClickHouse the (optional) analytics event sink.

pub mod analytics;
pub mod api_keys;
pub mod audiences;
pub mod audit;
pub mod cache;
pub mod clickhouse_sink;
pub mod events;
pub mod experiments;
pub mod export;
pub mod flags;
pub mod identities;
pub mod integrations;
pub mod journeys;
pub mod live_events;
pub mod messages;
pub mod metrics;
pub mod push;
pub mod sessions;
pub mod taxonomy;

use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;
use sqlx::postgres::{PgPool, PgPoolOptions};
use zanshin_core::{Error, Result};

pub use sqlx;

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    #[serde(default = "default_max_conns")]
    pub max_connections: u32,
    #[serde(default = "default_true")]
    pub migrate: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub url: String,
    #[serde(default = "default_key_prefix")]
    pub key_prefix: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClickHouseConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ch_url")]
    pub url: String,
    #[serde(default = "default_ch_user")]
    pub user: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_ch_db")]
    pub database: String,
}

fn default_max_conns() -> u32 {
    16
}
fn default_true() -> bool {
    true
}
fn default_key_prefix() -> String {
    "zanshin".into()
}
fn default_ch_url() -> String {
    "http://localhost:8123".into()
}
fn default_ch_user() -> String {
    "default".into()
}
fn default_ch_db() -> String {
    "zanshin".into()
}

#[derive(Clone)]
pub struct Stores {
    pub pg: PgPool,
    pub cache: cache::Cache,
    pub clickhouse: Option<clickhouse_sink::ClickHouse>,
}

impl Stores {
    pub async fn connect(
        db: &DatabaseConfig,
        redis: &RedisConfig,
        ch: &ClickHouseConfig,
    ) -> Result<Self> {
        let pg = PgPoolOptions::new()
            .max_connections(db.max_connections)
            .acquire_timeout(Duration::from_secs(10))
            .connect(&db.url)
            .await
            .context("connecting to postgres")?;
        tracing::info!("connected to postgres");

        if db.migrate {
            sqlx::migrate!("../../migrations")
                .run(&pg)
                .await
                .context("running migrations")?;
            tracing::info!("database migrations applied");
        }

        let cache = cache::Cache::connect(redis).await?;
        tracing::info!("connected to redis");

        let clickhouse = if ch.enabled {
            let c = clickhouse_sink::ClickHouse::connect(ch).await?;
            tracing::info!(url = %ch.url, "connected to clickhouse");
            Some(c)
        } else {
            None
        };

        Ok(Self {
            pg,
            cache,
            clickhouse,
        })
    }

    /// Readiness probe: every configured backend must answer.
    pub async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        self.cache.ping().await?;
        if let Some(ch) = &self.clickhouse {
            ch.ping().await?;
        }
        Ok(())
    }
}

pub(crate) fn map_sqlx(e: sqlx::Error) -> Error {
    match &e {
        sqlx::Error::RowNotFound => Error::not_found("row not found"),
        sqlx::Error::Database(db) if db.is_unique_violation() => Error::conflict("already exists"),
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) => {
            Error::unavailable("database unavailable")
        }
        _ => Error::internal(e),
    }
}
pub mod templates;
pub mod webhooks;
