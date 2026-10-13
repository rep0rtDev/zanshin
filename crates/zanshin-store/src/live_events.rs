use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{live_events::LiveEvent, rules::Condition, Error, Result};

use crate::map_sqlx;

#[derive(FromRow)]
struct Row {
    id: Uuid,
    key: String,
    name: String,
    description: String,
    enabled: bool,
    condition: Value,
    audiences: Vec<String>,
    starts_at: DateTime<Utc>,
    ends_at: Option<DateTime<Utc>>,
    duration_secs: Option<i64>,
    recurrence: Option<String>,
    payload: Value,
    tags: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<Row> for LiveEvent {
    type Error = Error;
    fn try_from(r: Row) -> Result<Self> {
        let condition: Condition = serde_json::from_value(r.condition).map_err(Error::internal)?;
        Ok(LiveEvent {
            id: r.id,
            key: r.key,
            name: r.name,
            description: r.description,
            enabled: r.enabled,
            condition,
            audiences: r.audiences,
            starts_at: r.starts_at,
            ends_at: r.ends_at,
            duration_secs: r.duration_secs,
            recurrence: r.recurrence,
            payload: r.payload,
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const COLS: &str = "id, key, name, description, enabled, condition, audiences, starts_at, ends_at, duration_secs, recurrence, payload, tags, version, created_at, updated_at";

#[derive(Clone)]
pub struct LiveEventRepo {
    pg: PgPool,
}

impl LiveEventRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<LiveEvent>> {
        let rows: Vec<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM live_events ORDER BY key"))
                .fetch_all(&self.pg)
                .await
                .map_err(map_sqlx)?;
        rows.into_iter().map(LiveEvent::try_from).collect()
    }

    pub async fn get(&self, key: &str) -> Result<LiveEvent> {
        let row: Option<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM live_events WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(LiveEvent::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("live event {key:?} not found")))
    }

    pub async fn create(&self, e: &LiveEvent) -> Result<LiveEvent> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO live_events (id, key, name, description, enabled, condition, audiences, starts_at, ends_at, duration_secs, recurrence, payload, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING {COLS}"
        ))
        .bind(e.id)
        .bind(&e.key)
        .bind(&e.name)
        .bind(&e.description)
        .bind(e.enabled)
        .bind(serde_json::to_value(&e.condition).map_err(Error::internal)?)
        .bind(&e.audiences)
        .bind(e.starts_at)
        .bind(e.ends_at)
        .bind(e.duration_secs)
        .bind(&e.recurrence)
        .bind(&e.payload)
        .bind(&e.tags)
        .fetch_one(&self.pg)
        .await
        .map_err(|err| match map_sqlx(err) {
            Error::Conflict(_) => Error::conflict(format!("live event {:?} already exists", e.key)),
            other => other,
        })?;
        LiveEvent::try_from(row)
    }

    pub async fn update(&self, e: &LiveEvent) -> Result<LiveEvent> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "UPDATE live_events SET name = $2, description = $3, enabled = $4, condition = $5, audiences = $6, starts_at = $7, ends_at = $8,
                                    duration_secs = $9, recurrence = $10, payload = $11, tags = $12, version = version + 1, updated_at = now()
             WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&e.key)
        .bind(&e.name)
        .bind(&e.description)
        .bind(e.enabled)
        .bind(serde_json::to_value(&e.condition).map_err(Error::internal)?)
        .bind(&e.audiences)
        .bind(e.starts_at)
        .bind(e.ends_at)
        .bind(e.duration_secs)
        .bind(&e.recurrence)
        .bind(&e.payload)
        .bind(&e.tags)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(LiveEvent::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("live event {:?} not found", e.key)))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM live_events WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!("live event {key:?} not found")));
        }
        Ok(())
    }
}
