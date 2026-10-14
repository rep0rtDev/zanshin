//! Persistence for export sinks, their run log, and configuration history snapshots.

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
    export::{ConfigSnapshot, ExportFormat, ExportRun, ExportSink, SinkKind},
    Error, Result,
};

use crate::map_sqlx;

#[derive(FromRow)]
struct SinkRow {
    id: Uuid,
    key: String,
    name: String,
    kind: String,
    config: Value,
    format: String,
    gzip: bool,
    enabled: bool,
    events: Vec<String>,
    interval_secs: i64,
    batch_size: i32,
    cursor_at: Option<DateTime<Utc>>,
    cursor_id: Option<Uuid>,
    next_run_at: DateTime<Utc>,
    last_run_at: Option<DateTime<Utc>>,
    last_error: String,
    tags: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<SinkRow> for ExportSink {
    type Error = Error;
    fn try_from(r: SinkRow) -> Result<Self> {
        Ok(ExportSink {
            id: r.id,
            key: r.key,
            name: r.name,
            kind: SinkKind::parse(&r.kind)
                .ok_or_else(|| Error::invalid(format!("unknown sink kind {}", r.kind)))?,
            config: r.config,
            format: ExportFormat::parse(&r.format).unwrap_or_default(),
            gzip: r.gzip,
            enabled: r.enabled,
            events: r.events,
            interval_secs: r.interval_secs,
            batch_size: r.batch_size,
            cursor_at: r.cursor_at,
            cursor_id: r.cursor_id,
            next_run_at: Some(r.next_run_at),
            last_run_at: r.last_run_at,
            last_error: r.last_error,
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const SINK_COLS: &str = "id, key, name, kind, config, format, gzip, enabled, events, interval_secs, batch_size, cursor_at, cursor_id, next_run_at, last_run_at, last_error, tags, version, created_at, updated_at";

#[derive(Clone)]
pub struct ExportSinkRepo {
    pg: PgPool,
}

impl ExportSinkRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<ExportSink>> {
        let rows: Vec<SinkRow> = sqlx::query_as(&format!(
            "SELECT {SINK_COLS} FROM export_sinks ORDER BY key"
        ))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        rows.into_iter().map(ExportSink::try_from).collect()
    }

    pub async fn get(&self, key: &str) -> Result<ExportSink> {
        let row: Option<SinkRow> = sqlx::query_as(&format!(
            "SELECT {SINK_COLS} FROM export_sinks WHERE key = $1"
        ))
        .bind(key)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.ok_or_else(|| Error::not_found(format!("export sink {key}")))?
            .try_into()
    }

    pub async fn create(&self, s: &ExportSink) -> Result<ExportSink> {
        let row: SinkRow = sqlx::query_as(&format!(
            "INSERT INTO export_sinks (id, key, name, kind, config, format, gzip, enabled, events, interval_secs, batch_size, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) RETURNING {SINK_COLS}"
        ))
        .bind(s.id)
        .bind(&s.key)
        .bind(&s.name)
        .bind(s.kind.as_str())
        .bind(&s.config)
        .bind(s.format.as_str())
        .bind(s.gzip)
        .bind(s.enabled)
        .bind(&s.events)
        .bind(s.interval_secs)
        .bind(s.batch_size)
        .bind(&s.tags)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.try_into()
    }

    pub async fn update(&self, s: &ExportSink) -> Result<ExportSink> {
        let row: Option<SinkRow> = sqlx::query_as(&format!(
            "UPDATE export_sinks SET name = $2, kind = $3, config = $4, format = $5, gzip = $6, enabled = $7, events = $8,
                    interval_secs = $9, batch_size = $10, tags = $11, version = version + 1, updated_at = now()
             WHERE key = $1 RETURNING {SINK_COLS}"
        ))
        .bind(&s.key)
        .bind(&s.name)
        .bind(s.kind.as_str())
        .bind(&s.config)
        .bind(s.format.as_str())
        .bind(s.gzip)
        .bind(s.enabled)
        .bind(&s.events)
        .bind(s.interval_secs)
        .bind(s.batch_size)
        .bind(&s.tags)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.ok_or_else(|| Error::not_found(format!("export sink {}", s.key)))?
            .try_into()
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let n = sqlx::query("DELETE FROM export_sinks WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?
            .rows_affected();
        if n == 0 {
            return Err(Error::not_found(format!("export sink {key}")));
        }
        Ok(())
    }

    /// Atomically claim sinks that are due, pushing their next run forward by one interval.
    pub async fn claim_due(&self, limit: i64) -> Result<Vec<ExportSink>> {
        let rows: Vec<SinkRow> = sqlx::query_as(&format!(
            "WITH due AS (
                SELECT id FROM export_sinks WHERE enabled AND next_run_at <= now()
                ORDER BY next_run_at LIMIT $1 FOR UPDATE SKIP LOCKED
             )
             UPDATE export_sinks s SET next_run_at = now() + make_interval(secs => s.interval_secs)
             FROM due WHERE s.id = due.id RETURNING {}",
            SINK_COLS
                .replace("id,", "s.id,")
                .split(", ")
                .map(|c| if c.starts_with("s.") {
                    c.to_string()
                } else {
                    format!("s.{c}")
                })
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        rows.into_iter().map(ExportSink::try_from).collect()
    }

    /// Run now: make the sink due immediately.
    pub async fn trigger(&self, key: &str) -> Result<()> {
        sqlx::query("UPDATE export_sinks SET next_run_at = now() WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn finish(
        &self,
        id: Uuid,
        cursor: Option<(DateTime<Utc>, Uuid)>,
        error: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE export_sinks SET cursor_at = COALESCE($2, cursor_at), cursor_id = COALESCE($3, cursor_id),
                    last_run_at = now(), last_error = $4 WHERE id = $1",
        )
        .bind(id)
        .bind(cursor.map(|c| c.0))
        .bind(cursor.map(|c| c.1))
        .bind(error)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn reset_cursor(&self, key: &str, to: Option<DateTime<Utc>>) -> Result<()> {
        let n = sqlx::query("UPDATE export_sinks SET cursor_at = $2, cursor_id = NULL, next_run_at = now() WHERE key = $1")
            .bind(key)
            .bind(to)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?
            .rows_affected();
        if n == 0 {
            return Err(Error::not_found(format!("export sink {key}")));
        }
        Ok(())
    }
}

#[derive(FromRow)]
struct RunRow {
    id: Uuid,
    sink_id: Uuid,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    status: String,
    rows: i64,
    bytes: i64,
    object: String,
    error: String,
}

impl From<RunRow> for ExportRun {
    fn from(r: RunRow) -> Self {
        ExportRun {
            id: r.id,
            sink_id: r.sink_id,
            started_at: r.started_at,
            finished_at: r.finished_at,
            status: r.status,
            rows: r.rows,
            bytes: r.bytes,
            object: r.object,
            error: r.error,
        }
    }
}

#[derive(Clone)]
pub struct ExportRunRepo {
    pg: PgPool,
}

impl ExportRunRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn start(&self, sink_id: Uuid) -> Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO export_runs (id, sink_id) VALUES ($1, $2)")
            .bind(id)
            .bind(sink_id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(id)
    }

    pub async fn finish(
        &self,
        id: Uuid,
        status: &str,
        rows: i64,
        bytes: i64,
        object: &str,
        error: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE export_runs SET finished_at = now(), status = $2, rows = $3, bytes = $4, object = $5, error = $6 WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .bind(rows)
        .bind(bytes)
        .bind(object)
        .bind(error)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn list(&self, sink_id: Uuid, limit: i64) -> Result<Vec<ExportRun>> {
        let rows: Vec<RunRow> = sqlx::query_as(
            "SELECT id, sink_id, started_at, finished_at, status, rows, bytes, object, error
             FROM export_runs WHERE sink_id = $1 ORDER BY started_at DESC LIMIT $2",
        )
        .bind(sink_id)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn purge(&self, keep: Duration) -> Result<u64> {
        Ok(sqlx::query("DELETE FROM export_runs WHERE started_at < $1")
            .bind(Utc::now() - keep)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?
            .rows_affected())
    }
}

#[derive(FromRow)]
struct SnapRow {
    id: Uuid,
    kind: String,
    key: String,
    version: i64,
    body: Option<Value>,
    deleted: bool,
    actor: String,
    at: DateTime<Utc>,
}

impl From<SnapRow> for ConfigSnapshot {
    fn from(r: SnapRow) -> Self {
        ConfigSnapshot {
            id: r.id,
            kind: r.kind,
            key: r.key,
            version: r.version,
            body: r.body,
            deleted: r.deleted,
            actor: r.actor,
            at: r.at,
        }
    }
}

const SNAP_COLS: &str = "id, kind, key, version, body, deleted, actor, at";

#[derive(Clone)]
pub struct ConfigHistoryRepo {
    pg: PgPool,
}

impl ConfigHistoryRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn insert(
        &self,
        kind: &str,
        key: &str,
        version: i64,
        body: Option<&Value>,
        deleted: bool,
        actor: &str,
    ) -> Result<ConfigSnapshot> {
        let row: SnapRow = sqlx::query_as(&format!(
            "INSERT INTO config_history (id, kind, key, version, body, deleted, actor) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {SNAP_COLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(kind)
        .bind(key)
        .bind(version)
        .bind(body)
        .bind(deleted)
        .bind(actor)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(row.into())
    }

    pub async fn list(
        &self,
        kind: Option<&str>,
        key: Option<&str>,
        limit: i64,
    ) -> Result<Vec<ConfigSnapshot>> {
        let rows: Vec<SnapRow> = sqlx::query_as(&format!(
            "SELECT {SNAP_COLS} FROM config_history WHERE ($1::text IS NULL OR kind = $1) AND ($2::text IS NULL OR key = $2)
             ORDER BY at DESC LIMIT $3"
        ))
        .bind(kind)
        .bind(key)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get(&self, id: Uuid) -> Result<ConfigSnapshot> {
        let row: Option<SnapRow> = sqlx::query_as(&format!(
            "SELECT {SNAP_COLS} FROM config_history WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(row
            .ok_or_else(|| Error::not_found(format!("snapshot {id}")))?
            .into())
    }

    /// Latest snapshot of an entity strictly before `at` (how it looked before a change at that time).
    pub async fn before(
        &self,
        kind: &str,
        key: &str,
        at: DateTime<Utc>,
    ) -> Result<Option<ConfigSnapshot>> {
        let row: Option<SnapRow> = sqlx::query_as(&format!(
            "SELECT {SNAP_COLS} FROM config_history WHERE kind = $1 AND key = $2 AND at < $3 ORDER BY at DESC LIMIT 1"
        ))
        .bind(kind)
        .bind(key)
        .bind(at)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(row.map(Into::into))
    }

    /// Latest snapshots recorded at or after `at` for an entity (the changes an undo reverts).
    pub async fn since(
        &self,
        kind: &str,
        key: &str,
        at: DateTime<Utc>,
    ) -> Result<Vec<ConfigSnapshot>> {
        let rows: Vec<SnapRow> = sqlx::query_as(&format!(
            "SELECT {SNAP_COLS} FROM config_history WHERE kind = $1 AND key = $2 AND at >= $3 ORDER BY at"
        ))
        .bind(kind)
        .bind(key)
        .bind(at)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn purge(&self, keep: Duration) -> Result<u64> {
        Ok(sqlx::query("DELETE FROM config_history WHERE at < $1")
            .bind(Utc::now() - keep)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?
            .rows_affected())
    }
}
