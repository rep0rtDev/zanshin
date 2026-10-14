//! Console users (password + TOTP), their server-side sessions and password-reset tokens.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{permissions::Permissions, Error, Result};

use crate::map_sqlx;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ConsoleUserRow {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    pub permissions: serde_json::Value,
    pub archetype: Option<String>,
    #[serde(skip_serializing)]
    pub totp_secret: Option<String>,
    pub totp_enabled: bool,
    #[serde(skip_serializing)]
    pub recovery_codes: Vec<String>,
    pub disabled: bool,
    pub must_change_password: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub password_changed_at: Option<DateTime<Utc>>,
    pub created_by: Option<String>,
}

impl ConsoleUserRow {
    pub fn permissions(&self) -> Permissions {
        serde_json::from_value(self.permissions.clone()).unwrap_or_else(|_| Permissions::read_all())
    }
}

const COLS: &str = "id, email, name, password_hash, permissions, archetype, totp_secret, totp_enabled, recovery_codes, disabled, must_change_password, created_at, updated_at, last_login_at, password_changed_at, created_by";

#[derive(Debug, Clone, FromRow)]
pub struct ConsoleSessionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UserPatch {
    pub name: Option<String>,
    pub permissions: Option<Permissions>,
    pub archetype: Option<Option<String>>,
    pub disabled: Option<bool>,
}

#[derive(Clone)]
pub struct ConsoleUserRepo {
    pg: PgPool,
}

impl ConsoleUserRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    pub async fn create(
        &self,
        email: &str,
        name: &str,
        password_hash: Option<&str>,
        permissions: &Permissions,
        archetype: Option<&str>,
        must_change_password: bool,
        created_by: &str,
    ) -> Result<ConsoleUserRow> {
        sqlx::query_as(&format!(
            "INSERT INTO console_users (id, email, name, password_hash, permissions, archetype, must_change_password, password_changed_at, created_by)
             VALUES ($1, lower($2), $3, $4, $5, $6, $7, CASE WHEN $4 IS NULL THEN NULL ELSE now() END, $8) RETURNING {COLS}"
        ))
        .bind(Uuid::now_v7())
        .bind(email)
        .bind(name)
        .bind(password_hash)
        .bind(serde_json::to_value(permissions).unwrap_or_default())
        .bind(archetype)
        .bind(must_change_password)
        .bind(created_by)
        .fetch_one(&self.pg)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                Error::conflict("a user with this email already exists")
            }
            _ => map_sqlx(e),
        })
    }

    pub async fn count(&self) -> Result<i64> {
        sqlx::query_scalar("SELECT count(*) FROM console_users")
            .fetch_one(&self.pg)
            .await
            .map_err(map_sqlx)
    }

    pub async fn list(&self) -> Result<Vec<ConsoleUserRow>> {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM console_users ORDER BY created_at"
        ))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn get(&self, id: Uuid) -> Result<ConsoleUserRow> {
        sqlx::query_as(&format!("SELECT {COLS} FROM console_users WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pg)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::not_found("user not found"))
    }

    pub async fn get_by_email(&self, email: &str) -> Result<Option<ConsoleUserRow>> {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM console_users WHERE email = lower($1)"
        ))
        .bind(email)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn update(&self, id: Uuid, patch: &UserPatch) -> Result<ConsoleUserRow> {
        sqlx::query_as(&format!(
            "UPDATE console_users SET
                name = COALESCE($2, name),
                permissions = COALESCE($3, permissions),
                archetype = CASE WHEN $4 THEN $5 ELSE archetype END,
                disabled = COALESCE($6, disabled),
                updated_at = now()
             WHERE id = $1 RETURNING {COLS}"
        ))
        .bind(id)
        .bind(&patch.name)
        .bind(
            patch
                .permissions
                .as_ref()
                .map(|p| serde_json::to_value(p).unwrap_or_default()),
        )
        .bind(patch.archetype.is_some())
        .bind(patch.archetype.clone().flatten())
        .bind(patch.disabled)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?
        .ok_or_else(|| Error::not_found("user not found"))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let n = sqlx::query("DELETE FROM console_users WHERE id = $1")
            .bind(id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?
            .rows_affected();
        (n > 0)
            .then_some(())
            .ok_or_else(|| Error::not_found("user not found"))
    }

    pub async fn set_password(&self, id: Uuid, hash: &str, must_change: bool) -> Result<()> {
        sqlx::query("UPDATE console_users SET password_hash = $2, must_change_password = $3, password_changed_at = now(), updated_at = now() WHERE id = $1")
            .bind(id).bind(hash).bind(must_change)
            .execute(&self.pg).await.map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn set_totp(
        &self,
        id: Uuid,
        secret: Option<&str>,
        enabled: bool,
        recovery_codes: &[String],
    ) -> Result<()> {
        sqlx::query("UPDATE console_users SET totp_secret = $2, totp_enabled = $3, recovery_codes = $4, updated_at = now() WHERE id = $1")
            .bind(id).bind(secret).bind(enabled).bind(recovery_codes)
            .execute(&self.pg).await.map_err(map_sqlx)?;
        Ok(())
    }

    /// Burn one recovery code; true when it matched.
    pub async fn consume_recovery_code(&self, id: Uuid, code_hash: &str) -> Result<bool> {
        let n = sqlx::query("UPDATE console_users SET recovery_codes = array_remove(recovery_codes, $2) WHERE id = $1 AND $2 = ANY(recovery_codes)")
            .bind(id).bind(code_hash)
            .execute(&self.pg).await.map_err(map_sqlx)?.rows_affected();
        Ok(n > 0)
    }

    pub async fn touch_login(&self, id: Uuid) -> Result<()> {
        sqlx::query("UPDATE console_users SET last_login_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    // ---- sessions ----

    pub async fn create_session(
        &self,
        user_id: Uuid,
        token_hash: &[u8],
        ttl: Duration,
        ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<ConsoleSessionRow> {
        sqlx::query_as(
            "INSERT INTO console_user_sessions (id, user_id, token_hash, expires_at, ip, user_agent)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, user_id, created_at, expires_at, last_seen_at, revoked_at, ip, user_agent",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(token_hash)
        .bind(Utc::now() + ttl)
        .bind(ip)
        .bind(user_agent.map(|u| u.chars().take(256).collect::<String>()))
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    /// Live session + its user for a cookie token; touches `last_seen_at` at most once a minute.
    pub async fn session_user(
        &self,
        token_hash: &[u8],
    ) -> Result<Option<(ConsoleSessionRow, ConsoleUserRow)>> {
        let session: Option<ConsoleSessionRow> = sqlx::query_as(
            "UPDATE console_user_sessions SET last_seen_at = CASE WHEN last_seen_at < now() - interval '1 minute' THEN now() ELSE last_seen_at END
             WHERE token_hash = $1 AND revoked_at IS NULL AND expires_at > now()
             RETURNING id, user_id, created_at, expires_at, last_seen_at, revoked_at, ip, user_agent",
        )
        .bind(token_hash)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        let Some(session) = session else {
            return Ok(None);
        };
        let user = self.get(session.user_id).await?;
        if user.disabled {
            return Ok(None);
        }
        Ok(Some((session, user)))
    }

    pub async fn list_sessions(&self, user_id: Uuid) -> Result<Vec<ConsoleSessionRow>> {
        sqlx::query_as(
            "SELECT id, user_id, created_at, expires_at, last_seen_at, revoked_at, ip, user_agent
             FROM console_user_sessions WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now()
             ORDER BY last_seen_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)
    }

    pub async fn revoke_session(&self, user_id: Uuid, id: Uuid) -> Result<()> {
        sqlx::query("UPDATE console_user_sessions SET revoked_at = now() WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL")
            .bind(id).bind(user_id)
            .execute(&self.pg).await.map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn revoke_all_sessions(&self, user_id: Uuid) -> Result<u64> {
        Ok(sqlx::query("UPDATE console_user_sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL")
            .bind(user_id)
            .execute(&self.pg).await.map_err(map_sqlx)?.rows_affected())
    }

    pub async fn purge_sessions(&self) -> Result<u64> {
        Ok(sqlx::query("DELETE FROM console_user_sessions WHERE expires_at < now() - interval '7 days' OR revoked_at < now() - interval '7 days'")
            .execute(&self.pg).await.map_err(map_sqlx)?.rows_affected())
    }

    // ---- password resets ----

    pub async fn create_reset(
        &self,
        user_id: Uuid,
        token_hash: &[u8],
        ttl: Duration,
    ) -> Result<()> {
        sqlx::query("INSERT INTO console_password_resets (token_hash, user_id, expires_at) VALUES ($1, $2, $3)")
            .bind(token_hash).bind(user_id).bind(Utc::now() + ttl)
            .execute(&self.pg).await.map_err(map_sqlx)?;
        Ok(())
    }

    /// Marks the token used and returns its user; `None` when unknown, used or expired.
    pub async fn consume_reset(&self, token_hash: &[u8]) -> Result<Option<Uuid>> {
        sqlx::query_scalar(
            "UPDATE console_password_resets SET used_at = now()
             WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() RETURNING user_id",
        )
        .bind(token_hash)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)
    }
}
