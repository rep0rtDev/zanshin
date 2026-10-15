//! Outbound webhooks: subscriptions to system events, delivered signed and batched with retries.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{flags::validate_key, Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Webhook {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    pub url: String,
    /// HMAC-SHA256 key for `X-Zanshin-Signature`; empty = unsigned.
    #[serde(default)]
    pub secret: String,
    /// Event type patterns: exact (`flag.updated`), prefix (`flag.*`) or everything (`*`).
    #[serde(default = "default_events")]
    pub events: Vec<String>,
    #[serde(default = "crate::flags::default_true")]
    pub enabled: bool,
    /// Flush after this many matching events (1 = one request per event).
    #[serde(default = "default_batch_size")]
    pub batch_size: i32,
    /// …or after this many seconds since the first buffered event (0 = immediately).
    #[serde(default)]
    pub batch_window_secs: i32,
    #[serde(default)]
    pub headers: Value,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: i32,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn default_events() -> Vec<String> {
    vec!["*".into()]
}
pub fn default_batch_size() -> i32 {
    1
}
pub fn default_max_attempts() -> i32 {
    5
}

impl Webhook {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        if !(self.url.starts_with("http://") || self.url.starts_with("https://")) {
            return Err(Error::invalid("url must be http(s)"));
        }
        if self.events.is_empty() {
            return Err(Error::invalid("events must not be empty"));
        }
        for p in &self.events {
            if p.is_empty()
                || p.len() > 128
                || !p
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '*' | '$'))
            {
                return Err(Error::invalid(format!("bad event pattern {p:?}")));
            }
        }
        if !(1..=500).contains(&self.batch_size) {
            return Err(Error::invalid("batch_size must be 1..=500"));
        }
        if !(0..=3600).contains(&self.batch_window_secs) {
            return Err(Error::invalid("batch_window_secs must be 0..=3600"));
        }
        if !(1..=20).contains(&self.max_attempts) {
            return Err(Error::invalid("max_attempts must be 1..=20"));
        }
        if !self.headers.is_null() && !self.headers.is_object() {
            return Err(Error::invalid("headers must be an object"));
        }
        Ok(())
    }

    pub fn matches(&self, event_type: &str) -> bool {
        self.events.iter().any(|p| pattern_matches(p, event_type))
    }

    pub fn masked(&self) -> Webhook {
        let mut w = self.clone();
        if !w.secret.is_empty() {
            w.secret = "••••".into();
        }
        w
    }
}

pub fn pattern_matches(pattern: &str, event_type: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match pattern.strip_suffix(".*") {
        Some(prefix) => event_type
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.')),
        None => pattern == event_type,
    }
}

/// One thing that happened, as sent to subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEvent {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub event_type: String,
    pub at: DateTime<Utc>,
    pub data: Value,
}

impl WebhookEvent {
    pub fn new(event_type: impl Into<String>, data: Value) -> Self {
        Self {
            id: Uuid::now_v7(),
            event_type: event_type.into(),
            at: Utc::now(),
            data,
        }
    }
}

/// Event types the server emits (for the console picker).
pub const EVENT_TYPES: &[&str] = &[
    "identity.created",
    "identity.authenticated",
    "identity.identified",
    "session.started",
    "session.ended",
    "experiment.joined",
    "experiment.exposed",
    "live_event.joined",
    "message.delivered",
    "message.read",
    "event.<name>",
    "flag.updated",
    "flag.deleted",
    "flag.rule.created",
    "flag.rule.updated",
    "flag.rule.deleted",
    "journey.step_reached",
    "message.direct_sent",
    "flag.killed",
    "flag.restored",
    "flag.schedule_applied",
    "change_request.created",
    "change_request.approved",
    "change_request.rejected",
    "change_request.applied",
    "experiment.guardrail_tripped",
    "audience.updated",
    "audience.deleted",
    "experiment.updated",
    "experiment.deleted",
    "live_event.updated",
    "live_event.deleted",
    "message.updated",
    "message.deleted",
    "webhook.test",
];

/// Map a synthetic `$event` name to its webhook type; custom events become `event.<name>`.
pub fn type_for_event(name: &str) -> String {
    match name {
        "$identity_create" => "identity.created".into(),
        "$authenticate" => "identity.authenticated".into(),
        "$identify" => "identity.identified".into(),
        "$session_start" => "session.started".into(),
        "$session_end" => "session.ended".into(),
        "$experiment_join" => "experiment.joined".into(),
        "$exposure" => "experiment.exposed".into(),
        "$live_event_join" => "live_event.joined".into(),
        "$message_read" => "message.read".into(),
        other => format!("event.{}", other.trim_start_matches('$')),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(pattern_matches("*", "flag.updated"));
        assert!(pattern_matches("flag.*", "flag.updated"));
        assert!(!pattern_matches("flag.*", "flags.updated"));
        assert!(!pattern_matches("flag.*", "flag"));
        assert!(pattern_matches("event.purchase", "event.purchase"));
        assert!(!pattern_matches("event.purchase", "event.purchases"));
        assert_eq!(type_for_event("$identity_create"), "identity.created");
        assert_eq!(type_for_event("purchase"), "event.purchase");
    }
}
