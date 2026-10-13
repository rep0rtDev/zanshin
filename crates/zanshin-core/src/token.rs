//! Session tokens: stateless HS256 JWTs for access, refresh tokens bound to a stored session.

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenKind {
    Access,
    Refresh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Identity id.
    pub sub: Uuid,
    /// External identity id.
    pub xid: String,
    /// Session id (shared by the access and refresh token of one session).
    pub sid: Uuid,
    pub typ: TokenKind,
    /// Unique token id; makes every issued token distinct even within one second.
    pub jti: Uuid,
    pub iat: i64,
    pub exp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
}

impl Claims {
    pub fn expires_at(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(self.exp, 0).unwrap_or_else(Utc::now)
    }
}

#[derive(Clone)]
pub struct TokenIssuer {
    enc: EncodingKey,
    dec: DecodingKey,
    issuer: String,
    access_ttl: Duration,
    refresh_ttl: Duration,
}

impl std::fmt::Debug for TokenIssuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenIssuer")
            .field("issuer", &self.issuer)
            .field("access_ttl", &self.access_ttl)
            .field("refresh_ttl", &self.refresh_ttl)
            .finish_non_exhaustive()
    }
}

pub struct IssuedSession {
    pub session_id: Uuid,
    pub access: String,
    pub refresh: String,
    pub refresh_expires_at: DateTime<Utc>,
}

impl TokenIssuer {
    pub fn new(
        secret: &[u8],
        issuer: impl Into<String>,
        access_ttl: std::time::Duration,
        refresh_ttl: std::time::Duration,
    ) -> Result<Self> {
        if secret.len() < 32 {
            return Err(Error::invalid("jwt secret must be at least 32 bytes"));
        }
        Ok(Self {
            enc: EncodingKey::from_secret(secret),
            dec: DecodingKey::from_secret(secret),
            issuer: issuer.into(),
            access_ttl: Duration::from_std(access_ttl).map_err(Error::internal)?,
            refresh_ttl: Duration::from_std(refresh_ttl).map_err(Error::internal)?,
        })
    }

    pub fn access_ttl(&self) -> Duration {
        self.access_ttl
    }

    pub fn refresh_ttl(&self) -> Duration {
        self.refresh_ttl
    }

    /// Issue a brand new session (new session id) for an identity.
    pub fn issue_new(&self, identity_id: Uuid, external_id: &str) -> Result<IssuedSession> {
        self.issue_for_session(Uuid::now_v7(), identity_id, external_id)
    }

    /// Issue a fresh token pair for an existing session id (refresh rotation).
    pub fn issue_for_session(
        &self,
        session_id: Uuid,
        identity_id: Uuid,
        external_id: &str,
    ) -> Result<IssuedSession> {
        let now = Utc::now();
        let access = self.sign(Claims {
            sub: identity_id,
            xid: external_id.to_string(),
            sid: session_id,
            typ: TokenKind::Access,
            jti: Uuid::new_v4(),
            iat: now.timestamp(),
            exp: (now + self.access_ttl).timestamp(),
            iss: Some(self.issuer.clone()),
        })?;
        let refresh_exp = now + self.refresh_ttl;
        let refresh = self.sign(Claims {
            sub: identity_id,
            xid: external_id.to_string(),
            sid: session_id,
            typ: TokenKind::Refresh,
            jti: Uuid::new_v4(),
            iat: now.timestamp(),
            exp: refresh_exp.timestamp(),
            iss: Some(self.issuer.clone()),
        })?;
        Ok(IssuedSession {
            session_id,
            access,
            refresh,
            refresh_expires_at: refresh_exp,
        })
    }

    fn sign(&self, claims: Claims) -> Result<String> {
        encode(&Header::new(Algorithm::HS256), &claims, &self.enc).map_err(Error::internal)
    }

    /// Verify signature, expiry and token kind.
    pub fn verify(&self, token: &str, expected: TokenKind) -> Result<Claims> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.leeway = 5;
        let data = decode::<Claims>(token, &self.dec, &validation).map_err(|e| {
            use jsonwebtoken::errors::ErrorKind::*;
            match e.kind() {
                ExpiredSignature => Error::unauthenticated("token expired"),
                _ => Error::unauthenticated("invalid token"),
            }
        })?;
        if data.claims.typ != expected {
            return Err(Error::unauthenticated("wrong token type"));
        }
        Ok(data.claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration as StdDuration;

    fn issuer() -> TokenIssuer {
        TokenIssuer::new(
            b"0123456789abcdef0123456789abcdef",
            "zanshin-test",
            StdDuration::from_secs(60),
            StdDuration::from_secs(3600),
        )
        .unwrap()
    }

    #[test]
    fn round_trip() {
        let iss = issuer();
        let id = Uuid::now_v7();
        let s = iss.issue_new(id, "player-1").unwrap();
        let a = iss.verify(&s.access, TokenKind::Access).unwrap();
        let r = iss.verify(&s.refresh, TokenKind::Refresh).unwrap();
        assert_eq!(a.sub, id);
        assert_eq!(a.sid, r.sid);
        assert_eq!(a.xid, "player-1");
        assert!(iss.verify(&s.access, TokenKind::Refresh).is_err());
    }

    #[test]
    fn rotation_within_same_second_yields_distinct_tokens() {
        let iss = issuer();
        let id = Uuid::now_v7();
        let a = iss.issue_new(id, "p").unwrap();
        let b = iss.issue_for_session(a.session_id, id, "p").unwrap();
        assert_ne!(a.refresh, b.refresh);
        assert_ne!(a.access, b.access);
    }

    #[test]
    fn rejects_short_secret() {
        assert!(TokenIssuer::new(
            b"short",
            "x",
            StdDuration::from_secs(1),
            StdDuration::from_secs(1)
        )
        .is_err());
    }

    #[test]
    fn rejects_other_secret() {
        let a = issuer();
        let b = TokenIssuer::new(
            b"fedcba9876543210fedcba9876543210",
            "zanshin-test",
            StdDuration::from_secs(60),
            StdDuration::from_secs(60),
        )
        .unwrap();
        let s = a.issue_new(Uuid::now_v7(), "p").unwrap();
        assert!(b.verify(&s.access, TokenKind::Access).is_err());
    }
}
