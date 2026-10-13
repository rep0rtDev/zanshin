//! Analytics event sink and queries: ClickHouse when enabled, PostgreSQL otherwise.

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;
use zanshin_core::{events::Event, Error, Result};

use crate::{clickhouse_sink::ClickHouse, map_sqlx};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Hour,
    Day,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bucket {
    pub bucket: DateTime<Utc>,
    pub count: i64,
    pub identities: i64,
    pub sum: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NameCount {
    pub name: String,
    pub count: i64,
    pub identities: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VariantMetric {
    pub variant: String,
    pub count: i64,
    pub identities: i64,
    pub sum: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredEvent {
    pub id: Uuid,
    pub event_time: DateTime<Utc>,
    pub name: String,
    pub value: String,
    pub metadata: serde_json::Value,
    pub experiments: serde_json::Value,
}

#[derive(Row, Serialize)]
struct ChRow<'a> {
    #[serde(with = "clickhouse::serde::uuid")]
    id: Uuid,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    event_time: DateTime<Utc>,
    #[serde(with = "clickhouse::serde::uuid")]
    identity_id: Uuid,
    external_id: &'a str,
    name: &'a str,
    value: &'a str,
    metadata: String,
    #[serde(with = "clickhouse::serde::uuid")]
    session_id: Uuid,
    experiments: Vec<(&'a str, &'a str)>,
}

#[derive(Row, Deserialize)]
struct ChBucket {
    #[serde(with = "clickhouse::serde::chrono::datetime")]
    bucket: DateTime<Utc>,
    count: u64,
    identities: u64,
    sum: f64,
}

#[derive(Row, Deserialize)]
struct ChName {
    name: String,
    count: u64,
    identities: u64,
}

#[derive(Row, Deserialize)]
struct ChVariant {
    variant: String,
    count: u64,
    identities: u64,
    sum: f64,
}

#[derive(Row, Deserialize)]
struct ChEvent {
    #[serde(with = "clickhouse::serde::uuid")]
    id: Uuid,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    event_time: DateTime<Utc>,
    name: String,
    value: String,
    metadata: String,
    experiments: Vec<(String, String)>,
}

#[derive(Clone)]
pub enum EventStore {
    ClickHouse(Box<ClickHouse>),
    Postgres(PgPool),
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ':' | '$'))
    {
        return Err(Error::invalid("invalid event name"));
    }
    Ok(())
}

fn ch_err(e: clickhouse::error::Error) -> Error {
    Error::internal(anyhow::anyhow!("clickhouse: {e}"))
}

impl EventStore {
    pub fn backend(&self) -> &'static str {
        match self {
            EventStore::ClickHouse(_) => "clickhouse",
            EventStore::Postgres(_) => "postgres",
        }
    }

    pub async fn insert_batch(&self, events: &[Event]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        match self {
            EventStore::ClickHouse(ch) => {
                let mut insert = ch.client.insert("events").map_err(ch_err)?;
                for e in events {
                    let row = ChRow {
                        id: e.id,
                        event_time: e.event_time,
                        identity_id: e.identity_id,
                        external_id: &e.external_id,
                        name: &e.name,
                        value: &e.value,
                        metadata: if e.metadata.is_null() {
                            String::new()
                        } else {
                            e.metadata.to_string()
                        },
                        session_id: e.session_id.unwrap_or(Uuid::nil()),
                        experiments: e
                            .experiments
                            .iter()
                            .map(|(k, v)| (k.as_str(), v.as_str()))
                            .collect(),
                    };
                    insert.write(&row).await.map_err(ch_err)?;
                }
                insert.end().await.map_err(ch_err)
            }
            EventStore::Postgres(pg) => {
                let mut qb = sqlx::QueryBuilder::new("INSERT INTO events (id, event_time, identity_id, external_id, name, value, metadata, session_id, experiments) ");
                qb.push_values(events, |mut b, e| {
                    let experiments: serde_json::Map<String, serde_json::Value> = e
                        .experiments
                        .iter()
                        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                        .collect();
                    b.push_bind(e.id)
                        .push_bind(e.event_time)
                        .push_bind(e.identity_id)
                        .push_bind(&e.external_id)
                        .push_bind(&e.name)
                        .push_bind(&e.value)
                        .push_bind(&e.metadata)
                        .push_bind(e.session_id)
                        .push_bind(serde_json::Value::Object(experiments));
                });
                qb.push(" ON CONFLICT (id) DO NOTHING");
                qb.build().execute(pg).await.map_err(map_sqlx)?;
                Ok(())
            }
        }
    }

    pub async fn timeseries(
        &self,
        name: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        granularity: Granularity,
    ) -> Result<Vec<Bucket>> {
        validate_name(name)?;
        match self {
            EventStore::ClickHouse(ch) => {
                let trunc = match granularity {
                    Granularity::Hour => "toStartOfHour",
                    Granularity::Day => "toStartOfDay",
                };
                let rows: Vec<ChBucket> = ch
                    .client
                    .query(&format!(
                        "SELECT {trunc}(event_time) AS bucket, count() AS count, uniqExact(identity_id) AS identities,
                                sum(toFloat64OrZero(value)) AS sum
                         FROM events WHERE name = ? AND event_time >= ? AND event_time < ? GROUP BY bucket ORDER BY bucket"
                    ))
                    .bind(name)
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows
                    .into_iter()
                    .map(|r| Bucket {
                        bucket: r.bucket,
                        count: r.count as i64,
                        identities: r.identities as i64,
                        sum: r.sum,
                    })
                    .collect())
            }
            EventStore::Postgres(pg) => {
                let unit = match granularity {
                    Granularity::Hour => "hour",
                    Granularity::Day => "day",
                };
                let rows: Vec<(DateTime<Utc>, i64, i64, f64)> = sqlx::query_as(
                    "SELECT date_trunc($4, event_time) AS bucket, count(*)::bigint, count(DISTINCT identity_id)::bigint,
                            COALESCE(sum(CASE WHEN value ~ '^-?[0-9]+(\\.[0-9]+)?$' THEN value::float8 ELSE 0 END), 0)::float8
                     FROM events WHERE name = $1 AND event_time >= $2 AND event_time < $3 GROUP BY bucket ORDER BY bucket",
                )
                .bind(name)
                .bind(from)
                .bind(to)
                .bind(unit)
                .fetch_all(pg)
                .await
                .map_err(map_sqlx)?;
                Ok(rows
                    .into_iter()
                    .map(|(bucket, count, identities, sum)| Bucket {
                        bucket,
                        count,
                        identities,
                        sum,
                    })
                    .collect())
            }
        }
    }

    pub async fn top_names(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<NameCount>> {
        let limit = limit.clamp(1, 500);
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChName> = ch
                    .client
                    .query("SELECT name, count() AS count, uniqExact(identity_id) AS identities FROM events WHERE event_time >= ? AND event_time < ? GROUP BY name ORDER BY count DESC LIMIT ?")
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .bind(limit as u64)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows
                    .into_iter()
                    .map(|r| NameCount {
                        name: r.name,
                        count: r.count as i64,
                        identities: r.identities as i64,
                    })
                    .collect())
            }
            EventStore::Postgres(pg) => {
                let rows: Vec<(String, i64, i64)> = sqlx::query_as(
                    "SELECT name, count(*)::bigint, count(DISTINCT identity_id)::bigint FROM events WHERE event_time >= $1 AND event_time < $2 GROUP BY name ORDER BY 2 DESC LIMIT $3",
                )
                .bind(from)
                .bind(to)
                .bind(limit)
                .fetch_all(pg)
                .await
                .map_err(map_sqlx)?;
                Ok(rows
                    .into_iter()
                    .map(|(name, count, identities)| NameCount {
                        name,
                        count,
                        identities,
                    })
                    .collect())
            }
        }
    }

    /// Per-variant totals of `metric` events tagged with `experiment`.
    pub async fn experiment_metric(
        &self,
        experiment: &str,
        metric: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<VariantMetric>> {
        validate_name(metric)?;
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChVariant> = ch
                    .client
                    .query(
                        "SELECT experiments[?] AS variant, count() AS count, uniqExact(identity_id) AS identities, sum(toFloat64OrZero(value)) AS sum
                         FROM events WHERE name = ? AND mapContains(experiments, ?) AND event_time >= ? AND event_time < ? GROUP BY variant ORDER BY variant",
                    )
                    .bind(experiment)
                    .bind(metric)
                    .bind(experiment)
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows
                    .into_iter()
                    .map(|r| VariantMetric {
                        variant: r.variant,
                        count: r.count as i64,
                        identities: r.identities as i64,
                        sum: r.sum,
                    })
                    .collect())
            }
            EventStore::Postgres(pg) => {
                let rows: Vec<(String, i64, i64, f64)> = sqlx::query_as(
                    "SELECT experiments->>$1 AS variant, count(*)::bigint, count(DISTINCT identity_id)::bigint,
                            COALESCE(sum(CASE WHEN value ~ '^-?[0-9]+(\\.[0-9]+)?$' THEN value::float8 ELSE 0 END), 0)::float8
                     FROM events WHERE name = $2 AND experiments ? $1 AND event_time >= $3 AND event_time < $4 GROUP BY 1 ORDER BY 1",
                )
                .bind(experiment)
                .bind(metric)
                .bind(from)
                .bind(to)
                .fetch_all(pg)
                .await
                .map_err(map_sqlx)?;
                Ok(rows
                    .into_iter()
                    .map(|(variant, count, identities, sum)| VariantMetric {
                        variant,
                        count,
                        identities,
                        sum,
                    })
                    .collect())
            }
        }
    }

    /// Most recent events of one identity.
    pub async fn recent_for_identity(
        &self,
        identity_id: Uuid,
        limit: i64,
    ) -> Result<Vec<StoredEvent>> {
        let limit = limit.clamp(1, 500);
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChEvent> = ch
                    .client
                    .query("SELECT id, event_time, name, value, metadata, experiments FROM events WHERE identity_id = ? ORDER BY event_time DESC LIMIT ?")
                    .bind(identity_id)
                    .bind(limit as u64)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows
                    .into_iter()
                    .map(|r| StoredEvent {
                        id: r.id,
                        event_time: r.event_time,
                        name: r.name,
                        value: r.value,
                        metadata: serde_json::from_str(&r.metadata)
                            .unwrap_or(serde_json::Value::Null),
                        experiments: serde_json::Value::Object(
                            r.experiments
                                .into_iter()
                                .map(|(k, v)| (k, serde_json::Value::String(v)))
                                .collect(),
                        ),
                    })
                    .collect())
            }
            EventStore::Postgres(pg) => {
                let rows: Vec<(Uuid, DateTime<Utc>, String, String, serde_json::Value, serde_json::Value)> =
                    sqlx::query_as("SELECT id, event_time, name, value, metadata, experiments FROM events WHERE identity_id = $1 ORDER BY event_time DESC LIMIT $2")
                        .bind(identity_id)
                        .bind(limit)
                        .fetch_all(pg)
                        .await
                        .map_err(map_sqlx)?;
                Ok(rows
                    .into_iter()
                    .map(
                        |(id, event_time, name, value, metadata, experiments)| StoredEvent {
                            id,
                            event_time,
                            name,
                            value,
                            metadata,
                            experiments,
                        },
                    )
                    .collect())
            }
        }
    }

    pub async fn delete_identity(&self, identity_id: Uuid) -> Result<()> {
        match self {
            EventStore::ClickHouse(ch) => ch
                .client
                .query("DELETE FROM events WHERE identity_id = ?")
                .bind(identity_id)
                .execute()
                .await
                .map_err(ch_err),
            EventStore::Postgres(pg) => {
                sqlx::query("DELETE FROM events WHERE identity_id = $1")
                    .bind(identity_id)
                    .execute(pg)
                    .await
                    .map_err(map_sqlx)?;
                Ok(())
            }
        }
    }
}
