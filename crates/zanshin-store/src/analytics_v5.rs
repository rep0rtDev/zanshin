//! Analytics v5 storage: saved SQL queries, report schedules, metric alerts and identity timeline
//! sources (play sessions, message deliveries).

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
    analytics_v5::{
        AlertAggregation, AlertCondition, AlertFire, MetricAlert, ReportSchedule, SavedQuery,
    },
    Error, Result,
};

use crate::map_sqlx;

fn tags(v: Value) -> Vec<String> {
    serde_json::from_value(v).unwrap_or_default()
}

// --- saved queries ---------------------------------------------------------------------------------

#[derive(FromRow)]
struct QRow {
    id: Uuid,
    key: String,
    name: String,
    description: String,
    sql: String,
    tags: Value,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<QRow> for SavedQuery {
    fn from(r: QRow) -> Self {
        SavedQuery {
            id: r.id,
            key: r.key,
            name: r.name,
            description: r.description,
            sql: r.sql,
            tags: tags(r.tags),
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

const QCOLS: &str = "id, key, name, description, sql, tags, version, created_at, updated_at";

#[derive(Clone)]
pub struct SavedQueryRepo {
    pg: PgPool,
}

impl SavedQueryRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }
    pub async fn list(&self) -> Result<Vec<SavedQuery>> {
        let rows: Vec<QRow> =
            sqlx::query_as(&format!("SELECT {QCOLS} FROM saved_queries ORDER BY key"))
                .fetch_all(&self.pg)
                .await
                .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
    pub async fn get(&self, key: &str) -> Result<SavedQuery> {
        let row: Option<QRow> =
            sqlx::query_as(&format!("SELECT {QCOLS} FROM saved_queries WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(Into::into)
            .ok_or_else(|| Error::not_found(format!("saved query {key:?} not found")))
    }
    pub async fn create(&self, q: &SavedQuery) -> Result<SavedQuery> {
        let row: QRow = sqlx::query_as(&format!(
            "INSERT INTO saved_queries (id, key, name, description, sql, tags) VALUES ($1, $2, $3, $4, $5, $6) RETURNING {QCOLS}"
        ))
        .bind(q.id)
        .bind(&q.key)
        .bind(&q.name)
        .bind(&q.description)
        .bind(&q.sql)
        .bind(Value::from(q.tags.clone()))
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(row.into())
    }
    pub async fn update(&self, q: &SavedQuery) -> Result<SavedQuery> {
        let row: Option<QRow> = sqlx::query_as(&format!(
            "UPDATE saved_queries SET name = $2, description = $3, sql = $4, tags = $5, version = version + 1, updated_at = now() WHERE key = $1 RETURNING {QCOLS}"
        ))
        .bind(&q.key)
        .bind(&q.name)
        .bind(&q.description)
        .bind(&q.sql)
        .bind(Value::from(q.tags.clone()))
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Into::into)
            .ok_or_else(|| Error::not_found(format!("saved query {:?} not found", q.key)))
    }
    pub async fn delete(&self, key: &str) -> Result<()> {
        let n = sqlx::query("DELETE FROM saved_queries WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if n.rows_affected() == 0 {
            return Err(Error::not_found(format!("saved query {key:?} not found")));
        }
        Ok(())
    }
}

// --- report schedules -------------------------------------------------------------------------------

#[derive(FromRow)]
struct RRow {
    id: Uuid,
    key: String,
    name: String,
    enabled: bool,
    query_key: String,
    cron: String,
    timezone: String,
    channels: Value,
    email: Option<Value>,
    max_rows: i64,
    last_run_at: Option<DateTime<Utc>>,
    last_status: String,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<RRow> for ReportSchedule {
    fn from(r: RRow) -> Self {
        ReportSchedule {
            id: r.id,
            key: r.key,
            name: r.name,
            enabled: r.enabled,
            query_key: r.query_key,
            cron: r.cron,
            timezone: r.timezone,
            channels: tags(r.channels),
            email: r.email.and_then(|v| serde_json::from_value(v).ok()),
            max_rows: r.max_rows,
            last_run_at: r.last_run_at,
            last_status: r.last_status,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

const RCOLS: &str = "id, key, name, enabled, query_key, cron, timezone, channels, email, max_rows, last_run_at, last_status, version, created_at, updated_at";

#[derive(Clone)]
pub struct ReportRepo {
    pg: PgPool,
}

impl ReportRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }
    pub async fn list(&self) -> Result<Vec<ReportSchedule>> {
        let rows: Vec<RRow> = sqlx::query_as(&format!(
            "SELECT {RCOLS} FROM report_schedules ORDER BY key"
        ))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
    pub async fn get(&self, key: &str) -> Result<ReportSchedule> {
        let row: Option<RRow> = sqlx::query_as(&format!(
            "SELECT {RCOLS} FROM report_schedules WHERE key = $1"
        ))
        .bind(key)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Into::into)
            .ok_or_else(|| Error::not_found(format!("report {key:?} not found")))
    }
    pub async fn create(&self, r: &ReportSchedule) -> Result<ReportSchedule> {
        let row: RRow = sqlx::query_as(&format!(
            "INSERT INTO report_schedules (id, key, name, enabled, query_key, cron, timezone, channels, email, max_rows)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING {RCOLS}"
        ))
        .bind(r.id)
        .bind(&r.key)
        .bind(&r.name)
        .bind(r.enabled)
        .bind(&r.query_key)
        .bind(&r.cron)
        .bind(&r.timezone)
        .bind(Value::from(r.channels.clone()))
        .bind(r.email.as_ref().map(|e| serde_json::to_value(e).unwrap_or(Value::Null)))
        .bind(r.max_rows)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(row.into())
    }
    pub async fn update(&self, r: &ReportSchedule) -> Result<ReportSchedule> {
        let row: Option<RRow> = sqlx::query_as(&format!(
            "UPDATE report_schedules SET name = $2, enabled = $3, query_key = $4, cron = $5, timezone = $6, channels = $7, email = $8, max_rows = $9,
                    version = version + 1, updated_at = now() WHERE key = $1 RETURNING {RCOLS}"
        ))
        .bind(&r.key)
        .bind(&r.name)
        .bind(r.enabled)
        .bind(&r.query_key)
        .bind(&r.cron)
        .bind(&r.timezone)
        .bind(Value::from(r.channels.clone()))
        .bind(r.email.as_ref().map(|e| serde_json::to_value(e).unwrap_or(Value::Null)))
        .bind(r.max_rows)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Into::into)
            .ok_or_else(|| Error::not_found(format!("report {:?} not found", r.key)))
    }
    pub async fn delete(&self, key: &str) -> Result<()> {
        let n = sqlx::query("DELETE FROM report_schedules WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if n.rows_affected() == 0 {
            return Err(Error::not_found(format!("report {key:?} not found")));
        }
        Ok(())
    }
    pub async fn mark_run(&self, id: Uuid, at: DateTime<Utc>, status: &str) -> Result<()> {
        sqlx::query("UPDATE report_schedules SET last_run_at = $2, last_status = $3 WHERE id = $1")
            .bind(id)
            .bind(at)
            .bind(status)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }
}

// --- metric alerts ----------------------------------------------------------------------------------

#[derive(FromRow)]
struct ARow {
    id: Uuid,
    key: String,
    name: String,
    enabled: bool,
    event: String,
    aggregation: String,
    field: String,
    condition: Value,
    window_secs: i64,
    cooldown_secs: i64,
    last_value: Option<f64>,
    last_checked_at: Option<DateTime<Utc>>,
    last_fired_at: Option<DateTime<Utc>>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<ARow> for MetricAlert {
    type Error = Error;
    fn try_from(r: ARow) -> Result<Self> {
        Ok(MetricAlert {
            id: r.id,
            key: r.key,
            name: r.name,
            enabled: r.enabled,
            event: r.event,
            aggregation: serde_json::from_value(Value::String(r.aggregation))
                .unwrap_or(AlertAggregation::Count),
            field: r.field,
            condition: serde_json::from_value::<AlertCondition>(r.condition)
                .map_err(|e| Error::internal(anyhow::anyhow!("alert condition: {e}")))?,
            window_secs: r.window_secs,
            cooldown_secs: r.cooldown_secs,
            last_value: r.last_value,
            last_checked_at: r.last_checked_at,
            last_fired_at: r.last_fired_at,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const ACOLS: &str = "id, key, name, enabled, event, aggregation, field, condition, window_secs, cooldown_secs, last_value, last_checked_at, last_fired_at, version, created_at, updated_at";

#[derive(Debug, Clone, serde::Serialize, FromRow)]
pub struct FireRow {
    pub id: Uuid,
    pub value: f64,
    pub baseline: Option<f64>,
    pub deviation: Option<f64>,
    pub text: String,
    pub fired_at: DateTime<Utc>,
}

fn agg_str(a: AlertAggregation) -> &'static str {
    match a {
        AlertAggregation::Count => "count",
        AlertAggregation::Unique => "unique",
        AlertAggregation::Sum => "sum",
    }
}

#[derive(Clone)]
pub struct AlertRepo {
    pg: PgPool,
}

impl AlertRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }
    pub async fn list(&self) -> Result<Vec<MetricAlert>> {
        let rows: Vec<ARow> =
            sqlx::query_as(&format!("SELECT {ACOLS} FROM metric_alerts ORDER BY key"))
                .fetch_all(&self.pg)
                .await
                .map_err(map_sqlx)?;
        rows.into_iter().map(MetricAlert::try_from).collect()
    }
    pub async fn get(&self, key: &str) -> Result<MetricAlert> {
        let row: Option<ARow> =
            sqlx::query_as(&format!("SELECT {ACOLS} FROM metric_alerts WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(MetricAlert::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("alert {key:?} not found")))
    }
    pub async fn create(&self, a: &MetricAlert) -> Result<MetricAlert> {
        let row: ARow = sqlx::query_as(&format!(
            "INSERT INTO metric_alerts (id, key, name, enabled, event, aggregation, field, condition, window_secs, cooldown_secs)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING {ACOLS}"
        ))
        .bind(a.id)
        .bind(&a.key)
        .bind(&a.name)
        .bind(a.enabled)
        .bind(&a.event)
        .bind(agg_str(a.aggregation))
        .bind(&a.field)
        .bind(serde_json::to_value(&a.condition).unwrap_or(Value::Null))
        .bind(a.window_secs)
        .bind(a.cooldown_secs)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        MetricAlert::try_from(row)
    }
    pub async fn update(&self, a: &MetricAlert) -> Result<MetricAlert> {
        let row: Option<ARow> = sqlx::query_as(&format!(
            "UPDATE metric_alerts SET name = $2, enabled = $3, event = $4, aggregation = $5, field = $6, condition = $7, window_secs = $8, cooldown_secs = $9,
                    version = version + 1, updated_at = now() WHERE key = $1 RETURNING {ACOLS}"
        ))
        .bind(&a.key)
        .bind(&a.name)
        .bind(a.enabled)
        .bind(&a.event)
        .bind(agg_str(a.aggregation))
        .bind(&a.field)
        .bind(serde_json::to_value(&a.condition).unwrap_or(Value::Null))
        .bind(a.window_secs)
        .bind(a.cooldown_secs)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(MetricAlert::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("alert {:?} not found", a.key)))
    }
    pub async fn delete(&self, key: &str) -> Result<()> {
        let n = sqlx::query("DELETE FROM metric_alerts WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if n.rows_affected() == 0 {
            return Err(Error::not_found(format!("alert {key:?} not found")));
        }
        Ok(())
    }
    pub async fn record_check(
        &self,
        id: Uuid,
        value: f64,
        at: DateTime<Utc>,
        fired: bool,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE metric_alerts SET last_value = $2, last_checked_at = $3,
                    last_fired_at = CASE WHEN $4 THEN $3 ELSE last_fired_at END WHERE id = $1",
        )
        .bind(id)
        .bind(value)
        .bind(at)
        .bind(fired)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }
    pub async fn record_fire(&self, id: Uuid, f: &AlertFire, at: DateTime<Utc>) -> Result<()> {
        sqlx::query(
            "INSERT INTO metric_alert_fires (id, alert_id, value, baseline, deviation, text, fired_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(Uuid::now_v7())
        .bind(id)
        .bind(f.value)
        .bind(f.baseline)
        .bind(f.deviation)
        .bind(&f.text)
        .bind(at)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }
    pub async fn fires(&self, id: Uuid, limit: i64) -> Result<Vec<FireRow>> {
        sqlx::query_as(
            "SELECT id, value, baseline, deviation, text, fired_at FROM metric_alert_fires WHERE alert_id = $1 ORDER BY fired_at DESC LIMIT $2",
        )
        .bind(id)
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }
}

// --- identity timeline sources ----------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, FromRow)]
pub struct PlaySessionRow {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_sec: Option<i32>,
    pub platform: String,
    pub app_version: String,
}

#[derive(Debug, Clone, serde::Serialize, FromRow)]
pub struct DeliveryRow {
    pub message_key: String,
    pub channel: String,
    pub status: String,
    pub detail: String,
    pub variant: String,
    pub at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct TimelineRepo {
    pg: PgPool,
}

impl TimelineRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }
    pub async fn sessions_for_identity(
        &self,
        identity_id: Uuid,
        limit: i64,
    ) -> Result<Vec<PlaySessionRow>> {
        sqlx::query_as(
            "SELECT id, started_at, last_seen_at, ended_at, duration_sec, platform, app_version FROM play_sessions
             WHERE identity_id = $1 ORDER BY started_at DESC LIMIT $2",
        )
        .bind(identity_id)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }
    pub async fn deliveries_for_identity(
        &self,
        identity_id: Uuid,
        limit: i64,
    ) -> Result<Vec<DeliveryRow>> {
        sqlx::query_as(
            "SELECT m.key AS message_key, d.channel, d.status, d.detail, d.variant, d.at FROM message_deliveries d
             JOIN messages m ON m.id = d.message_id WHERE d.identity_id = $1 ORDER BY d.at DESC LIMIT $2",
        )
        .bind(identity_id)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }
}
