//! OpenID Connect login for the admin console: authorization code flow with PKCE,
//! ID-token verification against the provider's JWKS, claim → role mapping and a
//! stateless HS256 cookie session.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{jwk::JwkSet, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use zanshin_core::{
    apikey::{ApiKeyKind, Role},
    Error, Result,
};

use crate::config::OidcConfig;

use super::apikeys::Principal;

pub const SESSION_COOKIE: &str = "zanshin_console";
pub const STATE_COOKIE: &str = "zanshin_oidc_state";
const STATE_TTL: Duration = Duration::from_secs(600);
const JWKS_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

/// A console user established through OIDC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsoleUser {
    pub sub: String,
    pub email: String,
    pub name: String,
    pub role: Role,
}

impl ConsoleUser {
    pub fn principal(&self) -> Principal {
        Principal {
            key_id: None,
            name: if self.email.is_empty() {
                self.sub.clone()
            } else {
                self.email.clone()
            },
            kind: ApiKeyKind::Admin,
            read_only: self.role == Role::Viewer,
            rate_limit_per_minute: None,
            role: self.role,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct StateClaims {
    state: String,
    nonce: String,
    verifier: String,
    exp: i64,
}

#[derive(Serialize, Deserialize)]
struct SessionClaims {
    sub: String,
    email: String,
    name: String,
    role: Role,
    iat: i64,
    exp: i64,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
}

#[derive(Deserialize)]
struct IdClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

/// Where to send the browser plus the state cookie to set.
pub struct LoginStart {
    pub redirect: String,
    pub state_cookie: String,
}

type CachedJwks = Option<(Instant, Arc<JwkSet>)>;

#[derive(Clone)]
pub struct OidcService {
    cfg: Option<Arc<OidcConfig>>,
    http: reqwest::Client,
    enc: EncodingKey,
    dec: DecodingKey,
    session_ttl: Duration,
    discovery: Arc<RwLock<Option<Arc<Discovery>>>>,
    jwks: Arc<RwLock<CachedJwks>>,
}

impl OidcService {
    pub fn new(cfg: Option<OidcConfig>, jwt_secret: &str, session_ttl: Duration) -> Self {
        // Separate key material from identity tokens so the two can never be confused.
        let secret = Sha256::new()
            .chain_update(b"zanshin-console:")
            .chain_update(jwt_secret.as_bytes())
            .finalize();
        Self {
            cfg: cfg.map(Arc::new),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
            enc: EncodingKey::from_secret(&secret),
            dec: DecodingKey::from_secret(&secret),
            session_ttl,
            discovery: Arc::new(RwLock::new(None)),
            jwks: Arc::new(RwLock::new(None)),
        }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.is_some()
    }

    pub fn session_ttl(&self) -> Duration {
        self.session_ttl
    }

    /// Cookies must be `Secure` when the console is served over https.
    pub fn secure_cookies(&self) -> bool {
        self.cfg
            .as_ref()
            .map(|c| c.redirect_url.starts_with("https://"))
            .unwrap_or(false)
    }

    fn cfg(&self) -> Result<&OidcConfig> {
        self.cfg
            .as_deref()
            .ok_or_else(|| Error::invalid("oidc login is not configured"))
    }

    async fn discovery(&self) -> Result<Arc<Discovery>> {
        if let Some(d) = self.discovery.read().await.clone() {
            return Ok(d);
        }
        let cfg = self.cfg()?;
        let url = format!(
            "{}/.well-known/openid-configuration",
            cfg.issuer.trim_end_matches('/')
        );
        let d: Discovery = self
            .http
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| Error::unavailable(format!("oidc discovery failed: {e}")))?
            .json()
            .await
            .map_err(|e| Error::unavailable(format!("oidc discovery malformed: {e}")))?;
        let d = Arc::new(d);
        *self.discovery.write().await = Some(d.clone());
        Ok(d)
    }

    async fn jwks(&self, force: bool) -> Result<Arc<JwkSet>> {
        if !force {
            if let Some((at, set)) = self.jwks.read().await.clone() {
                if at.elapsed() < JWKS_TTL {
                    return Ok(set);
                }
            }
        }
        let d = self.discovery().await?;
        let set: JwkSet = self
            .http
            .get(&d.jwks_uri)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| Error::unavailable(format!("oidc jwks fetch failed: {e}")))?
            .json()
            .await
            .map_err(|e| Error::unavailable(format!("oidc jwks malformed: {e}")))?;
        let set = Arc::new(set);
        *self.jwks.write().await = Some((Instant::now(), set.clone()));
        Ok(set)
    }

    /// Build the provider redirect and a signed state cookie carrying state/nonce/PKCE verifier.
    pub async fn begin_login(&self) -> Result<LoginStart> {
        let cfg = self.cfg()?;
        let d = self.discovery().await?;
        let state = random_token();
        let nonce = random_token();
        let verifier = random_token();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let now = chrono::Utc::now().timestamp();
        let state_cookie = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &StateClaims {
                state: state.clone(),
                nonce: nonce.clone(),
                verifier,
                exp: now + STATE_TTL.as_secs() as i64,
            },
            &self.enc,
        )
        .map_err(|e| Error::internal(anyhow::anyhow!("sign oidc state: {e}")))?;
        let mut url = url::Url::parse(&d.authorization_endpoint)
            .map_err(|e| Error::unavailable(format!("bad authorization_endpoint: {e}")))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &cfg.client_id)
            .append_pair("redirect_uri", &cfg.redirect_url)
            .append_pair("scope", &cfg.scopes.join(" "))
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        Ok(LoginStart {
            redirect: url.into(),
            state_cookie,
        })
    }

    /// Exchange the authorization code, verify the ID token and map the user to a role.
    pub async fn finish_login(
        &self,
        code: &str,
        state: &str,
        state_cookie: Option<&str>,
    ) -> Result<ConsoleUser> {
        let cfg = self.cfg()?;
        let st = state_cookie
            .ok_or_else(|| Error::unauthenticated("missing oidc state cookie"))
            .and_then(|c| {
                let mut v = Validation::new(Algorithm::HS256);
                v.validate_exp = true;
                jsonwebtoken::decode::<StateClaims>(c, &self.dec, &v)
                    .map(|d| d.claims)
                    .map_err(|_| Error::unauthenticated("invalid or expired oidc state"))
            })?;
        if st.state != state {
            return Err(Error::unauthenticated("oidc state mismatch"));
        }
        let d = self.discovery().await?;
        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", cfg.redirect_url.clone()),
            ("client_id", cfg.client_id.clone()),
            ("code_verifier", st.verifier.clone()),
        ];
        if let Some(secret) = &cfg.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        let tok: TokenResponse = self
            .http
            .post(&d.token_endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|e| Error::unavailable(format!("oidc token request failed: {e}")))?
            .error_for_status()
            .map_err(|e| Error::unauthenticated(format!("oidc token exchange rejected: {e}")))?
            .json()
            .await
            .map_err(|e| Error::unavailable(format!("oidc token response malformed: {e}")))?;
        let claims = self.verify_id_token(&tok.id_token, &d.issuer).await?;
        if claims.nonce.as_deref() != Some(st.nonce.as_str()) {
            return Err(Error::unauthenticated("oidc nonce mismatch"));
        }
        let email = claims.email.clone().unwrap_or_default().to_lowercase();
        if !cfg.allowed_domains.is_empty() {
            let domain = email.rsplit('@').next().unwrap_or("");
            if email.is_empty()
                || !cfg
                    .allowed_domains
                    .iter()
                    .any(|d| d.eq_ignore_ascii_case(domain))
            {
                return Err(Error::denied("email domain is not allowed"));
            }
        }
        let role = self.map_role(&claims)?;
        Ok(ConsoleUser {
            sub: claims.sub,
            email,
            name: claims
                .name
                .or(claims.preferred_username)
                .unwrap_or_default(),
            role,
        })
    }

    async fn verify_id_token(&self, token: &str, issuer: &str) -> Result<IdClaims> {
        let cfg = self.cfg()?;
        let header = jsonwebtoken::decode_header(token)
            .map_err(|_| Error::unauthenticated("malformed id_token"))?;
        if matches!(
            header.alg,
            Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
        ) {
            return Err(Error::unauthenticated(
                "id_token must be asymmetrically signed",
            ));
        }
        let find = |set: &JwkSet| match &header.kid {
            Some(kid) => set.find(kid).cloned(),
            None if set.keys.len() == 1 => set.keys.first().cloned(),
            None => None,
        };
        let jwk = match find(&*self.jwks(false).await?) {
            Some(k) => k,
            None => find(&*self.jwks(true).await?)
                .ok_or_else(|| Error::unauthenticated("id_token signed by unknown key"))?,
        };
        let key = DecodingKey::from_jwk(&jwk)
            .map_err(|e| Error::unavailable(format!("unsupported jwk: {e}")))?;
        let mut v = Validation::new(header.alg);
        v.set_audience(&[cfg.client_id.as_str()]);
        v.set_issuer(&[issuer, cfg.issuer.trim_end_matches('/')]);
        v.leeway = 60;
        jsonwebtoken::decode::<IdClaims>(token, &key, &v)
            .map(|d| d.claims)
            .map_err(|e| Error::unauthenticated(format!("invalid id_token: {e}")))
    }

    fn map_role(&self, claims: &IdClaims) -> Result<Role> {
        let cfg = self.cfg()?;
        let values: Vec<String> = match claims.rest.get(&cfg.role_claim) {
            Some(serde_json::Value::String(s)) => vec![s.clone()],
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            _ => vec![],
        };
        let mapped = values
            .iter()
            .filter_map(|v| cfg.role_map.get(v).copied())
            .max();
        mapped
            .or(cfg.default_role)
            .ok_or_else(|| Error::denied("no console role granted to this account"))
    }

    pub fn issue_session(&self, user: &ConsoleUser) -> Result<String> {
        let now = chrono::Utc::now().timestamp();
        jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &SessionClaims {
                sub: user.sub.clone(),
                email: user.email.clone(),
                name: user.name.clone(),
                role: user.role,
                iat: now,
                exp: now + self.session_ttl.as_secs() as i64,
            },
            &self.enc,
        )
        .map_err(|e| Error::internal(anyhow::anyhow!("sign console session: {e}")))
    }

    pub fn verify_session(&self, token: &str) -> Option<ConsoleUser> {
        let mut v = Validation::new(Algorithm::HS256);
        v.validate_exp = true;
        let c = jsonwebtoken::decode::<SessionClaims>(token, &self.dec, &v)
            .ok()?
            .claims;
        Some(ConsoleUser {
            sub: c.sub,
            email: c.email,
            name: c.name,
            role: c.role,
        })
    }
}

fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

/// Extract a named cookie from a `Cookie` header value.
pub fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}
