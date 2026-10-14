//! Identity lifecycle: authenticate / refresh / logout / properties / delete.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zanshin_core::{
    identity::validate_external_id,
    properties::{self, PropertyMap},
    token::{Claims, TokenIssuer, TokenKind},
    Error, Identity, Result,
};
use zanshin_store::{cache::Cache, identities::IdentityRepo, sessions::SessionRepo};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTokens {
    pub session_id: Uuid,
    pub created: bool,
    pub token: String,
    pub refresh_token: String,
    pub identity: Identity,
}

/// Verified caller of a session-authenticated request.
#[derive(Debug, Clone)]
pub struct SessionContext {
    pub claims: Claims,
}

impl SessionContext {
    pub fn identity_id(&self) -> Uuid {
        self.claims.sub
    }
}

#[derive(Clone)]
pub struct IdentityService {
    identities: IdentityRepo,
    sessions: SessionRepo,
    cache: Cache,
    tokens: TokenIssuer,
}

impl IdentityService {
    pub async fn lifetime_rows(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        limit: i64,
    ) -> Result<Vec<(f64, f64, f64, f64, f64, f64)>> {
        self.identities.lifetime_rows(from, to, limit).await
    }

    pub fn new(
        identities: IdentityRepo,
        sessions: SessionRepo,
        cache: Cache,
        tokens: TokenIssuer,
    ) -> Self {
        Self {
            identities,
            sessions,
            cache,
            tokens,
        }
    }

    pub fn tokens(&self) -> &TokenIssuer {
        &self.tokens
    }

    pub async fn authenticate(
        &self,
        external_id: &str,
        default: PropertyMap,
        custom: PropertyMap,
    ) -> Result<SessionTokens> {
        validate_external_id(external_id)?;
        properties::validate(&default)?;
        properties::validate(&custom)?;
        let (identity, created) = self
            .identities
            .authenticate(external_id, &default, &custom)
            .await?;
        if created {
            metrics::counter!("zanshin_identities_created_total").increment(1);
        }
        let issued = self.tokens.issue_new(identity.id, &identity.external_id)?;
        self.sessions
            .create(
                issued.session_id,
                identity.id,
                &refresh_hash(&issued.refresh),
                issued.refresh_expires_at,
            )
            .await?;
        metrics::counter!("zanshin_sessions_created_total").increment(1);
        Ok(SessionTokens {
            session_id: issued.session_id,
            created,
            token: issued.access,
            refresh_token: issued.refresh,
            identity,
        })
    }

    /// Link the identity to a new external id. If that id already belongs to another identity the
    /// current one is merged into it; otherwise the external id is renamed in place. Returns a
    /// fresh session for the resulting identity and the id of the identity that was merged away.
    pub async fn identify(
        &self,
        identity_id: Uuid,
        new_external_id: &str,
    ) -> Result<(SessionTokens, Option<Identity>)> {
        validate_external_id(new_external_id)?;
        let (identity, merged) = match self.identities.get_by_external_id(new_external_id).await {
            Ok(target) if target.id == identity_id => (target, None),
            Ok(target) => {
                let source = self.identities.get(identity_id).await?;
                let merged = self.identities.merge_into(identity_id, target.id).await?;
                metrics::counter!("zanshin_identities_merged_total").increment(1);
                (merged, Some(source))
            }
            Err(e) if e.is_not_found() => (
                self.identities.rename(identity_id, new_external_id).await?,
                None,
            ),
            Err(e) => return Err(e),
        };
        let issued = self.tokens.issue_new(identity.id, &identity.external_id)?;
        self.sessions
            .create(
                issued.session_id,
                identity.id,
                &refresh_hash(&issued.refresh),
                issued.refresh_expires_at,
            )
            .await?;
        Ok((
            SessionTokens {
                session_id: issued.session_id,
                created: false,
                token: issued.access,
                refresh_token: issued.refresh,
                identity,
            },
            merged,
        ))
    }

    pub async fn refresh(&self, refresh_token: &str) -> Result<SessionTokens> {
        let claims = self.tokens.verify(refresh_token, TokenKind::Refresh)?;
        let session = self.sessions.get(claims.sid).await?;
        if !session.is_active() {
            return Err(Error::unauthenticated("session revoked or expired"));
        }
        let identity = self
            .identities
            .get(claims.sub)
            .await
            .map_err(|_| Error::unauthenticated("identity no longer exists"))?;
        let issued =
            self.tokens
                .issue_for_session(claims.sid, identity.id, &identity.external_id)?;
        let rotated = self
            .sessions
            .rotate(
                claims.sid,
                &refresh_hash(refresh_token),
                &refresh_hash(&issued.refresh),
                issued.refresh_expires_at,
            )
            .await?;
        if !rotated {
            // A consumed refresh token was replayed: treat the whole session as compromised.
            self.revoke_session(claims.sid).await?;
            metrics::counter!("zanshin_refresh_replays_total").increment(1);
            return Err(Error::unauthenticated("refresh token already used"));
        }
        self.identities.touch(identity.id).await.ok();
        Ok(SessionTokens {
            session_id: claims.sid,
            created: false,
            token: issued.access,
            refresh_token: issued.refresh,
            identity,
        })
    }

    pub async fn logout(
        &self,
        access_token: Option<&str>,
        refresh_token: Option<&str>,
    ) -> Result<Uuid> {
        let mut sid = None;
        if let Some(t) = refresh_token {
            if let Ok(c) = self.tokens.verify(t, TokenKind::Refresh) {
                sid = Some(c.sid);
            }
        }
        if sid.is_none() {
            if let Some(t) = access_token {
                if let Ok(c) = self.tokens.verify(t, TokenKind::Access) {
                    sid = Some(c.sid);
                }
            }
        }
        match sid {
            Some(sid) => {
                self.revoke_session(sid).await?;
                Ok(sid)
            }
            None => Err(Error::unauthenticated("no valid token supplied")),
        }
    }

    async fn revoke_session(&self, sid: Uuid) -> Result<()> {
        self.sessions.revoke(sid).await?;
        self.cache
            .set_flag(
                &self.revoked_key(sid),
                Duration::from_secs(self.tokens.access_ttl().num_seconds().max(1) as u64 + 60),
            )
            .await
            .ok();
        Ok(())
    }

    fn revoked_key(&self, sid: Uuid) -> String {
        self.cache.key(&["revoked", &sid.to_string()])
    }

    /// Verify an access token and make sure its session has not been revoked.
    pub async fn verify_access(&self, token: &str) -> Result<SessionContext> {
        let claims = self.tokens.verify(token, TokenKind::Access)?;
        if self
            .cache
            .exists(&self.revoked_key(claims.sid))
            .await
            .unwrap_or(false)
        {
            return Err(Error::unauthenticated("session revoked"));
        }
        Ok(SessionContext { claims })
    }

    pub async fn get(&self, id: Uuid) -> Result<Identity> {
        self.identities.get(id).await
    }

    pub async fn get_by_external_id(&self, external_id: &str) -> Result<Identity> {
        self.identities.get_by_external_id(external_id).await
    }

    pub async fn list(&self, after: Option<Uuid>, limit: i64) -> Result<Vec<Identity>> {
        self.identities.list(after, limit).await
    }

    /// Create or update an identity without issuing a session (trusted backends).
    pub async fn upsert(
        &self,
        external_id: &str,
        default: PropertyMap,
        custom: PropertyMap,
    ) -> Result<Identity> {
        validate_external_id(external_id)?;
        properties::validate(&default)?;
        properties::validate(&custom)?;
        let (identity, created) = self
            .identities
            .authenticate(external_id, &default, &custom)
            .await?;
        if created {
            metrics::counter!("zanshin_identities_created_total").increment(1);
        }
        Ok(identity)
    }

    pub async fn update_properties(
        &self,
        id: Uuid,
        default: PropertyMap,
        custom: PropertyMap,
    ) -> Result<Identity> {
        properties::validate(&default)?;
        properties::validate(&custom)?;
        self.identities
            .update_properties(id, &default, &custom)
            .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let sids = self.sessions.revoke_all_for_identity(id).await?;
        for sid in sids {
            self.cache
                .set_flag(
                    &self.revoked_key(sid),
                    Duration::from_secs(self.tokens.access_ttl().num_seconds().max(1) as u64 + 60),
                )
                .await
                .ok();
        }
        self.identities.delete(id).await?;
        metrics::counter!("zanshin_identities_deleted_total").increment(1);
        Ok(())
    }

    pub async fn purge_expired_sessions(&self) -> Result<u64> {
        self.sessions.purge_expired().await
    }
}

fn refresh_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}
