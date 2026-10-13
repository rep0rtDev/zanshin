use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
    flags::{Flag, Rule, ValueType},
    Error, Result,
};

use crate::map_sqlx;

#[derive(FromRow)]
struct FlagRow {
    id: Uuid,
    key: String,
    name: String,
    description: String,
    value_type: String,
    default_value: Value,
    enabled: bool,
    rules: Value,
    salt: String,
    tags: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<FlagRow> for Flag {
    type Error = Error;
    fn try_from(r: FlagRow) -> Result<Self> {
        let rules: Vec<Rule> = serde_json::from_value(r.rules).map_err(Error::internal)?;
        Ok(Flag {
            id: r.id,
            key: r.key,
            name: r.name,
            description: r.description,
            value_type: ValueType::parse(&r.value_type)?,
            default_value: r.default_value,
            enabled: r.enabled,
            rules,
            salt: r.salt,
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const COLS: &str = "id, key, name, description, value_type, default_value, enabled, rules, salt, tags, version, created_at, updated_at";

/// Fields an admin may set; everything else is derived.
#[derive(Debug, Clone, Default)]
pub struct FlagInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub value_type: Option<ValueType>,
    pub default_value: Option<Value>,
    pub enabled: Option<bool>,
    pub rules: Option<Vec<Rule>>,
    pub salt: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct FlagRepo {
    pg: PgPool,
}

impl FlagRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<Flag>> {
        let rows: Vec<FlagRow> = sqlx::query_as(&format!("SELECT {COLS} FROM flags ORDER BY key"))
            .fetch_all(&self.pg)
            .await
            .map_err(map_sqlx)?;
        rows.into_iter().map(Flag::try_from).collect()
    }

    pub async fn get(&self, key: &str) -> Result<Flag> {
        let row: Option<FlagRow> =
            sqlx::query_as(&format!("SELECT {COLS} FROM flags WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(Flag::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("flag {key:?} not found")))
    }

    pub async fn create(&self, flag: &Flag) -> Result<Flag> {
        let row: FlagRow = sqlx::query_as(&format!(
            "INSERT INTO flags (id, key, name, description, value_type, default_value, enabled, rules, salt, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING {COLS}"
        ))
        .bind(flag.id)
        .bind(&flag.key)
        .bind(&flag.name)
        .bind(&flag.description)
        .bind(flag.value_type.as_str())
        .bind(&flag.default_value)
        .bind(flag.enabled)
        .bind(serde_json::to_value(&flag.rules).map_err(Error::internal)?)
        .bind(&flag.salt)
        .bind(&flag.tags)
        .fetch_one(&self.pg)
        .await
        .map_err(|e| match map_sqlx(e) {
            Error::Conflict(_) => Error::conflict(format!("flag {:?} already exists", flag.key)),
            other => other,
        })?;
        Flag::try_from(row)
    }

    /// Full replace of the mutable fields; bumps `version`.
    pub async fn update(&self, flag: &Flag) -> Result<Flag> {
        let row: Option<FlagRow> = sqlx::query_as(&format!(
            "UPDATE flags SET name = $2, description = $3, value_type = $4, default_value = $5, enabled = $6, rules = $7, salt = $8, tags = $9,
                              version = version + 1, updated_at = now()
             WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&flag.key)
        .bind(&flag.name)
        .bind(&flag.description)
        .bind(flag.value_type.as_str())
        .bind(&flag.default_value)
        .bind(flag.enabled)
        .bind(serde_json::to_value(&flag.rules).map_err(Error::internal)?)
        .bind(&flag.salt)
        .bind(&flag.tags)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Flag::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("flag {:?} not found", flag.key)))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM flags WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!("flag {key:?} not found")));
        }
        Ok(())
    }

    /// All overrides for one identity, keyed by flag key.
    pub async fn overrides_for(&self, identity_id: Uuid) -> Result<HashMap<String, Value>> {
        let rows: Vec<(String, Value)> =
            sqlx::query_as("SELECT f.key, o.value FROM flag_overrides o JOIN flags f ON f.id = o.flag_id WHERE o.identity_id = $1")
                .bind(identity_id)
                .fetch_all(&self.pg)
                .await
                .map_err(map_sqlx)?;
        Ok(rows.into_iter().collect())
    }

    pub async fn set_override(
        &self,
        flag_id: Uuid,
        identity_id: Uuid,
        value: &Value,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO flag_overrides (flag_id, identity_id, value) VALUES ($1, $2, $3)
             ON CONFLICT (flag_id, identity_id) DO UPDATE SET value = EXCLUDED.value, created_at = now()",
        )
        .bind(flag_id)
        .bind(identity_id)
        .bind(value)
        .execute(&self.pg)
        .await
        .map_err(|e| match map_sqlx(e) {
            Error::Internal(inner) if inner.to_string().contains("foreign key") => Error::not_found("identity not found"),
            other => other,
        })?;
        Ok(())
    }

    pub async fn delete_override(&self, flag_id: Uuid, identity_id: Uuid) -> Result<()> {
        let res = sqlx::query("DELETE FROM flag_overrides WHERE flag_id = $1 AND identity_id = $2")
            .bind(flag_id)
            .bind(identity_id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("override not found"));
        }
        Ok(())
    }

    pub async fn list_overrides(
        &self,
        flag_id: Uuid,
        limit: i64,
    ) -> Result<Vec<(Uuid, String, Value)>> {
        sqlx::query_as(
            "SELECT o.identity_id, i.external_id, o.value FROM flag_overrides o JOIN identities i ON i.id = o.identity_id
             WHERE o.flag_id = $1 ORDER BY o.created_at DESC LIMIT $2",
        )
        .bind(flag_id)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }
}
