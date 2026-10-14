use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{
    push::{Platform, PushToken},
    Error, Result,
};

use crate::map_sqlx;

#[derive(FromRow)]
struct Row {
    identity_id: Uuid,
    platform: String,
    token: String,
    device_id: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    failures: i32,
}

impl TryFrom<Row> for PushToken {
    type Error = Error;
    fn try_from(r: Row) -> Result<Self> {
        Ok(PushToken {
            identity_id: r.identity_id,
            platform: Platform::parse(&r.platform)?,
            token: r.token,
            device_id: r.device_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
            failures: r.failures,
        })
    }
}

const COLS: &str = "identity_id, platform, token, device_id, created_at, updated_at, failures";

#[derive(Clone)]
pub struct PushTokenRepo {
    pg: PgPool,
}

impl PushTokenRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    /// Register a token; a token moving to another identity (device re-login) is re-owned.
    pub async fn upsert(
        &self,
        identity_id: Uuid,
        platform: Platform,
        token: &str,
        device_id: &str,
    ) -> Result<PushToken> {
        let row: Row = sqlx::query_as(&format!(
            "INSERT INTO push_tokens (identity_id, platform, token, device_id) VALUES ($1, $2, $3, $4)
             ON CONFLICT (platform, token) DO UPDATE SET identity_id = EXCLUDED.identity_id, device_id = EXCLUDED.device_id,
                                                        updated_at = now(), failures = 0
             RETURNING {COLS}"
        ))
        .bind(identity_id)
        .bind(platform.as_str())
        .bind(token)
        .bind(device_id)
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        PushToken::try_from(row)
    }

    pub async fn delete(&self, identity_id: Uuid, token: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM push_tokens WHERE identity_id = $1 AND token = $2")
            .bind(identity_id)
            .bind(token)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("push token not found"));
        }
        Ok(())
    }

    pub async fn for_identity(&self, identity_id: Uuid) -> Result<Vec<PushToken>> {
        let rows: Vec<Row> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM push_tokens WHERE identity_id = $1 ORDER BY updated_at DESC"
        ))
        .bind(identity_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        rows.into_iter().map(PushToken::try_from).collect()
    }

    /// Identities that have at least one token, paginated by id.
    pub async fn identities_with_tokens(
        &self,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT DISTINCT identity_id FROM push_tokens WHERE identity_id > COALESCE($1, '00000000-0000-0000-0000-000000000000'::uuid) ORDER BY identity_id LIMIT $2",
        )
        .bind(after)
        .bind(limit.clamp(1, 5000))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Provider said the token is gone: drop it.
    pub async fn remove_invalid(&self, platform: Platform, token: &str) -> Result<()> {
        sqlx::query("DELETE FROM push_tokens WHERE platform = $1 AND token = $2")
            .bind(platform.as_str())
            .bind(token)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    /// Transient failure: count it; tokens failing 10 times in a row are dropped.
    pub async fn record_failure(&self, platform: Platform, token: &str) -> Result<()> {
        sqlx::query(
            "UPDATE push_tokens SET failures = failures + 1 WHERE platform = $1 AND token = $2",
        )
        .bind(platform.as_str())
        .bind(token)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        sqlx::query(
            "DELETE FROM push_tokens WHERE platform = $1 AND token = $2 AND failures >= 10",
        )
        .bind(platform.as_str())
        .bind(token)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn record_success(&self, platform: Platform, token: &str) -> Result<()> {
        sqlx::query("UPDATE push_tokens SET failures = 0, updated_at = now() WHERE platform = $1 AND token = $2 AND failures > 0").bind(platform.as_str()).bind(token).execute(&self.pg).await.map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn count(&self) -> Result<(i64, i64)> {
        let row: (i64, i64) = sqlx::query_as(
            "SELECT count(*)::bigint, count(DISTINCT identity_id)::bigint FROM push_tokens",
        )
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(row)
    }
}
