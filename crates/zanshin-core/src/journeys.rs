//! Journeys: multi-step automations entered by a trigger and executed per player with delays and
//! branches — "when a player buys for the first time, thank them, wait a day, offer a bundle".

use std::{collections::HashSet, str::FromStr};

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
    /// Hold until a specific instant (`at`) or until the next slot of a cron schedule (`cron`),
    /// resolved in UTC, in a fixed IANA zone (`timezone`) or in the player's own zone
    /// (`timezone = "player"`, read from `timezone_prop`, default `default.timezone`).
    TimeWindow {
        #[serde(default)]
        at: Option<DateTime<Utc>>,
        #[serde(default)]
        cron: Option<String>,
        #[serde(default)]
        timezone: Option<String>,
        #[serde(default)]
        timezone_prop: Option<String>,
    },
    /// Put the player into a live event on their own timeline: it starts for them now and runs
    /// `duration_secs` (default: the event's own duration, else until the event ends).
    LiveEvent {
        live_event: String,
        #[serde(default)]
        duration_secs: Option<i64>,
    },
    /// Undo what an earlier step granted: a flag override, an experiment assignment or a live
    /// event membership.
    Revert {
        step: String,
    },
    /// Enrol the player into another journey (journeys cannot form a cycle).
    EnterJourney {
        journey: String,
    },
    /// Random weighted branching (sticky per run) for A/B tests inside the flow.
    Split {
        branches: Vec<SplitBranch>,
    },
    Exit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitBranch {
    pub step: String,
    #[serde(default = "default_weight")]
    pub weight: f64,
}

fn default_weight() -> f64 {
    1.0
}

/// Conversion goal of a journey: a tracked event after entry (optionally within a deadline).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within_secs: Option<i64>,
}

/// Exit criteria: events that end an active run early (e.g. the purchase a nudge is about).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExitWhen {
    #[serde(default)]
    pub events: Vec<String>,
    /// Also exit when the goal is reached.
    #[serde(default)]
    pub on_goal: bool,
}

pub const PLAYER_TZ: &str = "player";
pub const MAX_LIVE_EVENT_SECS: i64 = 365 * 86_400;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<Goal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_when: Option<ExitWhen>,
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
        if let Some(g) = &self.goal {
            if g.event.is_empty() || g.event.len() > 128 {
                return Err(Error::invalid("goal.event must be 1..128 chars"));
            }
            if g.within_secs.is_some_and(|w| w <= 0) {
                return Err(Error::invalid("goal.within_secs must be > 0"));
            }
        }
        if let Some(x) = &self.exit_when {
            if x.events.iter().any(|e| e.is_empty() || e.len() > 128) {
                return Err(Error::invalid(
                    "exit_when.events: names must be 1..128 chars",
                ));
            }
            if x.on_goal && self.goal.is_none() {
                return Err(Error::invalid("exit_when.on_goal needs a goal"));
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
                Action::Split { branches } => {
                    if branches.is_empty() || branches.len() > 10 {
                        return Err(Error::invalid(format!("step {:?}: 1..10 branches", s.id)));
                    }
                    for b in branches {
                        if !ids.contains(b.step.as_str()) {
                            return Err(Error::invalid(format!(
                                "step {:?}: unknown branch step {:?}",
                                s.id, b.step
                            )));
                        }
                        if b.weight <= 0.0 || !b.weight.is_finite() {
                            return Err(Error::invalid(format!(
                                "step {:?}: weight must be > 0",
                                s.id
                            )));
                        }
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
                Action::TimeWindow {
                    at, cron, timezone, ..
                } => {
                    match (at, cron) {
                        (Some(_), None) => {}
                        (None, Some(expr)) => {
                            if cron::Schedule::from_str(expr).is_err() {
                                return Err(Error::invalid(format!(
                                    "step {:?}: invalid cron expression {expr:?} (sec min hour dom month dow [year])",
                                    s.id
                                )));
                            }
                        }
                        _ => {
                            return Err(Error::invalid(format!(
                                "step {:?}: exactly one of at / cron is required",
                                s.id
                            )))
                        }
                    }
                    if let Some(tz) = timezone {
                        if tz != PLAYER_TZ && tz.parse::<chrono_tz::Tz>().is_err() {
                            return Err(Error::invalid(format!(
                                "step {:?}: unknown timezone {tz:?} (IANA name or \"player\")",
                                s.id
                            )));
                        }
                    }
                }
                Action::LiveEvent {
                    live_event,
                    duration_secs,
                } => {
                    validate_key(live_event)?;
                    if duration_secs.is_some_and(|d| d <= 0 || d > MAX_LIVE_EVENT_SECS) {
                        return Err(Error::invalid(format!(
                            "step {:?}: duration_secs must be 1s..365d",
                            s.id
                        )));
                    }
                }
                Action::Revert { step } => {
                    let Some(target) = self.steps.iter().find(|t| &t.id == step) else {
                        return Err(Error::invalid(format!(
                            "step {:?}: unknown target {step:?}",
                            s.id
                        )));
                    };
                    if target.id == s.id || !target.action.is_revertible() {
                        return Err(Error::invalid(format!(
                            "step {:?}: {step:?} is not a flag override / experiment assignment / live event step",
                            s.id
                        )));
                    }
                }
                Action::EnterJourney { journey } => {
                    validate_key(journey)?;
                    if journey == &self.key {
                        return Err(Error::invalid(format!(
                            "step {:?}: a journey cannot enter itself",
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

    pub fn status(&self, now: DateTime<Utc>) -> Status {
        if !self.enabled {
            Status::Disabled
        } else if self.starts_at.is_some_and(|s| now < s) {
            Status::Scheduled
        } else if self.ends_at.is_some_and(|e| now >= e) {
            Status::Terminated
        } else {
            Status::Running
        }
    }

    /// Keys of the journeys this one enrols players into.
    pub fn child_journeys(&self) -> impl Iterator<Item = &str> {
        self.steps.iter().filter_map(|s| match &s.action {
            Action::EnterJourney { journey } => Some(journey.as_str()),
            _ => None,
        })
    }
}

impl Action {
    pub fn is_revertible(&self) -> bool {
        matches!(
            self,
            Action::FlagOverride { .. }
                | Action::ExperimentAssign { .. }
                | Action::LiveEvent { .. }
        )
    }
}

/// Lifecycle derived from `enabled` and the activity window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Disabled,
    Scheduled,
    Running,
    Terminated,
}

/// Find a cycle among journeys linked by `EnterJourney` steps, starting from `start`.
/// `children` yields the child keys of a journey key. Returns the path of the cycle if any.
pub fn find_cycle<'a>(
    start: &'a str,
    children: &dyn Fn(&str) -> Vec<String>,
) -> Option<Vec<String>> {
    fn walk(
        key: &str,
        children: &dyn Fn(&str) -> Vec<String>,
        path: &mut Vec<String>,
        seen: &mut HashSet<String>,
    ) -> bool {
        if let Some(i) = path.iter().position(|p| p == key) {
            path.drain(..i);
            path.push(key.to_string());
            return true;
        }
        if !seen.insert(key.to_string()) {
            return false;
        }
        path.push(key.to_string());
        for c in children(key) {
            if walk(&c, children, path, seen) {
                return true;
            }
        }
        path.pop();
        false
    }
    let mut path = Vec::new();
    walk(start, children, &mut path, &mut HashSet::new()).then_some(path)
}

/// Next slot of a cron schedule strictly after `now`, resolved in `tz`.
pub fn next_cron(now: DateTime<Utc>, expr: &str, tz: chrono_tz::Tz) -> Option<DateTime<Utc>> {
    let schedule = cron::Schedule::from_str(expr).ok()?;
    schedule
        .after(&now.with_timezone(&tz))
        .next()
        .map(|d| d.with_timezone(&Utc))
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
    fn cron_and_cycles() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        // 09:00 every Saturday, Berlin time → Sat 2026-01-03 08:00Z.
        let at = next_cron(now, "0 0 9 * * Sat", chrono_tz::Europe::Berlin).unwrap();
        assert_eq!(at, Utc.with_ymd_and_hms(2026, 1, 3, 8, 0, 0).unwrap());
        assert!(next_cron(now, "nope", chrono_tz::UTC).is_none());
        let graph = |k: &str| -> Vec<String> {
            match k {
                "a" => vec!["b".into()],
                "b" => vec!["c".into(), "d".into()],
                "c" => vec!["a".into()],
                _ => vec![],
            }
        };
        assert_eq!(
            find_cycle("a", &graph),
            Some(vec!["a".into(), "b".into(), "c".into(), "a".into()])
        );
        assert_eq!(find_cycle("d", &graph), None);
    }

    #[test]
    fn local_time_rolls_over() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let at = next_local_time(now, 9, 0, Some("Europe/Berlin"));
        assert_eq!(at, Utc.with_ymd_and_hms(2026, 1, 2, 8, 0, 0).unwrap());
        let at = next_local_time(now, 15, 30, None);
        assert_eq!(at, Utc.with_ymd_and_hms(2026, 1, 1, 15, 30, 0).unwrap());
    }
}
