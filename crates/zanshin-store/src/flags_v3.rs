//! Flags v3 storage: scheduled flag changes, change requests with approvals, evaluation counters.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{Error, Result};

use crate::map_sqlx;

/// A flag patch applied at `at` by the housekeeping job.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FlagSchedule {
    pub id: Uuid,
    pub flag_key: String,
    pub at: DateTime<Utc>,
    pub patch: Value,
    /// `pending` / `applied` / `failed` / `cancelled`.
    pub status: String,
    pub created_by: String,
    pub note: String,
    pub error: String,
    pub created_at: DateTime<Utc>,
    pub applied_at: Option<DateTime<Utc>>,
}

/// A proposed configuration change waiting for approval.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ChangeRequest {
    pub id: Uuid,
    pub kind: String,
    pub key: String,
    pub body: Value,
    pub title: String,
    pub description: String,
    /// `open` / `approved` / `rejected` / `applied` / `cancelled`.
    pub status: String,
    pub author: String,
    /// `[{ "by": name, "at": time, "comment": text }]`.
    pub approvals: Value,
    pub required_approvals: i32,
    pub error: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub applied_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct EvalStatRow {
    pub flag_key: String,
    pub variant: String,
    pub day: NaiveDate,
    pub count: i64,
}

#[derive(Clone)]
pub struct FlagsV3Repo {
    pg: PgPool,
}

const SCHED_COLS: &str =
    "id, flag_key, at, patch, status, created_by, note, error, created_at, applied_at";
const CR_COLS: &str = "id, kind, key, body, title, description, status, author, approvals, required_approvals, error, created_at, updated_at, applied_at";

impl FlagsV3Repo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    // ---- schedules ----------------------------------------------------------------------

    pub async fn schedules(&self, flag_key: &str) -> Result<Vec<FlagSchedule>> {
        sqlx::query_as(&format!(
            "SELECT {SCHED_COLS} FROM flag_schedules WHERE flag_key = $1 ORDER BY at DESC LIMIT 200"
        ))
        .bind(flag_key)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn due_schedules(&self, now: DateTime<Utc>) -> Result<Vec<FlagSchedule>> {
        sqlx::query_as(&format!(
            "SELECT {SCHED_COLS} FROM flag_schedules WHERE status = 'pending' AND at <= $1 ORDER BY at LIMIT 100"
        ))
        .bind(now)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn create_schedule(
        &self,
        flag_key: &str,
        at: DateTime<Utc>,
        patch: &Value,
        created_by: &str,
        note: &str,
    ) -> Result<FlagSchedule> {
        sqlx::query_as(&format!(
            "INSERT INTO flag_schedules (id, flag_key, at, patch, created_by, note) VALUES ($1, $2, $3, $4, $5, $6) RETURNING {SCHED_COLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(flag_key)
        .bind(at)
        .bind(patch)
        .bind(created_by)
        .bind(note)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn finish_schedule(&self, id: Uuid, status: &str, error: &str) -> Result<()> {
        sqlx::query(
            "UPDATE flag_schedules SET status = $2, error = $3, applied_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .bind(error)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn cancel_schedule(&self, flag_key: &str, id: Uuid) -> Result<()> {
        let res = sqlx::query(
            "UPDATE flag_schedules SET status = 'cancelled' WHERE id = $1 AND flag_key = $2 AND status = 'pending'",
        )
        .bind(id)
        .bind(flag_key)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!("pending schedule {id} not found")));
        }
        Ok(())
    }

    // ---- change requests ----------------------------------------------------------------

    pub async fn change_requests(
        &self,
        status: Option<&str>,
        limit: i64,
    ) -> Result<Vec<ChangeRequest>> {
        sqlx::query_as(&format!(
            "SELECT {CR_COLS} FROM change_requests WHERE ($1::text IS NULL OR status = $1) ORDER BY created_at DESC LIMIT $2"
        ))
        .bind(status)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn change_request(&self, id: Uuid) -> Result<ChangeRequest> {
        let r: Option<ChangeRequest> = sqlx::query_as(&format!(
            "SELECT {CR_COLS} FROM change_requests WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        r.ok_or_else(|| Error::not_found(format!("change request {id} not found")))
    }

    pub async fn create_change_request(
        &self,
        kind: &str,
        key: &str,
        body: &Value,
        title: &str,
        description: &str,
        author: &str,
        required_approvals: i32,
    ) -> Result<ChangeRequest> {
        sqlx::query_as(&format!(
            "INSERT INTO change_requests (id, kind, key, body, title, description, author, required_approvals)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {CR_COLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(kind)
        .bind(key)
        .bind(body)
        .bind(title)
        .bind(description)
        .bind(author)
        .bind(required_approvals)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn update_change_request(
        &self,
        id: Uuid,
        status: &str,
        approvals: &Value,
        error: &str,
        applied: bool,
    ) -> Result<ChangeRequest> {
        sqlx::query_as(&format!(
            "UPDATE change_requests SET status = $2, approvals = $3, error = $4, updated_at = now(),
                                        applied_at = CASE WHEN $5 THEN now() ELSE applied_at END
             WHERE id = $1 RETURNING {CR_COLS}"
        ))
        .bind(id)
        .bind(status)
        .bind(approvals)
        .bind(error)
        .bind(applied)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    // ---- evaluation counters ------------------------------------------------------------

    pub async fn add_eval_counts(&self, counts: &[(String, String, i64)]) -> Result<()> {
        for (flag, variant, n) in counts {
            sqlx::query(
                "INSERT INTO flag_eval_stats (flag_key, variant, day, count) VALUES ($1, $2, current_date, $3)
                 ON CONFLICT (flag_key, variant, day) DO UPDATE SET count = flag_eval_stats.count + EXCLUDED.count",
            )
            .bind(flag)
            .bind(variant)
            .bind(n)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        }
        Ok(())
    }

    pub async fn eval_stats(&self, flag_key: &str, days: i64) -> Result<Vec<EvalStatRow>> {
        sqlx::query_as(
            "SELECT flag_key, variant, day, count FROM flag_eval_stats
             WHERE flag_key = $1 AND day >= current_date - $2::int ORDER BY day, variant",
        )
        .bind(flag_key)
        .bind(days as i32)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Flags with evaluations in the window → total count; flags absent from the result are stale.
    pub async fn eval_totals(&self, days: i64) -> Result<Vec<(String, i64, NaiveDate)>> {
        sqlx::query_as(
            "SELECT flag_key, sum(count)::bigint, max(day) FROM flag_eval_stats
             WHERE day >= current_date - $1::int GROUP BY flag_key",
        )
        .bind(days as i32)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }
}
