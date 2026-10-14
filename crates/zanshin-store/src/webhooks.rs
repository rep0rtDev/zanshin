use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{webhooks::Webhook, Error, Result};

use crate::map_sqlx;

#[derive(FromRow)]
struct Row {
    id: Uuid,
    key: String,
    name: String,
    url: String,
    secret: String,
    events: Vec<String>,
    enabled: bool,
    batch_size: i32,
    batch_window_secs: i32,
    headers: Value,
    max_attempts: i32,
    tags: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<Row> for Webhook {
    fn from(r: Row) -> Self {
        Webhook {
            id: r.id,
            key: r.key,
            name: r.name,
            url: r.url,
            secret: r.secret,
            events: r.events,
            enabled: r.enabled,
            batch_size: r.batch_size,
            batch_window_secs: r.batch_window_secs,
            headers: r.headers,
            max_attempts: r.max_attempts,
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Delivery {
    pub id: Uuid,
    pub webhook_id: Uuid,
    pub event_types: Vec<String>,
    pub payload: Value,
    pub status: String,
    pub attempts: i32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_status: Option<i32>,
    pub last_error: String,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

const COLS: &str = "id, key, name, url, secret, events, enabled, batch_size, batch_window_secs, headers, max_attempts, tags, version, created_at, updated_at";
const DCOLS: &str = "id, webhook_id, event_types, payload, status, attempts, next_attempt_at, last_status, last_error, created_at, delivered_at";

#[derive(Clone)]
pub struct WebhookRepo {
    pg: PgPool,
}

impl WebhookRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<Webhook>> {
        let rows: Vec<Row> = sqlx::query_as(&format!("SELECT {COLS} FROM webhooks ORDER BY key"))
            .fetch_all(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(Webhook::from).collect())
    }

    pub async fn get(&self, key: &str) -> Result<Webhook> {
        let row: Option<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM webhooks WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(Webhook::from)
            .ok_or_else(|| Error::not_found(format!("webhook {key:?} not found")))
    }

    pub async fn create(&self, w: &Webhook) -> Result<Webhook> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO webhooks (id, key, name, url, secret, events, enabled, batch_size, batch_window_secs, headers, max_attempts, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) RETURNING {COLS}"
        ))
        .bind(w.id)
        .bind(&w.key)
        .bind(&w.name)
        .bind(&w.url)
        .bind(&w.secret)
        .bind(&w.events)
        .bind(w.enabled)
        .bind(w.batch_size)
        .bind(w.batch_window_secs)
        .bind(&w.headers)
        .bind(w.max_attempts)
        .bind(&w.tags)
        .fetch_one(&self.pg)
        .await
        .map_err(|err| match map_sqlx(err) {
            Error::Conflict(_) => Error::conflict(format!("webhook {:?} already exists", w.key)),
            other => other,
        })?;
        Ok(row.into())
    }

    pub async fn update(&self, w: &Webhook) -> Result<Webhook> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "UPDATE webhooks SET name = $2, url = $3, secret = $4, events = $5, enabled = $6, batch_size = $7, batch_window_secs = $8,
                                 headers = $9, max_attempts = $10, tags = $11, version = version + 1, updated_at = now()
             WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&w.key)
        .bind(&w.name)
        .bind(&w.url)
        .bind(&w.secret)
        .bind(&w.events)
        .bind(w.enabled)
        .bind(w.batch_size)
        .bind(w.batch_window_secs)
        .bind(&w.headers)
        .bind(w.max_attempts)
        .bind(&w.tags)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Webhook::from)
            .ok_or_else(|| Error::not_found(format!("webhook {:?} not found", w.key)))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM webhooks WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!("webhook {key:?} not found")));
        }
        Ok(())
    }

    pub async fn insert_delivery(
        &self,
        webhook_id: Uuid,
        event_types: &[String],
        payload: &Value,
    ) -> Result<Delivery> {
        sqlx::query_as(&format!(
            "INSERT INTO webhook_deliveries (id, webhook_id, event_types, payload, status, next_attempt_at)
             VALUES ($1, $2, $3, $4, 'pending', now()) RETURNING {DCOLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(webhook_id)
        .bind(event_types)
        .bind(payload)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn mark_delivery(
        &self,
        id: Uuid,
        status: &str,
        attempts: i32,
        next_attempt_at: Option<DateTime<Utc>>,
        last_status: Option<i32>,
        last_error: &str,
    ) -> Result<Delivery> {
        sqlx::query_as(&format!(
            "UPDATE webhook_deliveries SET status = $2, attempts = $3, next_attempt_at = $4, last_status = $5, last_error = $6,
                                           delivered_at = CASE WHEN $2 = 'sent' THEN now() ELSE delivered_at END
             WHERE id = $1 RETURNING {DCOLS}"
        ))
        .bind(id)
        .bind(status)
        .bind(attempts)
        .bind(next_attempt_at)
        .bind(last_status)
        .bind(&last_error[..last_error.len().min(1000)])
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Deliveries waiting for a (re)try.
    pub async fn due(&self, now: DateTime<Utc>, limit: i64) -> Result<Vec<Delivery>> {
        sqlx::query_as(&format!(
            "SELECT {DCOLS} FROM webhook_deliveries
             WHERE status IN ('pending', 'failed') AND next_attempt_at IS NOT NULL AND next_attempt_at <= $1
             ORDER BY next_attempt_at LIMIT $2"
        ))
        .bind(now)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn get_delivery(&self, webhook_id: Uuid, id: Uuid) -> Result<Delivery> {
        let row: Option<Delivery> = sqlx::query_as(&format!(
            "SELECT {DCOLS} FROM webhook_deliveries WHERE webhook_id = $1 AND id = $2"
        ))
        .bind(webhook_id)
        .bind(id)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.ok_or_else(|| Error::not_found("delivery not found"))
    }

    pub async fn deliveries(
        &self,
        webhook_id: Uuid,
        status: Option<&str>,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<Delivery>> {
        sqlx::query_as(&format!(
            "SELECT {DCOLS} FROM webhook_deliveries
             WHERE webhook_id = $1 AND ($2::text IS NULL OR status = $2) AND ($3::uuid IS NULL OR id < $3)
             ORDER BY id DESC LIMIT $4"
        ))
        .bind(webhook_id)
        .bind(status)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn delivery_stats(&self, webhook_id: Uuid) -> Result<Vec<(String, i64)>> {
        sqlx::query_as(
            "SELECT status, count(*)::bigint FROM webhook_deliveries WHERE webhook_id = $1 GROUP BY 1 ORDER BY 1",
        )
        .bind(webhook_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn purge_deliveries(&self, older_than: DateTime<Utc>) -> Result<u64> {
        let res = sqlx::query(
            "DELETE FROM webhook_deliveries WHERE status IN ('sent', 'dead') AND created_at < $1",
        )
        .bind(older_than)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(res.rows_affected())
    }
}
