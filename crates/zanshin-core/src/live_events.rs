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
    /// Optional variants: each participant is deterministically assigned one (by weight) and
    /// receives its `payload` (falls back to the event payload) and its flag overrides on top of
    /// the event-level ones. Events are tagged `live:<key> → variant` for per-variant metrics.
    #[serde(default)]
    pub variants: Vec<LiveEventVariant>,
    /// Monitor metrics (catalog keys) evaluated over the event's participants per run.
    #[serde(default)]
    pub metrics: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveEventVariant {
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "one")]
    pub weight: f64,
    /// Payload for this variant; `None` = the event payload.
    #[serde(default)]
    pub payload: Option<Value>,
    #[serde(default)]
    pub flag_overrides: serde_json::Map<String, Value>,
}

/// Event tag prefix used in the per-event `experiments` map: `live:<event key>` → variant
/// (or `default` when the event has no variants).
pub const LIVE_TAG_PREFIX: &str = "live:";
pub const DEFAULT_VARIANT: &str = "default";

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

/// A player's membership record: when they joined and, for events granted by a journey on the
/// player's own timeline, the personal window during which the event is active for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Join {
    pub joined_at: DateTime<Utc>,
    #[serde(default)]
    pub window_start: Option<DateTime<Utc>>,
    #[serde(default)]
    pub window_end: Option<DateTime<Utc>>,
}

impl Join {
    pub fn at(joined_at: DateTime<Utc>) -> Self {
        Self {
            joined_at,
            window_start: None,
            window_end: None,
        }
    }

    /// Personal window still ahead of or around `now`.
    pub fn window_open(&self, now: DateTime<Utc>) -> bool {
        self.window_end.is_some_and(|e| now < e)
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
    /// Variant assigned to this player (events with variants only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub version: i64,
}

impl LiveEvent {
    pub fn validate(&self) -> Result<()> {
        if self.participation_secs.is_some_and(|s| s <= 0) {
            return Err(Error::invalid("participation_secs must be positive"));
        }
        validate_key(&self.key)?;
        let mut keys = std::collections::HashSet::new();
        for v in &self.variants {
            if v.key.is_empty() || v.key.len() > 64 || !keys.insert(v.key.as_str()) {
                return Err(Error::invalid(format!(
                    "variant keys must be unique, 1..64 chars ({:?})",
                    v.key
                )));
            }
            if v.weight <= 0.0 || !v.weight.is_finite() {
                return Err(Error::invalid(format!(
                    "variant {:?}: weight must be > 0",
                    v.key
                )));
            }
        }
        for m in &self.metrics {
            validate_key(m)?;
        }
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

    /// Deterministic variant for a subject (identity id), by weight; `None` without variants.
    pub fn variant_for(&self, subject: &str) -> Option<&LiveEventVariant> {
        if self.variants.is_empty() {
            return None;
        }
        let total: f64 = self.variants.iter().map(|v| v.weight.max(0.0)).sum();
        if total <= 0.0 {
            return self.variants.first();
        }
        let point = crate::hash::bucket("live_event", &self.key, subject) as f64
            / crate::hash::BUCKETS as f64
            * total;
        let mut acc = 0.0;
        for v in &self.variants {
            acc += v.weight.max(0.0);
            if point < acc {
                return Some(v);
            }
        }
        self.variants.last()
    }

    /// Apply the subject's variant to a view: value and `variant` field.
    pub fn personalize(&self, view: &mut ActiveEvent, subject: &str) {
        if let Some(v) = self.variant_for(subject) {
            view.variant = Some(v.key.clone());
            if let Some(p) = &v.payload {
                view.value = p.clone();
            }
        }
    }

    /// Event-level flag overrides with the variant's on top.
    pub fn overrides_for(&self, variant: Option<&str>) -> serde_json::Map<String, Value> {
        let mut out = self.flag_overrides.clone();
        if let Some(v) = variant.and_then(|k| self.variants.iter().find(|v| v.key == k)) {
            for (k, val) in &v.flag_overrides {
                out.insert(k.clone(), val.clone());
            }
        }
        out
    }

    /// Tag written into each analytics event of a participant.
    pub fn tag(&self, variant: Option<&str>) -> (String, String) {
        (
            format!("{LIVE_TAG_PREFIX}{}", self.key),
            variant.unwrap_or(DEFAULT_VARIANT).to_string(),
        )
    }

    pub fn targets(&self, props: &PropertyMap, audiences: &dyn Fn(&str) -> bool) -> bool {
        self.enabled
            && self.condition.matches(&Context { props })
            && self.audiences.iter().all(|a| audiences(a))
    }

    pub fn duration(&self) -> Option<Duration> {
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

    /// View honouring a personal window (journey-granted event on the player's own clock):
    /// inside the window the event is active regardless of its schedule; once the window has
    /// passed the membership no longer counts.
    pub fn view_join(&self, now: DateTime<Utc>, join: Option<&Join>) -> ActiveEvent {
        if let Some(j) = join {
            if let (Some(ws), Some(we)) = (j.window_start, j.window_end) {
                if now >= we {
                    return self.view_for(now, None);
                }
                let active = now >= ws;
                let window = Occurrence { start: ws, end: we };
                return ActiveEvent {
                    key: self.key.clone(),
                    name: self.name.clone(),
                    description: self.description.clone(),
                    value: self.payload.clone(),
                    active,
                    current: active.then_some(window.clone()),
                    next: if active { self.next(now) } else { Some(window) },
                    join_required: false,
                    joined_at: Some(j.joined_at),
                    participation_ends_at: active.then_some(we),
                    variant: None,
                    version: self.version,
                };
            }
        }
        self.view_for(now, join.map(|j| j.joined_at))
    }

    /// View for a specific player given when (if ever) they joined. Explicit-join events are only
    /// active for joined players; `participation_secs` ends participation early; `sticky`
    /// participants stay active until their timer runs out even outside a window.
    pub fn view_for(&self, now: DateTime<Utc>, joined_at: Option<DateTime<Utc>>) -> ActiveEvent {
        let current = self.current(now);
        // Participation is counted per run: a join carried over from an earlier occurrence
        // (sticky / recurring events) restarts its timer when the next run begins.
        let timer_end = joined_at.and_then(|j| {
            self.participation_secs.map(|s| {
                let base = current.as_ref().map_or(j, |c| c.start.max(j));
                base + Duration::seconds(s)
            })
        });
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
            variant: None,
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
            variants: Vec::new(),
            metrics: Vec::new(),
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
