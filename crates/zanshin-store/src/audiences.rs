use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::Serialize;
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
    kind: String,
    recompute: String,
    schedule_cron: String,
    source: String,
    copied_from: Option<String>,
    last_computed_at: Option<DateTime<Utc>>,
    member_count: i64,
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
            kind: r.kind.parse()?,
            recompute: r.recompute.parse()?,
            schedule_cron: r.schedule_cron,
            source: r.source,
            copied_from: r.copied_from,
            last_computed_at: r.last_computed_at,
            member_count: r.member_count,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

const COLS: &str = "id, key, name, description, condition, percentage, enabled, salt, tags, kind, recompute, schedule_cron, source, copied_from, last_computed_at, member_count, version, created_at, updated_at";

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
            "INSERT INTO audiences (id, key, name, description, condition, percentage, enabled, salt, tags,
                                    kind, recompute, schedule_cron, source, copied_from)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) RETURNING {COLS}"
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
        .bind(a.kind.as_str())
        .bind(a.recompute.as_str())
        .bind(&a.schedule_cron)
        .bind(&a.source)
        .bind(&a.copied_from)
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
                                  kind = $9, recompute = $10, schedule_cron = $11, source = $12,
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
        .bind(a.kind.as_str())
        .bind(a.recompute.as_str())
        .bind(&a.schedule_cron)
        .bind(&a.source)
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

/// A stored audience member joined with identity basics.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Member {
    pub identity_id: Uuid,
    pub external_id: String,
    pub joined_at: DateTime<Utc>,
    pub source: String,
}

/// Manual include / exclude entry.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ListEntry {
    pub identity_id: Uuid,
    pub external_id: String,
    pub mode: String,
    pub until: Option<DateTime<Utc>>,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

/// Per-identity list state used when resolving membership.
#[derive(Debug, Clone, Default)]
pub struct IdentityLists {
    pub included: HashSet<Uuid>,
    pub excluded: HashSet<Uuid>,
    pub stored: HashSet<Uuid>,
}

/// Where an audience key is referenced.
#[derive(Debug, Clone, Serialize)]
pub struct UsageRef {
    pub kind: String,
    pub key: String,
    pub name: String,
}

impl AudienceRepo {
    /// Include / exclude lists plus stored membership for one identity, in two queries.
    pub async fn lists_for_identity(&self, identity_id: Uuid) -> Result<IdentityLists> {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT audience_id, mode FROM audience_lists
             WHERE identity_id = $1 AND (until IS NULL OR until > now())",
        )
        .bind(identity_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        let mut out = IdentityLists::default();
        for (id, mode) in rows {
            if mode == "include" {
                out.included.insert(id);
            } else {
                out.excluded.insert(id);
            }
        }
        let stored: Vec<(Uuid,)> =
            sqlx::query_as("SELECT audience_id FROM audience_members WHERE identity_id = $1")
                .bind(identity_id)
                .fetch_all(&self.pg)
                .await
                .map_err(map_sqlx)?;
        out.stored = stored.into_iter().map(|(id,)| id).collect();
        Ok(out)
    }

    /// Record the identity as a member (keeps the original `joined_at`).
    pub async fn add_members(&self, audience_id: Uuid, ids: &[Uuid], source: &str) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let r = sqlx::query(
            "INSERT INTO audience_members (audience_id, identity_id, source)
             SELECT $1, unnest($2::uuid[]), $3 ON CONFLICT DO NOTHING",
        )
        .bind(audience_id)
        .bind(ids)
        .bind(source)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(r.rows_affected())
    }

    pub async fn remove_members(&self, audience_id: Uuid, ids: &[Uuid]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let r = sqlx::query(
            "DELETE FROM audience_members WHERE audience_id = $1 AND identity_id = ANY($2)",
        )
        .bind(audience_id)
        .bind(ids)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(r.rows_affected())
    }

    /// Replace rule-derived membership for a batch of identities: `matching` join, the rest of
    /// `evaluated` leave. Include-list members are never touched.
    pub async fn sync_members(
        &self,
        audience_id: Uuid,
        matching: &[Uuid],
        evaluated: &[Uuid],
    ) -> Result<()> {
        self.add_members(audience_id, matching, "rule").await?;
        sqlx::query(
            "DELETE FROM audience_members
             WHERE audience_id = $1 AND source = 'rule' AND identity_id = ANY($2) AND NOT identity_id = ANY($3)",
        )
        .bind(audience_id)
        .bind(evaluated)
        .bind(matching)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn members(
        &self,
        audience_id: Uuid,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<Member>> {
        sqlx::query_as(
            "SELECT m.identity_id, i.external_id, m.joined_at, m.source
             FROM audience_members m JOIN identities i ON i.id = m.identity_id
             WHERE m.audience_id = $1 AND ($2::uuid IS NULL OR m.identity_id > $2)
             ORDER BY m.identity_id LIMIT $3",
        )
        .bind(audience_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn member_count(&self, audience_id: Uuid) -> Result<i64> {
        let (n,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM audience_members WHERE audience_id = $1")
                .bind(audience_id)
                .fetch_one(&self.pg)
                .await
                .map_err(map_sqlx)?;
        Ok(n)
    }

    pub async fn mark_computed(&self, audience_id: Uuid) -> Result<i64> {
        let n = self.member_count(audience_id).await?;
        sqlx::query(
            "UPDATE audiences SET member_count = $2, last_computed_at = now() WHERE id = $1",
        )
        .bind(audience_id)
        .bind(n)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(n)
    }

    pub async fn lists(&self, audience_id: Uuid) -> Result<Vec<ListEntry>> {
        sqlx::query_as(
            "SELECT l.identity_id, i.external_id, l.mode, l.until, l.reason, l.created_at
             FROM audience_lists l JOIN identities i ON i.id = l.identity_id
             WHERE l.audience_id = $1 ORDER BY l.created_at DESC LIMIT 5000",
        )
        .bind(audience_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn set_list(
        &self,
        audience_id: Uuid,
        identity_id: Uuid,
        mode: &str,
        until: Option<DateTime<Utc>>,
        reason: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO audience_lists (audience_id, identity_id, mode, until, reason)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (audience_id, identity_id) DO UPDATE
               SET mode = EXCLUDED.mode, until = EXCLUDED.until, reason = EXCLUDED.reason, created_at = now()",
        )
        .bind(audience_id)
        .bind(identity_id)
        .bind(mode)
        .bind(until)
        .bind(reason)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        if mode == "include" {
            self.add_members(audience_id, &[identity_id], "include")
                .await?;
        } else {
            self.remove_members(audience_id, &[identity_id]).await?;
        }
        Ok(())
    }

    pub async fn unset_list(&self, audience_id: Uuid, identity_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM audience_lists WHERE audience_id = $1 AND identity_id = $2")
            .bind(audience_id)
            .bind(identity_id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        sqlx::query(
            "DELETE FROM audience_members WHERE audience_id = $1 AND identity_id = $2 AND source = 'include'",
        )
        .bind(audience_id)
        .bind(identity_id)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    /// Flags, experiments, live events and messages whose targeting references the audience key.
    pub async fn usage(&self, key: &str) -> Result<Vec<UsageRef>> {
        let mut out = Vec::new();
        let needle = format!("%\"{}\"%", key.replace('%', "\\%").replace('_', "\\_"));
        for (kind, table, cond_col, aud_col) in [
            ("flag", "flags", "rules::text", None),
            (
                "experiment",
                "experiments",
                "condition::text",
                Some("audiences"),
            ),
            (
                "live_event",
                "live_events",
                "condition::text",
                Some("audiences"),
            ),
            ("message", "messages", "condition::text", Some("audiences")),
        ] {
            let sql = match aud_col {
                Some(col) => format!(
                    "SELECT key, name FROM {table} WHERE $1 = ANY({col}) OR {cond_col} LIKE $2 ORDER BY key"
                ),
                None => format!("SELECT key, name FROM {table} WHERE {cond_col} LIKE $2 ORDER BY key"),
            };
            let rows: Vec<(String, String)> = sqlx::query_as(&sql)
                .bind(key)
                .bind(&needle)
                .fetch_all(&self.pg)
                .await
                .map_err(map_sqlx)?;
            out.extend(rows.into_iter().map(|(k, n)| UsageRef {
                kind: kind.to_string(),
                key: k,
                name: n,
            }));
        }
        Ok(out)
    }
}
