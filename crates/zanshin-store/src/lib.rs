//! Persistence layer. PostgreSQL is the system of record, Redis the shared cache /
//! pub-sub bus, ClickHouse the (optional) analytics event sink.

pub mod analytics;
pub mod analytics_v5;
pub mod api_keys;
pub mod audiences;
pub mod audit;
pub mod cache;
pub mod clickhouse_sink;
pub mod console_users;
pub mod crypto;
pub mod currency;
pub mod events;
pub mod experiments;
pub mod export;
pub mod flags;
pub mod flags_v3;
pub mod holdouts;
pub mod identities;
pub mod integrations;
pub mod journeys;
pub mod live_events;
pub mod locks;
pub mod messages;
pub mod metrics;
pub mod notifications;
pub mod projects;
pub mod push;
pub mod rewrap;
pub mod sessions;
pub mod taxonomy;

use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::str::FromStr;
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
    /// Events older than this are expired by ClickHouse TTL (0 = keep forever).
    #[serde(default = "default_ch_retention_days")]
    pub retention_days: u32,
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
fn default_ch_retention_days() -> u32 {
    400
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
        Self::connect_project(db, redis, ch, None).await
    }

    /// PostgreSQL schema that holds a project's tables (`None` = the default project in `public`).
    pub fn schema_for(project: Option<&str>) -> Option<String> {
        project
            .filter(|p| !p.is_empty() && *p != "default")
            .map(|p| format!("zanshin_{p}"))
    }

    /// Connect the stores of one project: its own PostgreSQL schema (`zanshin_<slug>`, created and
    /// migrated here), Redis key prefix `<prefix>:<slug>` and ClickHouse database `<db>_<slug>`.
    /// `None` / `"default"` is the original single-tenant layout.
    pub async fn connect_project(
        db: &DatabaseConfig,
        redis: &RedisConfig,
        ch: &ClickHouseConfig,
        project: Option<&str>,
    ) -> Result<Self> {
        let schema = Self::schema_for(project);
        let slug = project.filter(|_| schema.is_some());
        let mut opts =
            sqlx::postgres::PgConnectOptions::from_str(&db.url).context("parsing postgres url")?;
        if let Some(schema) = &schema {
            let base = PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(Duration::from_secs(10))
                .connect(&db.url)
                .await
                .context("connecting to postgres")?;
            sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS \"{schema}\""))
                .execute(&base)
                .await
                .context("creating project schema")?;
            base.close().await;
            opts = opts.options([("search_path", schema.as_str())]);
        }
        let pg = PgPoolOptions::new()
            .max_connections(db.max_connections)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(opts)
            .await
            .context("connecting to postgres")?;
        tracing::info!(project = slug.unwrap_or("default"), "connected to postgres");

        if db.migrate {
            sqlx::migrate!("../../migrations")
                .run(&pg)
                .await
                .context("running migrations")?;
            tracing::info!("database migrations applied");
        }

        let mut cache = cache::Cache::connect(redis).await?;
        if let Some(slug) = slug {
            cache = cache.with_prefix(format!("{}:{slug}", redis.key_prefix));
        }
        tracing::info!("connected to redis");

        let clickhouse = if ch.enabled {
            let mut ch_cfg = ch.clone();
            if let Some(slug) = slug {
                ch_cfg.database = format!("{}_{slug}", ch.database);
            }
            let c = clickhouse_sink::ClickHouse::connect(&ch_cfg).await?;
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

    /// Apply pending migrations without starting anything else (`zanshin --migrate`).
    pub async fn migrate_only(db: &DatabaseConfig) -> Result<()> {
        let pg = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(10))
            .connect(&db.url)
            .await
            .context("connecting to postgres")?;
        sqlx::migrate!("../../migrations")
            .run(&pg)
            .await
            .context("running migrations")?;
        Ok(())
    }

    /// Migrations bundled in this binary that the database has not applied yet (empty = up to date).
    pub async fn pending_migrations(db: &DatabaseConfig) -> Result<Vec<String>> {
        let pg = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .connect(&db.url)
            .await
            .context("connecting to postgres")?;
        let applied: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations")
            .fetch_all(&pg)
            .await
            .unwrap_or_default();
        Ok(sqlx::migrate!("../../migrations")
            .iter()
            .filter(|m| !applied.contains(&m.version))
            .map(|m| format!("{} {}", m.version, m.description))
            .collect())
    }

    /// Raw facts for `zanshin doctor`: database clock, configured integrations
    /// `(key, kind, enabled)` and the number of active console users.
    pub async fn doctor_facts(
        &self,
    ) -> Result<(
        chrono::DateTime<chrono::Utc>,
        Vec<(String, String, bool)>,
        i64,
    )> {
        let now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT now()")
            .fetch_one(&self.pg)
            .await
            .map_err(map_sqlx)?;
        let integrations: Vec<(String, String, bool)> =
            sqlx::query_as("SELECT key, kind, enabled FROM integrations ORDER BY key")
                .fetch_all(&self.pg)
                .await
                .map_err(map_sqlx)?;
        let users: i64 =
            sqlx::query_scalar("SELECT count(*) FROM console_users WHERE NOT disabled")
                .fetch_one(&self.pg)
                .await
                .map_err(map_sqlx)?;
        Ok((now, integrations, users))
    }

    /// Per-backend readiness report; `ok` is false when any configured backend fails.
    pub async fn health(&self) -> Health {
        let started = std::time::Instant::now();
        let pg = sqlx::query("SELECT 1")
            .execute(&self.pg)
            .await
            .map(|_| ())
            .map_err(|e| map_sqlx(e).to_string());
        let pg_ms = started.elapsed().as_millis() as u64;
        let started = std::time::Instant::now();
        let redis = self.cache.ping().await.map_err(|e| e.to_string());
        let redis_ms = started.elapsed().as_millis() as u64;
        let clickhouse = match &self.clickhouse {
            Some(ch) => {
                let started = std::time::Instant::now();
                let r = ch.ping().await.map_err(|e| e.to_string());
                Some(Component::from(r, started.elapsed().as_millis() as u64))
            }
            None => None,
        };
        let pg = Component::from(pg, pg_ms);
        let redis = Component::from(redis, redis_ms);
        let ok = pg.ok && redis.ok && clickhouse.as_ref().map(|c| c.ok).unwrap_or(true);
        Health {
            ok,
            postgres: pg,
            redis,
            clickhouse,
            pool: PoolStats {
                size: self.pg.size(),
                idle: self.pg.num_idle() as u32,
            },
        }
    }

    /// [`Self::health`] bounded to 5s so a hung backend cannot stall the probe.
    pub async fn ping_timeout(&self) -> Health {
        match tokio::time::timeout(Duration::from_secs(5), self.health()).await {
            Ok(h) => h,
            Err(_) => Health {
                ok: false,
                postgres: Component::from(Err("probe timed out".into()), 5000),
                redis: Component::from(Err("probe timed out".into()), 5000),
                clickhouse: None,
                pool: PoolStats {
                    size: self.pg.size(),
                    idle: self.pg.num_idle() as u32,
                },
            },
        }
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct Component {
    pub ok: bool,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Component {
    fn from(r: std::result::Result<(), String>, latency_ms: u64) -> Self {
        Self {
            ok: r.is_ok(),
            latency_ms,
            error: r.err(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PoolStats {
    pub size: u32,
    pub idle: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Health {
    pub ok: bool,
    pub postgres: Component,
    pub redis: Component,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clickhouse: Option<Component>,
    pub pool: PoolStats,
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
