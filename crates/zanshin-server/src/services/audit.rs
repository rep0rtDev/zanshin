//! Audit trail of admin API calls.

use chrono::Utc;
use zanshin_core::Result;
use zanshin_store::audit::{AuditEntry, AuditFilter, AuditRepo, NewAuditEntry};

use crate::{config::AuditConfig, services::notifications::NotificationService};
use std::sync::{Arc, RwLock};
use zanshin_core::notifications::Notice;

#[derive(Clone)]
pub struct AuditService {
    repo: AuditRepo,
    cfg: AuditConfig,
    notifier: Arc<RwLock<Option<NotificationService>>>,
}

impl AuditService {
    pub fn new(repo: AuditRepo, cfg: AuditConfig) -> Self {
        Self {
            repo,
            cfg,
            notifier: Arc::new(RwLock::new(None)),
        }
    }

    /// Route successful configuration changes to notification channels.
    pub fn attach_notifier(&self, n: NotificationService) {
        *self.notifier.write().unwrap() = Some(n);
    }

    fn notify_config_change(&self, e: &NewAuditEntry) {
        if !(200..300).contains(&e.status) || e.method == "GET" {
            return;
        }
        let Some(rest) = e.path.strip_prefix("/admin/v1/") else {
            return;
        };
        let mut parts = rest.split('/');
        let (Some(kind), key) = (parts.next(), parts.next()) else {
            return;
        };
        const SKIP: &[&str] = &[
            "audit",
            "login",
            "me",
            "system",
            "analytics",
            "events",
            "identities",
            "data",
            "export",
            "import",
            "currency-rates",
            "notification-channels",
            "status",
        ];
        if SKIP.contains(&kind) || kind.is_empty() {
            return;
        }
        let Some(n) = self.notifier.read().unwrap().clone() else {
            return;
        };
        let topic = if e.method == "DELETE" {
            "config.deleted"
        } else {
            "config.changed"
        };
        let body_key = e
            .body
            .as_ref()
            .and_then(|b| b.get("key"))
            .and_then(|k| k.as_str())
            .map(str::to_string);
        let what = match key
            .filter(|k| !k.is_empty())
            .map(str::to_string)
            .or(body_key)
        {
            Some(k) => format!("{kind} `{k}`"),
            None => kind.to_string(),
        };
        let verb = match e.method.as_str() {
            "POST" => "created / ran",
            "DELETE" => "deleted",
            _ => "updated",
        };
        let text = format!("{what} {verb} by {} ({})", e.actor_name, e.actor_kind);
        n.notify(
            Notice::new(
                topic,
                format!(
                    "Config {}",
                    if e.method == "DELETE" {
                        "deleted"
                    } else {
                        "changed"
                    }
                ),
                text,
            )
            .kind(kind)
            .field("method", e.method.clone())
            .field("path", e.path.clone()),
        );
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    pub fn store_bodies(&self) -> bool {
        self.cfg.store_bodies
    }

    /// Fire-and-forget: the request must never wait on (or fail because of) the audit insert.
    pub fn record(&self, entry: NewAuditEntry) {
        if !self.cfg.enabled {
            return;
        }
        metrics::counter!("zanshin_audit_entries_total", "method" => entry.method.clone())
            .increment(1);
        self.notify_config_change(&entry);
        let repo = self.repo.clone();
        tokio::spawn(async move {
            if let Err(e) = repo.insert(&entry).await {
                tracing::warn!(error = %e, "audit insert failed");
            }
        });
    }

    pub async fn get(&self, id: uuid::Uuid) -> Result<AuditEntry> {
        self.repo.get(id).await
    }

    pub async fn list(&self, f: &AuditFilter) -> Result<Vec<AuditEntry>> {
        self.repo.list(f).await
    }

    pub async fn prune(&self) -> Result<u64> {
        let before = Utc::now()
            - chrono::Duration::from_std(self.cfg.retention).unwrap_or(chrono::Duration::days(90));
        self.repo.prune(before).await
    }
}

/// Replace values of secret-looking keys before persisting a request body.
pub fn redact(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                let lk = k.to_ascii_lowercase();
                if lk.contains("secret")
                    || lk.contains("password")
                    || lk.contains("token")
                    || lk == "key_hash"
                    || lk.ends_with("_key")
                {
                    *val = serde_json::Value::String("[redacted]".into());
                } else {
                    redact(val);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}
