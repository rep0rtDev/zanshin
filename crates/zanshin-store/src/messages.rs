use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
    messages::{InboxMessage, MessageCampaign},
    rules::Condition,
    Error, Result,
};

use crate::map_sqlx;

#[derive(FromRow)]
struct Row {
    id: Uuid,
    key: String,
    name: String,
    enabled: bool,
    title: String,
    body: String,
    image_url: String,
    payload: Value,
    condition: Value,
    audiences: Vec<String>,
    percentage: f64,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    ttl_secs: Option<i64>,
    priority: i32,
    tags: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<Row> for MessageCampaign {
    type Error = Error;
    fn try_from(r: Row) -> Result<Self> {
        let condition: Condition = serde_json::from_value(r.condition).map_err(Error::internal)?;
        Ok(MessageCampaign {
            id: r.id,
            key: r.key,
            name: r.name,
            enabled: r.enabled,
            title: r.title,
            body: r.body,
            image_url: r.image_url,
            payload: r.payload,
            condition,
            audiences: r.audiences,
            percentage: r.percentage,
            starts_at: r.starts_at,
            ends_at: r.ends_at,
            ttl_secs: r.ttl_secs,
            priority: r.priority,
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

#[derive(FromRow)]
struct InboxRow {
    id: Uuid,
    key: Option<String>,
    title: String,
    body: String,
    image_url: String,
    payload: Value,
    priority: i32,
    delivered_at: DateTime<Utc>,
    read_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
}

impl From<InboxRow> for InboxMessage {
    fn from(r: InboxRow) -> Self {
        InboxMessage {
            id: r.id,
            key: r.key,
            title: r.title,
            body: r.body,
            image_url: r.image_url,
            payload: r.payload,
            priority: r.priority,
            delivered_at: r.delivered_at,
            read_at: r.read_at,
            expires_at: r.expires_at,
        }
    }
}

const COLS: &str = "id, key, name, enabled, title, body, image_url, payload, condition, audiences, percentage, starts_at, ends_at, ttl_secs, priority, tags, version, created_at, updated_at";
const INBOX_COLS: &str =
    "id, key, title, body, image_url, payload, priority, delivered_at, read_at, expires_at";

/// A rendered copy ready to be inserted into an inbox.
pub struct Delivery<'a> {
    pub message_id: Option<Uuid>,
    pub key: Option<&'a str>,
    pub title: &'a str,
    pub body: &'a str,
    pub image_url: &'a str,
    pub payload: &'a Value,
    pub priority: i32,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct MessageRepo {
    pg: PgPool,
}

impl MessageRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<MessageCampaign>> {
        let rows: Vec<Row> = sqlx::query_as(&format!("SELECT {COLS} FROM messages ORDER BY key"))
            .fetch_all(&self.pg)
            .await
            .map_err(map_sqlx)?;
        rows.into_iter().map(MessageCampaign::try_from).collect()
    }

    pub async fn get(&self, key: &str) -> Result<MessageCampaign> {
        let row: Option<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM messages WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(MessageCampaign::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("message {key:?} not found")))
    }

    pub async fn create(&self, m: &MessageCampaign) -> Result<MessageCampaign> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO messages (id, key, name, enabled, title, body, image_url, payload, condition, audiences, percentage, starts_at, ends_at, ttl_secs, priority, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) RETURNING {COLS}"
        ))
        .bind(m.id)
        .bind(&m.key)
        .bind(&m.name)
        .bind(m.enabled)
        .bind(&m.title)
        .bind(&m.body)
        .bind(&m.image_url)
        .bind(&m.payload)
        .bind(serde_json::to_value(&m.condition).map_err(Error::internal)?)
        .bind(&m.audiences)
        .bind(m.percentage)
        .bind(m.starts_at)
        .bind(m.ends_at)
        .bind(m.ttl_secs)
        .bind(m.priority)
        .bind(&m.tags)
        .fetch_one(&self.pg)
        .await
        .map_err(|err| match map_sqlx(err) {
            Error::Conflict(_) => Error::conflict(format!("message {:?} already exists", m.key)),
            other => other,
        })?;
        MessageCampaign::try_from(row)
    }

    pub async fn update(&self, m: &MessageCampaign) -> Result<MessageCampaign> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "UPDATE messages SET name = $2, enabled = $3, title = $4, body = $5, image_url = $6, payload = $7, condition = $8, audiences = $9,
                                 percentage = $10, starts_at = $11, ends_at = $12, ttl_secs = $13, priority = $14, tags = $15,
                                 version = version + 1, updated_at = now()
             WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&m.key)
        .bind(&m.name)
        .bind(m.enabled)
        .bind(&m.title)
        .bind(&m.body)
        .bind(&m.image_url)
        .bind(&m.payload)
        .bind(serde_json::to_value(&m.condition).map_err(Error::internal)?)
        .bind(&m.audiences)
        .bind(m.percentage)
        .bind(m.starts_at)
        .bind(m.ends_at)
        .bind(m.ttl_secs)
        .bind(m.priority)
        .bind(&m.tags)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(MessageCampaign::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("message {:?} not found", m.key)))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM messages WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!("message {key:?} not found")));
        }
        Ok(())
    }

    // ---- inbox -------------------------------------------------------------------------------

    /// Campaign ids already delivered to this identity (consumed or not).
    pub async fn delivered_campaigns(&self, identity_id: Uuid) -> Result<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT message_id FROM inbox WHERE identity_id = $1 AND message_id IS NOT NULL",
        )
        .bind(identity_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Insert a copy; a campaign copy that already exists is left untouched. Returns the inbox id.
    pub async fn deliver(&self, identity_id: Uuid, d: Delivery<'_>) -> Result<Option<Uuid>> {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "INSERT INTO inbox (id, identity_id, message_id, key, title, body, image_url, payload, priority, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (identity_id, message_id) WHERE message_id IS NOT NULL DO NOTHING RETURNING id",
        )
        .bind(Uuid::now_v7())
        .bind(identity_id)
        .bind(d.message_id)
        .bind(d.key)
        .bind(d.title)
        .bind(d.body)
        .bind(d.image_url)
        .bind(d.payload)
        .bind(d.priority)
        .bind(d.expires_at)
        .fetch_optional(&self.pg)
        .await
        .map_err(|e| match map_sqlx(e) {
            Error::Internal(inner) if inner.to_string().contains("foreign key") => Error::not_found("identity not found"),
            other => other,
        })?;
        Ok(row.map(|r| r.0))
    }

    pub async fn inbox(&self, identity_id: Uuid, limit: i64) -> Result<Vec<InboxMessage>> {
        let rows: Vec<InboxRow> = sqlx::query_as(&format!(
            "SELECT {INBOX_COLS} FROM inbox WHERE identity_id = $1 AND consumed_at IS NULL AND (expires_at IS NULL OR expires_at > now())
             ORDER BY priority DESC, delivered_at DESC LIMIT $2"
        ))
        .bind(identity_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn mark_read(&self, identity_id: Uuid, id: Uuid) -> Result<InboxMessage> {
        let row: Option<InboxRow> = sqlx::query_as(&format!(
            "UPDATE inbox SET read_at = COALESCE(read_at, now()) WHERE id = $1 AND identity_id = $2 AND consumed_at IS NULL RETURNING {INBOX_COLS}"
        ))
        .bind(id)
        .bind(identity_id)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Into::into)
            .ok_or_else(|| Error::not_found("message not found"))
    }

    pub async fn consume(&self, identity_id: Uuid, id: Uuid) -> Result<()> {
        let res = sqlx::query("UPDATE inbox SET consumed_at = now(), read_at = COALESCE(read_at, now()) WHERE id = $1 AND identity_id = $2 AND consumed_at IS NULL")
            .bind(id)
            .bind(identity_id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("message not found"));
        }
        Ok(())
    }

    pub async fn campaign_stats(&self, message_id: Uuid) -> Result<(i64, i64, i64)> {
        let row: (i64, i64, i64) = sqlx::query_as(
            "SELECT count(*), count(read_at), count(consumed_at) FROM inbox WHERE message_id = $1",
        )
        .bind(message_id)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(row)
    }

    pub async fn purge_expired(&self) -> Result<u64> {
        let res = sqlx::query("DELETE FROM inbox WHERE (expires_at IS NOT NULL AND expires_at < now() - interval '7 days') OR consumed_at < now() - interval '30 days'")
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(res.rows_affected())
    }
}
