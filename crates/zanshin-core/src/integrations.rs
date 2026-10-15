//! Outbound delivery integrations (webhook, OneSignal, FCM, APNs, Facebook) referenced by campaign channels.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{flags::validate_key, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationKind {
    Webhook,
    Onesignal,
    Fcm,
    Apns,
    Facebook,
    Smtp,
    Ses,
    Sendgrid,
    Resend,
}

impl IntegrationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            IntegrationKind::Webhook => "webhook",
            IntegrationKind::Onesignal => "onesignal",
            IntegrationKind::Fcm => "fcm",
            IntegrationKind::Apns => "apns",
            IntegrationKind::Facebook => "facebook",
            IntegrationKind::Smtp => "smtp",
            IntegrationKind::Ses => "ses",
            IntegrationKind::Sendgrid => "sendgrid",
            IntegrationKind::Resend => "resend",
        }
    }
}

impl std::str::FromStr for IntegrationKind {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "webhook" => Ok(IntegrationKind::Webhook),
            "onesignal" => Ok(IntegrationKind::Onesignal),
            "fcm" => Ok(IntegrationKind::Fcm),
            "apns" => Ok(IntegrationKind::Apns),
            "facebook" => Ok(IntegrationKind::Facebook),
            "smtp" => Ok(IntegrationKind::Smtp),
            "ses" => Ok(IntegrationKind::Ses),
            "sendgrid" => Ok(IntegrationKind::Sendgrid),
            "resend" => Ok(IntegrationKind::Resend),
            other => Err(Error::invalid(format!(
                "unknown integration kind {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Integration {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    pub kind: IntegrationKind,
    /// Provider settings; secret-looking keys are masked when read back through the API.
    #[serde(default)]
    pub config: Value,
    #[serde(default = "crate::flags::default_true")]
    pub enabled: bool,
    /// Delivery health, maintained by the server on every attempt.
    #[serde(default)]
    pub health: IntegrationHealth,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Rolling delivery health of an integration: last attempt outcome plus lifetime counters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationHealth {
    /// `sent`, `failed` or `skipped` for the most recent attempt; `None` if never used.
    pub last_status: Option<String>,
    /// Detail of the most recent failure (kept until the next failure).
    pub last_error: Option<String>,
    pub last_at: Option<DateTime<Utc>>,
    pub sent_count: i64,
    pub failed_count: i64,
}

pub const SECRET_KEYS: &[&str] = &[
    "secret",
    "api_key",
    "access_token",
    "private_key",
    "service_account",
    "token",
    "password",
    "secret_key",
];

impl Integration {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        let cfg = self
            .config
            .as_object()
            .ok_or_else(|| Error::invalid("config must be an object"))?;
        let need = |k: &str| -> Result<()> {
            match cfg.get(k) {
                Some(Value::String(s)) if !s.is_empty() => Ok(()),
                _ => Err(Error::invalid(format!(
                    "config.{k} is required for {}",
                    self.kind.as_str()
                ))),
            }
        };
        match self.kind {
            IntegrationKind::Webhook => {
                need("url")?;
                let url = self.config["url"].as_str().unwrap_or_default();
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(Error::invalid("config.url must be http(s)"));
                }
            }
            IntegrationKind::Onesignal => {
                need("app_id")?;
                need("api_key")?;
            }
            // Delivered through the server's configured push providers (push.* config).
            IntegrationKind::Fcm | IntegrationKind::Apns => {}
            IntegrationKind::Facebook => need("access_token")?,
            IntegrationKind::Smtp => {
                need("host")?;
                need("from")?;
            }
            IntegrationKind::Ses => {
                need("region")?;
                need("access_key")?;
                need("secret_key")?;
                need("from")?;
            }
            IntegrationKind::Sendgrid | IntegrationKind::Resend => {
                need("api_key")?;
                need("from")?;
            }
        }
        Ok(())
    }

    /// Copy with secret values replaced by `"••••"` for API responses.
    pub fn masked(&self) -> Integration {
        let mut out = self.clone();
        if let Some(obj) = out.config.as_object_mut() {
            for (k, v) in obj.iter_mut() {
                if SECRET_KEYS.iter().any(|s| k.contains(s)) && v.is_string() {
                    *v = Value::String("••••".into());
                }
            }
        }
        out
    }

    /// Merge a patch config over the stored one, keeping stored secrets where the patch sends the mask.
    pub fn merge_config(stored: &Value, patch: Value) -> Value {
        let mut out = patch;
        if let (Some(o), Some(s)) = (out.as_object_mut(), stored.as_object()) {
            for (k, v) in o.iter_mut() {
                if v.as_str() == Some("••••") {
                    if let Some(sv) = s.get(k) {
                        *v = sv.clone();
                    }
                }
            }
        }
        out
    }
}
