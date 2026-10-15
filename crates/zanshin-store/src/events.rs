//! Analytics event sink and queries: ClickHouse when enabled, PostgreSQL otherwise.

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;
use zanshin_core::{analytics::EventFilter, events::Event, Error, Result};

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
pub struct VariantDayMetric {
    pub variant: String,
    pub day: String,
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
    country: &'a str,
    platform: &'a str,
    app_version: &'a str,
}

/// One event as written to an export sink.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExportedEvent {
    pub id: Uuid,
    pub event_time: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
    pub identity_id: Uuid,
    pub external_id: String,
    pub name: String,
    pub value: String,
    pub metadata: Value,
    pub session_id: Option<Uuid>,
    pub experiments: Value,
}

#[derive(Row, Deserialize)]
struct ChExport {
    #[serde(with = "clickhouse::serde::uuid")]
    id: Uuid,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    event_time: DateTime<Utc>,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    received_at: DateTime<Utc>,
    #[serde(with = "clickhouse::serde::uuid")]
    identity_id: Uuid,
    external_id: String,
    name: String,
    value: String,
    metadata: String,
    #[serde(with = "clickhouse::serde::uuid")]
    session_id: Uuid,
    experiments: Vec<(String, String)>,
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
struct ChIdentityValue {
    #[serde(with = "clickhouse::serde::uuid")]
    identity_id: Uuid,
    variant: String,
    count: u64,
    sum: f64,
}

/// One player's contribution to a metric: event count and summed value (or metadata field).
#[derive(Debug, Clone)]
pub struct IdentityMetric {
    pub identity_id: Uuid,
    pub variant: String,
    pub count: i64,
    pub sum: f64,
}

#[derive(Row, Deserialize)]
struct ChVariantDay {
    variant: String,
    day: String,
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

/// Constant-shape filter clauses: every dimension is always bound (empty = no restriction), so
/// the SQL text never changes with the filter. ClickHouse binds positionally (`?`), Postgres by
/// number starting at `$base`.
const CH_FILTER: &str = " AND (? = '' OR country = ?) AND (? = '' OR platform = ?) AND (? = '' OR app_version = ?) AND (? = '' OR experiments[?] = ?) AND (? = '1' OR NOT mapContains(experiments, '$test'))";

fn pg_filter(base: usize) -> String {
    let (c, p, v, e, x, t) = (base, base + 1, base + 2, base + 3, base + 4, base + 5);
    format!(" AND (${c} = '' OR country = ${c}) AND (${p} = '' OR platform = ${p}) AND (${v} = '' OR app_version = ${v}) AND (${e} = '' OR experiments->>${e} = ${x}) AND (${t} = '1' OR NOT (experiments ? '$test'))")
}

fn fparts(f: &EventFilter) -> [String; 6] {
    let g = |o: &Option<String>| o.clone().unwrap_or_default();
    [
        g(&f.country),
        g(&f.platform),
        g(&f.app_version),
        g(&f.experiment),
        g(&f.variant),
        if f.include_test {
            "1".into()
        } else {
            String::new()
        },
    ]
}

trait BindFilter: Sized {
    fn bind_filter(self, f: &EventFilter) -> Self;
}

impl BindFilter for clickhouse::query::Query {
    fn bind_filter(self, f: &EventFilter) -> Self {
        let [c, p, v, e, x, t] = fparts(f);
        self.bind(c.clone())
            .bind(c)
            .bind(p.clone())
            .bind(p)
            .bind(v.clone())
            .bind(v)
            .bind(e.clone())
            .bind(e)
            .bind(x)
            .bind(t)
    }
}

impl<'q, O> BindFilter
    for sqlx::query::QueryAs<'q, sqlx::Postgres, O, sqlx::postgres::PgArguments>
{
    fn bind_filter(self, f: &EventFilter) -> Self {
        let [c, p, v, e, x, t] = fparts(f);
        self.bind(c).bind(p).bind(v).bind(e).bind(x).bind(t)
    }
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
                        country: &e.country,
                        platform: &e.platform,
                        app_version: &e.app_version,
                    };
                    insert.write(&row).await.map_err(ch_err)?;
                }
                insert.end().await.map_err(ch_err)
            }
            EventStore::Postgres(pg) => {
                let mut qb = sqlx::QueryBuilder::new("INSERT INTO events (id, event_time, identity_id, external_id, name, value, metadata, session_id, experiments, country, platform, app_version) ");
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
                        .push_bind(serde_json::Value::Object(experiments))
                        .push_bind(&e.country)
                        .push_bind(&e.platform)
                        .push_bind(&e.app_version);
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
    /// Per-player metric totals for everyone tagged with `experiment` in the window (CUPED,
    /// per-unit statistics). `variant` is the tag value.
    pub async fn experiment_metric_by_identity(
        &self,
        experiment: &str,
        metric: &str,
        field: Option<&str>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<IdentityMetric>> {
        validate_name(metric)?;
        let field = field.unwrap_or("");
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChIdentityValue> = ch
                    .client
                    .query(
                        "SELECT identity_id, experiments[?] AS variant, count() AS count,
                                sum(if(? = '', toFloat64OrZero(value), toFloat64OrZero(JSONExtractRaw(metadata, ?)))) AS sum
                         FROM events WHERE name = ? AND mapContains(experiments, ?) AND event_time >= ? AND event_time < ?
                         GROUP BY identity_id, variant ORDER BY identity_id LIMIT ?",
                    )
                    .bind(experiment)
                    .bind(field)
                    .bind(field)
                    .bind(metric)
                    .bind(experiment)
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .bind(limit as u64)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows
                    .into_iter()
                    .map(|r| IdentityMetric {
                        identity_id: r.identity_id,
                        variant: r.variant,
                        count: r.count as i64,
                        sum: r.sum,
                    })
                    .collect())
            }
            EventStore::Postgres(pg) => {
                let rows: Vec<(Uuid, String, i64, f64)> = sqlx::query_as(
                    "SELECT identity_id, experiments->>$1 AS variant, count(*)::bigint,
                            COALESCE(sum(CASE WHEN (CASE WHEN $5 = '' THEN value ELSE metadata->>$5 END) ~ '^-?[0-9]+(\\.[0-9]+)?$'
                                              THEN (CASE WHEN $5 = '' THEN value ELSE metadata->>$5 END)::float8 ELSE 0 END), 0)::float8
                     FROM events WHERE name = $2 AND experiments ? $1 AND event_time >= $3 AND event_time < $4
                     GROUP BY 1, 2 ORDER BY 1 LIMIT $6",
                )
                .bind(experiment)
                .bind(metric)
                .bind(from)
                .bind(to)
                .bind(field)
                .bind(limit)
                .fetch_all(pg)
                .await
                .map_err(map_sqlx)?;
                Ok(rows
                    .into_iter()
                    .map(|(identity_id, variant, count, sum)| IdentityMetric {
                        identity_id,
                        variant,
                        count,
                        sum,
                    })
                    .collect())
            }
        }
    }

    /// Per-player totals of `metric` for the given players in a window, regardless of experiment
    /// tags — the pre-experiment covariate for CUPED. `variant` is empty.
    pub async fn metric_by_identity(
        &self,
        metric: &str,
        field: Option<&str>,
        identities: &[Uuid],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<IdentityMetric>> {
        validate_name(metric)?;
        if identities.is_empty() {
            return Ok(vec![]);
        }
        let field = field.unwrap_or("");
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChIdentityValue> = ch
                    .client
                    .query(
                        "SELECT identity_id, '' AS variant, count() AS count,
                                sum(if(? = '', toFloat64OrZero(value), toFloat64OrZero(JSONExtractRaw(metadata, ?)))) AS sum
                         FROM events WHERE name = ? AND identity_id IN ? AND event_time >= ? AND event_time < ?
                         GROUP BY identity_id",
                    )
                    .bind(field)
                    .bind(field)
                    .bind(metric)
                    .bind(identities)
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows
                    .into_iter()
                    .map(|r| IdentityMetric {
                        identity_id: r.identity_id,
                        variant: String::new(),
                        count: r.count as i64,
                        sum: r.sum,
                    })
                    .collect())
            }
            EventStore::Postgres(pg) => {
                let rows: Vec<(Uuid, i64, f64)> = sqlx::query_as(
                    "SELECT identity_id, count(*)::bigint,
                            COALESCE(sum(CASE WHEN (CASE WHEN $5 = '' THEN value ELSE metadata->>$5 END) ~ '^-?[0-9]+(\\.[0-9]+)?$'
                                              THEN (CASE WHEN $5 = '' THEN value ELSE metadata->>$5 END)::float8 ELSE 0 END), 0)::float8
                     FROM events WHERE name = $1 AND identity_id = ANY($2) AND event_time >= $3 AND event_time < $4
                     GROUP BY 1",
                )
                .bind(metric)
                .bind(identities)
                .bind(from)
                .bind(to)
                .bind(field)
                .fetch_all(pg)
                .await
                .map_err(map_sqlx)?;
                Ok(rows
                    .into_iter()
                    .map(|(identity_id, count, sum)| IdentityMetric {
                        identity_id,
                        variant: String::new(),
                        count,
                        sum,
                    })
                    .collect())
            }
        }
    }

    pub async fn experiment_metric(
        &self,
        experiment: &str,
        metric: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<VariantMetric>> {
        self.experiment_metric_field(experiment, metric, None, from, to)
            .await
    }

    /// Like [`experiment_metric`](Self::experiment_metric) but summing a metadata key instead of
    /// the event value when `field` is given.
    pub async fn experiment_metric_field(
        &self,
        experiment: &str,
        metric: &str,
        field: Option<&str>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<VariantMetric>> {
        validate_name(metric)?;
        let field = field.unwrap_or("");
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChVariant> = ch
                    .client
                    .query(
                        "SELECT experiments[?] AS variant, count() AS count, uniqExact(identity_id) AS identities,
                                sum(if(? = '', toFloat64OrZero(value), toFloat64OrZero(JSONExtractRaw(metadata, ?)))) AS sum
                         FROM events WHERE name = ? AND mapContains(experiments, ?) AND event_time >= ? AND event_time < ? GROUP BY variant ORDER BY variant",
                    )
                    .bind(experiment)
                    .bind(field)
                    .bind(field)
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
                            COALESCE(sum(CASE WHEN (CASE WHEN $5 = '' THEN value ELSE metadata->>$5 END) ~ '^-?[0-9]+(\\.[0-9]+)?$'
                                              THEN (CASE WHEN $5 = '' THEN value ELSE metadata->>$5 END)::float8 ELSE 0 END), 0)::float8
                     FROM events WHERE name = $2 AND experiments ? $1 AND event_time >= $3 AND event_time < $4 GROUP BY 1 ORDER BY 1",
                )
                .bind(experiment)
                .bind(metric)
                .bind(from)
                .bind(to)
                .bind(field)
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

    /// Per-variant, per-day (UTC) totals of `metric` events tagged with `experiment`.
    pub async fn experiment_metric_series(
        &self,
        experiment: &str,
        metric: &str,
        field: Option<&str>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<VariantDayMetric>> {
        validate_name(metric)?;
        let field = field.unwrap_or("");
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChVariantDay> = ch
                    .client
                    .query(
                        "SELECT experiments[?] AS variant, toString(toDate(event_time)) AS day, count() AS count, uniqExact(identity_id) AS identities,
                                sum(if(? = '', toFloat64OrZero(value), toFloat64OrZero(JSONExtractRaw(metadata, ?)))) AS sum
                         FROM events WHERE name = ? AND mapContains(experiments, ?) AND event_time >= ? AND event_time < ? GROUP BY variant, day ORDER BY day, variant",
                    )
                    .bind(experiment)
                    .bind(field)
                    .bind(field)
                    .bind(metric)
                    .bind(experiment)
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows
                    .into_iter()
                    .map(|r| VariantDayMetric {
                        variant: r.variant,
                        day: r.day,
                        count: r.count as i64,
                        identities: r.identities as i64,
                        sum: r.sum,
                    })
                    .collect())
            }
            EventStore::Postgres(pg) => {
                let rows: Vec<(String, String, i64, i64, f64)> = sqlx::query_as(
                    "SELECT experiments->>$1 AS variant, to_char(date_trunc('day', event_time), 'YYYY-MM-DD') AS day, count(*)::bigint, count(DISTINCT identity_id)::bigint,
                            COALESCE(sum(CASE WHEN (CASE WHEN $5 = '' THEN value ELSE metadata->>$5 END) ~ '^-?[0-9]+(\\.[0-9]+)?$'
                                              THEN (CASE WHEN $5 = '' THEN value ELSE metadata->>$5 END)::float8 ELSE 0 END), 0)::float8
                     FROM events WHERE name = $2 AND experiments ? $1 AND event_time >= $3 AND event_time < $4 GROUP BY 1, 2 ORDER BY 2, 1",
                )
                .bind(experiment)
                .bind(metric)
                .bind(from)
                .bind(to)
                .bind(field)
                .fetch_all(pg)
                .await
                .map_err(map_sqlx)?;
                Ok(rows
                    .into_iter()
                    .map(|(variant, day, count, identities, sum)| VariantDayMetric {
                        variant,
                        day,
                        count,
                        identities,
                        sum,
                    })
                    .collect())
            }
        }
    }

    /// Distinct active identities per UTC day.
    pub async fn active_per_day(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        filter: &EventFilter,
    ) -> Result<Vec<(String, i64)>> {
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChDay> = ch
                    .client
                    .query(&format!("SELECT toString(toDate(event_time)) AS day, uniqExact(identity_id) AS n FROM events WHERE event_time >= ? AND event_time < ?{CH_FILTER} GROUP BY day ORDER BY day"))
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .bind_filter(filter)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows.into_iter().map(|r| (r.day, r.n as i64)).collect())
            }
            EventStore::Postgres(pg) => sqlx::query_as(&format!(
                "SELECT to_char(date_trunc('day', event_time), 'YYYY-MM-DD'), count(DISTINCT identity_id)::bigint
                 FROM events WHERE event_time >= $1 AND event_time < $2{} GROUP BY 1 ORDER BY 1", pg_filter(3)
            ))
            .bind(from)
            .bind(to)
            .bind_filter(filter)
            .fetch_all(pg)
            .await
            .map_err(map_sqlx),
        }
    }

    /// Distinct identities active over the whole range (WAU / MAU).
    pub async fn active_in_range(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        filter: &EventFilter,
    ) -> Result<i64> {
        match self {
            EventStore::ClickHouse(ch) => {
                let n: u64 = ch
                    .client
                    .query(&format!("SELECT uniqExact(identity_id) FROM events WHERE event_time >= ? AND event_time < ?{CH_FILTER}"))
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .bind_filter(filter)
                    .fetch_one()
                    .await
                    .map_err(ch_err)?;
                Ok(n as i64)
            }
            EventStore::Postgres(pg) => {
                let (n,): (i64,) = sqlx::query_as(&format!("SELECT count(DISTINCT identity_id)::bigint FROM events WHERE event_time >= $1 AND event_time < $2{}", pg_filter(3)))
                    .bind(from)
                    .bind(to)
                    .bind_filter(filter)
                    .fetch_one(pg)
                    .await
                    .map_err(map_sqlx)?;
                Ok(n)
            }
        }
    }

    /// Revenue per UTC day over the given event names: sum(metadata.amount), paying identities, purchases.
    pub async fn revenue_per_day(
        &self,
        names: &[String],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        filter: &EventFilter,
    ) -> Result<Vec<(String, f64, i64, i64)>> {
        if names.is_empty() {
            return Ok(vec![]);
        }
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChRevenue> = ch
                    .client
                    .query(&format!("SELECT toString(toDate(event_time)) AS day, sum(toFloat64OrZero(JSONExtractRaw(metadata, 'amount'))) AS amount, uniqExact(identity_id) AS payers, count() AS purchases
                            FROM events WHERE name IN ? AND event_time >= ? AND event_time < ?{CH_FILTER} GROUP BY day ORDER BY day"))
                    .bind(names)
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .bind_filter(filter)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows.into_iter().map(|r| (r.day, r.amount, r.payers as i64, r.purchases as i64)).collect())
            }
            EventStore::Postgres(pg) => sqlx::query_as(&format!(
                "SELECT to_char(date_trunc('day', event_time), 'YYYY-MM-DD'),
                        COALESCE(sum(CASE WHEN metadata->>'amount' ~ '^-?[0-9]+(\\.[0-9]+)?$' THEN (metadata->>'amount')::float8 ELSE 0 END), 0)::float8,
                        count(DISTINCT identity_id)::bigint, count(*)::bigint
                 FROM events WHERE name = ANY($1) AND event_time >= $2 AND event_time < $3{} GROUP BY 1 ORDER BY 1", pg_filter(4)
            ))
            .bind(names)
            .bind(from)
            .bind(to)
            .bind_filter(filter)
            .fetch_all(pg)
            .await
            .map_err(map_sqlx),
        }
    }

    /// Raw rows for funnel folding: (identity, name, time, metadata), oldest first, capped.
    pub async fn funnel_rows(
        &self,
        names: &[String],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: i64,
        filter: &EventFilter,
    ) -> Result<Vec<FunnelRow>> {
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChFunnelRow> = ch
                    .client
                    .query(&format!("SELECT identity_id, name, toUnixTimestamp64Milli(event_time) AS ts, metadata, session_id FROM events WHERE name IN ? AND event_time >= ? AND event_time < ?{CH_FILTER} ORDER BY identity_id, event_time LIMIT ?"))
                    .bind(names)
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .bind_filter(filter)
                    .bind(limit)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let at = DateTime::<Utc>::from_timestamp_millis(r.ts).unwrap_or(from);
                        let session = (!r.session_id.is_nil()).then_some(r.session_id);
                        (r.identity_id, r.name, at, serde_json::from_str(&r.metadata).unwrap_or(Value::Null), session)
                    })
                    .collect())
            }
            EventStore::Postgres(pg) => sqlx::query_as(&format!(
                "SELECT identity_id, name, event_time, metadata, session_id FROM events WHERE name = ANY($1) AND event_time >= $2 AND event_time < $3{} ORDER BY identity_id, event_time LIMIT $4", pg_filter(5)
            ))
            .bind(names)
            .bind(from)
            .bind(to)
            .bind(limit)
            .bind_filter(filter)
            .fetch_all(pg)
            .await
            .map_err(map_sqlx),
        }
    }

    /// Every `(identity, name, unix_ms)` in the window, oldest first — path analysis input.
    pub async fn paths_rows(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<(Uuid, String, i64)>> {
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChPathRow> = ch
                    .client
                    .query("SELECT identity_id, name, toUnixTimestamp64Milli(event_time) AS ts FROM events WHERE event_time >= fromUnixTimestamp64Milli(?) AND event_time < fromUnixTimestamp64Milli(?) ORDER BY event_time LIMIT ?")
                    .bind(from.timestamp_millis())
                    .bind(to.timestamp_millis())
                    .bind(limit)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows
                    .into_iter()
                    .map(|r| (r.identity_id, r.name, r.ts))
                    .collect())
            }
            EventStore::Postgres(pg) => {
                let rows: Vec<(Uuid, String, DateTime<Utc>)> = sqlx::query_as(
                    "SELECT identity_id, name, event_time FROM events WHERE event_time >= $1 AND event_time < $2 ORDER BY event_time LIMIT $3",
                )
                .bind(from)
                .bind(to)
                .bind(limit)
                .fetch_all(pg)
                .await
                .map_err(map_sqlx)?;
                Ok(rows
                    .into_iter()
                    .map(|(id, name, at)| (id, name, at.timestamp_millis()))
                    .collect())
            }
        }
    }

    /// Single aggregate of one event over a window: `count`, `unique` identities or `sum` of a
    /// numeric metadata field — metric alerts input.
    pub async fn window_value(
        &self,
        name: &str,
        aggregation: &str,
        field: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<f64> {
        validate_name(name)?;
        match self {
            EventStore::ClickHouse(ch) => {
                let expr = match aggregation {
                    "unique" => "toFloat64(uniqExact(identity_id))".to_string(),
                    "sum" => {
                        if !field
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
                        {
                            return Err(Error::invalid("bad field"));
                        }
                        format!("sum(toFloat64OrZero(JSONExtractRaw(metadata, '{field}')))")
                    }
                    _ => "toFloat64(count())".to_string(),
                };
                let v: Vec<f64> = ch
                    .client
                    .query(&format!("SELECT {expr} FROM events WHERE name = ? AND event_time >= fromUnixTimestamp64Milli(?) AND event_time < fromUnixTimestamp64Milli(?)"))
                    .bind(name)
                    .bind(from.timestamp_millis())
                    .bind(to.timestamp_millis())
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(v.first().copied().unwrap_or(0.0))
            }
            EventStore::Postgres(pg) => {
                let expr = match aggregation {
                    "unique" => "count(DISTINCT identity_id)::float8",
                    "sum" => "coalesce(sum(CASE WHEN metadata->>$4 ~ '^-?[0-9]+(\\.[0-9]+)?$' THEN (metadata->>$4)::float8 ELSE 0 END), 0)",
                    _ => "count(*)::float8",
                };
                let v: (f64,) = sqlx::query_as(&format!(
                    "SELECT {expr} FROM events WHERE name = $1 AND event_time >= $2 AND event_time < $3 AND ($4 = $4)"
                ))
                .bind(name)
                .bind(from)
                .bind(to)
                .bind(field)
                .fetch_one(pg)
                .await
                .map_err(map_sqlx)?;
                Ok(v.0)
            }
        }
    }

    /// Distinct (identity, active day) pairs for the given identities — retention input.
    pub async fn active_days(
        &self,
        identities: &[Uuid],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        filter: &EventFilter,
    ) -> Result<Vec<(Uuid, String)>> {
        if identities.is_empty() {
            return Ok(vec![]);
        }
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChActiveDay> = ch
                    .client
                    .query(&format!("SELECT DISTINCT identity_id, toString(toDate(event_time)) AS day FROM events WHERE identity_id IN ? AND event_time >= ? AND event_time < ?{CH_FILTER}"))
                    .bind(identities)
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .bind_filter(filter)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows.into_iter().map(|r| (r.identity_id, r.day)).collect())
            }
            EventStore::Postgres(pg) => sqlx::query_as(&format!(
                "SELECT DISTINCT identity_id, to_char(date_trunc('day', event_time), 'YYYY-MM-DD') FROM events
                 WHERE identity_id = ANY($1) AND event_time >= $2 AND event_time < $3{}", pg_filter(4)
            ))
            .bind(identities)
            .bind(from)
            .bind(to)
            .bind_filter(filter)
            .fetch_all(pg)
            .await
            .map_err(map_sqlx),
        }
    }

    /// Most recent events of one identity.
    /// Events after the `(received_at, id)` cursor in arrival order, for export sinks.
    pub async fn export_batch(
        &self,
        after: Option<(DateTime<Utc>, Uuid)>,
        names: &[String],
        limit: i64,
    ) -> Result<Vec<ExportedEvent>> {
        let limit = limit.clamp(1, 100_000);
        let (at, id) = after.unwrap_or((
            DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default(),
            Uuid::nil(),
        ));
        match self {
            EventStore::ClickHouse(ch) => {
                let name_filter = if names.is_empty() {
                    ""
                } else {
                    " AND name IN ?"
                };
                let mut q = ch.client.query(&format!(
                    "SELECT id, event_time, received_at, identity_id, external_id, name, value, metadata, session_id, experiments
                     FROM events WHERE (received_at, id) > (?, ?){name_filter} ORDER BY received_at, id LIMIT ?"
                ))
                .bind(at.timestamp_millis() as f64 / 1000.0)
                .bind(id);
                if !names.is_empty() {
                    q = q.bind(names);
                }
                let rows: Vec<ChExport> = q.bind(limit as u64).fetch_all().await.map_err(ch_err)?;
                Ok(rows
                    .into_iter()
                    .map(|r| ExportedEvent {
                        id: r.id,
                        event_time: r.event_time,
                        received_at: r.received_at,
                        identity_id: r.identity_id,
                        external_id: r.external_id,
                        name: r.name,
                        value: r.value,
                        metadata: serde_json::from_str(&r.metadata).unwrap_or(Value::Null),
                        session_id: (!r.session_id.is_nil()).then_some(r.session_id),
                        experiments: Value::Object(
                            r.experiments
                                .into_iter()
                                .map(|(k, v)| (k, Value::String(v)))
                                .collect(),
                        ),
                    })
                    .collect())
            }
            EventStore::Postgres(pg) => {
                #[allow(clippy::type_complexity)]
                let rows: Vec<(Uuid, DateTime<Utc>, DateTime<Utc>, Uuid, String, String, String, Value, Option<Uuid>, Value)> = sqlx::query_as(
                    "SELECT id, event_time, received_at, identity_id, external_id, name, value, metadata, session_id, experiments
                     FROM events WHERE (received_at, id) > ($1, $2) AND (cardinality($3::text[]) = 0 OR name = ANY($3))
                     ORDER BY received_at, id LIMIT $4",
                )
                .bind(at)
                .bind(id)
                .bind(names)
                .bind(limit)
                .fetch_all(pg)
                .await
                .map_err(map_sqlx)?;
                Ok(rows
                    .into_iter()
                    .map(
                        |(
                            id,
                            event_time,
                            received_at,
                            identity_id,
                            external_id,
                            name,
                            value,
                            metadata,
                            session_id,
                            experiments,
                        )| ExportedEvent {
                            id,
                            event_time,
                            received_at,
                            identity_id,
                            external_id,
                            name,
                            value,
                            metadata,
                            session_id,
                            experiments,
                        },
                    )
                    .collect())
            }
        }
    }

    pub async fn recent_for_identity(
        &self,
        identity_id: Uuid,
        limit: i64,
    ) -> Result<Vec<StoredEvent>> {
        self.search_for_identity(identity_id, &EventSearch::default(), limit)
            .await
    }

    /// Player event history filtered by exact name, free-text match on value/metadata and time window.
    pub async fn search_for_identity(
        &self,
        identity_id: Uuid,
        search: &EventSearch,
        limit: i64,
    ) -> Result<Vec<StoredEvent>> {
        let limit = limit.clamp(1, 500);
        let name = search.name.clone().unwrap_or_default();
        let q: String = search
            .q
            .clone()
            .unwrap_or_default()
            .chars()
            .take(128)
            .collect();
        let from_ts = search.from.map(|t| t.timestamp()).unwrap_or(0);
        let to_ts = search.to.map(|t| t.timestamp()).unwrap_or(0);
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChEvent> = ch
                    .client
                    .query(
                        "SELECT id, event_time, name, value, metadata, experiments FROM events \
                         WHERE identity_id = ? AND (? = '' OR name = ?) \
                         AND (? = '' OR positionCaseInsensitive(concat(value, ' ', metadata), ?) > 0) \
                         AND (? = 0 OR event_time >= toDateTime64(?, 3)) \
                         AND (? = 0 OR event_time <= toDateTime64(?, 3)) \
                         ORDER BY event_time DESC LIMIT ?",
                    )
                    .bind(identity_id)
                    .bind(name.as_str())
                    .bind(name.as_str())
                    .bind(q.as_str())
                    .bind(q.as_str())
                    .bind(from_ts)
                    .bind(from_ts)
                    .bind(to_ts)
                    .bind(to_ts)
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
                let rows: Vec<(
                    Uuid,
                    DateTime<Utc>,
                    String,
                    String,
                    serde_json::Value,
                    serde_json::Value,
                )> = sqlx::query_as(
                    "SELECT id, event_time, name, value, metadata, experiments FROM events \
                         WHERE identity_id = $1 AND ($3::text = '' OR name = $3::text) \
                         AND ($4::text = '' OR (value || ' ' || metadata::text) ILIKE '%' || $4::text || '%') \
                         AND ($5::timestamptz IS NULL OR event_time >= $5) \
                         AND ($6::timestamptz IS NULL OR event_time <= $6) \
                         ORDER BY event_time DESC LIMIT $2",
                )
                .bind(identity_id)
                .bind(limit)
                .bind(&name)
                .bind(&q)
                .bind(search.from)
                .bind(search.to)
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

    /// Delete events older than `before` (optionally only the given names). Returns rows affected (0 on ClickHouse, which mutates asynchronously).
    pub async fn delete_before(&self, before: DateTime<Utc>, names: &[String]) -> Result<u64> {
        match self {
            EventStore::ClickHouse(ch) => {
                let filter = if names.is_empty() { "" } else { " AND name IN ?" };
                let mut q = ch
                    .client
                    .query(&format!("ALTER TABLE events DELETE WHERE event_time < ?{filter}"))
                    .bind(before.timestamp_millis() as f64 / 1000.0);
                if !names.is_empty() {
                    q = q.bind(names);
                }
                q.execute().await.map_err(ch_err)?;
                Ok(0)
            }
            EventStore::Postgres(pg) => Ok(sqlx::query(
                "DELETE FROM events WHERE event_time < $1 AND (cardinality($2::text[]) = 0 OR name = ANY($2))",
            )
            .bind(before)
            .bind(names)
            .execute(pg)
            .await
            .map_err(map_sqlx)?
            .rows_affected()),
        }
    }

    /// Retention purge in bounded batches (PostgreSQL only; ClickHouse expires rows by table TTL).
    pub async fn purge_before(&self, before: DateTime<Utc>, limit: u32) -> Result<u64> {
        match self {
            EventStore::ClickHouse(_) => Ok(0),
            EventStore::Postgres(pg) => Ok(sqlx::query(
                "DELETE FROM events WHERE id IN (SELECT id FROM events WHERE event_time < $1 ORDER BY event_time LIMIT $2)",
            )
            .bind(before)
            .bind(limit as i64)
            .execute(pg)
            .await
            .map_err(map_sqlx)?
            .rows_affected()),
        }
    }

    /// Distinct identities and events per country over the range (player map).
    pub async fn by_country(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        filter: &EventFilter,
    ) -> Result<Vec<(String, i64, i64)>> {
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChCountry> = ch
                    .client
                    .query(&format!("SELECT country, uniqExact(identity_id) AS identities, count() AS events FROM events WHERE event_time >= ? AND event_time < ?{CH_FILTER} GROUP BY country ORDER BY identities DESC LIMIT 300"))
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .bind_filter(filter)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows.into_iter().map(|r| (r.country, r.identities as i64, r.events as i64)).collect())
            }
            EventStore::Postgres(pg) => sqlx::query_as(&format!(
                "SELECT country, count(DISTINCT identity_id)::bigint, count(*)::bigint FROM events WHERE event_time >= $1 AND event_time < $2{} GROUP BY 1 ORDER BY 2 DESC LIMIT 300", pg_filter(3)
            ))
            .bind(from)
            .bind(to)
            .bind_filter(filter)
            .fetch_all(pg)
            .await
            .map_err(map_sqlx),
        }
    }

    /// Ad-hoc explorer: one event name, bucketed by day/hour and optionally split by a dimension
    /// (`country`, `platform`, `app_version`, `experiment:<key>`, `meta:<field>`). Returns
    /// (bucket, group, count, identities, sum(value), sum(metadata.field)).
    #[allow(clippy::too_many_arguments)]
    pub async fn explore(
        &self,
        name: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        granularity: Granularity,
        group_by: Option<&str>,
        sum_field: Option<&str>,
        filter: &EventFilter,
        limit: i64,
    ) -> Result<Vec<ExploreRow>> {
        validate_name(name)?;
        let ident = |s: &str| {
            s.len() <= 64
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ':' | '$'))
        };
        if let Some(f) = sum_field {
            if !ident(f) {
                return Err(Error::invalid("invalid metadata field"));
            }
        }
        let group_kind = match group_by {
            None | Some("") => GroupBy::None,
            Some("country") => GroupBy::Country,
            Some("platform") => GroupBy::Platform,
            Some("app_version") => GroupBy::AppVersion,
            Some(g) if g.starts_with("experiment:") && ident(&g[11..]) => {
                GroupBy::Experiment(g[11..].to_string())
            }
            Some(g) if g.starts_with("meta:") && ident(&g[5..]) => {
                GroupBy::Meta(g[5..].to_string())
            }
            Some(_) => return Err(Error::invalid(
                "group_by must be country, platform, app_version, experiment:<key> or meta:<field>",
            )),
        };
        let sum_field = sum_field.unwrap_or("amount");
        match self {
            EventStore::ClickHouse(ch) => {
                let trunc = match granularity {
                    Granularity::Hour => "toStartOfHour",
                    Granularity::Day => "toStartOfDay",
                };
                let grp = match &group_kind {
                    GroupBy::None => "''".to_string(),
                    GroupBy::Country => "country".into(),
                    GroupBy::Platform => "platform".into(),
                    GroupBy::AppVersion => "app_version".into(),
                    GroupBy::Experiment(k) => format!("experiments['{}']", k.replace('\'', "")),
                    GroupBy::Meta(f) => {
                        format!("JSONExtractString(metadata, '{}')", f.replace('\'', ""))
                    }
                };
                let rows: Vec<ChExplore> = ch
                    .client
                    .query(&format!(
                        "SELECT {trunc}(event_time) AS bucket, toString({grp}) AS grp, count() AS count, uniqExact(identity_id) AS identities,
                                sum(toFloat64OrZero(value)) AS sum, sum(toFloat64OrZero(JSONExtractRaw(metadata, '{sf}'))) AS field_sum
                         FROM events WHERE name = ? AND event_time >= ? AND event_time < ?{CH_FILTER}
                         GROUP BY bucket, grp ORDER BY bucket, count DESC LIMIT ?",
                        sf = sum_field.replace('\'', "")
                    ))
                    .bind(name)
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .bind_filter(filter)
                    .bind(limit)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        (
                            r.bucket,
                            r.grp,
                            r.count as i64,
                            r.identities as i64,
                            r.sum,
                            r.field_sum,
                        )
                    })
                    .collect())
            }
            EventStore::Postgres(pg) => {
                let trunc = match granularity {
                    Granularity::Hour => "hour",
                    Granularity::Day => "day",
                };
                let grp = match &group_kind {
                    GroupBy::None => "''".to_string(),
                    GroupBy::Country => "country".into(),
                    GroupBy::Platform => "platform".into(),
                    GroupBy::AppVersion => "app_version".into(),
                    GroupBy::Experiment(_) => "COALESCE(experiments->>$5, '')".into(),
                    GroupBy::Meta(_) => "COALESCE(metadata->>$5, '')".into(),
                };
                let key = match &group_kind {
                    GroupBy::Experiment(k) | GroupBy::Meta(k) => k.clone(),
                    _ => String::new(),
                };
                sqlx::query_as(&format!(
                    "SELECT date_trunc('{trunc}', event_time), {grp}::text, count(*)::bigint, count(DISTINCT identity_id)::bigint,
                            COALESCE(sum(CASE WHEN value ~ '^-?[0-9]+(\\.[0-9]+)?$' THEN value::float8 ELSE 0 END), 0)::float8,
                            COALESCE(sum(CASE WHEN metadata->>$6 ~ '^-?[0-9]+(\\.[0-9]+)?$' THEN (metadata->>$6)::float8 ELSE 0 END), 0)::float8
                     FROM events WHERE name = $1 AND event_time >= $2 AND event_time < $3{} GROUP BY 1, 2 ORDER BY 1, 3 DESC LIMIT $4",
                    pg_filter(7)
                ))
                .bind(name)
                .bind(from)
                .bind(to)
                .bind(limit)
                .bind(key)
                .bind(sum_field)
                .bind_filter(filter)
                .fetch_all(pg)
                .await
                .map_err(map_sqlx)
            }
        }
    }

    /// Revenue per (identity, UTC day) for the given identities — LTV input.
    pub async fn revenue_by_identity(
        &self,
        identities: &[Uuid],
        names: &[String],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<(Uuid, String, f64)>> {
        if identities.is_empty() || names.is_empty() {
            return Ok(vec![]);
        }
        match self {
            EventStore::ClickHouse(ch) => {
                let rows: Vec<ChIdentityRevenue> = ch
                    .client
                    .query("SELECT identity_id, toString(toDate(event_time)) AS day, sum(toFloat64OrZero(JSONExtractRaw(metadata, 'amount'))) AS amount FROM events WHERE identity_id IN ? AND name IN ? AND event_time >= ? AND event_time < ? GROUP BY identity_id, day")
                    .bind(identities)
                    .bind(names)
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .bind(to.timestamp_millis() as f64 / 1000.0)
                    .fetch_all()
                    .await
                    .map_err(ch_err)?;
                Ok(rows.into_iter().map(|r| (r.identity_id, r.day, r.amount)).collect())
            }
            EventStore::Postgres(pg) => sqlx::query_as(
                "SELECT identity_id, to_char(date_trunc('day', event_time), 'YYYY-MM-DD'),
                        COALESCE(sum(CASE WHEN metadata->>'amount' ~ '^-?[0-9]+(\\.[0-9]+)?$' THEN (metadata->>'amount')::float8 ELSE 0 END), 0)::float8
                 FROM events WHERE identity_id = ANY($1) AND name = ANY($2) AND event_time >= $3 AND event_time < $4 GROUP BY 1, 2",
            )
            .bind(identities)
            .bind(names)
            .bind(from)
            .bind(to)
            .fetch_all(pg)
            .await
            .map_err(map_sqlx),
        }
    }

    /// Total events and distinct identities since `from` (status board: events per minute).
    pub async fn count_since(&self, from: DateTime<Utc>) -> Result<(i64, i64)> {
        match self {
            EventStore::ClickHouse(ch) => {
                let (n, ids): (u64, u64) = ch
                    .client
                    .query("SELECT count(), uniqExact(identity_id) FROM events WHERE event_time >= ?")
                    .bind(from.timestamp_millis() as f64 / 1000.0)
                    .fetch_one()
                    .await
                    .map_err(ch_err)?;
                Ok((n as i64, ids as i64))
            }
            EventStore::Postgres(pg) => sqlx::query_as("SELECT count(*)::bigint, count(DISTINCT identity_id)::bigint FROM events WHERE event_time >= $1")
                .bind(from)
                .fetch_one(pg)
                .await
                .map_err(map_sqlx),
        }
    }

    pub async fn delete_identity(&self, identity_id: Uuid) -> Result<()> {
        match self {
            EventStore::ClickHouse(ch) => ch
                .client
                .query("DELETE FROM events WHERE identity_id = ? AND name != '$identity_delete'")
                .bind(identity_id)
                .execute()
                .await
                .map_err(ch_err),
            EventStore::Postgres(pg) => {
                sqlx::query(
                    "DELETE FROM events WHERE identity_id = $1 AND name <> '$identity_delete'",
                )
                .bind(identity_id)
                .execute(pg)
                .await
                .map_err(map_sqlx)?;
                Ok(())
            }
        }
    }
}

#[derive(Row, Deserialize)]
struct ChDay {
    day: String,
    n: u64,
}

#[derive(Row, Deserialize)]
struct ChRevenue {
    day: String,
    amount: f64,
    payers: u64,
    purchases: u64,
}

#[derive(Row, Deserialize)]
struct ChPathRow {
    #[serde(with = "clickhouse::serde::uuid")]
    identity_id: Uuid,
    name: String,
    ts: i64,
}

#[derive(Row, Deserialize)]
struct ChFunnelRow {
    #[serde(with = "clickhouse::serde::uuid")]
    identity_id: Uuid,
    name: String,
    ts: i64,
    metadata: String,
    #[serde(with = "clickhouse::serde::uuid")]
    session_id: Uuid,
}

#[derive(Row, Deserialize)]
struct ChActiveDay {
    #[serde(with = "clickhouse::serde::uuid")]
    identity_id: Uuid,
    day: String,
}

pub type FunnelRow = (Uuid, String, DateTime<Utc>, Value, Option<Uuid>);
pub type ExploreRow = (DateTime<Utc>, String, i64, i64, f64, f64);

enum GroupBy {
    None,
    Country,
    Platform,
    AppVersion,
    Experiment(String),
    Meta(String),
}

#[derive(Row, Deserialize)]
struct ChCountry {
    country: String,
    identities: u64,
    events: u64,
}

#[derive(Row, Deserialize)]
struct ChExplore {
    #[serde(with = "clickhouse::serde::chrono::datetime")]
    bucket: DateTime<Utc>,
    grp: String,
    count: u64,
    identities: u64,
    sum: f64,
    field_sum: f64,
}

#[derive(Row, Deserialize)]
struct ChIdentityRevenue {
    #[serde(with = "clickhouse::serde::uuid")]
    identity_id: Uuid,
    day: String,
    amount: f64,
}

/// Filters for [`EventStore::search_for_identity`].
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct EventSearch {
    pub name: Option<String>,
    pub q: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}
