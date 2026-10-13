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
fn default_log_level() -> String {
    "info,sqlx=warn,hyper=warn,tower_http=info".into()
}
fn default_clickhouse() -> ClickHouseConfig {
    serde_json::from_str("{}").expect("clickhouse defaults")
}

impl Config {
    /// Layering: built-in defaults < YAML file < `ZANSHIN__*` environment variables.
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
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
        let cfg: Config = fig.extract().context("invalid configuration")?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.auth.jwt_secret.len() >= 32,
            "auth.jwt_secret must be at least 32 characters"
        );
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
