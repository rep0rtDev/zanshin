//! Request budgets: one Redis token bucket per caller and scope.

use zanshin_store::cache::Cache;

use crate::config::LimitsConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Ip,
    Client,
    Session,
    Server,
    Admin,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::Ip => "ip",
            Scope::Client => "client",
            Scope::Session => "session",
            Scope::Server => "server",
            Scope::Admin => "admin",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Decision {
    pub allowed: bool,
    /// Sustained budget per minute.
    pub limit: u32,
    pub remaining: u32,
    /// Seconds until one more request would be admitted (0 when allowed).
    pub retry_after: f64,
}

#[derive(Clone)]
pub struct RateLimiter {
    cache: Cache,
    cfg: LimitsConfig,
}

impl RateLimiter {
    pub fn new(cache: Cache, cfg: LimitsConfig) -> Self {
        Self { cache, cfg }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    fn per_minute(&self, scope: Scope) -> u32 {
        match scope {
            Scope::Ip => self.cfg.ip_per_minute,
            Scope::Client => self.cfg.client_per_minute,
            Scope::Session => self.cfg.session_per_minute,
            Scope::Server => self.cfg.server_per_minute,
            Scope::Admin => self.cfg.admin_per_minute,
        }
    }

    /// Admit one request for `id` within `scope`. `override_per_minute` replaces the scope
    /// budget (per-key quotas). Redis failures fail open so a cache outage never takes the API down.
    pub async fn check(
        &self,
        scope: Scope,
        id: &str,
        override_per_minute: Option<u32>,
    ) -> Decision {
        let per_minute = override_per_minute.unwrap_or_else(|| self.per_minute(scope));
        if !self.cfg.enabled || per_minute == 0 {
            return Decision {
                allowed: true,
                limit: per_minute,
                remaining: per_minute,
                retry_after: 0.0,
            };
        }
        let capacity = ((per_minute as f64) * self.cfg.burst_factor.max(1.0)).ceil() as u32;
        let rate = per_minute as f64 / 60.0;
        let key = self.cache.key(&["rl", scope.as_str(), id]);
        match self.cache.take_tokens(&key, capacity, rate, 1).await {
            Ok((allowed, remaining, retry_after)) => {
                if !allowed {
                    metrics::counter!("zanshin_rate_limited_total", "scope" => scope.as_str())
                        .increment(1);
                }
                Decision {
                    allowed,
                    limit: per_minute,
                    remaining,
                    retry_after,
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "rate limiter unavailable, failing open");
                metrics::counter!("zanshin_rate_limiter_errors_total").increment(1);
                Decision {
                    allowed: true,
                    limit: per_minute,
                    remaining: capacity,
                    retry_after: 0.0,
                }
            }
        }
    }
}
