//! Journeys: multi-step automations entered by a trigger and executed per player with delays and
//! branches — "when a player buys for the first time, thank them, wait a day, offer a bundle".

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    flags::validate_key,
    properties::PropertyMap,
    rules::{Condition, Context},
    Error, Result,
};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Trigger {
    /// Only via the admin API / server call.
    #[default]
    Manual,
    /// A tracked event with this name (custom or synthetic `$…`).
    Event {
        name: String,
    },
    IdentityCreated,
    /// The player is (newly) a member of this audience, checked whenever they produce an event.
    AudienceEnter {
        audience: String,
    },
    /// A live event occurrence starts; every targeted player is entered once per occurrence.
    LiveEventStart {
        live_event: String,
    },
}

impl Trigger {
    /// Webhook-bus event type that fires this trigger, if any.
    pub fn event_type(&self) -> Option<String> {
        match self {
            Trigger::Event { name } => Some(crate::webhooks::type_for_event(name)),
            Trigger::IdentityCreated => Some("identity.created".into()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Inbox message (inline copy, or rendered from `template_key`).
    Message {
        #[serde(default)]
        title: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        image_url: String,
        #[serde(default)]
        payload: Value,
        #[serde(default)]
        template_key: Option<String>,
        #[serde(default)]
        push: bool,
    },
    /// Record an analytics event for the player.
    Track {
        name: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        metadata: Value,
    },
    FlagOverride {
        flag: String,
        value: Value,
    },
    ExperimentAssign {
        experiment: String,
        variant: String,
    },
    /// Send through a delivery integration (webhook, OneSignal, …).
    Integration {
        integration: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        payload: Value,
    },
    Wait {
        secs: i64,
    },
    /// Wait until the next `hour:minute` in the player's time zone (`timezone_prop`, default `default.timezone`).
    WaitUntil {
        hour: u32,
        #[serde(default)]
        minute: u32,
        #[serde(default)]
        timezone_prop: Option<String>,
    },
    /// Jump to `then` when the condition matches, otherwise to `else` (or the next step).
    Branch {
        condition: Condition,
        then: String,
        #[serde(default, rename = "else")]
        otherwise: Option<String>,
    },
    Goto {
        step: String,
    },
    Exit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    #[serde(flatten)]
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Journey {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "crate::flags::default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub trigger: Trigger,
    #[serde(default = "Condition::always")]
    pub condition: Condition,
    #[serde(default)]
    pub audiences: Vec<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Minimum seconds between two entries of the same player (0 = none).
    #[serde(default)]
    pub cooldown_secs: i64,
    /// Lifetime cap of entries per player.
    #[serde(default)]
    pub max_entries: Option<i32>,
    #[serde(default)]
    pub starts_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ends_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub const MAX_STEPS: usize = 100;

impl Journey {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        if self.steps.is_empty() {
            return Err(Error::invalid("a journey needs at least one step"));
        }
        if self.steps.len() > MAX_STEPS {
            return Err(Error::invalid(format!("at most {MAX_STEPS} steps")));
        }
        let mut ids = HashSet::new();
        for s in &self.steps {
            if s.id.is_empty() || s.id.len() > 64 {
                return Err(Error::invalid("step id must be 1..64 chars"));
            }
            if !ids.insert(s.id.as_str()) {
                return Err(Error::invalid(format!("duplicate step id {:?}", s.id)));
            }
        }
        for s in &self.steps {
            match &s.action {
                Action::Message {
                    title,
                    template_key,
                    ..
                } => {
                    if title.trim().is_empty() && template_key.is_none() {
                        return Err(Error::invalid(format!(
                            "step {:?}: title or template_key required",
                            s.id
                        )));
                    }
                    if let Some(t) = template_key {
                        validate_key(t)?;
                    }
                }
                Action::Track { name, .. } => {
                    if name.is_empty() || name.len() > 128 || name.starts_with('$') {
                        return Err(Error::invalid(format!("step {:?}: bad event name", s.id)));
                    }
                }
                Action::FlagOverride { flag, .. } => validate_key(flag)?,
                Action::ExperimentAssign {
                    experiment,
                    variant,
                } => {
                    validate_key(experiment)?;
                    if variant.is_empty() {
                        return Err(Error::invalid(format!("step {:?}: variant required", s.id)));
                    }
                }
                Action::Integration { integration, .. } => validate_key(integration)?,
                Action::Wait { secs } => {
                    if *secs <= 0 || *secs > 90 * 86_400 {
                        return Err(Error::invalid(format!("step {:?}: wait 1s..90d", s.id)));
                    }
                }
                Action::WaitUntil { hour, minute, .. } => {
                    if *hour > 23 || *minute > 59 {
                        return Err(Error::invalid(format!("step {:?}: bad time", s.id)));
                    }
                }
                Action::Branch {
                    then, otherwise, ..
                } => {
                    if !ids.contains(then.as_str()) {
                        return Err(Error::invalid(format!(
                            "step {:?}: unknown target {then:?}",
                            s.id
                        )));
                    }
                    if let Some(o) = otherwise {
                        if !ids.contains(o.as_str()) {
                            return Err(Error::invalid(format!(
                                "step {:?}: unknown target {o:?}",
                                s.id
                            )));
                        }
                    }
                }
                Action::Goto { step } => {
                    if !ids.contains(step.as_str()) {
                        return Err(Error::invalid(format!(
                            "step {:?}: unknown target {step:?}",
                            s.id
                        )));
                    }
                }
                Action::Exit => {}
            }
        }
        for a in &self.audiences {
            validate_key(a)?;
        }
        match &self.trigger {
            Trigger::Event { name } if name.is_empty() || name.len() > 128 => {
                return Err(Error::invalid("trigger event name must be 1..128 chars"))
            }
            Trigger::AudienceEnter { audience } => validate_key(audience)?,
            Trigger::LiveEventStart { live_event } => validate_key(live_event)?,
            _ => {}
        }
        if self.cooldown_secs < 0 {
            return Err(Error::invalid("cooldown_secs must be >= 0"));
        }
        if self.max_entries.is_some_and(|m| m <= 0) {
            return Err(Error::invalid("max_entries must be positive"));
        }
        if let (Some(s), Some(e)) = (self.starts_at, self.ends_at) {
            if e <= s {
                return Err(Error::invalid("ends_at must be after starts_at"));
            }
        }
        Ok(())
    }

    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.enabled
            && self.starts_at.is_none_or(|s| now >= s)
            && self.ends_at.is_none_or(|e| now < e)
    }

    pub fn targets(&self, props: &PropertyMap, audiences: &dyn Fn(&str) -> bool) -> bool {
        self.condition.matches(&Context { props }) && self.audiences.iter().all(|a| audiences(a))
    }

    pub fn step_index(&self, id: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.id == id)
    }
}

/// Next occurrence of `hour:minute` in `tz` strictly after `now`.
pub fn next_local_time(
    now: DateTime<Utc>,
    hour: u32,
    minute: u32,
    tz: Option<&str>,
) -> DateTime<Utc> {
    use chrono::{Duration, TimeZone, Timelike};
    let tz: chrono_tz::Tz = tz.and_then(|t| t.parse().ok()).unwrap_or(chrono_tz::UTC);
    let local = now.with_timezone(&tz);
    let mut candidate = local
        .date_naive()
        .and_hms_opt(hour, minute, 0)
        .expect("validated time");
    let mut at = tz
        .from_local_datetime(&candidate)
        .earliest()
        .unwrap_or_else(|| tz.from_utc_datetime(&candidate));
    if at <= local || (at.hour() == local.hour() && at.minute() == local.minute() && at <= local) {
        candidate += Duration::days(1);
        at = tz
            .from_local_datetime(&candidate)
            .earliest()
            .unwrap_or_else(|| tz.from_utc_datetime(&candidate));
    }
    at.with_timezone(&Utc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn local_time_rolls_over() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let at = next_local_time(now, 9, 0, Some("Europe/Berlin"));
        assert_eq!(at, Utc.with_ymd_and_hms(2026, 1, 2, 8, 0, 0).unwrap());
        let at = next_local_time(now, 15, 30, None);
        assert_eq!(at, Utc.with_ymd_and_hms(2026, 1, 1, 15, 30, 0).unwrap());
    }
}
