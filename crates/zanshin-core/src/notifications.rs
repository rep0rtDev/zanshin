//! Notification channels: Slack / Discord / generic webhook targets that receive short messages
//! about configuration changes, failed export runs, unhealthy integrations and alerts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{flags::validate_key, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Slack,
    Discord,
    Webhook,
}

impl ChannelKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChannelKind::Slack => "slack",
            ChannelKind::Discord => "discord",
            ChannelKind::Webhook => "webhook",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "slack" => Some(ChannelKind::Slack),
            "discord" => Some(ChannelKind::Discord),
            "webhook" => Some(ChannelKind::Webhook),
            _ => None,
        }
    }
}

/// Topics a channel can subscribe to (`*` = all).
pub const TOPICS: &[&str] = &[
    "config.changed",
    "config.deleted",
    "export.failed",
    "integration.unhealthy",
    "alert",
    "test",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannel {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    pub kind: ChannelKind,
    /// Incoming-webhook URL (Slack / Discord) or any HTTPS endpoint for `webhook`.
    pub url: String,
    /// Subscribed topics; empty or `["*"]` = everything.
    #[serde(default)]
    pub topics: Vec<String>,
    /// Only config changes of these entity kinds (`flags`, `experiments`, …); empty = all.
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default = "crate::flags::default_true")]
    pub enabled: bool,
    /// Extra headers for `webhook` channels.
    #[serde(default)]
    pub headers: Value,
    #[serde(default)]
    pub last_status: Option<i32>,
    #[serde(default)]
    pub last_error: String,
    #[serde(default)]
    pub last_sent_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl NotificationChannel {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        if !(self.url.starts_with("https://") || self.url.starts_with("http://")) {
            return Err(Error::invalid("url must be http(s)"));
        }
        match self.kind {
            ChannelKind::Slack if !self.url.contains("hooks.slack.com") => {
                return Err(Error::invalid(
                    "slack channels need an incoming webhook URL (hooks.slack.com/services/…)",
                ))
            }
            ChannelKind::Discord if !self.url.contains("discord") => {
                return Err(Error::invalid(
                    "discord channels need a webhook URL (discord.com/api/webhooks/…)",
                ))
            }
            _ => {}
        }
        for t in &self.topics {
            if t != "*" && !TOPICS.contains(&t.as_str()) {
                return Err(Error::invalid(format!("unknown topic {t:?}")));
            }
        }
        if !self.headers.is_null() && !self.headers.is_object() {
            return Err(Error::invalid("headers must be an object"));
        }
        Ok(())
    }

    pub fn wants(&self, topic: &str, kind: Option<&str>) -> bool {
        if !self.enabled {
            return false;
        }
        let topic_ok = self.topics.is_empty() || self.topics.iter().any(|t| t == "*" || t == topic);
        let kind_ok = match kind {
            Some(k) => self.kinds.is_empty() || self.kinds.iter().any(|x| x == k),
            None => true,
        };
        topic_ok && kind_ok
    }
}

/// A message to fan out to channels.
#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    pub topic: String,
    pub title: String,
    pub text: String,
    /// Entity kind for config topics (`flags`, …); used for per-channel filtering.
    pub kind: Option<String>,
    #[serde(default)]
    pub fields: Vec<(String, String)>,
    pub at: DateTime<Utc>,
}

impl Notice {
    pub fn new(topic: &str, title: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            topic: topic.to_string(),
            title: title.into(),
            text: text.into(),
            kind: None,
            fields: Vec::new(),
            at: Utc::now(),
        }
    }

    pub fn kind(mut self, k: impl Into<String>) -> Self {
        self.kind = Some(k.into());
        self
    }

    pub fn field(mut self, k: &str, v: impl Into<String>) -> Self {
        self.fields.push((k.to_string(), v.into()));
        self
    }
}
