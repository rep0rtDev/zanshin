use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{audiences::Audience, rules::Condition, Error, Result};

use crate::map_sqlx;

#[derive(FromRow)]
struct Row {
    id: Uuid,
    key: String,
    name: String,
    description: String,
    condition: Value,
    percentage: f64,
    enabled: bool,
    salt: String,
    tags: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<Row> for Audience {
    type Error = Error;
    fn try_from(r: Row) -> Result<Self> {
        let condition: Condition = serde_json::from_value(r.condition).map_err(Error::internal)?;
        Ok(Audience {
            id: r.id,
            key: r.key,
            name: r.name,
            description: r.description,
            condition,
            percentage: r.percentage,
            enabled: r.enabled,
            salt: r.salt,
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const COLS: &str = "id, key, name, description, condition, percentage, enabled, salt, tags, version, created_at, updated_at";

#[derive(Clone)]
pub struct AudienceRepo {
    pg: PgPool,
}

impl AudienceRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<Audience>> {
        let rows: Vec<Row> = sqlx::query_as(&format!("SELECT {COLS} FROM audiences ORDER BY key"))
            .fetch_all(&self.pg)
            .await
            .map_err(map_sqlx)?;
        rows.into_iter().map(Audience::try_from).collect()
    }

    pub async fn get(&self, key: &str) -> Result<Audience> {
        let row: Option<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM audiences WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(Audience::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("audience {key:?} not found")))
    }

    pub async fn create(&self, a: &Audience) -> Result<Audience> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO audiences (id, key, name, description, condition, percentage, enabled, salt, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {COLS}"
        ))
        .bind(a.id)
        .bind(&a.key)
        .bind(&a.name)
        .bind(&a.description)
        .bind(serde_json::to_value(&a.condition).map_err(Error::internal)?)
        .bind(a.percentage)
        .bind(a.enabled)
        .bind(&a.salt)
        .bind(&a.tags)
        .fetch_one(&self.pg)
        .await
        .map_err(|e| match map_sqlx(e) {
            Error::Conflict(_) => Error::conflict(format!("audience {:?} already exists", a.key)),
            other => other,
        })?;
        Audience::try_from(row)
    }

    pub async fn update(&self, a: &Audience) -> Result<Audience> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "UPDATE audiences SET name = $2, description = $3, condition = $4, percentage = $5, enabled = $6, salt = $7, tags = $8,
                                  version = version + 1, updated_at = now()
             WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&a.key)
        .bind(&a.name)
        .bind(&a.description)
        .bind(serde_json::to_value(&a.condition).map_err(Error::internal)?)
        .bind(a.percentage)
        .bind(a.enabled)
        .bind(&a.salt)
        .bind(&a.tags)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Audience::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("audience {:?} not found", a.key)))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM audiences WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!("audience {key:?} not found")));
        }
        Ok(())
    }
}
