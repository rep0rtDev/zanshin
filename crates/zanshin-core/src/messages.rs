//! In-app messages: campaigns targeted at identities, delivered into a per-identity inbox.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    flags::validate_key,
    hash,
    properties::PropertyMap,
    rules::{Condition, Context},
    Error, Result,
};

pub const MAX_TEXT: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageCampaign {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    pub enabled: bool,
    /// Title / body may reference properties: `Hello {{custom.nickname}}`.
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub image_url: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default = "Condition::always")]
    pub condition: Condition,
    #[serde(default)]
    pub audiences: Vec<String>,
    #[serde(default = "default_percentage")]
    pub percentage: f64,
    #[serde(default)]
    pub starts_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ends_at: Option<DateTime<Utc>>,
    /// Seconds a delivered copy stays in the inbox; `None` = until consumed.
    #[serde(default)]
    pub ttl_secs: Option<i64>,
    #[serde(default)]
    pub priority: i32,
    /// Also send a push notification to the identity's registered devices on delivery.
    #[serde(default)]
    pub push: bool,
    /// Store the message in the player's inbox (default). `false` = push / integrations only.
    #[serde(default = "default_true")]
    pub inbox: bool,
    /// Render title/body/payload from this template (per player locale) instead of the inline fields.
    #[serde(default)]
    pub template_key: Option<String>,
    #[serde(default)]
    pub schedule: Schedule,
    /// Delivery channels: `inbox` plus integration keys.
    #[serde(default = "default_channels")]
    pub channels: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_percentage() -> f64 {
    100.0
}

pub fn default_channels() -> Vec<String> {
    vec!["inbox".into()]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    #[default]
    Start,
    End,
}

/// When a campaign is delivered. `once` = on first eligible inbox fetch while live; `cron` = once per
/// cron occurrence (UTC); `live_event` = once per occurrence of a live event, offset from its start/end.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    #[default]
    Once,
    Cron {
        cron: String,
        /// How long after the fire time the occurrence is still delivered (default 24h).
        #[serde(default)]
        window_secs: Option<i64>,
    },
    LiveEvent {
        live_event: String,
        #[serde(default)]
        anchor: Anchor,
        #[serde(default)]
        offset_secs: i64,
        #[serde(default)]
        window_secs: Option<i64>,
    },
}

pub const DEFAULT_WINDOW_SECS: i64 = 86_400;

impl Schedule {
    pub fn validate(&self) -> Result<()> {
        match self {
            Schedule::Once => Ok(()),
            Schedule::Cron { cron, window_secs } => {
                cron.parse::<cron::Schedule>()
                    .map_err(|e| Error::invalid(format!("bad cron {cron:?}: {e}")))?;
                if window_secs.is_some_and(|w| w <= 0) {
                    return Err(Error::invalid("window_secs must be positive"));
                }
                Ok(())
            }
            Schedule::LiveEvent {
                live_event,
                window_secs,
                ..
            } => {
                validate_key(live_event)?;
                if window_secs.is_some_and(|w| w <= 0) {
                    return Err(Error::invalid("window_secs must be positive"));
                }
                Ok(())
            }
        }
    }

    pub fn live_event_key(&self) -> Option<&str> {
        match self {
            Schedule::LiveEvent { live_event, .. } => Some(live_event),
            _ => None,
        }
    }

    /// For cron schedules: the most recent fire time at or before `now` that is still inside the window.
    pub fn cron_occurrence(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let Schedule::Cron { cron, window_secs } = self else {
            return None;
        };
        let window = window_secs.unwrap_or(DEFAULT_WINDOW_SECS);
        let sched = cron.parse::<cron::Schedule>().ok()?;
        sched
            .after(&(now - chrono::Duration::seconds(window)))
            .take_while(|t| *t <= now)
            .last()
    }

    /// For live-event schedules: the delivery instant derived from one event occurrence, if `now` is inside its window.
    pub fn event_occurrence(
        &self,
        starts_at: DateTime<Utc>,
        ends_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        let Schedule::LiveEvent {
            anchor,
            offset_secs,
            window_secs,
            ..
        } = self
        else {
            return None;
        };
        let base = match anchor {
            Anchor::Start => starts_at,
            Anchor::End => ends_at,
        };
        let at = base + chrono::Duration::seconds(*offset_secs);
        let window = window_secs.unwrap_or(DEFAULT_WINDOW_SECS);
        (now >= at && now < at + chrono::Duration::seconds(window)).then_some(at)
    }
}

impl MessageCampaign {
    pub fn validate(&self) -> Result<()> {
        self.schedule.validate()?;
        if self.channels.is_empty() {
            return Err(Error::invalid("channels must not be empty"));
        }
        for c in &self.channels {
            if c != "inbox" {
                validate_key(c)?;
            }
        }
        if let Some(t) = &self.template_key {
            validate_key(t)?;
        }
        validate_key(&self.key)?;
        self.condition.validate()?;
        if self.title.trim().is_empty() || self.title.len() > 512 {
            return Err(Error::invalid("title must be 1..512 chars"));
        }
        if self.body.len() > MAX_TEXT || self.image_url.len() > 2048 {
            return Err(Error::invalid("body or image_url too long"));
        }
        if !(0.0..=100.0).contains(&self.percentage) || self.percentage.is_nan() {
            return Err(Error::invalid("percentage must be within 0..=100"));
        }
        if let (Some(s), Some(e)) = (self.starts_at, self.ends_at) {
            if e <= s {
                return Err(Error::invalid("ends_at must be after starts_at"));
            }
        }
        if self.ttl_secs.is_some_and(|t| t <= 0) {
            return Err(Error::invalid("ttl_secs must be positive"));
        }
        Ok(())
    }

    /// Delivery occurrence for a plain / cron campaign at `now` (`Some(None)` = once, no occurrence key).
    pub fn occurrence(&self, now: DateTime<Utc>) -> Option<Option<DateTime<Utc>>> {
        match &self.schedule {
            Schedule::Once => Some(None),
            Schedule::Cron { .. } => self.schedule.cron_occurrence(now).map(Some),
            Schedule::LiveEvent { .. } => None,
        }
    }

    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.enabled
            && self.starts_at.is_none_or(|s| now >= s)
            && self.ends_at.is_none_or(|e| now < e)
    }

    pub fn targets(
        &self,
        subject: &str,
        props: &PropertyMap,
        audiences: &dyn Fn(&str) -> bool,
    ) -> bool {
        self.condition.matches(&Context { props })
            && self.audiences.iter().all(|a| audiences(a))
            && hash::in_rollout(
                &format!("message:{}", self.key),
                "",
                subject,
                self.percentage,
            )
    }

    /// Render `{{prop.path}}` placeholders from flattened properties; unknown paths become empty.
    pub fn render(template: &str, props: &PropertyMap) -> String {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            match after.find("}}") {
                Some(end) => {
                    let key = after[..end].trim();
                    match props.get(key) {
                        Some(Value::String(s)) => out.push_str(s),
                        Some(Value::Null) | None => {}
                        Some(v) => out.push_str(&v.to_string()),
                    }
                    rest = &after[end + 2..];
                }
                None => {
                    out.push_str(&rest[start..]);
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }
}

/// A delivered copy in an identity's inbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboxMessage {
    pub id: Uuid,
    /// Campaign key, or `null` for direct messages.
    pub key: Option<String>,
    pub title: String,
    pub body: String,
    pub image_url: String,
    pub payload: Value,
    pub priority: i32,
    pub delivered_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn render_placeholders() {
        let mut p = PropertyMap::new();
        p.insert("custom.nickname".into(), json!("Neo"));
        p.insert("custom.level".into(), json!(42));
        assert_eq!(
            MessageCampaign::render(
                "Hi {{custom.nickname}}, lvl {{ custom.level }}! {{missing}}.",
                &p
            ),
            "Hi Neo, lvl 42! ."
        );
        assert_eq!(MessageCampaign::render("broken {{x", &p), "broken {{x");
    }
}
