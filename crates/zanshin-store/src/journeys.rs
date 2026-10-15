use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
    journeys::{Journey, Step, Trigger},
    rules::Condition,
    Error, Result,
};

use crate::map_sqlx;

#[derive(FromRow)]
struct Row {
    id: Uuid,
    key: String,
    name: String,
    description: String,
    enabled: bool,
    trigger: Value,
    condition: Value,
    audiences: Vec<String>,
    steps: Value,
    cooldown_secs: i64,
    max_entries: Option<i32>,
    goal: Option<Value>,
    exit_when: Option<Value>,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    tags: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<Row> for Journey {
    type Error = Error;
    fn try_from(r: Row) -> Result<Self> {
        let bad = |what: &str, e: serde_json::Error| {
            Error::invalid(format!("stored journey {what} is invalid: {e}"))
        };
        Ok(Journey {
            id: r.id,
            key: r.key,
            name: r.name,
            description: r.description,
            enabled: r.enabled,
            trigger: serde_json::from_value::<Trigger>(r.trigger).map_err(|e| bad("trigger", e))?,
            condition: serde_json::from_value::<Condition>(r.condition)
                .map_err(|e| bad("condition", e))?,
            audiences: r.audiences,
            steps: serde_json::from_value::<Vec<Step>>(r.steps).map_err(|e| bad("steps", e))?,
            cooldown_secs: r.cooldown_secs,
            max_entries: r.max_entries,
            goal: r.goal.and_then(|v| serde_json::from_value(v).ok()),
            exit_when: r.exit_when.and_then(|v| serde_json::from_value(v).ok()),
            starts_at: r.starts_at,
            ends_at: r.ends_at,
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Run {
    pub id: Uuid,
    pub journey_id: Uuid,
    pub identity_id: Uuid,
    pub step: i32,
    pub status: String,
    pub wake_at: Option<DateTime<Utc>>,
    pub context: Value,
    pub error: String,
    pub entered_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub converted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct RunView {
    pub id: Uuid,
    pub journey_id: Uuid,
    pub identity_id: Uuid,
    pub external_id: String,
    pub step: i32,
    pub status: String,
    pub wake_at: Option<DateTime<Utc>>,
    pub error: String,
    pub entered_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct LogRow {
    run_id: Uuid,
    step: i32,
    action: String,
    detail: String,
    at: DateTime<Utc>,
}

/// Time to complete (runs that reached the end), in seconds.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Timing {
    pub completed: i64,
    pub min_secs: Option<f64>,
    pub avg_secs: Option<f64>,
    pub max_secs: Option<f64>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct LogEntry {
    pub step: i32,
    pub action: String,
    pub detail: String,
    pub at: DateTime<Utc>,
}

const COLS: &str = "id, key, name, description, enabled, trigger, condition, audiences, steps, cooldown_secs, max_entries, starts_at, ends_at, tags, version, created_at, updated_at, goal, exit_when";
const RCOLS: &str =
    "id, journey_id, identity_id, step, status, wake_at, context, error, entered_at, updated_at, converted_at, branch";

#[derive(Clone)]
pub struct JourneyRepo {
    pg: PgPool,
}

impl JourneyRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<Journey>> {
        let rows: Vec<Row> = sqlx::query_as(&format!("SELECT {COLS} FROM journeys ORDER BY key"))
            .fetch_all(&self.pg)
            .await
            .map_err(map_sqlx)?;
        rows.into_iter().map(Journey::try_from).collect()
    }

    pub async fn get(&self, key: &str) -> Result<Journey> {
        let row: Option<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM journeys WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(Journey::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("journey {key:?} not found")))
    }

    pub async fn create(&self, j: &Journey) -> Result<Journey> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO journeys (id, key, name, description, enabled, trigger, condition, audiences, steps, cooldown_secs, max_entries, starts_at, ends_at, tags, goal, exit_when)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) RETURNING {COLS}"
        ))
        .bind(j.id)
        .bind(&j.key)
        .bind(&j.name)
        .bind(&j.description)
        .bind(j.enabled)
        .bind(serde_json::to_value(&j.trigger).unwrap_or(Value::Null))
        .bind(serde_json::to_value(&j.condition).unwrap_or(Value::Null))
        .bind(&j.audiences)
        .bind(serde_json::to_value(&j.steps).unwrap_or(Value::Null))
        .bind(j.cooldown_secs)
        .bind(j.max_entries)
        .bind(j.starts_at)
        .bind(j.ends_at)
        .bind(&j.tags)
        .bind(serde_json::to_value(&j.goal).ok().filter(|v| !v.is_null()))
        .bind(serde_json::to_value(&j.exit_when).ok().filter(|v| !v.is_null()))
        .fetch_one(&self.pg)
        .await
        .map_err(|err| match map_sqlx(err) {
            Error::Conflict(_) => Error::conflict(format!("journey {:?} already exists", j.key)),
            other => other,
        })?;
        Journey::try_from(row)
    }

    pub async fn update(&self, j: &Journey) -> Result<Journey> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "UPDATE journeys SET name = $2, description = $3, enabled = $4, trigger = $5, condition = $6, audiences = $7, steps = $8,
                                 cooldown_secs = $9, max_entries = $10, starts_at = $11, ends_at = $12, tags = $13, goal = $14, exit_when = $15,
                                 version = version + 1, updated_at = now()
             WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&j.key)
        .bind(&j.name)
        .bind(&j.description)
        .bind(j.enabled)
        .bind(serde_json::to_value(&j.trigger).unwrap_or(Value::Null))
        .bind(serde_json::to_value(&j.condition).unwrap_or(Value::Null))
        .bind(&j.audiences)
        .bind(serde_json::to_value(&j.steps).unwrap_or(Value::Null))
        .bind(j.cooldown_secs)
        .bind(j.max_entries)
        .bind(j.starts_at)
        .bind(j.ends_at)
        .bind(&j.tags)
        .bind(serde_json::to_value(&j.goal).ok().filter(|v| !v.is_null()))
        .bind(serde_json::to_value(&j.exit_when).ok().filter(|v| !v.is_null()))
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Journey::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("journey {:?} not found", j.key)))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM journeys WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!("journey {key:?} not found")));
        }
        Ok(())
    }

    // ---- runs

    pub async fn start_run(
        &self,
        journey_id: Uuid,
        identity_id: Uuid,
        context: &Value,
    ) -> Result<Run> {
        sqlx::query_as(&format!(
            "INSERT INTO journey_runs (id, journey_id, identity_id, context, wake_at) VALUES ($1, $2, $3, $4, now()) RETURNING {RCOLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(journey_id)
        .bind(identity_id)
        .bind(context)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// (entries, last entered_at, has active run) for one player in one journey.
    pub async fn entry_state(
        &self,
        journey_id: Uuid,
        identity_id: Uuid,
    ) -> Result<(i64, Option<DateTime<Utc>>, bool)> {
        sqlx::query_as(
            "SELECT count(*)::bigint, max(entered_at), bool_or(status = 'active')
             FROM journey_runs WHERE journey_id = $1 AND identity_id = $2",
        )
        .bind(journey_id)
        .bind(identity_id)
        .fetch_one(&self.pg)
        .await
        .map(
            |(n, at, active): (i64, Option<DateTime<Utc>>, Option<bool>)| {
                (n, at, active.unwrap_or(false))
            },
        )
        .map_err(map_sqlx)
    }

    pub async fn has_run_with_context(
        &self,
        journey_id: Uuid,
        identity_id: Uuid,
        key: &str,
        value: &str,
    ) -> Result<bool> {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT count(*)::bigint FROM journey_runs WHERE journey_id = $1 AND identity_id = $2 AND context->>$3 = $4",
        )
        .bind(journey_id)
        .bind(identity_id)
        .bind(key)
        .bind(value)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(n > 0)
    }

    pub async fn advance(
        &self,
        id: Uuid,
        step: i32,
        status: &str,
        wake_at: Option<DateTime<Utc>>,
        error: &str,
    ) -> Result<Run> {
        sqlx::query_as(&format!(
            "UPDATE journey_runs SET step = $2, status = $3, wake_at = $4, error = $5, updated_at = now() WHERE id = $1 RETURNING {RCOLS}"
        ))
        .bind(id)
        .bind(step)
        .bind(status)
        .bind(wake_at)
        .bind(&error[..error.len().min(1000)])
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Active runs whose wake time has come. Claims them by pushing `wake_at` forward so another
    /// replica does not pick them up in the same tick.
    pub async fn claim_due(&self, now: DateTime<Utc>, limit: i64) -> Result<Vec<Run>> {
        sqlx::query_as(&format!(
            "UPDATE journey_runs SET wake_at = $1 + interval '60 seconds'
             WHERE id IN (SELECT id FROM journey_runs WHERE status = 'active' AND wake_at IS NOT NULL AND wake_at <= $1 ORDER BY wake_at LIMIT $2 FOR UPDATE SKIP LOCKED)
             RETURNING {RCOLS}"
        ))
        .bind(now)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn log(&self, run_id: Uuid, step: i32, action: &str, detail: &str) -> Result<()> {
        sqlx::query("INSERT INTO journey_run_log (id, run_id, step, action, detail) VALUES ($1, $2, $3, $4, $5)")
            .bind(Uuid::now_v7())
            .bind(run_id)
            .bind(step)
            .bind(action)
            .bind(&detail[..detail.len().min(1000)])
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn get_run(&self, journey_id: Uuid, id: Uuid) -> Result<Run> {
        let row: Option<Run> = sqlx::query_as(&format!(
            "SELECT {RCOLS} FROM journey_runs WHERE journey_id = $1 AND id = $2"
        ))
        .bind(journey_id)
        .bind(id)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.ok_or_else(|| Error::not_found("run not found"))
    }

    pub async fn run_log(&self, run_id: Uuid) -> Result<Vec<LogEntry>> {
        sqlx::query_as(
            "SELECT step, action, detail, at FROM journey_run_log WHERE run_id = $1 ORDER BY at",
        )
        .bind(run_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn runs(
        &self,
        journey_id: Uuid,
        status: Option<&str>,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<RunView>> {
        sqlx::query_as(
            "SELECT r.id, r.journey_id, r.identity_id, i.external_id, r.step, r.status, r.wake_at, r.error, r.entered_at, r.updated_at
             FROM journey_runs r JOIN identities i ON i.id = r.identity_id
             WHERE r.journey_id = $1 AND ($2::text IS NULL OR r.status = $2) AND ($3::uuid IS NULL OR r.id < $3)
             ORDER BY r.id DESC LIMIT $4",
        )
        .bind(journey_id)
        .bind(status)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    #[allow(clippy::type_complexity)]
    pub async fn runs_for_identity(&self, identity_id: Uuid) -> Result<Vec<(String, Run)>> {
        let rows: Vec<(String, Uuid, Uuid, Uuid, i32, String, Option<DateTime<Utc>>, Value, String, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, String)> = sqlx::query_as(
            "SELECT j.key, r.id, r.journey_id, r.identity_id, r.step, r.status, r.wake_at, r.context, r.error, r.entered_at, r.updated_at, r.converted_at, r.branch
             FROM journey_runs r JOIN journeys j ON j.id = r.journey_id WHERE r.identity_id = $1 ORDER BY r.entered_at DESC LIMIT 100",
        )
        .bind(identity_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    key,
                    id,
                    journey_id,
                    identity_id,
                    step,
                    status,
                    wake_at,
                    context,
                    error,
                    entered_at,
                    updated_at,
                    converted_at,
                    branch,
                )| {
                    (
                        key,
                        Run {
                            id,
                            journey_id,
                            identity_id,
                            step,
                            status,
                            wake_at,
                            context,
                            error,
                            entered_at,
                            updated_at,
                            converted_at,
                            branch,
                        },
                    )
                },
            )
            .collect())
    }

    /// (status, count) and (step, active count) for the funnel view.
    pub async fn stats(&self, journey_id: Uuid) -> Result<(Vec<(String, i64)>, Vec<(i32, i64)>)> {
        let by_status: Vec<(String, i64)> = sqlx::query_as(
            "SELECT status, count(*)::bigint FROM journey_runs WHERE journey_id = $1 GROUP BY 1 ORDER BY 1",
        )
        .bind(journey_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        let by_step: Vec<(i32, i64)> = sqlx::query_as(
            "SELECT step, count(*)::bigint FROM journey_runs WHERE journey_id = $1 AND status = 'active' GROUP BY 1 ORDER BY 1",
        )
        .bind(journey_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok((by_status, by_step))
    }

    /// Min / avg / max time from entry to completion over finished (`done`) runs.
    pub async fn timing(&self, journey_id: Uuid) -> Result<Timing> {
        let (min_secs, avg_secs, max_secs, completed): (
            Option<f64>,
            Option<f64>,
            Option<f64>,
            i64,
        ) = sqlx::query_as(
            "SELECT min(x), avg(x), max(x), count(*)::bigint FROM (
                   SELECT extract(epoch FROM updated_at - entered_at)::float8 AS x
                   FROM journey_runs WHERE journey_id = $1 AND status = 'done') t",
        )
        .bind(journey_id)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(Timing {
            completed,
            min_secs,
            avg_secs,
            max_secs,
        })
    }

    /// Distinct runs that executed each step index (the per-step counts drawn on the canvas).
    pub async fn passed(&self, journey_id: Uuid) -> Result<Vec<(i32, i64)>> {
        sqlx::query_as(
            "SELECT l.step, count(DISTINCT l.run_id)::bigint FROM journey_run_log l
             JOIN journey_runs r ON r.id = l.run_id WHERE r.journey_id = $1 GROUP BY 1 ORDER BY 1",
        )
        .bind(journey_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Step logs for a set of runs (identity history).
    pub async fn logs_for_runs(&self, ids: &[Uuid]) -> Result<Vec<(Uuid, LogEntry)>> {
        let rows: Vec<LogRow> = sqlx::query_as(
            "SELECT run_id, step, action, detail, at FROM journey_run_log WHERE run_id = ANY($1) ORDER BY at",
        )
        .bind(ids)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.run_id,
                    LogEntry {
                        step: r.step,
                        action: r.action,
                        detail: r.detail,
                        at: r.at,
                    },
                )
            })
            .collect())
    }

    pub async fn purge_runs(&self, older_than: DateTime<Utc>) -> Result<u64> {
        let res =
            sqlx::query("DELETE FROM journey_runs WHERE status <> 'active' AND updated_at < $1")
                .bind(older_than)
                .execute(&self.pg)
                .await
                .map_err(map_sqlx)?;
        Ok(res.rows_affected())
    }
}

/// Conversion summary of a journey: total / converted runs, and the same split by branch.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Conversions {
    pub runs: i64,
    pub converted: i64,
    pub by_branch: Vec<(String, i64, i64)>,
}

impl JourneyRepo {
    pub async fn set_branch(&self, run_id: Uuid, branch: &str) -> Result<()> {
        sqlx::query("UPDATE journey_runs SET branch = $2 WHERE id = $1")
            .bind(run_id)
            .bind(branch)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    /// Mark unconverted runs of this player in this journey (entered after `since`) as converted.
    pub async fn mark_converted(
        &self,
        journey_id: Uuid,
        identity_id: Uuid,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<Uuid>> {
        let ids: Vec<(Uuid,)> = sqlx::query_as(
            "UPDATE journey_runs SET converted_at = now(), updated_at = now()
             WHERE journey_id = $1 AND identity_id = $2 AND converted_at IS NULL
               AND ($3::timestamptz IS NULL OR entered_at >= $3)
             RETURNING id",
        )
        .bind(journey_id)
        .bind(identity_id)
        .bind(since)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(ids.into_iter().map(|(i,)| i).collect())
    }

    pub async fn active_runs_for(&self, journey_id: Uuid, identity_id: Uuid) -> Result<Vec<Run>> {
        sqlx::query_as(&format!(
            "SELECT {RCOLS} FROM journey_runs WHERE journey_id = $1 AND identity_id = $2 AND status = 'active'"
        ))
        .bind(journey_id)
        .bind(identity_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn conversions(&self, journey_id: Uuid) -> Result<Conversions> {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            "SELECT branch, count(*)::bigint, count(converted_at)::bigint FROM journey_runs
             WHERE journey_id = $1 GROUP BY branch ORDER BY branch",
        )
        .bind(journey_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(Conversions {
            runs: rows.iter().map(|r| r.1).sum(),
            converted: rows.iter().map(|r| r.2).sum(),
            by_branch: rows,
        })
    }
}
