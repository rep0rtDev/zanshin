//! Console users: Argon2id passwords, TOTP second factor with recovery codes, server-side
//! sessions (opaque cookie token, SHA-256 stored), admin-issued password-reset links.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zanshin_core::{
    apikey::ApiKeyKind,
    permissions::{archetype, Permissions},
    Error, Result,
};
use zanshin_store::console_users::{ConsoleSessionRow, ConsoleUserRepo, ConsoleUserRow, UserPatch};

use super::apikeys::Principal;

pub const SESSION_COOKIE: &str = "zanshin_console_user";
const RESET_TTL: Duration = Duration::hours(24);
const TOTP_STEP: i64 = 30;
const TOTP_DIGITS: u32 = 6;
const RECOVERY_CODES: usize = 10;
const MIN_PASSWORD: usize = 12;

#[derive(Clone)]
pub struct ConsoleUserService {
    repo: ConsoleUserRepo,
    session_ttl: std::time::Duration,
    issuer: String,
}

#[allow(clippy::large_enum_variant)]
pub enum LoginOutcome {
    /// Password ok, TOTP required: the client must repeat with `code`.
    MfaRequired,
    Ok {
        user: ConsoleUserRow,
        token: String,
        session: ConsoleSessionRow,
    },
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::internal(anyhow::anyhow!("argon2: {e}")))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| {
            Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
        .unwrap_or(false)
}

fn check_password_policy(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(Error::invalid(format!(
            "password must be at least {MIN_PASSWORD} characters"
        )));
    }
    if password.len() > 256 {
        return Err(Error::invalid("password too long"));
    }
    let lowered = password.to_ascii_lowercase();
    if ["password", "zanshin", "12345678", "qwerty"]
        .iter()
        .any(|w| lowered.contains(w))
    {
        return Err(Error::invalid("password is too guessable"));
    }
    Ok(())
}

/// RFC 6238 TOTP (SHA-1, 30 s, 6 digits) — what Google Authenticator / 1Password / Authy expect.
pub fn totp_code(secret_b32: &str, unix_time: i64) -> Option<String> {
    let secret = base32::decode(base32::Alphabet::Rfc4648 { padding: false }, secret_b32)?;
    let counter = (unix_time / TOTP_STEP) as u64;
    let mut mac = Hmac::<Sha1>::new_from_slice(&secret).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let bin = ((digest[offset] as u32 & 0x7f) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | digest[offset + 3] as u32;
    Some(format!(
        "{:0width$}",
        bin % 10u32.pow(TOTP_DIGITS),
        width = TOTP_DIGITS as usize
    ))
}

fn totp_matches(secret_b32: &str, code: &str) -> bool {
    let now = Utc::now().timestamp();
    let code = code.trim();
    (-1..=1).any(|w| totp_code(secret_b32, now + w * TOTP_STEP).as_deref() == Some(code))
}

fn new_totp_secret() -> String {
    let mut buf = [0u8; 20];
    OsRng.fill_bytes(&mut buf);
    base32::encode(base32::Alphabet::Rfc4648 { padding: false }, &buf)
}

fn recovery_code() -> String {
    let mut buf = [0u8; 5];
    OsRng.fill_bytes(&mut buf);
    let s = base32::encode(base32::Alphabet::Rfc4648 { padding: false }, &buf).to_lowercase();
    format!("{}-{}", &s[..4], &s[4..8])
}

fn recovery_hash(code: &str) -> String {
    hex(&Sha256::digest(
        code.trim().to_lowercase().replace('-', "").as_bytes(),
    ))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub struct MfaSetup {
    pub secret: String,
    pub otpauth_url: String,
}

impl ConsoleUserService {
    pub fn new(repo: ConsoleUserRepo, session_ttl: std::time::Duration, issuer: String) -> Self {
        Self {
            repo,
            session_ttl,
            issuer,
        }
    }

    pub fn repo(&self) -> &ConsoleUserRepo {
        &self.repo
    }

    pub fn session_ttl(&self) -> std::time::Duration {
        self.session_ttl
    }

    pub fn principal_for(user: &ConsoleUserRow) -> Principal {
        let permissions = user.permissions();
        Principal {
            key_id: None,
            user_id: Some(user.id),
            name: user.email.clone(),
            kind: ApiKeyKind::Admin,
            read_only: !permissions.can_write_anything(),
            rate_limit_per_minute: None,
            role: permissions.legacy_role(),
            permissions: Some(permissions),
        }
    }

    pub fn resolve_permissions(
        archetype_name: Option<&str>,
        explicit: Option<Permissions>,
    ) -> Result<Permissions> {
        let p = match (archetype_name, explicit) {
            (_, Some(p)) => p,
            (Some(a), None) => {
                archetype(a).ok_or_else(|| Error::invalid(format!("unknown archetype {a:?}")))?
            }
            (None, None) => Permissions::read_all(),
        };
        p.validate().map_err(Error::invalid)?;
        if p.is_empty() {
            return Err(Error::invalid("permissions must grant at least one action"));
        }
        Ok(p)
    }

    pub async fn create(
        &self,
        email: &str,
        name: &str,
        password: Option<&str>,
        archetype_name: Option<&str>,
        permissions: Option<Permissions>,
        created_by: &str,
    ) -> Result<ConsoleUserRow> {
        let email = email.trim();
        if !email.contains('@') || email.len() > 254 {
            return Err(Error::invalid("a valid email is required"));
        }
        if name.trim().is_empty() || name.len() > 128 {
            return Err(Error::invalid("name must be 1..128 characters"));
        }
        let hash = match password {
            Some(p) => {
                check_password_policy(p)?;
                Some(hash_password(p)?)
            }
            None => None,
        };
        let perms = Self::resolve_permissions(archetype_name, permissions)?;
        self.repo
            .create(
                email,
                name.trim(),
                hash.as_deref(),
                &perms,
                archetype_name,
                password.is_some(),
                created_by,
            )
            .await
    }

    pub async fn update(
        &self,
        id: Uuid,
        mut patch: UserPatch,
        archetype_name: Option<Option<&str>>,
    ) -> Result<ConsoleUserRow> {
        if let Some(Some(a)) = archetype_name {
            if patch.permissions.is_none() {
                patch.permissions = Some(Self::resolve_permissions(Some(a), None)?);
            }
            patch.archetype = Some(Some(a.to_string()));
        } else if let Some(None) = archetype_name {
            patch.archetype = Some(None);
        }
        if let Some(p) = &patch.permissions {
            p.validate().map_err(Error::invalid)?;
            if p.is_empty() {
                return Err(Error::invalid("permissions must grant at least one action"));
            }
        }
        let user = self.repo.update(id, &patch).await?;
        if patch.disabled == Some(true) || patch.permissions.is_some() {
            // Permission changes take effect immediately: drop live sessions.
            self.repo.revoke_all_sessions(id).await?;
        }
        Ok(user)
    }

    pub async fn login(
        &self,
        email: &str,
        password: &str,
        code: Option<&str>,
        ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<LoginOutcome> {
        let invalid = || Error::unauthenticated("invalid email or password");
        let user = self
            .repo
            .get_by_email(email.trim())
            .await?
            .ok_or_else(invalid)?;
        let hash = user.password_hash.as_deref().ok_or_else(invalid)?;
        if user.disabled || !verify_password(password, hash) {
            return Err(invalid());
        }
        if user.totp_enabled {
            let Some(code) = code.map(str::trim).filter(|c| !c.is_empty()) else {
                return Ok(LoginOutcome::MfaRequired);
            };
            let secret = user.totp_secret.as_deref().unwrap_or_default();
            if !totp_matches(secret, code) {
                let burned = self
                    .repo
                    .consume_recovery_code(user.id, &recovery_hash(code))
                    .await?;
                if !burned {
                    return Err(Error::unauthenticated("invalid one-time code"));
                }
            }
        }
        let token = random_token(32);
        let session = self
            .repo
            .create_session(
                user.id,
                &token_hash(&token),
                Duration::from_std(self.session_ttl).unwrap_or_else(|_| Duration::hours(12)),
                ip,
                user_agent,
            )
            .await?;
        self.repo.touch_login(user.id).await?;
        Ok(LoginOutcome::Ok {
            user,
            token,
            session,
        })
    }

    pub async fn verify_session(
        &self,
        token: &str,
    ) -> Result<Option<(ConsoleSessionRow, ConsoleUserRow)>> {
        self.repo.session_user(&token_hash(token)).await
    }

    pub async fn logout(&self, user_id: Uuid, session_id: Uuid) -> Result<()> {
        self.repo.revoke_session(user_id, session_id).await
    }

    pub async fn change_password(
        &self,
        user: &ConsoleUserRow,
        current: &str,
        new: &str,
    ) -> Result<()> {
        let hash = user
            .password_hash
            .as_deref()
            .ok_or_else(|| Error::denied("password login is not set up for this user"))?;
        if !verify_password(current, hash) {
            return Err(Error::unauthenticated("current password is wrong"));
        }
        check_password_policy(new)?;
        self.repo
            .set_password(user.id, &hash_password(new)?, false)
            .await?;
        self.repo.revoke_all_sessions(user.id).await?;
        Ok(())
    }

    /// Admin-issued reset: returns the one-time token (the operator hands the link to the user;
    /// no mail transport is assumed).
    pub async fn issue_reset(&self, user_id: Uuid) -> Result<String> {
        self.repo.get(user_id).await?;
        let token = random_token(32);
        self.repo
            .create_reset(user_id, &token_hash(&token), RESET_TTL)
            .await?;
        Ok(token)
    }

    pub async fn complete_reset(&self, token: &str, new_password: &str) -> Result<ConsoleUserRow> {
        check_password_policy(new_password)?;
        let user_id = self
            .repo
            .consume_reset(&token_hash(token))
            .await?
            .ok_or_else(|| Error::unauthenticated("reset link is invalid or expired"))?;
        self.repo
            .set_password(user_id, &hash_password(new_password)?, false)
            .await?;
        self.repo.revoke_all_sessions(user_id).await?;
        self.repo.get(user_id).await
    }

    pub async fn mfa_setup(&self, user: &ConsoleUserRow) -> Result<MfaSetup> {
        if user.totp_enabled {
            return Err(Error::conflict(
                "two-factor authentication is already enabled",
            ));
        }
        let secret = new_totp_secret();
        self.repo
            .set_totp(user.id, Some(&secret), false, &[])
            .await?;
        let label = format!("{}:{}", self.issuer, user.email);
        let otpauth_url = format!(
            "otpauth://totp/{}?secret={}&issuer={}&algorithm=SHA1&digits=6&period=30",
            urlencode(&label),
            secret,
            urlencode(&self.issuer)
        );
        Ok(MfaSetup {
            secret,
            otpauth_url,
        })
    }

    /// Confirms the authenticator works and returns the recovery codes (shown once).
    pub async fn mfa_enable(&self, user: &ConsoleUserRow, code: &str) -> Result<Vec<String>> {
        let secret = user
            .totp_secret
            .as_deref()
            .ok_or_else(|| Error::invalid("call mfa/setup first"))?;
        if !totp_matches(secret, code) {
            return Err(Error::unauthenticated("invalid one-time code"));
        }
        let codes: Vec<String> = (0..RECOVERY_CODES).map(|_| recovery_code()).collect();
        let hashes: Vec<String> = codes.iter().map(|c| recovery_hash(c)).collect();
        self.repo
            .set_totp(user.id, Some(secret), true, &hashes)
            .await?;
        Ok(codes)
    }

    pub async fn mfa_disable(
        &self,
        user: &ConsoleUserRow,
        code: &str,
        by_admin: bool,
    ) -> Result<()> {
        if !by_admin {
            let secret = user.totp_secret.as_deref().unwrap_or_default();
            if !user.totp_enabled || !totp_matches(secret, code) {
                return Err(Error::unauthenticated("invalid one-time code"));
            }
        }
        self.repo.set_totp(user.id, None, false, &[]).await
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_rfc6238_vector() {
        // RFC 6238 appendix B, SHA-1, secret "12345678901234567890" (base32 GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ), 8 digits truncated to 6.
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert_eq!(totp_code(secret, 59).as_deref(), Some("287082"));
        assert_eq!(totp_code(secret, 1111111109).as_deref(), Some("081804"));
    }

    #[test]
    fn password_roundtrip() {
        let h = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &h));
        assert!(!verify_password("wrong", &h));
    }
}
