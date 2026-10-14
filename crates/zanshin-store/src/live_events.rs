use std::collections::HashMap;

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
    join_mode: String,
    sticky: bool,
    participation_secs: Option<i64>,
    flag_overrides: Value,
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
            join_mode: r.join_mode.parse()?,
            sticky: r.sticky,
            participation_secs: r.participation_secs,
            flag_overrides: match r.flag_overrides {
                Value::Object(m) => m,
                _ => Default::default(),
            },
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const COLS: &str = "id, key, name, description, enabled, condition, audiences, starts_at, ends_at, duration_secs, recurrence, payload, join_mode, sticky, participation_secs, flag_overrides, tags, version, created_at, updated_at";

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
            "INSERT INTO live_events (id, key, name, description, enabled, condition, audiences, starts_at, ends_at, duration_secs, recurrence, payload, tags,
                                      join_mode, sticky, participation_secs, flag_overrides)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17) RETURNING {COLS}"
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
        .bind(e.join_mode.as_str())
        .bind(e.sticky)
        .bind(e.participation_secs)
        .bind(Value::Object(e.flag_overrides.clone()))
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
                                    duration_secs = $9, recurrence = $10, payload = $11, tags = $12,
                                    join_mode = $13, sticky = $14, participation_secs = $15, flag_overrides = $16,
                                    version = version + 1, updated_at = now()
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
        .bind(e.join_mode.as_str())
        .bind(e.sticky)
        .bind(e.participation_secs)
        .bind(Value::Object(e.flag_overrides.clone()))
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

/// Participant of a live event.
#[derive(Debug, Clone, serde::Serialize, FromRow)]
pub struct Participant {
    pub identity_id: Uuid,
    pub external_id: String,
    pub joined_at: DateTime<Utc>,
    pub left_at: Option<DateTime<Utc>>,
}

impl LiveEventRepo {
    /// Record a join (idempotent; re-joining after leaving resets the timer).
    pub async fn join(&self, live_event_id: Uuid, identity_id: Uuid) -> Result<DateTime<Utc>> {
        let (joined_at,): (DateTime<Utc>,) = sqlx::query_as(
            "INSERT INTO live_event_members (live_event_id, identity_id) VALUES ($1, $2)
             ON CONFLICT (live_event_id, identity_id) DO UPDATE
               SET joined_at = CASE WHEN live_event_members.left_at IS NULL THEN live_event_members.joined_at ELSE now() END,
                   left_at = NULL
             RETURNING joined_at",
        )
        .bind(live_event_id)
        .bind(identity_id)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(joined_at)
    }

    pub async fn leave(&self, live_event_id: Uuid, identity_id: Uuid) -> Result<bool> {
        let r = sqlx::query(
            "UPDATE live_event_members SET left_at = now()
             WHERE live_event_id = $1 AND identity_id = $2 AND left_at IS NULL",
        )
        .bind(live_event_id)
        .bind(identity_id)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(r.rows_affected() > 0)
    }

    /// Active joins for one identity: event id → joined_at.
    pub async fn joins_for(&self, identity_id: Uuid) -> Result<HashMap<Uuid, DateTime<Utc>>> {
        let rows: Vec<(Uuid, DateTime<Utc>)> = sqlx::query_as(
            "SELECT live_event_id, joined_at FROM live_event_members WHERE identity_id = $1 AND left_at IS NULL",
        )
        .bind(identity_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().collect())
    }

    pub async fn participants(
        &self,
        live_event_id: Uuid,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<Participant>> {
        sqlx::query_as(
            "SELECT m.identity_id, i.external_id, m.joined_at, m.left_at
             FROM live_event_members m JOIN identities i ON i.id = m.identity_id
             WHERE m.live_event_id = $1 AND ($2::uuid IS NULL OR m.identity_id > $2)
             ORDER BY m.identity_id LIMIT $3",
        )
        .bind(live_event_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Joins per day for the timeline (UTC buckets).
    pub async fn join_timeline(&self, live_event_id: Uuid) -> Result<Vec<(DateTime<Utc>, i64)>> {
        sqlx::query_as(
            "SELECT date_trunc('day', joined_at) AS day, count(*)::bigint FROM live_event_members
             WHERE live_event_id = $1 GROUP BY 1 ORDER BY 1",
        )
        .bind(live_event_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn participant_count(&self, live_event_id: Uuid) -> Result<(i64, i64)> {
        sqlx::query_as(
            "SELECT count(*)::bigint, count(*) FILTER (WHERE left_at IS NULL)::bigint
             FROM live_event_members WHERE live_event_id = $1",
        )
        .bind(live_event_id)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }
}
