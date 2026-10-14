//! Cluster-wide job locks on top of Postgres transaction-scoped advisory locks. Every replica runs
//! the same housekeeping loop; wrapping a job in [`JobLock::try_run`] guarantees only one replica
//! executes it at a time without any extra infrastructure. The lock lives inside a transaction on a
//! dedicated pool connection, so it is released on commit, on rollback, when the job future is
//! cancelled (the connection returns to the pool and sqlx rolls the open transaction back) and when
//! a crashed replica's connection dies — a stale lock can never outlive its holder.

use std::future::Future;

use sqlx::PgPool;
use zanshin_core::Result;

use crate::map_sqlx;

#[derive(Clone)]
pub struct JobLock {
    pg: PgPool,
}

/// Outcome of a lock attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lock<T> {
    /// The job ran on this replica.
    Ran(T),
    /// Another replica holds the lock; the job was skipped.
    Skipped,
}

impl JobLock {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    /// Stable 64-bit key for a job name (same on every replica).
    pub fn key(name: &str) -> i64 {
        xxhash_rust::xxh3::xxh3_64(format!("zanshin:job:{name}").as_bytes()) as i64
    }

    /// Run `job` if nobody else holds the lock for `name`. The lock is taken inside a transaction
    /// on a dedicated pool connection for the duration of the job and released when the transaction
    /// ends — including when this future is dropped mid-job.
    pub async fn try_run<F, Fut, T>(&self, name: &str, job: F) -> Result<Lock<T>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let key = Self::key(name);
        let mut tx = self.pg.begin().await.map_err(map_sqlx)?;
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
            .bind(key)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_sqlx)?;
        if !acquired {
            metrics::counter!("zanshin_jobs_skipped_total", "job" => name.to_string()).increment(1);
            return Ok(Lock::Skipped);
        }
        let started = std::time::Instant::now();
        let out = job().await;
        metrics::histogram!("zanshin_job_seconds", "job" => name.to_string())
            .record(started.elapsed().as_secs_f64());
        tx.rollback().await.map_err(map_sqlx)?;
        Ok(Lock::Ran(out))
    }

    /// Whether the lock for `name` is currently held by any replica (diagnostics).
    pub async fn is_held(&self, name: &str) -> Result<bool> {
        let key = Self::key(name);
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND objid = ($1::bigint & 4294967295) AND classid = (($1::bigint >> 32) & 4294967295))",
        )
        .bind(key)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }
}
