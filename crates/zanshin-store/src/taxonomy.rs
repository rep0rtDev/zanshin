//! Taxonomy: category labels, event definitions, value schemas, property configuration,
//! instance settings, play sessions and identity merges.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{Error, Result};

use crate::map_sqlx;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Label {
    pub id: Uuid,
    pub name: String,
    pub color: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LabelInput {
    pub name: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EventDefinition {
    pub name: String,
    pub description: String,
    pub value_type: String,
    pub metadata_schema: Option<Value>,
    pub exclude_from_activity: bool,
    pub is_core: bool,
    pub discovered: bool,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventDefinitionInput {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub value_type: Option<String>,
    #[serde(default)]
    pub metadata_schema: Option<Value>,
    #[serde(default)]
    pub exclude_from_activity: Option<bool>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

pub const VALUE_TYPES: &[&str] = &["any", "number", "string", "object", "revenue"];

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ValueSchema {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub schema: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ValueSchemaInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PropertyConfig {
    pub key: String,
    pub namespace: String,
    pub read_only: bool,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PropertyConfigInput {
    #[serde(default = "default_ns")]
    pub namespace: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub description: String,
}

fn default_ns() -> String {
    "custom".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PlaySession {
    pub id: Uuid,
    pub identity_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_sec: Option<i32>,
    pub platform: String,
    pub app_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct IdentityMerge {
    pub id: Uuid,
    pub from_identity: Uuid,
    pub from_external: String,
    pub into_identity: Uuid,
    pub into_external: String,
    pub merged_at: DateTime<Utc>,
}

const LABEL_COLS: &str = "id, name, color, description, created_at, updated_at";
const EVDEF_COLS: &str = "name, description, value_type, metadata_schema, exclude_from_activity, is_core, discovered, tags, created_at, updated_at";
const SCHEMA_COLS: &str = "id, name, description, schema, created_at, updated_at";
const PROP_COLS: &str = "key, namespace, read_only, description, created_at, updated_at";
const SESSION_COLS: &str =
    "id, identity_id, started_at, last_seen_at, ended_at, duration_sec, platform, app_version";

fn prefix_cols(cols: &str, alias: &str) -> String {
    cols.split(", ")
        .map(|c| format!("{alias}.{c}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Clone)]
pub struct TaxonomyRepo {
    pg: PgPool,
}

impl TaxonomyRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    // ---- labels -------------------------------------------------------------------------

    pub async fn list_labels(&self) -> Result<Vec<Label>> {
        sqlx::query_as(&format!("SELECT {LABEL_COLS} FROM labels ORDER BY name"))
            .fetch_all(&self.pg)
            .await
            .map_err(map_sqlx)
    }

    pub async fn create_label(&self, input: LabelInput) -> Result<Label> {
        validate_label_name(&input.name)?;
        sqlx::query_as(&format!(
            "INSERT INTO labels (id, name, color, description) VALUES ($1, $2, $3, $4) RETURNING {LABEL_COLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(&input.name)
        .bind(&input.color)
        .bind(&input.description)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn update_label(&self, id: Uuid, input: LabelInput) -> Result<Label> {
        validate_label_name(&input.name)?;
        let old: Option<(String,)> = sqlx::query_as("SELECT name FROM labels WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pg)
            .await
            .map_err(map_sqlx)?;
        let Some((old_name,)) = old else {
            return Err(Error::not_found("label"));
        };
        let mut tx = self.pg.begin().await.map_err(map_sqlx)?;
        let row: Label = sqlx::query_as(&format!(
            "UPDATE labels SET name = $2, color = $3, description = $4, updated_at = now() WHERE id = $1 RETURNING {LABEL_COLS}"
        ))
        .bind(id)
        .bind(&input.name)
        .bind(&input.color)
        .bind(&input.description)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        if old_name != input.name {
            for table in TAGGED_TABLES {
                sqlx::query(&format!(
                    "UPDATE {table} SET tags = array_replace(tags, $1, $2) WHERE $1 = ANY(tags)"
                ))
                .bind(&old_name)
                .bind(&input.name)
                .execute(&mut *tx)
                .await
                .map_err(map_sqlx)?;
            }
        }
        tx.commit().await.map_err(map_sqlx)?;
        Ok(row)
    }

    pub async fn delete_label(&self, id: Uuid) -> Result<()> {
        let mut tx = self.pg.begin().await.map_err(map_sqlx)?;
        let old: Option<(String,)> =
            sqlx::query_as("DELETE FROM labels WHERE id = $1 RETURNING name")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_sqlx)?;
        let Some((name,)) = old else {
            return Err(Error::not_found("label"));
        };
        for table in TAGGED_TABLES {
            sqlx::query(&format!(
                "UPDATE {table} SET tags = array_remove(tags, $1) WHERE $1 = ANY(tags)"
            ))
            .bind(&name)
            .execute(&mut *tx)
            .await
            .map_err(map_sqlx)?;
        }
        tx.commit().await.map_err(map_sqlx)
    }

    /// Where a label is used: table → number of rows carrying it.
    pub async fn label_usage(&self, name: &str) -> Result<Vec<(String, i64)>> {
        let mut out = Vec::new();
        for table in TAGGED_TABLES {
            let (n,): (i64,) = sqlx::query_as(&format!(
                "SELECT count(*) FROM {table} WHERE $1 = ANY(tags)"
            ))
            .bind(name)
            .fetch_one(&self.pg)
            .await
            .map_err(map_sqlx)?;
            out.push((table.to_string(), n));
        }
        Ok(out)
    }

    /// Make sure every tag used on an entity exists in the registry (auto-registers new ones).
    pub async fn ensure_labels(&self, tags: &[String]) -> Result<()> {
        for t in tags {
            validate_label_name(t)?;
            sqlx::query(
                "INSERT INTO labels (id, name) VALUES ($1, $2) ON CONFLICT (name) DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(t)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        }
        Ok(())
    }

    /// Register every tag used on any entity as a label (idempotent).
    pub async fn sync_labels(&self) -> Result<u64> {
        let mut n = 0;
        for table in TAGGED_TABLES {
            let r = sqlx::query(&format!(
                "INSERT INTO labels (id, name)
                 SELECT gen_random_uuid(), t FROM (SELECT DISTINCT unnest(tags) AS t FROM {table}) x
                 WHERE t <> '' ON CONFLICT (name) DO NOTHING"
            ))
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
            n += r.rows_affected();
        }
        Ok(n)
    }

    // ---- event definitions --------------------------------------------------------------

    pub async fn list_event_definitions(&self) -> Result<Vec<EventDefinition>> {
        sqlx::query_as(&format!(
            "SELECT {EVDEF_COLS} FROM event_definitions ORDER BY is_core DESC, name"
        ))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn get_event_definition(&self, name: &str) -> Result<Option<EventDefinition>> {
        sqlx::query_as(&format!(
            "SELECT {EVDEF_COLS} FROM event_definitions WHERE name = $1"
        ))
        .bind(name)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn upsert_event_definition(
        &self,
        name: &str,
        input: EventDefinitionInput,
    ) -> Result<EventDefinition> {
        let vt = input.value_type.clone().unwrap_or_else(|| "any".into());
        if !VALUE_TYPES.contains(&vt.as_str()) {
            return Err(Error::invalid(format!("unknown value_type {vt:?}")));
        }
        if let Some(existing) = self.get_event_definition(name).await? {
            if existing.is_core
                && (input
                    .value_type
                    .as_ref()
                    .is_some_and(|v| *v != existing.value_type)
                    || input
                        .metadata_schema
                        .as_ref()
                        .is_some_and(|s| Some(s) != existing.metadata_schema.as_ref()))
            {
                return Err(Error::invalid(format!(
                    "{name:?} is a core event: its value type and metadata schema are immutable (description, tags and activity flag can change)"
                )));
            }
        }
        if let Some(s) = &input.metadata_schema {
            if !s.is_null() && !s.is_object() {
                return Err(Error::invalid(
                    "metadata_schema must be a JSON Schema object",
                ));
            }
        }
        sqlx::query_as(&format!(
            "INSERT INTO event_definitions (name, description, value_type, metadata_schema, exclude_from_activity, tags)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (name) DO UPDATE SET
               description = COALESCE($7, event_definitions.description),
               value_type = COALESCE($8, event_definitions.value_type),
               metadata_schema = CASE WHEN $9 THEN $4 ELSE event_definitions.metadata_schema END,
               exclude_from_activity = COALESCE($10, event_definitions.exclude_from_activity),
               tags = COALESCE($11, event_definitions.tags),
               discovered = false,
               updated_at = now()
             RETURNING {EVDEF_COLS}"
        ))
        .bind(name)
        .bind(input.description.clone().unwrap_or_default())
        .bind(&vt)
        .bind(input.metadata_schema.clone().filter(|v| !v.is_null()))
        .bind(input.exclude_from_activity.unwrap_or(false))
        .bind(input.tags.clone().unwrap_or_default())
        .bind(input.description)
        .bind(input.value_type)
        .bind(input.metadata_schema.is_some())
        .bind(input.exclude_from_activity)
        .bind(input.tags)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Register a never-seen event name as discovered (value_type any). No-op if it exists.
    pub async fn discover_event(&self, name: &str) -> Result<bool> {
        let r = sqlx::query(
            "INSERT INTO event_definitions (name, discovered) VALUES ($1, true) ON CONFLICT (name) DO NOTHING",
        )
        .bind(name)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(r.rows_affected() > 0)
    }

    pub async fn delete_event_definition(&self, name: &str) -> Result<()> {
        let r = sqlx::query("DELETE FROM event_definitions WHERE name = $1 AND NOT is_core")
            .bind(name)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if r.rows_affected() == 0 {
            return Err(Error::not_found(
                "event definition (core events cannot be deleted)",
            ));
        }
        Ok(())
    }

    // ---- value schemas ------------------------------------------------------------------

    pub async fn list_schemas(&self) -> Result<Vec<ValueSchema>> {
        sqlx::query_as(&format!(
            "SELECT {SCHEMA_COLS} FROM value_schemas ORDER BY name"
        ))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn get_schema(&self, id: Uuid) -> Result<Option<ValueSchema>> {
        sqlx::query_as(&format!(
            "SELECT {SCHEMA_COLS} FROM value_schemas WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn create_schema(&self, input: ValueSchemaInput) -> Result<ValueSchema> {
        validate_label_name(&input.name)?;
        sqlx::query_as(&format!(
            "INSERT INTO value_schemas (id, name, description, schema) VALUES ($1, $2, $3, $4) RETURNING {SCHEMA_COLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(&input.name)
        .bind(&input.description)
        .bind(&input.schema)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn update_schema(&self, id: Uuid, input: ValueSchemaInput) -> Result<ValueSchema> {
        validate_label_name(&input.name)?;
        sqlx::query_as(&format!(
            "UPDATE value_schemas SET name = $2, description = $3, schema = $4, updated_at = now() WHERE id = $1 RETURNING {SCHEMA_COLS}"
        ))
        .bind(id)
        .bind(&input.name)
        .bind(&input.description)
        .bind(&input.schema)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?
        .ok_or_else(|| Error::not_found("schema"))
    }

    pub async fn delete_schema(&self, id: Uuid) -> Result<()> {
        let r = sqlx::query("DELETE FROM value_schemas WHERE id = $1")
            .bind(id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if r.rows_affected() == 0 {
            return Err(Error::not_found("schema"));
        }
        Ok(())
    }

    pub async fn schema_usage(&self, id: Uuid) -> Result<(i64, i64)> {
        let (f,): (i64,) = sqlx::query_as("SELECT count(*) FROM flags WHERE schema_id = $1")
            .bind(id)
            .fetch_one(&self.pg)
            .await
            .map_err(map_sqlx)?;
        let (l,): (i64,) = sqlx::query_as("SELECT count(*) FROM live_events WHERE schema_id = $1")
            .bind(id)
            .fetch_one(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok((f, l))
    }

    // ---- property configuration ---------------------------------------------------------

    pub async fn list_property_config(&self) -> Result<Vec<PropertyConfig>> {
        sqlx::query_as(&format!(
            "SELECT {PROP_COLS} FROM property_config ORDER BY namespace, key"
        ))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn read_only_keys(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT key FROM property_config WHERE read_only AND namespace = 'custom'",
        )
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    pub async fn upsert_property_config(
        &self,
        key: &str,
        input: PropertyConfigInput,
    ) -> Result<PropertyConfig> {
        if !matches!(input.namespace.as_str(), "default" | "custom") {
            return Err(Error::invalid("namespace must be default or custom"));
        }
        sqlx::query_as(&format!(
            "INSERT INTO property_config (key, namespace, read_only, description) VALUES ($1, $2, $3, $4)
             ON CONFLICT (key) DO UPDATE SET namespace = $2, read_only = $3, description = $4, updated_at = now()
             RETURNING {PROP_COLS}"
        ))
        .bind(key)
        .bind(&input.namespace)
        .bind(input.read_only)
        .bind(&input.description)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn delete_property_config(&self, key: &str) -> Result<()> {
        let r = sqlx::query("DELETE FROM property_config WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if r.rows_affected() == 0 {
            return Err(Error::not_found("property"));
        }
        Ok(())
    }

    /// Property keys seen across identities, with a sample JSON type, per namespace.
    pub async fn observed_properties(
        &self,
        namespace: &str,
        limit: i64,
    ) -> Result<Vec<(String, String, i64)>> {
        let col = match namespace {
            "default" => "default_properties",
            "computed" => "computed_properties",
            "custom" => "custom_properties",
            _ => {
                return Err(Error::invalid(
                    "namespace must be default, computed or custom",
                ))
            }
        };
        let rows: Vec<(String, String, i64)> = sqlx::query_as(&format!(
            "SELECT k, max(jsonb_typeof(v)) AS t, count(*) AS n
             FROM (SELECT id FROM identities ORDER BY last_seen_at DESC LIMIT $1) i
             JOIN identities x ON x.id = i.id, jsonb_each(x.{col}) AS e(k, v)
             GROUP BY k ORDER BY n DESC, k LIMIT 500"
        ))
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows)
    }

    // ---- settings -----------------------------------------------------------------------

    pub async fn get_setting(&self, key: &str) -> Result<Option<Value>> {
        let r: Option<(Value,)> = sqlx::query_as("SELECT value FROM settings WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(r.map(|r| r.0))
    }

    pub async fn all_settings(&self) -> Result<Vec<(String, Value)>> {
        sqlx::query_as("SELECT key, value FROM settings ORDER BY key")
            .fetch_all(&self.pg)
            .await
            .map_err(map_sqlx)
    }

    pub async fn set_setting(&self, key: &str, value: &Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ($1, $2)
             ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = now()",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    // ---- play sessions ------------------------------------------------------------------

    pub async fn open_session(
        &self,
        id: Uuid,
        identity_id: Uuid,
        at: DateTime<Utc>,
        platform: &str,
        app_version: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO play_sessions (id, identity_id, started_at, last_seen_at, platform, app_version)
             VALUES ($1, $2, $3, $3, $4, $5) ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(identity_id)
        .bind(at)
        .bind(platform)
        .bind(app_version)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn touch_session(&self, id: Uuid, at: DateTime<Utc>) -> Result<()> {
        sqlx::query(
            "UPDATE play_sessions SET last_seen_at = GREATEST(last_seen_at, $2) WHERE id = $1 AND ended_at IS NULL",
        )
        .bind(id)
        .bind(at)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    /// Close a session; returns (identity_id, duration) when it was still open.
    pub async fn close_session(&self, id: Uuid, at: DateTime<Utc>) -> Result<Option<(Uuid, i32)>> {
        let r: Option<(Uuid, i32)> = sqlx::query_as(
            "UPDATE play_sessions
             SET ended_at = GREATEST($2, last_seen_at),
                 duration_sec = GREATEST(0, EXTRACT(EPOCH FROM GREATEST($2, last_seen_at) - started_at))::int
             WHERE id = $1 AND ended_at IS NULL
             RETURNING identity_id, duration_sec",
        )
        .bind(id)
        .bind(at)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(r)
    }

    /// Close sessions idle for longer than `idle_secs`. Returns the closed rows.
    pub async fn close_idle_sessions(
        &self,
        idle_secs: i64,
        limit: i64,
    ) -> Result<Vec<PlaySession>> {
        sqlx::query_as(&format!(
            "WITH c AS (
                SELECT id FROM play_sessions
                WHERE ended_at IS NULL AND last_seen_at < now() - make_interval(secs => $1)
                ORDER BY last_seen_at LIMIT $2
             )
             UPDATE play_sessions p
             SET ended_at = last_seen_at,
                 duration_sec = GREATEST(0, EXTRACT(EPOCH FROM last_seen_at - started_at))::int
             FROM c WHERE p.id = c.id
             RETURNING {}",
            prefix_cols(SESSION_COLS, "p")
        ))
        .bind(idle_secs as f64)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn sessions_for_identity(
        &self,
        identity_id: Uuid,
        limit: i64,
    ) -> Result<Vec<PlaySession>> {
        sqlx::query_as(&format!(
            "SELECT {SESSION_COLS} FROM play_sessions WHERE identity_id = $1 ORDER BY started_at DESC LIMIT $2"
        ))
        .bind(identity_id)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    // ---- merges -------------------------------------------------------------------------

    pub async fn record_merge(&self, m: &IdentityMerge) -> Result<()> {
        sqlx::query(
            "INSERT INTO identity_merges (id, from_identity, from_external, into_identity, into_external, merged_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(m.id)
        .bind(m.from_identity)
        .bind(&m.from_external)
        .bind(m.into_identity)
        .bind(&m.into_external)
        .bind(m.merged_at)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn merges_into(&self, identity_id: Uuid) -> Result<Vec<IdentityMerge>> {
        sqlx::query_as(
            "SELECT id, from_identity, from_external, into_identity, into_external, merged_at
             FROM identity_merges WHERE into_identity = $1 ORDER BY merged_at DESC",
        )
        .bind(identity_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }
}

pub const TAGGED_TABLES: &[&str] = &[
    "flags",
    "audiences",
    "experiments",
    "live_events",
    "messages",
    "event_definitions",
];

fn validate_label_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(Error::invalid("name must be 1..64 characters"));
    }
    if name.chars().any(|c| c.is_control() || c == ',') {
        return Err(Error::invalid(
            "name must not contain commas or control characters",
        ));
    }
    Ok(())
}
