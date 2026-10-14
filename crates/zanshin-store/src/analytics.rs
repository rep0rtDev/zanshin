use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
    analytics::{Annotation, Dashboard, Widget},
    Error, Result,
};

use crate::map_sqlx;

#[derive(FromRow)]
struct Row {
    id: Uuid,
    key: String,
    name: String,
    description: String,
    widgets: Value,
    filters: Value,
    tags: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<Row> for Dashboard {
    type Error = Error;
    fn try_from(r: Row) -> Result<Self> {
        Ok(Dashboard {
            id: r.id,
            key: r.key,
            name: r.name,
            description: r.description,
            widgets: serde_json::from_value::<Vec<Widget>>(r.widgets)
                .map_err(|e| Error::invalid(format!("stored widgets are invalid: {e}")))?,
            filters: r.filters,
            tags: r.tags,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const COLS: &str =
    "id, key, name, description, widgets, filters, tags, version, created_at, updated_at";

#[derive(Clone)]
pub struct DashboardRepo {
    pg: PgPool,
}

impl DashboardRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn list(&self) -> Result<Vec<Dashboard>> {
        let rows: Vec<Row> = sqlx::query_as(&format!("SELECT {COLS} FROM dashboards ORDER BY key"))
            .fetch_all(&self.pg)
            .await
            .map_err(map_sqlx)?;
        rows.into_iter().map(Dashboard::try_from).collect()
    }

    pub async fn get(&self, key: &str) -> Result<Dashboard> {
        let row: Option<Row> =
            sqlx::query_as(&format!("SELECT {COLS} FROM dashboards WHERE key = $1"))
                .bind(key)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(Dashboard::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("dashboard {key:?} not found")))
    }

    pub async fn create(&self, d: &Dashboard) -> Result<Dashboard> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO dashboards (id, key, name, description, widgets, filters, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {COLS}"
        ))
        .bind(d.id)
        .bind(&d.key)
        .bind(&d.name)
        .bind(&d.description)
        .bind(serde_json::to_value(&d.widgets).unwrap_or(Value::Null))
        .bind(&d.filters)
        .bind(&d.tags)
        .fetch_one(&self.pg)
        .await
        .map_err(|err| match map_sqlx(err) {
            Error::Conflict(_) => Error::conflict(format!("dashboard {:?} already exists", d.key)),
            other => other,
        })?;
        Dashboard::try_from(row)
    }

    pub async fn update(&self, d: &Dashboard) -> Result<Dashboard> {
        let row: Option<Row> = sqlx::query_as(&format!(
            "UPDATE dashboards SET name = $2, description = $3, widgets = $4, filters = $5, tags = $6,
                                   version = version + 1, updated_at = now()
             WHERE key = $1 RETURNING {COLS}"
        ))
        .bind(&d.key)
        .bind(&d.name)
        .bind(&d.description)
        .bind(serde_json::to_value(&d.widgets).unwrap_or(Value::Null))
        .bind(&d.filters)
        .bind(&d.tags)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Dashboard::try_from)
            .transpose()?
            .ok_or_else(|| Error::not_found(format!("dashboard {:?} not found", d.key)))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM dashboards WHERE key = $1")
            .bind(key)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found(format!("dashboard {key:?} not found")));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct AnnotationRepo {
    pg: PgPool,
}

#[derive(FromRow)]
struct ARow {
    id: Uuid,
    at: DateTime<Utc>,
    kind: String,
    key: String,
    title: String,
    body: String,
    created_at: DateTime<Utc>,
}

impl From<ARow> for Annotation {
    fn from(r: ARow) -> Self {
        Annotation {
            id: r.id,
            at: r.at,
            kind: r.kind,
            key: r.key,
            title: r.title,
            body: r.body,
            created_at: r.created_at,
        }
    }
}

impl AnnotationRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn insert(
        &self,
        at: DateTime<Utc>,
        kind: &str,
        key: &str,
        title: &str,
        body: &str,
    ) -> Result<Annotation> {
        let row: ARow = sqlx::query_as(
            "INSERT INTO annotations (id, at, kind, key, title, body) VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, at, kind, key, title, body, created_at",
        )
        .bind(Uuid::now_v7())
        .bind(at)
        .bind(kind)
        .bind(key)
        .bind(title)
        .bind(body)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(row.into())
    }

    pub async fn list(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Annotation>> {
        let rows: Vec<ARow> = sqlx::query_as(
            "SELECT id, at, kind, key, title, body, created_at FROM annotations WHERE at >= $1 AND at < $2 ORDER BY at DESC LIMIT $3",
        )
        .bind(from)
        .bind(to)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(Annotation::from).collect())
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("DELETE FROM annotations WHERE id = $1")
            .bind(id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("annotation not found"));
        }
        Ok(())
    }

    pub async fn purge(&self, older_than: DateTime<Utc>) -> Result<u64> {
        let res = sqlx::query("DELETE FROM annotations WHERE kind <> 'note' AND at < $1")
            .bind(older_than)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(res.rows_affected())
    }
}

/// Player-model metrics that live in Postgres regardless of the event backend.
#[derive(Clone)]
pub struct PlayerMetricsRepo {
    pg: PgPool,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct DayCount {
    pub day: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SessionDay {
    pub day: String,
    pub sessions: i64,
    pub avg_secs: f64,
}

impl PlayerMetricsRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    /// Identities created per UTC day.
    pub async fn new_identities(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<DayCount>> {
        sqlx::query_as(
            "SELECT to_char(date_trunc('day', created_at), 'YYYY-MM-DD') AS day, count(*)::bigint AS count
             FROM identities WHERE created_at >= $1 AND created_at < $2 GROUP BY 1 ORDER BY 1",
        )
        .bind(from)
        .bind(to)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Play sessions started per UTC day with average duration (ended sessions only for the average).
    pub async fn sessions(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<SessionDay>> {
        sqlx::query_as(
            "SELECT to_char(date_trunc('day', started_at), 'YYYY-MM-DD') AS day, count(*)::bigint AS sessions,
                    COALESCE(avg(duration_sec), 0)::float8 AS avg_secs
             FROM play_sessions WHERE started_at >= $1 AND started_at < $2 GROUP BY 1 ORDER BY 1",
        )
        .bind(from)
        .bind(to)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// (identity_id, created day) for cohorts created in the range.
    pub async fn cohorts(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<(Uuid, String)>> {
        sqlx::query_as(
            "SELECT id, to_char(date_trunc('day', created_at), 'YYYY-MM-DD') FROM identities
             WHERE created_at >= $1 AND created_at < $2 ORDER BY created_at LIMIT $3",
        )
        .bind(from)
        .bind(to)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn total_identities(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT count(*)::bigint FROM identities")
            .fetch_one(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(n)
    }

    /// Players with an open play session seen within `window_secs`.
    pub async fn online_now(&self, window_secs: i64) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT count(DISTINCT identity_id)::bigint FROM play_sessions WHERE ended_at IS NULL AND last_seen_at > now() - make_interval(secs => $1)",
        )
        .bind(window_secs as f64)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(n)
    }

    /// Identities created in the range, optionally narrowed by default-property platform / country.
    pub async fn cohorts_filtered(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        platform: Option<&str>,
        country: Option<&str>,
        limit: i64,
    ) -> Result<Vec<(Uuid, String)>> {
        sqlx::query_as(
            "SELECT id, to_char(date_trunc('day', created_at), 'YYYY-MM-DD') FROM identities
             WHERE created_at >= $1 AND created_at < $2
               AND ($3 = '' OR default_properties->>'platform' = $3)
               AND ($4 = '' OR default_properties->>'country' = $4)
             ORDER BY created_at LIMIT $5",
        )
        .bind(from)
        .bind(to)
        .bind(platform.unwrap_or(""))
        .bind(country.unwrap_or(""))
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Playtime per UTC day: sessions, distinct players, total / average / median seconds
    /// (ended sessions only for durations).
    pub async fn playtime(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        platform: Option<&str>,
        app_version: Option<&str>,
    ) -> Result<Vec<PlaytimeDay>> {
        sqlx::query_as(
            "SELECT to_char(date_trunc('day', started_at), 'YYYY-MM-DD') AS day,
                    count(*)::bigint AS sessions,
                    count(DISTINCT identity_id)::bigint AS players,
                    COALESCE(sum(duration_sec), 0)::bigint AS total_secs,
                    COALESCE(avg(duration_sec), 0)::float8 AS avg_secs,
                    COALESCE(percentile_cont(0.5) WITHIN GROUP (ORDER BY duration_sec), 0)::float8 AS median_secs
             FROM play_sessions
             WHERE started_at >= $1 AND started_at < $2
               AND ($3 = '' OR platform = $3) AND ($4 = '' OR app_version = $4)
             GROUP BY 1 ORDER BY 1",
        )
        .bind(from)
        .bind(to)
        .bind(platform.unwrap_or(""))
        .bind(app_version.unwrap_or(""))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Session-length histogram (seconds upper bounds: 60, 300, 900, 1800, 3600, +inf).
    pub async fn duration_buckets(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<(String, i64)>> {
        sqlx::query_as(
            "SELECT CASE WHEN duration_sec < 60 THEN '<1m' WHEN duration_sec < 300 THEN '1-5m' WHEN duration_sec < 900 THEN '5-15m'
                         WHEN duration_sec < 1800 THEN '15-30m' WHEN duration_sec < 3600 THEN '30-60m' ELSE '>60m' END AS bucket,
                    count(*)::bigint
             FROM play_sessions WHERE ended_at IS NOT NULL AND duration_sec IS NOT NULL AND started_at >= $1 AND started_at < $2
             GROUP BY 1",
        )
        .bind(from)
        .bind(to)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Operational failures since `from`: (source, count) over message deliveries, webhook
    /// deliveries, export runs and journey step errors.
    pub async fn failures_since(&self, from: DateTime<Utc>) -> Result<Vec<(String, i64)>> {
        sqlx::query_as(
            "SELECT 'message_deliveries', count(*)::bigint FROM message_deliveries WHERE status = 'failed' AND at >= $1
             UNION ALL SELECT 'webhook_deliveries', count(*)::bigint FROM webhook_deliveries WHERE status IN ('failed', 'dead') AND created_at >= $1
             UNION ALL SELECT 'export_runs', count(*)::bigint FROM export_runs WHERE status = 'failed' AND started_at >= $1",
        )
        .bind(from)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct PlaytimeDay {
    pub day: String,
    pub sessions: i64,
    pub players: i64,
    pub total_secs: i64,
    pub avg_secs: f64,
    pub median_secs: f64,
}

/// Advertising spend per day / platform / country / campaign — CPI, RoAS and LTV input.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AdSpendRow {
    pub day: chrono::NaiveDate,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub campaign: String,
    pub spend: f64,
    #[serde(default)]
    pub installs: i32,
    #[serde(default = "default_currency")]
    pub currency: String,
}

fn default_currency() -> String {
    "USD".into()
}

#[derive(Clone)]
pub struct AdSpendRepo {
    pg: PgPool,
}

impl AdSpendRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn upsert_many(&self, rows: &[AdSpendRow]) -> Result<u64> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut qb = sqlx::QueryBuilder::new(
            "INSERT INTO ad_spend (day, platform, country, campaign, spend, installs, currency) ",
        );
        qb.push_values(rows, |mut b, r| {
            b.push_bind(r.day)
                .push_bind(&r.platform)
                .push_bind(&r.country)
                .push_bind(&r.campaign)
                .push_bind(r.spend)
                .push_bind(r.installs)
                .push_bind(&r.currency);
        });
        qb.push(" ON CONFLICT (day, platform, country, campaign) DO UPDATE SET spend = EXCLUDED.spend, installs = EXCLUDED.installs, currency = EXCLUDED.currency, updated_at = now()");
        Ok(qb
            .build()
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?
            .rows_affected())
    }

    pub async fn list(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<AdSpendRow>> {
        sqlx::query_as("SELECT day, platform, country, campaign, spend, installs, currency FROM ad_spend WHERE day >= $1::date AND day <= $2::date ORDER BY day, platform, country, campaign")
            .bind(from.date_naive())
            .bind(to.date_naive())
            .fetch_all(&self.pg)
            .await
            .map_err(map_sqlx)
    }

    pub async fn delete(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<u64> {
        Ok(
            sqlx::query("DELETE FROM ad_spend WHERE day >= $1::date AND day <= $2::date")
                .bind(from.date_naive())
                .bind(to.date_naive())
                .execute(&self.pg)
                .await
                .map_err(map_sqlx)?
                .rows_affected(),
        )
    }
}
