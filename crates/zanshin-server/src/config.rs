use std::collections::HashMap;
use std::{net::SocketAddr, path::Path, time::Duration};

use anyhow::Context;
use figment::{
    providers::{Env, Format, Serialized, Yaml},
    Figment,
};
use serde::{Deserialize, Serialize};
use zanshin_store::{ClickHouseConfig, DatabaseConfig, RedisConfig};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// The merged configuration as loaded (defaults + file + environment), kept for
    /// `GET /admin/v1/system/config`; see [`Config::redacted`].
    #[serde(skip)]
    pub effective: serde_json::Value,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    #[serde(default = "default_clickhouse")]
    pub clickhouse: ClickHouseConfig,
    pub auth: AuthConfig,
    #[serde(default)]
    pub admin: AdminConfig,
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub push: PushConfig,
    #[serde(default)]
    pub limits: LimitsConfig,
    #[serde(default)]
    pub audit: AuditConfig,
    #[serde(default)]
    pub taxonomy: TaxonomyConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub retention: RetentionConfig,
    #[serde(default)]
    pub security: SecurityConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    /// Data keys for encrypting stored secrets (integration/sink configs, webhook secrets,
    /// notification URLs/headers) at rest: `kid:base64(32 bytes)[,kid2:...]`. The first key seals
    /// new values, all of them can open; rotate by prepending a key and running
    /// `zanshin --rewrap-secrets`. Unset = plaintext at rest.
    #[serde(default)]
    pub data_keys: Option<String>,
}

/// Event taxonomy enforcement and player-session tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxonomyConfig {
    /// `off`: accept anything; `warn`: store but report violations; `strict`: reject violations.
    #[serde(default = "default_taxonomy_mode")]
    pub mode: String,
    /// Register unknown event names automatically (as "discovered").
    #[serde(default = "default_true")]
    pub auto_discover: bool,
    /// Path to a MaxMind GeoLite2/GeoIP2 City or Country .mmdb; empty disables geo-IP.
    #[serde(default)]
    pub geoip_db: String,
    /// A play session with no activity for this long is closed by housekeeping.
    #[serde(default = "default_session_inactivity", with = "humantime_serde")]
    pub session_inactivity: Duration,
    /// Number of rejected/invalid events kept for the taxonomy debugger.
    #[serde(default = "default_rejected_keep")]
    pub rejected_keep: usize,
    /// Optional USD-base rate feed (`{"date": "YYYY-MM-DD", "rates": {"EUR": 0.92}}`, e.g.
    /// `https://api.frankfurter.app/latest?from=USD`), polled every 6h to convert revenue to USD.
    #[serde(default)]
    pub currency_rates_url: Option<String>,
}

impl Default for TaxonomyConfig {
    fn default() -> Self {
        Self {
            mode: default_taxonomy_mode(),
            auto_discover: true,
            geoip_db: String::new(),
            session_inactivity: default_session_inactivity(),
            rejected_keep: default_rejected_keep(),
            currency_rates_url: None,
        }
    }
}

fn default_taxonomy_mode() -> String {
    "warn".into()
}

fn default_session_inactivity() -> Duration {
    Duration::from_secs(30 * 60)
}

fn default_rejected_keep() -> usize {
    200
}

/// Push notification delivery. `provider`: `none` (default), `log` (record only; dev/test),
/// `live` (FCM HTTP v1 and/or APNs, whichever is configured).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PushConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub fcm: FcmConfig,
    #[serde(default)]
    pub apns: ApnsConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FcmConfig {
    #[serde(default)]
    pub project_id: String,
    /// Path to a Google service-account JSON file (client_email + private_key).
    #[serde(default)]
    pub service_account_file: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApnsConfig {
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub key_id: String,
    /// Path to the .p8 signing key.
    #[serde(default)]
    pub key_file: String,
    #[serde(default)]
    pub bundle_id: String,
    #[serde(default)]
    pub sandbox: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_http_addr")]
    pub http_addr: SocketAddr,
    #[serde(default = "default_grpc_addr")]
    pub grpc_addr: SocketAddr,
    /// Allowed CORS origins for browser clients. Empty = allow any origin.
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// Maximum request body size in bytes.
    #[serde(default = "default_body_limit")]
    pub body_limit: usize,
    /// Per-request timeout.
    #[serde(default = "default_request_timeout", with = "humantime_serde")]
    pub request_timeout: Duration,
    /// Trust `X-Forwarded-For` / `X-Real-IP` from the immediate peer (set when behind a reverse proxy).
    #[serde(default)]
    pub trust_proxy: bool,
    /// Emit `Strict-Transport-Security` (only when TLS is terminated in front of Zanshin).
    #[serde(default)]
    pub hsts: bool,
    /// Production guard: refuse to start with placeholder secrets, wildcard CORS or plain-text logs.
    #[serde(default)]
    pub production: bool,
    /// After SIGTERM, keep serving (with `/readyz` → 503) for this long so load balancers drain
    /// before the listeners close.
    #[serde(default = "default_shutdown_grace", with = "humantime_serde")]
    pub shutdown_grace: Duration,
    /// Hard deadline for in-flight requests and WebSocket streams after the drain period.
    #[serde(default = "default_shutdown_timeout", with = "humantime_serde")]
    pub shutdown_timeout: Duration,
    /// Largest gRPC message accepted (bytes).
    #[serde(default = "default_grpc_max_message")]
    pub grpc_max_message_bytes: usize,
    /// Maximum concurrent in-flight gRPC requests per process (0 = unlimited).
    #[serde(default = "default_grpc_concurrency")]
    pub grpc_concurrency: usize,
    /// Maximum concurrent in-flight HTTP requests per process before answering 503 (0 = unlimited).
    /// Health probes, `/metrics` and WebSocket streams are not counted.
    #[serde(default = "default_http_concurrency")]
    pub http_concurrency: usize,
}

/// Distributed tracing. Spans are exported over OTLP/gRPC when `otlp_endpoint` is set
/// (Jaeger, Tempo, Honeycomb, Datadog agent, ...); Prometheus metrics are always on at `/metrics`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelemetryConfig {
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
    #[serde(default = "default_service_name")]
    pub service_name: String,
    /// Head sampling ratio for traces, 0.0–1.0.
    #[serde(default = "default_sample_ratio")]
    pub sample_ratio: f64,
    /// Extra resource attributes (`deployment.environment=prod`).
    #[serde(default)]
    pub resource: HashMap<String, String>,
}

/// How long housekeeping keeps operational data. `0s` disables a purge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionConfig {
    /// Analytics events in the PostgreSQL fallback store (ClickHouse uses `clickhouse.retention_days`).
    #[serde(default = "default_events_retention", with = "humantime_serde")]
    pub events: Duration,
    #[serde(default = "default_export_runs_retention", with = "humantime_serde")]
    pub export_runs: Duration,
    #[serde(default = "default_config_history_retention", with = "humantime_serde")]
    pub config_history: Duration,
    #[serde(default = "default_config_history_retention", with = "humantime_serde")]
    pub annotations: Duration,
    #[serde(default = "default_journey_runs_retention", with = "humantime_serde")]
    pub journey_runs: Duration,
    #[serde(
        default = "default_webhook_deliveries_retention",
        with = "humantime_serde"
    )]
    pub webhook_deliveries: Duration,
    /// Rows deleted per purge statement (keeps vacuum pressure and lock time bounded).
    #[serde(default = "default_purge_batch")]
    pub purge_batch: u32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        serde_json::from_str("{}").expect("retention defaults")
    }
}

/// Request budgets. Each scope is a token bucket per caller that refills at `*_per_minute / 60`
/// tokens per second and holds `burst_factor` minutes worth of tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Unauthenticated or unidentified requests, keyed by client IP (e.g. `/v1/authenticate`).
    #[serde(default = "default_ip_per_minute")]
    pub ip_per_minute: u32,
    /// Requests carrying a client API key, keyed by key (overridable per key).
    #[serde(default = "default_client_per_minute")]
    pub client_per_minute: u32,
    /// Requests carrying a session token, keyed by identity.
    #[serde(default = "default_session_per_minute")]
    pub session_per_minute: u32,
    /// Requests carrying a server API key, keyed by key (overridable per key).
    #[serde(default = "default_server_per_minute")]
    pub server_per_minute: u32,
    /// Requests carrying an admin API key, keyed by key (overridable per key).
    #[serde(default = "default_admin_per_minute")]
    pub admin_per_minute: u32,
    #[serde(default = "default_burst_factor")]
    pub burst_factor: f64,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ip_per_minute: default_ip_per_minute(),
            client_per_minute: default_client_per_minute(),
            session_per_minute: default_session_per_minute(),
            server_per_minute: default_server_per_minute(),
            admin_per_minute: default_admin_per_minute(),
            burst_factor: default_burst_factor(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Store the (redacted) JSON request body of mutating admin calls.
    #[serde(default = "default_true")]
    pub store_bodies: bool,
    /// Entries older than this are pruned by housekeeping.
    #[serde(default = "default_audit_retention", with = "humantime_serde")]
    pub retention: Duration,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            store_bodies: true,
            retention: default_audit_retention(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// HS256 secret for session tokens (>= 32 bytes).
    pub jwt_secret: String,
    #[serde(default = "default_issuer")]
    pub issuer: String,
    #[serde(default = "default_access_ttl", with = "humantime_serde")]
    pub access_ttl: Duration,
    #[serde(default = "default_refresh_ttl", with = "humantime_serde")]
    pub refresh_ttl: Duration,
    /// Allow `Authenticate` without an API key (development only).
    #[serde(default)]
    pub allow_anonymous_authenticate: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdminConfig {
    /// Bootstrap admin API key. Use it to mint real keys through the admin API, then drop it.
    #[serde(default)]
    pub bootstrap_key: Option<String>,
    /// Single sign-on for the console via any OpenID Connect provider.
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
    /// SAML 2.0 single sign-on for the admin console (SP-initiated, HTTP-POST binding).
    #[serde(default)]
    pub saml: Option<SamlConfig>,
    /// SCIM 2.0 provisioning of console users (bearer token).
    #[serde(default)]
    pub scim: Option<ScimConfig>,
    /// Lifetime of a console login (cookie session) established through OIDC.
    #[serde(default = "default_console_session_ttl", with = "humantime_serde")]
    pub console_session_ttl: Duration,
    /// Enables "Delete all identities" on an audience in the console (development
    /// environments only; refused when `server.production` is set).
    #[serde(default)]
    pub audience_delete_all_enabled: bool,
}

/// OpenID Connect login for the admin console (authorization code + PKCE).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    /// Issuer URL; `{issuer}/.well-known/openid-configuration` must be reachable.
    pub issuer: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    /// Public URL of `/console/auth/callback` as registered at the provider.
    pub redirect_url: String,
    #[serde(default = "default_oidc_scopes")]
    pub scopes: Vec<String>,
    /// ID-token claim holding the user's groups/roles (string or array of strings).
    #[serde(default = "default_role_claim")]
    pub role_claim: String,
    /// Maps claim values to console roles; the most privileged match wins.
    #[serde(default)]
    pub role_map: HashMap<String, zanshin_core::apikey::Role>,
    /// Role for users with no matching claim value. `None` = reject login.
    #[serde(default)]
    pub default_role: Option<zanshin_core::apikey::Role>,
    /// Only allow emails from these domains (empty = any).
    #[serde(default)]
    pub allowed_domains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamlConfig {
    /// IdP single-sign-on URL (HTTP-Redirect binding) the browser is sent to.
    pub idp_sso_url: String,
    /// Expected `Issuer` of responses; empty = not checked.
    #[serde(default)]
    pub idp_entity_id: String,
    /// IdP signing certificate(s), PEM or bare base64 DER. Any of them may sign.
    pub idp_certs: Vec<String>,
    /// Our entity id (defaults to the metadata URL).
    #[serde(default)]
    pub sp_entity_id: String,
    /// Public URL of `/console/auth/saml/acs`.
    pub acs_url: String,
    /// Assertion attribute holding the email (default: NameID when it is an email, else `email` / `mail`).
    #[serde(default = "default_saml_email_attr")]
    pub email_attribute: String,
    #[serde(default = "default_saml_name_attr")]
    pub name_attribute: String,
    /// Attribute holding groups / roles.
    #[serde(default = "default_saml_role_attr")]
    pub role_attribute: String,
    #[serde(default)]
    pub role_map: HashMap<String, zanshin_core::apikey::Role>,
    #[serde(default)]
    pub default_role: Option<zanshin_core::apikey::Role>,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Accept RSA-SHA1 signatures (legacy IdPs). Off by default.
    #[serde(default)]
    pub allow_sha1: bool,
}

fn default_saml_email_attr() -> String {
    "email".into()
}
fn default_saml_name_attr() -> String {
    "name".into()
}
fn default_saml_role_attr() -> String {
    "groups".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScimConfig {
    /// Bearer token the identity provider presents (or `token_file`).
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub token_file: Option<String>,
    /// Archetype for users created without a matching group (`None` = minimal viewer permissions).
    #[serde(default)]
    pub default_archetype: Option<String>,
    /// SCIM group `displayName` → console archetype.
    #[serde(default)]
    pub group_archetypes: HashMap<String, String>,
}

impl ScimConfig {
    pub fn token(&self) -> String {
        if let Some(f) = &self.token_file {
            if let Ok(t) = std::fs::read_to_string(f) {
                return t.trim().to_string();
            }
        }
        self.token.clone()
    }
}

fn default_console_session_ttl() -> Duration {
    Duration::from_secs(12 * 3600)
}

fn default_oidc_scopes() -> Vec<String> {
    vec!["openid".into(), "email".into(), "profile".into()]
}

fn default_role_claim() -> String {
    "groups".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub json: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            json: false,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_ip_per_minute() -> u32 {
    120
}
fn default_client_per_minute() -> u32 {
    600
}
fn default_session_per_minute() -> u32 {
    600
}
fn default_server_per_minute() -> u32 {
    6000
}
fn default_admin_per_minute() -> u32 {
    1200
}
fn default_burst_factor() -> f64 {
    2.0
}
fn default_audit_retention() -> Duration {
    Duration::from_secs(90 * 24 * 3600)
}
fn default_http_addr() -> SocketAddr {
    "0.0.0.0:7550".parse().unwrap()
}
fn default_grpc_addr() -> SocketAddr {
    "0.0.0.0:7549".parse().unwrap()
}
fn default_body_limit() -> usize {
    1024 * 1024
}
fn default_request_timeout() -> Duration {
    Duration::from_secs(30)
}
fn default_issuer() -> String {
    "zanshin".into()
}
fn default_access_ttl() -> Duration {
    Duration::from_secs(60 * 60)
}
fn default_refresh_ttl() -> Duration {
    Duration::from_secs(60 * 60 * 24 * 30)
}
fn default_shutdown_grace() -> Duration {
    Duration::from_secs(5)
}
fn default_shutdown_timeout() -> Duration {
    Duration::from_secs(30)
}
fn default_grpc_max_message() -> usize {
    4 * 1024 * 1024
}
fn default_grpc_concurrency() -> usize {
    1024
}
fn default_http_concurrency() -> usize {
    1024
}
fn default_service_name() -> String {
    "zanshin".into()
}
fn default_sample_ratio() -> f64 {
    1.0
}
fn default_events_retention() -> Duration {
    Duration::from_secs(400 * 24 * 3600)
}
fn default_export_runs_retention() -> Duration {
    Duration::from_secs(30 * 24 * 3600)
}
fn default_config_history_retention() -> Duration {
    Duration::from_secs(400 * 24 * 3600)
}
fn default_journey_runs_retention() -> Duration {
    Duration::from_secs(90 * 24 * 3600)
}
fn default_webhook_deliveries_retention() -> Duration {
    Duration::from_secs(14 * 24 * 3600)
}
fn default_purge_batch() -> u32 {
    10_000
}
fn default_log_level() -> String {
    "info,sqlx=warn,hyper=warn,tower_http=info".into()
}
fn default_clickhouse() -> ClickHouseConfig {
    serde_json::from_str("{}").expect("clickhouse defaults")
}

impl Config {
    /// Layering: built-in defaults < YAML file < `ZANSHIN__*` environment variables.
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        Self::load_file_secrets()?;
        let mut fig = Figment::new().merge(Serialized::defaults(serde_json::json!({
            "server": {},
            "database": { "url": "postgres://zanshin:zanshin@localhost:5432/zanshin" },
            "redis": { "url": "redis://localhost:6379/0" },
            "clickhouse": {},
            "auth": {},
            "admin": {},
            "log": {},
        })));
        if let Some(p) = path {
            fig = fig.merge(Yaml::file(p));
        } else if Path::new("zanshin.yml").exists() {
            fig = fig.merge(Yaml::file("zanshin.yml"));
        }
        fig = fig.merge(
            Env::prefixed("ZANSHIN__")
                .split("__")
                .map(|k| k.as_str().to_lowercase().into()),
        );
        let mut cfg: Config = fig.extract().context("invalid configuration")?;
        cfg.effective = fig.extract().unwrap_or(serde_json::Value::Null);
        cfg.validate()?;
        Ok(cfg)
    }

    /// Effective configuration with secrets masked: any key that looks like a credential
    /// (`*secret*`, `*password*`, `*token*`, `*_key`, `bootstrap_key`, …) becomes `"***"`, and
    /// user-info is stripped from connection URLs.
    pub fn redacted(&self) -> serde_json::Value {
        fn secret_key(k: &str) -> bool {
            let k = k.to_ascii_lowercase();
            [
                "secret",
                "password",
                "passwd",
                "token",
                "credential",
                "private",
            ]
            .iter()
            .any(|w| k.contains(w))
                || k.ends_with("_key")
                || k == "key"
        }
        fn mask_url(v: &str) -> String {
            match v.split_once("://") {
                Some((scheme, rest)) if rest.contains('@') => {
                    let (_, host) = rest.rsplit_once('@').unwrap_or(("", rest));
                    format!("{scheme}://***@{host}")
                }
                _ => v.to_string(),
            }
        }
        fn walk(v: &serde_json::Value) -> serde_json::Value {
            match v {
                serde_json::Value::Object(m) => serde_json::Value::Object(
                    m.iter()
                        .map(|(k, v)| {
                            let out = if secret_key(k) && !v.is_null() && !v.is_object() {
                                serde_json::Value::String("***".into())
                            } else if let (true, Some(s)) =
                                (k.ends_with("url") || k.ends_with("dsn"), v.as_str())
                            {
                                serde_json::Value::String(mask_url(s))
                            } else {
                                walk(v)
                            };
                            (k.clone(), out)
                        })
                        .collect(),
                ),
                serde_json::Value::Array(a) => {
                    serde_json::Value::Array(a.iter().map(walk).collect())
                }
                other => other.clone(),
            }
        }
        walk(&self.effective)
    }

    /// `ZANSHIN_FILE__SECTION__KEY=/run/secrets/x` reads the file and exposes its trimmed content
    /// as `ZANSHIN__SECTION__KEY`, so Docker/Kubernetes secrets never have to live in the
    /// environment or a config file.
    fn load_file_secrets() -> anyhow::Result<()> {
        for (k, path) in std::env::vars() {
            let Some(rest) = k.strip_prefix("ZANSHIN_FILE__") else {
                continue;
            };
            let target = format!("ZANSHIN__{rest}");
            if std::env::var_os(&target).is_some() {
                continue;
            }
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading secret file {path} for {target}"))?;
            std::env::set_var(&target, raw.trim());
        }
        Ok(())
    }

    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.auth.jwt_secret.len() >= 32,
            "auth.jwt_secret must be at least 32 characters"
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.telemetry.sample_ratio),
            "telemetry.sample_ratio must be within 0.0..=1.0"
        );
        if self.server.production {
            let placeholder = |s: &str| s.contains("change-me") || s.contains("changeme");
            anyhow::ensure!(
                !placeholder(&self.auth.jwt_secret),
                "server.production: auth.jwt_secret is a placeholder"
            );
            if let Some(k) = &self.admin.bootstrap_key {
                anyhow::ensure!(
                    !placeholder(k),
                    "server.production: admin.bootstrap_key is a placeholder"
                );
            }
            anyhow::ensure!(
                !self.auth.allow_anonymous_authenticate,
                "server.production: auth.allow_anonymous_authenticate must be false"
            );
            anyhow::ensure!(
                !self.server.cors_origins.is_empty()
                    && !self.server.cors_origins.iter().any(|o| o == "*"),
                "server.production: server.cors_origins must list explicit origins"
            );
            anyhow::ensure!(
                self.database.url.contains("sslmode=") || self.database.url.contains("localhost"),
                "server.production: database.url must set sslmode (or point at localhost)"
            );
        }
        anyhow::ensure!(
            self.auth.access_ttl < self.auth.refresh_ttl,
            "auth.access_ttl must be shorter than auth.refresh_ttl"
        );
        if let Some(k) = &self.admin.bootstrap_key {
            anyhow::ensure!(
                k.len() >= 16,
                "admin.bootstrap_key must be at least 16 characters"
            );
        }
        Ok(())
    }

    pub fn redacted_yaml(&self) -> anyhow::Result<String> {
        let mut v = serde_json::json!({
            "server": self.server,
            "database": { "url": redact_url(&self.database.url), "max_connections": self.database.max_connections, "migrate": self.database.migrate },
            "redis": { "url": redact_url(&self.redis.url), "key_prefix": self.redis.key_prefix },
            "clickhouse": { "enabled": self.clickhouse.enabled, "url": self.clickhouse.url, "user": self.clickhouse.user, "password": "***", "database": self.clickhouse.database },
            "auth": { "jwt_secret": "***", "issuer": self.auth.issuer, "access_ttl": humantime::format_duration(self.auth.access_ttl).to_string(), "refresh_ttl": humantime::format_duration(self.auth.refresh_ttl).to_string(), "allow_anonymous_authenticate": self.auth.allow_anonymous_authenticate },
            "admin": { "bootstrap_key": self.admin.bootstrap_key.as_ref().map(|_| "***") },
            "security": { "data_keys": self.security.data_keys.as_ref().map(|_| "***") },
            "log": self.log,
        });
        if let Some(s) = v.get_mut("server") {
            if let Some(t) = s.get_mut("request_timeout") {
                *t = serde_json::Value::String(
                    humantime::format_duration(self.server.request_timeout).to_string(),
                );
            }
        }
        Ok(serde_yaml_like(&v))
    }
}

fn redact_url(url: &str) -> String {
    match url
        .find("://")
        .map(|i| i + 3)
        .and_then(|start| url[start..].find('@').map(|at| (start, start + at)))
    {
        Some((start, at)) => format!("{}***{}", &url[..start], &url[at..]),
        None => url.to_string(),
    }
}

/// Minimal YAML rendering for the `--check-config` output (we do not pull serde_yaml for this alone).
fn serde_yaml_like(v: &serde_json::Value) -> String {
    fn walk(v: &serde_json::Value, indent: usize, out: &mut String) {
        let pad = " ".repeat(indent);
        match v {
            serde_json::Value::Object(m) => {
                for (k, val) in m {
                    match val {
                        serde_json::Value::Object(_) => {
                            out.push_str(&format!("{pad}{k}:\n"));
                            walk(val, indent + 2, out);
                        }
                        serde_json::Value::Array(a) => {
                            out.push_str(&format!(
                                "{pad}{k}: {}\n",
                                serde_json::to_string(a).unwrap_or_default()
                            ));
                        }
                        other => out.push_str(&format!("{pad}{k}: {other}\n")),
                    }
                }
            }
            other => out.push_str(&format!("{pad}{other}\n")),
        }
    }
    let mut s = String::new();
    walk(v, 0, &mut s);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_credentials() {
        assert_eq!(
            redact_url("postgres://u:p@host/db"),
            "postgres://***@host/db"
        );
        assert_eq!(redact_url("redis://host:6379"), "redis://host:6379");
    }
}
