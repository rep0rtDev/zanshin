//! Event export sinks (S3 / HTTP / BigQuery), their runs, and configuration history snapshots.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{flags::validate_key, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SinkKind {
    S3,
    Http,
    #[serde(rename = "bigquery")]
    BigQuery,
}

impl SinkKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SinkKind::S3 => "s3",
            SinkKind::Http => "http",
            SinkKind::BigQuery => "bigquery",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "s3" => Some(SinkKind::S3),
            "http" => Some(SinkKind::Http),
            "bigquery" => Some(SinkKind::BigQuery),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    #[default]
    Jsonl,
    Csv,
}

impl ExportFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExportFormat::Jsonl => "jsonl",
            ExportFormat::Csv => "csv",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "jsonl" => Some(ExportFormat::Jsonl),
            "csv" => Some(ExportFormat::Csv),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSink {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    pub kind: SinkKind,
    /// Provider settings; secret-looking keys are masked when read back through the API.
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub format: ExportFormat,
    #[serde(default = "crate::flags::default_true")]
    pub gzip: bool,
    #[serde(default = "crate::flags::default_true")]
    pub enabled: bool,
    /// Event names to export; empty = every event.
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default = "default_interval")]
    pub interval_secs: i64,
    #[serde(default = "default_batch")]
    pub batch_size: i32,
    #[serde(default)]
    pub cursor_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub cursor_id: Option<Uuid>,
    #[serde(default)]
    pub next_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_error: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_interval() -> i64 {
    300
}
fn default_batch() -> i32 {
    10_000
}

impl ExportSink {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        if !(30..=7 * 86_400).contains(&self.interval_secs) {
            return Err(Error::invalid("interval_secs must be within 30s..7d"));
        }
        if !(1..=100_000).contains(&self.batch_size) {
            return Err(Error::invalid("batch_size must be within 1..100000"));
        }
        if !self.config.is_object() {
            return Err(Error::invalid("config must be an object"));
        }
        let need = |k: &str| -> Result<()> {
            match self.config.get(k).and_then(|v| v.as_str()) {
                Some(s) if !s.trim().is_empty() => Ok(()),
                _ => Err(Error::invalid(format!("config.{k} is required"))),
            }
        };
        match self.kind {
            SinkKind::S3 => {
                need("bucket")?;
                need("region")?;
                need("access_key")?;
                need("secret_key")?;
            }
            SinkKind::Http => {
                need("url")?;
                let url = self.config["url"].as_str().unwrap_or_default();
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(Error::invalid("config.url must be http(s)"));
                }
            }
            SinkKind::BigQuery => {
                need("project")?;
                need("dataset")?;
                need("table")?;
                let sa = &self.config["service_account"];
                if sa.get("client_email").and_then(|v| v.as_str()).is_none()
                    || sa.get("private_key").and_then(|v| v.as_str()).is_none()
                {
                    return Err(Error::invalid(
                        "config.service_account must contain client_email and private_key",
                    ));
                }
                if self.format != ExportFormat::Jsonl {
                    return Err(Error::invalid(
                        "bigquery sinks stream rows and ignore format; use jsonl",
                    ));
                }
            }
        }
        for e in &self.events {
            if e.is_empty() || e.len() > 128 {
                return Err(Error::invalid("event names must be 1..128 chars"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRun {
    pub id: Uuid,
    pub sink_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: String,
    pub rows: i64,
    pub bytes: i64,
    pub object: String,
    pub error: String,
}

/// A configuration entity as it looked right after a change (or `deleted`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub id: Uuid,
    pub kind: String,
    pub key: String,
    pub version: i64,
    pub body: Option<Value>,
    pub deleted: bool,
    pub actor: String,
    pub at: DateTime<Utc>,
}

/// Field-level difference between two entity JSON objects (top-level keys only; nested values compared whole).
pub fn diff_fields(before: &Value, after: &Value) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    if let (Some(a), Some(b)) = (before.as_object(), after.as_object()) {
        for (k, v) in b {
            if VOLATILE.contains(&k.as_str()) {
                continue;
            }
            if a.get(k) != Some(v) {
                keys.push(k.clone());
            }
        }
        for k in a.keys() {
            if !VOLATILE.contains(&k.as_str()) && !b.contains_key(k) {
                keys.push(k.clone());
            }
        }
    }
    keys.sort();
    keys
}

/// Keys that differ between servers without being a configuration change.
pub const VOLATILE: &[&str] = &[
    "id",
    "version",
    "created_at",
    "updated_at",
    "cursor_at",
    "cursor_id",
    "next_run_at",
    "last_run_at",
    "last_error",
    "last_recomputed_at",
    "member_count",
    "stats",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn diff_ignores_volatile() {
        let a = json!({"key": "x", "enabled": true, "version": 1, "updated_at": "a"});
        let b = json!({"key": "x", "enabled": false, "version": 2, "updated_at": "b", "name": "n"});
        assert_eq!(diff_fields(&a, &b), vec!["enabled", "name"]);
    }

    #[test]
    fn sink_validation() {
        let now = Utc::now();
        let mut s = ExportSink {
            id: Uuid::now_v7(),
            key: "s3".into(),
            name: String::new(),
            kind: SinkKind::S3,
            config: json!({"bucket": "b", "region": "eu-central-1", "access_key": "a", "secret_key": "s"}),
            format: ExportFormat::Jsonl,
            gzip: true,
            enabled: true,
            events: vec![],
            interval_secs: 300,
            batch_size: 100,
            cursor_at: None,
            cursor_id: None,
            next_run_at: None,
            last_run_at: None,
            last_error: String::new(),
            tags: vec![],
            version: 1,
            created_at: now,
            updated_at: now,
        };
        assert!(s.validate().is_ok());
        s.config = json!({"bucket": "b"});
        assert!(s.validate().is_err());
        s.kind = SinkKind::Http;
        s.config = json!({"url": "ftp://x"});
        assert!(s.validate().is_err());
        s.config = json!({"url": "https://x"});
        s.interval_secs = 1;
        assert!(s.validate().is_err());
    }
}
