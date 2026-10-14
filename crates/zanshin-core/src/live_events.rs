//! Live events: time-boxed, optionally recurring content windows targeted at identities.

use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    flags::validate_key,
    properties::PropertyMap,
    rules::{Condition, Context},
    Error, Result,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveEvent {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub enabled: bool,
    #[serde(default = "Condition::always")]
    pub condition: Condition,
    #[serde(default)]
    pub audiences: Vec<String>,
    /// First (or only) occurrence start.
    pub starts_at: DateTime<Utc>,
    /// Hard end of the whole event; `None` = open-ended (one-shot events need `duration_secs`).
    #[serde(default)]
    pub ends_at: Option<DateTime<Utc>>,
    /// Length of each occurrence. Required when `recurrence` is set.
    #[serde(default)]
    pub duration_secs: Option<i64>,
    /// Cron expression (`sec min hour day-of-month month day-of-week [year]`) in UTC.
    #[serde(default)]
    pub recurrence: Option<String>,
    /// Free-form payload handed to clients while the event is active.
    #[serde(default)]
    pub payload: Value,
    /// `auto`: every targeted player is in while a window is open. `explicit`: the player must
    /// join (`POST /v1/live-events/{key}/join`) during a window.
    #[serde(default)]
    pub join_mode: JoinMode,
    /// Once joined, the player stays a participant across windows (and after targeting changes).
    #[serde(default)]
    pub sticky: bool,
    /// Per-player participation length in seconds from join time (capped by the window end
    /// unless `sticky`).
    #[serde(default)]
    pub participation_secs: Option<i64>,
    /// Flag values that override normal evaluation for participants while the event is active
    /// for them (`{ "flag_key": value }`).
    #[serde(default)]
    pub flag_overrides: serde_json::Map<String, Value>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// An occurrence window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Occurrence {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinMode {
    #[default]
    Auto,
    Explicit,
}

impl JoinMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            JoinMode::Auto => "auto",
            JoinMode::Explicit => "explicit",
        }
    }
}

impl std::str::FromStr for JoinMode {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "auto" => Ok(JoinMode::Auto),
            "explicit" => Ok(JoinMode::Explicit),
            other => Err(Error::invalid(format!("unknown join_mode {other:?}"))),
        }
    }
}

/// What a client sees for an event that targets it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveEvent {
    pub key: String,
    pub name: String,
    pub description: String,
    pub value: Value,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<Occurrence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<Occurrence>,
    /// The player must call join before the event becomes active for them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub join_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joined_at: Option<DateTime<Utc>>,
    /// When the player's participation ends (participation timer or window end).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participation_ends_at: Option<DateTime<Utc>>,
    pub version: i64,
}

impl LiveEvent {
    pub fn validate(&self) -> Result<()> {
        if self.participation_secs.is_some_and(|s| s <= 0) {
            return Err(Error::invalid("participation_secs must be positive"));
        }
        validate_key(&self.key)?;
        self.condition.validate()?;
        if let Some(d) = self.duration_secs {
            if d <= 0 || d > 366 * 86_400 {
                return Err(Error::invalid("duration_secs must be within 1..=31622400"));
            }
        }
        if let Some(e) = self.ends_at {
            if e <= self.starts_at {
                return Err(Error::invalid("ends_at must be after starts_at"));
            }
        }
        match &self.recurrence {
            Some(expr) => {
                cron::Schedule::from_str(expr)
                    .map_err(|e| Error::invalid(format!("invalid recurrence: {e}")))?;
                if self.duration_secs.is_none() {
                    return Err(Error::invalid("recurring events need duration_secs"));
                }
            }
            None => {
                if self.ends_at.is_none() && self.duration_secs.is_none() {
                    return Err(Error::invalid(
                        "one-shot events need ends_at or duration_secs",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn targets(&self, props: &PropertyMap, audiences: &dyn Fn(&str) -> bool) -> bool {
        self.enabled
            && self.condition.matches(&Context { props })
            && self.audiences.iter().all(|a| audiences(a))
    }

    fn duration(&self) -> Option<Duration> {
        self.duration_secs.map(Duration::seconds)
    }

    /// Occurrence containing `now`, if any.
    pub fn current(&self, now: DateTime<Utc>) -> Option<Occurrence> {
        if now < self.starts_at || self.ends_at.is_some_and(|e| now >= e) {
            return None;
        }
        match &self.recurrence {
            None => {
                let end = match (self.ends_at, self.duration()) {
                    (_, Some(d)) => {
                        (self.starts_at + d).min(self.ends_at.unwrap_or(DateTime::<Utc>::MAX_UTC))
                    }
                    (Some(e), None) => e,
                    (None, None) => return None,
                };
                (now < end).then_some(Occurrence {
                    start: self.starts_at,
                    end,
                })
            }
            Some(expr) => {
                let schedule = cron::Schedule::from_str(expr).ok()?;
                let d = self.duration()?;
                let floor = (now - d).max(self.starts_at - Duration::seconds(1));
                let start = schedule.after(&floor).next()?;
                let end = self.clamp_end(start + d);
                (start <= now && now < end && start >= self.starts_at)
                    .then_some(Occurrence { start, end })
            }
        }
    }

    /// Next occurrence starting after `now`, if any.
    /// The most recent occurrence that already ended before `now` (within the last 7 days).
    pub fn previous(&self, now: DateTime<Utc>) -> Option<Occurrence> {
        let mut best: Option<Occurrence> = None;
        let mut t = now - chrono::Duration::days(7);
        for _ in 0..512 {
            let Some(o) = self.view(t).current.or_else(|| self.next(t)) else {
                break;
            };
            if o.end > now {
                break;
            }
            best = Some(o.clone());
            t = o.end;
        }
        best
    }

    pub fn next(&self, now: DateTime<Utc>) -> Option<Occurrence> {
        match &self.recurrence {
            None => (now < self.starts_at)
                .then(|| self.current(self.starts_at))
                .flatten(),
            Some(expr) => {
                let schedule = cron::Schedule::from_str(expr).ok()?;
                let d = self.duration()?;
                let from = now.max(self.starts_at - Duration::seconds(1));
                let start = schedule.after(&from).next()?;
                if self.ends_at.is_some_and(|e| start >= e) {
                    return None;
                }
                Some(Occurrence {
                    start,
                    end: self.clamp_end(start + d),
                })
            }
        }
    }

    fn clamp_end(&self, end: DateTime<Utc>) -> DateTime<Utc> {
        match self.ends_at {
            Some(e) if e < end => e,
            _ => end,
        }
    }

    /// Schedule-only view (no participant state).
    pub fn view(&self, now: DateTime<Utc>) -> ActiveEvent {
        self.view_for(now, None)
    }

    /// View for a specific player given when (if ever) they joined. Explicit-join events are only
    /// active for joined players; `participation_secs` ends participation early; `sticky`
    /// participants stay active until their timer runs out even outside a window.
    pub fn view_for(&self, now: DateTime<Utc>, joined_at: Option<DateTime<Utc>>) -> ActiveEvent {
        let current = self.current(now);
        let timer_end =
            joined_at.and_then(|j| self.participation_secs.map(|s| j + Duration::seconds(s)));
        let joined = joined_at.is_some();
        let (active, participation_ends_at) = match (&current, timer_end) {
            (Some(c), Some(t)) => {
                let end = if self.sticky { t } else { t.min(c.end) };
                (
                    now < end && (joined || self.join_mode == JoinMode::Auto),
                    Some(end),
                )
            }
            (Some(c), None) => (joined || self.join_mode == JoinMode::Auto, Some(c.end)),
            (None, Some(t)) if self.sticky && joined => (now < t, Some(t)),
            (None, _) => (false, None),
        };
        ActiveEvent {
            key: self.key.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            value: self.payload.clone(),
            active,
            next: self.next(now),
            current,
            join_required: self.join_mode == JoinMode::Explicit && !joined,
            joined_at,
            participation_ends_at: if active { participation_ends_at } else { None },
            version: self.version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(
        starts: &str,
        ends: Option<&str>,
        duration: Option<i64>,
        recurrence: Option<&str>,
    ) -> LiveEvent {
        let p = t;
        LiveEvent {
            id: Uuid::nil(),
            key: "weekend_xp".into(),
            name: String::new(),
            description: String::new(),
            enabled: true,
            condition: Condition::always(),
            audiences: vec![],
            starts_at: p(starts),
            ends_at: ends.map(p),
            duration_secs: duration,
            recurrence: recurrence.map(String::from),
            payload: serde_json::json!({"xp": 2}),
            join_mode: JoinMode::Auto,
            sticky: false,
            participation_secs: None,
            flag_overrides: Default::default(),
            tags: vec![],
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn t(s: &str) -> DateTime<Utc> {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
            .unwrap()
            .and_utc()
    }

    #[test]
    fn one_shot_window() {
        let e = ev(
            "2026-01-10 00:00:00",
            Some("2026-01-12 00:00:00"),
            None,
            None,
        );
        e.validate().unwrap();
        assert!(e.current(t("2026-01-09 23:59:59")).is_none());
        assert_eq!(
            e.current(t("2026-01-10 12:00:00")).unwrap().end,
            t("2026-01-12 00:00:00")
        );
        assert!(e.current(t("2026-01-12 00:00:00")).is_none());
        assert_eq!(
            e.next(t("2026-01-01 00:00:00")).unwrap().start,
            t("2026-01-10 00:00:00")
        );
        assert!(e.next(t("2026-01-11 00:00:00")).is_none());
    }

    #[test]
    fn recurring_weekend() {
        // Every Saturday 18:00 UTC for 48 h, from Jan 2026.
        let e = ev(
            "2026-01-01 00:00:00",
            None,
            Some(48 * 3600),
            Some("0 0 18 * * Sat *"),
        );
        e.validate().unwrap();
        let sat = t("2026-01-03 18:00:00");
        assert!(e.current(t("2026-01-03 17:59:59")).is_none());
        let occ = e.current(t("2026-01-04 12:00:00")).unwrap();
        assert_eq!(occ.start, sat);
        assert_eq!(occ.end, t("2026-01-05 18:00:00"));
        assert!(e.current(t("2026-01-06 00:00:00")).is_none());
        assert_eq!(
            e.next(t("2026-01-06 00:00:00")).unwrap().start,
            t("2026-01-10 18:00:00")
        );
        let v = e.view(t("2026-01-04 12:00:00"));
        assert!(v.active && v.next.is_some());
    }

    #[test]
    fn validation() {
        assert!(ev("2026-01-01 00:00:00", None, None, None)
            .validate()
            .is_err());
        assert!(
            ev("2026-01-01 00:00:00", None, None, Some("0 0 18 * * Sat *"))
                .validate()
                .is_err()
        );
        assert!(
            ev("2026-01-01 00:00:00", None, Some(10), Some("not a cron"))
                .validate()
                .is_err()
        );
        assert!(ev(
            "2026-01-02 00:00:00",
            Some("2026-01-01 00:00:00"),
            None,
            None
        )
        .validate()
        .is_err());
    }
}
