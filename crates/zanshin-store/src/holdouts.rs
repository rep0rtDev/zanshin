//! Holdout groups: deterministic slices of players kept out of the experiments that reference them.

use sqlx::{FromRow, PgPool};
use zanshin_core::{experiments::Holdout, Error, Result};

use crate::map_sqlx;

#[derive(FromRow)]
struct Row {
    key: String,
    name: String,
    description: String,
    percentage: f64,
    salt: String,
    enabled: bool,
    version: i64,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<Row> for Holdout {
    fn from(r: Row) -> Self {
        Holdout {
            key: r.key,
            name: r.name,
            description: r.description,
            percentage: r.percentage,
            salt: r.salt,
            enabled: r.enabled,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

const COLS: &str =
    "key, name, description, percentage, salt, enabled, version, created_at, updated_at";

#[derive(Clone)]
pub struct HoldoutRepo {
    pg: PgPool,
}

impl HoldoutRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<Holdout>> {
        let rows: Vec<Row> = sqlx::query_as(&format!("SELECT {COLS} FROM holdouts ORDER BY key"))
            .fetch_all(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(Holdout::from).collect())
    }

    pub async fn get(&self, key: &str) -> Result<Holdout> {
        let row: Option<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM holdouts WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(Holdout::from)
            .ok_or_else(|| Error::not_found(format!("holdout {key:?} not found")))
    }

    pub async fn create(&self, h: &Holdout) -> Result<Holdout> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO holdouts (key, name, description, percentage, salt, enabled)
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING {COLS}"
        ))
        .bind(&h.key)
        .bind(&h.name)
        .bind(&h.description)
        .bind(h.percentage)
        .bind(&h.salt)
        .bind(h.enabled)
        .fetch_one(&self.pg)
        .await
        .map_err(|e| match map_sqlx(e) {
            Error::Conflict(_) => Error::conflict(format!("holdout {:?} already exists", h.key)),
            other => other,
        })?;
        Ok(row.into())
    }

    pub async fn update(&self, h: &Holdout) -> Result<Holdout> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "UPDATE holdouts SET name = $2, description = $3, percentage = $4, salt = $5, enabled = $6,
                                 version = version + 1, updated_at = now()
             WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&h.key)
        .bind(&h.name)
        .bind(&h.description)
        .bind(h.percentage)
        .bind(&h.salt)
        .bind(h.enabled)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Holdout::from)
            .ok_or_else(|| Error::not_found(format!("holdout {:?} not found", h.key)))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM holdouts WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!("holdout {key:?} not found")));
        }
        Ok(())
    }

    /// Experiment keys referencing this holdout (for the 409-on-delete check).
    pub async fn used_by(&self, key: &str) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT key FROM experiments WHERE holdout = $1 ORDER BY key")
                .bind(key)
                .fetch_all(&self.pg)
                .await
                .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }
}
