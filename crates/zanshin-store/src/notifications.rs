//! Notification channels (Slack / Discord / webhook) persisted in PostgreSQL.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use zanshin_core::{
    notifications::{ChannelKind, NotificationChannel},
    Error, Result,
};

use crate::map_sqlx;

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    key: String,
    name: String,
    kind: String,
    url: String,
    topics: Vec<String>,
    kinds: Vec<String>,
    enabled: bool,
    headers: serde_json::Value,
    last_status: Option<i32>,
    last_error: String,
    last_sent_at: Option<DateTime<Utc>>,
    tags: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<Row> for NotificationChannel {
    type Error = Error;
    fn try_from(r: Row) -> Result<Self> {
        Ok(NotificationChannel {
            id: r.id,
            key: r.key,
            name: r.name,
            kind: ChannelKind::parse(&r.kind)
                .ok_or_else(|| Error::invalid(format!("unknown channel kind {}", r.kind)))?,
            url: r.url,
            topics: r.topics,
            kinds: r.kinds,
            enabled: r.enabled,
            headers: r.headers,
            last_status: r.last_status,
            last_error: r.last_error,
            last_sent_at: r.last_sent_at,
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const COLS: &str = "id, key, name, kind, url, topics, kinds, enabled, headers, last_status, last_error, last_sent_at, tags, version, created_at, updated_at";

#[derive(Clone)]
pub struct NotificationRepo {
    pool: PgPool,
}

impl NotificationRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<NotificationChannel>> {
        let rows: Vec<Row> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM notification_channels ORDER BY key"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx)?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn get(&self, key: &str) -> Result<NotificationChannel> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM notification_channels WHERE key = $1"
        ))
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx)?;
        row.ok_or_else(|| Error::not_found(format!("notification channel {key}")))?
            .try_into()
    }

    pub async fn create(&self, c: &NotificationChannel) -> Result<NotificationChannel> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO notification_channels (id, key, name, kind, url, topics, kinds, enabled, headers, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING {COLS}"
        ))
        .bind(c.id)
        .bind(&c.key)
        .bind(&c.name)
        .bind(c.kind.as_str())
        .bind(&c.url)
        .bind(&c.topics)
        .bind(&c.kinds)
        .bind(c.enabled)
        .bind(&c.headers)
        .bind(&c.tags)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)?;
        row.try_into()
    }

    pub async fn update(&self, c: &NotificationChannel) -> Result<NotificationChannel> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "UPDATE notification_channels SET name = $2, kind = $3, url = $4, topics = $5, kinds = $6,
                    enabled = $7, headers = $8, tags = $9, version = version + 1, updated_at = now()
             WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&c.key)
        .bind(&c.name)
        .bind(c.kind.as_str())
        .bind(&c.url)
        .bind(&c.topics)
        .bind(&c.kinds)
        .bind(c.enabled)
        .bind(&c.headers)
        .bind(&c.tags)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx)?;
        row.ok_or_else(|| Error::not_found(format!("notification channel {}", c.key)))?
            .try_into()
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let n = sqlx::query("DELETE FROM notification_channels WHERE key = $1")
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(map_sqlx)?
            .rows_affected();
        if n == 0 {
            return Err(Error::not_found(format!("notification channel {key}")));
        }
        Ok(())
    }

    pub async fn mark(&self, id: Uuid, status: Option<i32>, error: &str) -> Result<()> {
        sqlx::query(
            "UPDATE notification_channels SET last_status = $2, last_error = $3, last_sent_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .bind(error)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }
}
