//! Audit trail of admin API calls.

use chrono::Utc;
use zanshin_core::Result;
use zanshin_store::audit::{AuditEntry, AuditFilter, AuditRepo, NewAuditEntry};

use crate::config::AuditConfig;

#[derive(Clone)]
pub struct AuditService {
    repo: AuditRepo,
    cfg: AuditConfig,
}

impl AuditService {
    pub fn new(repo: AuditRepo, cfg: AuditConfig) -> Self {
        Self { repo, cfg }
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
