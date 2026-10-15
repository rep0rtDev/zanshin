use chrono::{DateTime, Utc};
use serde::Serialize;
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
    push: bool,
    inbox: bool,
    template_key: Option<String>,
    schedule: Value,
    channels: Vec<String>,
    tags: Vec<String>,
    frequency_cap: Option<Value>,
    quiet_hours: Option<Value>,
    variants: Value,
    push_options: Option<Value>,
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
            push: r.push,
            inbox: r.inbox,
            template_key: r.template_key,
            schedule: serde_json::from_value(r.schedule).unwrap_or_default(),
            channels: r.channels,
            tags: r.tags,
            frequency_cap: r.frequency_cap.and_then(|v| serde_json::from_value(v).ok()),
            quiet_hours: r.quiet_hours.and_then(|v| serde_json::from_value(v).ok()),
            variants: serde_json::from_value(r.variants).unwrap_or_default(),
            push_options: r.push_options.and_then(|v| serde_json::from_value(v).ok()),
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

const COLS: &str = "id, key, name, enabled, title, body, image_url, payload, condition, audiences, percentage, starts_at, ends_at, ttl_secs, priority, push, inbox, template_key, schedule, channels, tags, version, created_at, updated_at, frequency_cap, quiet_hours, variants, push_options";
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
    pub occurrence: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct DeliveryRecord {
    pub id: Uuid,
    pub message_id: Uuid,
    pub identity_id: Uuid,
    pub external_id: String,
    pub occurrence: Option<DateTime<Utc>>,
    pub channel: String,
    pub status: String,
    pub detail: String,
    pub at: DateTime<Utc>,
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
            "INSERT INTO messages (id, key, name, enabled, title, body, image_url, payload, condition, audiences, percentage, starts_at, ends_at, ttl_secs, priority, tags, push, template_key, schedule, channels, inbox, frequency_cap, quiet_hours, variants, push_options)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25) RETURNING {COLS}"
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
        .bind(m.push)
        .bind(&m.template_key)
        .bind(serde_json::to_value(&m.schedule).unwrap_or(Value::Null))
        .bind(&m.channels)
        .bind(m.inbox)
        .bind(serde_json::to_value(&m.frequency_cap).ok().filter(|v| !v.is_null()))
        .bind(serde_json::to_value(&m.quiet_hours).ok().filter(|v| !v.is_null()))
        .bind(serde_json::to_value(&m.variants).unwrap_or(Value::Array(vec![])))
        .bind(serde_json::to_value(&m.push_options).ok().filter(|v| !v.is_null()))
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
                                 percentage = $10, starts_at = $11, ends_at = $12, ttl_secs = $13, priority = $14, tags = $15, push = $16, template_key = $17, schedule = $18, channels = $19, inbox = $20, frequency_cap = $21, quiet_hours = $22, variants = $23, push_options = $24,
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
        .bind(m.push)
        .bind(&m.template_key)
        .bind(serde_json::to_value(&m.schedule).unwrap_or(Value::Null))
        .bind(&m.channels)
        .bind(m.inbox)
        .bind(serde_json::to_value(&m.frequency_cap).ok().filter(|v| !v.is_null()))
        .bind(serde_json::to_value(&m.quiet_hours).ok().filter(|v| !v.is_null()))
        .bind(serde_json::to_value(&m.variants).unwrap_or(Value::Array(vec![])))
        .bind(serde_json::to_value(&m.push_options).ok().filter(|v| !v.is_null()))
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
            "INSERT INTO inbox (id, identity_id, message_id, key, title, body, image_url, payload, priority, expires_at, occurrence)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (identity_id, message_id, COALESCE(occurrence, 'epoch'::timestamptz)) WHERE message_id IS NOT NULL DO NOTHING RETURNING id",
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
        .bind(d.occurrence)
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

    /// (message_id, occurrence) pairs already delivered to an identity.
    pub async fn delivered_occurrences(
        &self,
        identity_id: Uuid,
    ) -> Result<Vec<(Uuid, Option<DateTime<Utc>>)>> {
        sqlx::query_as(
            "SELECT message_id, occurrence FROM inbox WHERE identity_id = $1 AND message_id IS NOT NULL",
        )
        .bind(identity_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// `(identity_id, message_id, occurrence)` for every delivered copy of the given campaigns at
    /// or after `since` (copies without an occurrence are always included). One query per sweep
    /// instead of one per identity.
    pub async fn delivered_for_messages(
        &self,
        message_ids: &[Uuid],
        since: DateTime<Utc>,
    ) -> Result<Vec<(Uuid, Uuid, Option<DateTime<Utc>>)>> {
        sqlx::query_as(
            "SELECT identity_id, message_id, occurrence FROM inbox
             WHERE message_id = ANY($1) AND (occurrence IS NULL OR occurrence >= $2)",
        )
        .bind(message_ids)
        .bind(since)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn delete_inbox(&self, identity_id: Uuid, id: Uuid) -> Result<bool> {
        let res = sqlx::query("DELETE FROM inbox WHERE identity_id = $1 AND id = $2")
            .bind(identity_id)
            .bind(id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn record_delivery(
        &self,
        message_id: Uuid,
        identity_id: Uuid,
        occurrence: Option<DateTime<Utc>>,
        channel: &str,
        status: &str,
        detail: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO message_deliveries (id, message_id, identity_id, occurrence, channel, status, detail, variant)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::now_v7())
        .bind(message_id)
        .bind(identity_id)
        .bind(occurrence)
        .bind(channel)
        .bind(status)
        .bind(&detail[..detail.len().min(1000)])
        .bind("")
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn deliveries(
        &self,
        message_id: Uuid,
        status: Option<&str>,
        channel: Option<&str>,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<DeliveryRecord>> {
        sqlx::query_as(
            "SELECT d.id, d.message_id, d.identity_id, i.external_id, d.occurrence, d.channel, d.status, d.detail, d.at
             FROM message_deliveries d JOIN identities i ON i.id = d.identity_id
             WHERE d.message_id = $1 AND ($2::text IS NULL OR d.status = $2) AND ($3::text IS NULL OR d.channel = $3)
               AND ($4::uuid IS NULL OR d.id < $4)
             ORDER BY d.id DESC LIMIT $5",
        )
        .bind(message_id)
        .bind(status)
        .bind(channel)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// (channel, status, count) per campaign.
    pub async fn delivery_stats(&self, message_id: Uuid) -> Result<Vec<(String, String, i64)>> {
        sqlx::query_as(
            "SELECT channel, status, count(*)::bigint FROM message_deliveries WHERE message_id = $1 GROUP BY 1, 2 ORDER BY 1, 2",
        )
        .bind(message_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn mark_pushed(&self, inbox_id: Uuid) -> Result<()> {
        sqlx::query("UPDATE inbox SET pushed_at = now() WHERE id = $1")
            .bind(inbox_id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
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

/// A delivery deferred by quiet hours, retried by housekeeping at `due_at`.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Deferral {
    pub id: Uuid,
    pub message_id: Uuid,
    pub identity_id: Uuid,
    pub occurrence: Option<DateTime<Utc>>,
    pub due_at: DateTime<Utc>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct VariantStat {
    pub variant: String,
    pub sent: i64,
    pub read: i64,
    pub consumed: i64,
}

impl MessageRepo {
    /// Inbox deliveries to one player since `since` (all campaigns, or one when `message_id` is set).
    pub async fn sent_since(
        &self,
        identity_id: Uuid,
        message_id: Option<Uuid>,
        since: DateTime<Utc>,
    ) -> Result<i64> {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*)::bigint FROM message_deliveries
             WHERE identity_id = $1 AND channel = 'inbox' AND status = 'sent' AND at >= $2
               AND ($3::uuid IS NULL OR message_id = $3)",
        )
        .bind(identity_id)
        .bind(since)
        .bind(message_id)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(n)
    }

    /// Tag the inbox delivery row of a campaign occurrence with the served copy variant.
    pub async fn set_delivery_variant(
        &self,
        message_id: Uuid,
        identity_id: Uuid,
        occurrence: Option<DateTime<Utc>>,
        variant: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE message_deliveries SET variant = $4
             WHERE message_id = $1 AND identity_id = $2 AND occurrence IS NOT DISTINCT FROM $3 AND channel = 'inbox'",
        )
        .bind(message_id)
        .bind(identity_id)
        .bind(occurrence)
        .bind(variant)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    /// Sent / read / consumed per copy variant (joined to the inbox by player + campaign key).
    pub async fn variant_stats(&self, message_id: Uuid, key: &str) -> Result<Vec<VariantStat>> {
        sqlx::query_as(
            "SELECT d.variant, count(*)::bigint AS sent,
                    count(i.read_at)::bigint AS read,
                    (count(*) - count(i.id))::bigint AS consumed
             FROM message_deliveries d
             LEFT JOIN inbox i ON i.identity_id = d.identity_id AND i.key = $2
                               AND i.delivered_at BETWEEN d.at - interval '5 seconds' AND d.at + interval '5 seconds'
             WHERE d.message_id = $1 AND d.channel = 'inbox' AND d.status = 'sent'
             GROUP BY d.variant ORDER BY d.variant",
        )
        .bind(message_id)
        .bind(key)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn defer(
        &self,
        message_id: Uuid,
        identity_id: Uuid,
        occurrence: Option<DateTime<Utc>>,
        due_at: DateTime<Utc>,
        reason: &str,
    ) -> Result<bool> {
        let r = sqlx::query(
            "INSERT INTO message_deferrals (id, message_id, identity_id, occurrence, due_at, reason)
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(message_id)
        .bind(identity_id)
        .bind(occurrence)
        .bind(due_at)
        .bind(reason)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(r.rows_affected() > 0)
    }

    pub async fn due_deferrals(&self, now: DateTime<Utc>, limit: i64) -> Result<Vec<Deferral>> {
        sqlx::query_as(
            "SELECT id, message_id, identity_id, occurrence, due_at, reason FROM message_deferrals
             WHERE due_at <= $1 ORDER BY due_at LIMIT $2",
        )
        .bind(now)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn deferrals_for(&self, message_id: Uuid) -> Result<Vec<Deferral>> {
        sqlx::query_as(
            "SELECT id, message_id, identity_id, occurrence, due_at, reason FROM message_deferrals
             WHERE message_id = $1 ORDER BY due_at LIMIT 500",
        )
        .bind(message_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn delete_deferral(&self, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM message_deferrals WHERE id = $1")
            .bind(id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }
}
