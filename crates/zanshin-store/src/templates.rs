use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
    templates::{Format, Template, Translation},
    Error, Result,
};

use crate::map_sqlx;

#[derive(FromRow)]
struct Row {
    id: Uuid,
    key: String,
    name: String,
    description: String,
    format: String,
    default_locale: String,
    translations: Value,
    image_url: String,
    payload: Value,
    tags: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<Row> for Template {
    type Error = Error;
    fn try_from(r: Row) -> Result<Self> {
        Ok(Template {
            id: r.id,
            key: r.key,
            name: r.name,
            description: r.description,
            format: r.format.parse::<Format>()?,
            default_locale: r.default_locale,
            translations:
                serde_json::from_value::<std::collections::BTreeMap<String, Translation>>(
                    r.translations,
                )
                .map_err(|e| Error::invalid(format!("bad translations: {e}")))?,
            image_url: r.image_url,
            payload: r.payload,
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const COLS: &str = "id, key, name, description, format, default_locale, translations, image_url, payload, tags, version, created_at, updated_at";

#[derive(Clone)]
pub struct TemplateRepo {
    pg: PgPool,
}

impl TemplateRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<Template>> {
        let rows: Vec<Row> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM message_templates ORDER BY key"
        ))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        rows.into_iter().map(Template::try_from).collect()
    }

    pub async fn get(&self, key: &str) -> Result<Template> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM message_templates WHERE key = $1"
        ))
        .bind(key)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Template::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("message_templates {key:?} not found")))
    }

    pub async fn create(&self, m: &Template) -> Result<Template> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO message_templates (id, key, name, description, format, default_locale, translations, image_url, payload, tags) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING {COLS}"
        ))
        .bind(m.id)
        .bind(&m.key)
        .bind(&m.name)
        .bind(&m.description)
        .bind(m.format.as_str())
        .bind(&m.default_locale)
        .bind(serde_json::to_value(&m.translations).unwrap_or(Value::Null))
        .bind(&m.image_url)
        .bind(&m.payload)
        .bind(&m.tags)
        .fetch_one(&self.pg)
        .await
        .map_err(|err| match map_sqlx(err) {
            Error::Conflict(_) => Error::conflict(format!("message_templates {:?} already exists", m.key)),
            other => other,
        })?;
        Template::try_from(row)
    }

    pub async fn update(&self, m: &Template) -> Result<Template> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "UPDATE message_templates SET name = $2, description = $3, format = $4, default_locale = $5, translations = $6, image_url = $7, payload = $8, tags = $9, version = version + 1, updated_at = now() WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&m.key)
        .bind(&m.name)
        .bind(&m.description)
        .bind(m.format.as_str())
        .bind(&m.default_locale)
        .bind(serde_json::to_value(&m.translations).unwrap_or(Value::Null))
        .bind(&m.image_url)
        .bind(&m.payload)
        .bind(&m.tags)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Template::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("message_templates {:?} not found", m.key)))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM message_templates WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!(
                "message_templates {key:?} not found"
            )));
        }
        Ok(())
    }

    /// Campaign keys rendering from this template.
    pub async fn used_by(&self, key: &str) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT key FROM messages WHERE template_key = $1 ORDER BY key")
                .bind(key)
                .fetch_all(&self.pg)
                .await
                .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(|(k,)| k).collect())
    }
}
