use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
    experiments::{Experiment, ExperimentVariant, Status},
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
    hypothesis: String,
    status: String,
    condition: Value,
    audiences: Vec<String>,
    percentage: f64,
    variants: Value,
    control: Option<String>,
    flag_key: Option<String>,
    salt: String,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    tags: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<Row> for Experiment {
    type Error = Error;
    fn try_from(r: Row) -> Result<Self> {
        let condition: Condition = serde_json::from_value(r.condition).map_err(Error::internal)?;
        let variants: Vec<ExperimentVariant> =
            serde_json::from_value(r.variants).map_err(Error::internal)?;
        Ok(Experiment {
            id: r.id,
            key: r.key,
            name: r.name,
            description: r.description,
            hypothesis: r.hypothesis,
            status: Status::parse(&r.status)?,
            condition,
            audiences: r.audiences,
            percentage: r.percentage,
            variants,
            control: r.control,
            flag_key: r.flag_key,
            salt: r.salt,
            starts_at: r.starts_at,
            ends_at: r.ends_at,
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const COLS: &str = "id, key, name, description, hypothesis, status, condition, audiences, percentage, variants, control, flag_key, salt, starts_at, ends_at, tags, version, created_at, updated_at";

/// Stored assignment of one identity.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AssignmentRow {
    pub experiment_id: Uuid,
    pub identity_id: Uuid,
    pub variant: String,
    pub assigned_at: DateTime<Utc>,
    pub exposed_at: Option<DateTime<Utc>>,
    pub exposures: i64,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct VariantStats {
    pub variant: String,
    pub assigned: i64,
    pub exposed: i64,
    pub exposures: i64,
}

#[derive(Clone)]
pub struct ExperimentRepo {
    pg: PgPool,
}

impl ExperimentRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<Experiment>> {
        let rows: Vec<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM experiments ORDER BY key"))
                .fetch_all(&self.pg)
                .await
                .map_err(map_sqlx)?;
        rows.into_iter().map(Experiment::try_from).collect()
    }

    pub async fn get(&self, key: &str) -> Result<Experiment> {
        let row: Option<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM experiments WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(Experiment::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("experiment {key:?} not found")))
    }

    pub async fn create(&self, e: &Experiment) -> Result<Experiment> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO experiments (id, key, name, description, hypothesis, status, condition, audiences, percentage, variants, control, flag_key, salt, starts_at, ends_at, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) RETURNING {COLS}"
        ))
        .bind(e.id)
        .bind(&e.key)
        .bind(&e.name)
        .bind(&e.description)
        .bind(&e.hypothesis)
        .bind(e.status.as_str())
        .bind(serde_json::to_value(&e.condition).map_err(Error::internal)?)
        .bind(&e.audiences)
        .bind(e.percentage)
        .bind(serde_json::to_value(&e.variants).map_err(Error::internal)?)
        .bind(&e.control)
        .bind(&e.flag_key)
        .bind(&e.salt)
        .bind(e.starts_at)
        .bind(e.ends_at)
        .bind(&e.tags)
        .fetch_one(&self.pg)
        .await
        .map_err(|err| match map_sqlx(err) {
            Error::Conflict(_) => Error::conflict(format!("experiment {:?} already exists", e.key)),
            other => other,
        })?;
        Experiment::try_from(row)
    }

    pub async fn update(&self, e: &Experiment) -> Result<Experiment> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "UPDATE experiments SET name = $2, description = $3, hypothesis = $4, status = $5, condition = $6, audiences = $7, percentage = $8,
                                    variants = $9, control = $10, flag_key = $11, salt = $12, starts_at = $13, ends_at = $14, tags = $15,
                                    version = version + 1, updated_at = now()
             WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&e.key)
        .bind(&e.name)
        .bind(&e.description)
        .bind(&e.hypothesis)
        .bind(e.status.as_str())
        .bind(serde_json::to_value(&e.condition).map_err(Error::internal)?)
        .bind(&e.audiences)
        .bind(e.percentage)
        .bind(serde_json::to_value(&e.variants).map_err(Error::internal)?)
        .bind(&e.control)
        .bind(&e.flag_key)
        .bind(&e.salt)
        .bind(e.starts_at)
        .bind(e.ends_at)
        .bind(&e.tags)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Experiment::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("experiment {:?} not found", e.key)))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM experiments WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!("experiment {key:?} not found")));
        }
        Ok(())
    }

    // ---- assignments -------------------------------------------------------------------------

    /// Existing assignments of one identity: experiment id → variant key.
    pub async fn assignments_for(&self, identity_id: Uuid) -> Result<HashMap<Uuid, String>> {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT experiment_id, variant FROM experiment_assignments WHERE identity_id = $1",
        )
        .bind(identity_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().collect())
    }

    /// Current assignments as (experiment key, variant) pairs.
    pub async fn assignment_keys_for(&self, identity_id: Uuid) -> Result<Vec<(String, String)>> {
        sqlx::query_as(
            "SELECT e.key, a.variant FROM experiment_assignments a JOIN experiments e ON e.id = a.experiment_id
             WHERE a.identity_id = $1 AND e.status IN ('running', 'paused') ORDER BY e.key",
        )
        .bind(identity_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Insert an assignment unless one exists; returns the variant that is now stored
    /// (the existing one wins a race).
    pub async fn assign(
        &self,
        experiment_id: Uuid,
        identity_id: Uuid,
        variant: &str,
    ) -> Result<String> {
        let inserted: Option<(String,)> = sqlx::query_as(
            "INSERT INTO experiment_assignments (experiment_id, identity_id, variant) VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING RETURNING variant",
        )
        .bind(experiment_id)
        .bind(identity_id)
        .bind(variant)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        if let Some((v,)) = inserted {
            return Ok(v);
        }
        let existing: (String,) = sqlx::query_as("SELECT variant FROM experiment_assignments WHERE experiment_id = $1 AND identity_id = $2")
            .bind(experiment_id)
            .bind(identity_id)
            .fetch_one(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(existing.0)
    }

    /// Admin: pin a variant regardless of targeting.
    pub async fn force_assign(
        &self,
        experiment_id: Uuid,
        identity_id: Uuid,
        variant: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO experiment_assignments (experiment_id, identity_id, variant) VALUES ($1, $2, $3)
             ON CONFLICT (experiment_id, identity_id) DO UPDATE SET variant = EXCLUDED.variant, assigned_at = now(), exposed_at = NULL, exposures = 0",
        )
        .bind(experiment_id)
        .bind(identity_id)
        .bind(variant)
        .execute(&self.pg)
        .await
        .map_err(|e| match map_sqlx(e) {
            Error::Internal(inner) if inner.to_string().contains("foreign key") => Error::not_found("identity not found"),
            other => other,
        })?;
        Ok(())
    }

    pub async fn get_assignment(
        &self,
        experiment_id: Uuid,
        identity_id: Uuid,
    ) -> Result<Option<AssignmentRow>> {
        sqlx::query_as("SELECT experiment_id, identity_id, variant, assigned_at, exposed_at, exposures FROM experiment_assignments WHERE experiment_id = $1 AND identity_id = $2")
            .bind(experiment_id)
            .bind(identity_id)
            .fetch_optional(&self.pg)
            .await
            .map_err(map_sqlx)
    }

    pub async fn delete_assignment(&self, experiment_id: Uuid, identity_id: Uuid) -> Result<()> {
        let res = sqlx::query(
            "DELETE FROM experiment_assignments WHERE experiment_id = $1 AND identity_id = $2",
        )
        .bind(experiment_id)
        .bind(identity_id)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("assignment not found"));
        }
        Ok(())
    }

    /// Record that the identity actually saw the variant. Returns false when not assigned.
    pub async fn expose(&self, experiment_id: Uuid, identity_id: Uuid) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE experiment_assignments SET exposures = exposures + 1, exposed_at = COALESCE(exposed_at, now())
             WHERE experiment_id = $1 AND identity_id = $2",
        )
        .bind(experiment_id)
        .bind(identity_id)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn results(&self, experiment_id: Uuid) -> Result<Vec<VariantStats>> {
        sqlx::query_as(
            "SELECT variant, count(*) AS assigned, count(exposed_at) AS exposed, COALESCE(sum(exposures), 0)::bigint AS exposures
             FROM experiment_assignments WHERE experiment_id = $1 GROUP BY variant ORDER BY variant",
        )
        .bind(experiment_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Drop every assignment (e.g. when an experiment is restarted with a new salt).
    pub async fn reset(&self, experiment_id: Uuid) -> Result<u64> {
        let res = sqlx::query("DELETE FROM experiment_assignments WHERE experiment_id = $1")
            .bind(experiment_id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(res.rows_affected())
    }
}
