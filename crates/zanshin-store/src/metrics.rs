use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
    metrics::{Aggregation, Metric, MetricKind},
    Error, Result,
};

use crate::map_sqlx;

#[derive(FromRow)]
struct Row {
    id: Uuid,
    key: String,
    name: String,
    description: String,
    kind: String,
    event_name: String,
    aggregation: String,
    field: String,
    tags: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<Row> for Metric {
    type Error = Error;
    fn try_from(r: Row) -> Result<Self> {
        Ok(Metric {
            id: r.id,
            key: r.key,
            name: r.name,
            description: r.description,
            kind: r.kind.parse::<MetricKind>()?,
            event_name: r.event_name,
            aggregation: r.aggregation.parse::<Aggregation>()?,
            field: r.field,
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const COLS: &str =
    "id, key, name, description, kind, event_name, aggregation, field, tags, version, created_at, updated_at";

#[derive(Clone)]
pub struct MetricRepo {
    pg: PgPool,
}

impl MetricRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<Metric>> {
        let rows: Vec<Row> = sqlx::query_as(&format!("SELECT {COLS} FROM metrics ORDER BY key"))
            .fetch_all(&self.pg)
            .await
            .map_err(map_sqlx)?;
        rows.into_iter().map(Metric::try_from).collect()
    }

    pub async fn get(&self, key: &str) -> Result<Metric> {
        let row: Option<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM metrics WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(Metric::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("metric {key:?} not found")))
    }

    pub async fn create(&self, m: &Metric) -> Result<Metric> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO metrics (id, key, name, description, kind, event_name, aggregation, field, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {COLS}"
        ))
        .bind(m.id)
        .bind(&m.key)
        .bind(&m.name)
        .bind(&m.description)
        .bind(m.kind.as_str())
        .bind(&m.event_name)
        .bind(m.aggregation.as_str())
        .bind(&m.field)
        .bind(&m.tags)
        .fetch_one(&self.pg)
        .await
        .map_err(|err| match map_sqlx(err) {
            Error::Conflict(_) => Error::conflict(format!("metric {:?} already exists", m.key)),
            other => other,
        })?;
        Metric::try_from(row)
    }

    pub async fn update(&self, m: &Metric) -> Result<Metric> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "UPDATE metrics SET name = $2, description = $3, kind = $4, event_name = $5, aggregation = $6, field = $7, tags = $8,
                                version = version + 1, updated_at = now()
             WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&m.key)
        .bind(&m.name)
        .bind(&m.description)
        .bind(m.kind.as_str())
        .bind(&m.event_name)
        .bind(m.aggregation.as_str())
        .bind(&m.field)
        .bind(&m.tags)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Metric::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("metric {:?} not found", m.key)))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM metrics WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!("metric {key:?} not found")));
        }
        Ok(())
    }

    /// Experiments referencing a metric key.
    pub async fn used_by(&self, key: &str) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT key FROM experiments WHERE $1 = ANY(metrics) ORDER BY key")
                .bind(key)
                .fetch_all(&self.pg)
                .await
                .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(|(k,)| k).collect())
    }
}
