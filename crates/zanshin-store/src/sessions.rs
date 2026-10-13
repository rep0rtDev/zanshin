use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{Error, Result};

use crate::map_sqlx;

#[derive(Debug, Clone, FromRow)]
pub struct SessionRow {
    pub id: Uuid,
    pub identity_id: Uuid,
    pub refresh_hash: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub rotated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl SessionRow {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none() && self.expires_at > Utc::now()
    }
}

#[derive(Clone)]
pub struct SessionRepo {
    pg: PgPool,
}

impl SessionRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn create(
        &self,
        id: Uuid,
        identity_id: Uuid,
        refresh_hash: &[u8],
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO sessions (id, identity_id, refresh_hash, expires_at) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(identity_id)
            .bind(refresh_hash)
            .bind(expires_at)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn get(&self, id: Uuid) -> Result<SessionRow> {
        sqlx::query_as("SELECT id, identity_id, refresh_hash, created_at, rotated_at, expires_at, revoked_at FROM sessions WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pg)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::unauthenticated("session not found"))
    }

    /// Atomically rotate the refresh token: succeeds only if the old hash still matches
    /// and the session is active, which makes replay of a consumed refresh token fail.
    pub async fn rotate(
        &self,
        id: Uuid,
        old_hash: &[u8],
        new_hash: &[u8],
        expires_at: DateTime<Utc>,
    ) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE sessions SET refresh_hash = $3, rotated_at = now(), expires_at = $4
             WHERE id = $1 AND refresh_hash = $2 AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(id)
        .bind(old_hash)
        .bind(new_hash)
        .bind(expires_at)
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn revoke(&self, id: Uuid) -> Result<()> {
        sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn revoke_all_for_identity(&self, identity_id: Uuid) -> Result<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "UPDATE sessions SET revoked_at = now() WHERE identity_id = $1 AND revoked_at IS NULL RETURNING id",
        )
        .bind(identity_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Housekeeping: drop sessions long past expiry.
    pub async fn purge_expired(&self) -> Result<u64> {
        let res = sqlx::query("DELETE FROM sessions WHERE expires_at < now() - interval '7 days'")
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(res.rows_affected())
    }
}
