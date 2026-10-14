//! Analytics events: client/server-reported facts about an identity, plus the computed
//! properties derived from them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{properties::PropertyMap, Error, Result};

pub const MAX_BATCH: usize = 500;
pub const MAX_NAME: usize = 128;
pub const MAX_VALUE: usize = 1024;
pub const MAX_METADATA_BYTES: usize = 8 * 1024;
/// Events older than this (or further in the future than 5 min) are clamped to receipt time.
pub const MAX_SKEW_PAST_DAYS: i64 = 30;

/// Names reserved for events the server emits itself.
pub const EV_AUTHENTICATE: &str = "$authenticate";
pub const EV_EXPOSURE: &str = "$exposure";
pub const EV_MESSAGE_READ: &str = "$message_read";
pub const EV_IDENTITY_CREATE: &str = "$identity_create";
pub const EV_SESSION_START: &str = "$session_start";
pub const EV_SESSION_END: &str = "$session_end";
pub const EV_IDENTIFY: &str = "$identify";
pub const EV_EXPERIMENT_JOIN: &str = "$experiment_join";
pub const EV_LIVE_EVENT_JOIN: &str = "$live_event_join";

/// One event as reported by a client.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventInput {
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub metadata: Option<Map<String, Value>>,
    #[serde(default)]
    pub timestamp: Option<DateTime<Utc>>,
    /// Client-side idempotency key; duplicates within a batch are dropped.
    #[serde(default)]
    pub id: Option<Uuid>,
}

impl EventInput {
    pub fn validate(&self, allow_system: bool) -> Result<()> {
        let n = &self.name;
        if n.is_empty()
            || n.len() > MAX_NAME
            || !n
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ':' | '$'))
        {
            return Err(Error::invalid(format!("invalid event name {n:?}")));
        }
        if n.starts_with('$') && !allow_system {
            return Err(Error::invalid(format!("event name {n:?} is reserved")));
        }
        if self.value.as_ref().is_some_and(|v| v.len() > MAX_VALUE) {
            return Err(Error::invalid("event value too long"));
        }
        if let Some(m) = &self.metadata {
            let size = serde_json::to_vec(m).map(|b| b.len()).unwrap_or(usize::MAX);
            if size > MAX_METADATA_BYTES {
                return Err(Error::invalid("event metadata too large"));
            }
        }
        Ok(())
    }
}

/// A fully resolved event ready for the sink.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: Uuid,
    pub event_time: DateTime<Utc>,
    pub identity_id: Uuid,
    pub external_id: String,
    pub name: String,
    pub value: String,
    pub metadata: Value,
    pub session_id: Option<Uuid>,
    /// experiment key → variant the identity was assigned to when the event happened.
    pub experiments: Vec<(String, String)>,
}

impl Event {
    pub fn from_input(
        input: EventInput,
        identity_id: Uuid,
        external_id: &str,
        session_id: Option<Uuid>,
        experiments: Vec<(String, String)>,
        now: DateTime<Utc>,
    ) -> Self {
        let event_time = match input.timestamp {
            Some(t)
                if t <= now + chrono::Duration::minutes(5)
                    && t >= now - chrono::Duration::days(MAX_SKEW_PAST_DAYS) =>
            {
                t
            }
            _ => now,
        };
        Event {
            id: input.id.unwrap_or_else(Uuid::now_v7),
            event_time,
            identity_id,
            external_id: external_id.to_string(),
            name: input.name,
            value: input.value.unwrap_or_default(),
            metadata: input.metadata.map(Value::Object).unwrap_or(Value::Null),
            session_id,
            experiments,
        }
    }

    pub fn numeric_value(&self) -> Option<f64> {
        self.value
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
    }
}

/// Fold a batch of events for one identity into a `computed.*` patch.
///
/// Maintained keys: `event_count`, `last_event_at`, `first_event_at`, and per event name
/// `events.<name>.count`, `events.<name>.last_at`, `events.<name>.sum` (numeric values only).
/// `$authenticate` additionally maintains `session_count`, `first_seen_at`, `last_seen_at`.
pub fn fold_computed(current: &PropertyMap, events: &[Event]) -> PropertyMap {
    let mut patch = PropertyMap::new();
    let num = |m: &PropertyMap, k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let mut event_count = num(current, "event_count");
    let mut last_event: Option<DateTime<Utc>> = current.get("last_event_at").and_then(parse_ts);
    let mut first_event: Option<DateTime<Utc>> = current.get("first_event_at").and_then(parse_ts);
    for e in events {
        if e.name == EV_AUTHENTICATE {
            let sessions = cur(&patch, current, "session_count")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                + 1.0;
            patch.insert("session_count".into(), json_num(sessions));
            patch.insert(
                "last_seen_at".into(),
                Value::String(e.event_time.to_rfc3339()),
            );
            if cur(&patch, current, "first_seen_at").is_none() {
                patch.insert(
                    "first_seen_at".into(),
                    Value::String(e.event_time.to_rfc3339()),
                );
            }
            continue;
        }
        if matches!(
            e.name.as_str(),
            EV_IDENTITY_CREATE
                | EV_SESSION_START
                | EV_IDENTIFY
                | EV_LIVE_EVENT_JOIN
                | EV_EXPERIMENT_JOIN
        ) {
            continue;
        }
        if e.name == EV_SESSION_END {
            if let Some(d) = e.numeric_value() {
                let total = cur(&patch, current, "play_time_sec")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
                    + d;
                patch.insert("play_time_sec".into(), json_num(total));
            }
            continue;
        }
        event_count += 1.0;
        last_event = Some(last_event.map_or(e.event_time, |t| t.max(e.event_time)));
        first_event = Some(first_event.map_or(e.event_time, |t| t.min(e.event_time)));
        if e.name.starts_with('$') {
            continue;
        }
        let safe = e.name.replace('.', "_");
        let count_key = format!("events.{safe}.count");
        let last_key = format!("events.{safe}.last_at");
        let c = cur(&patch, current, &count_key)
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
            + 1.0;
        patch.insert(count_key, json_num(c));
        let prev = cur(&patch, current, &last_key).as_ref().and_then(parse_ts);
        if prev.is_none_or(|p| e.event_time >= p) {
            patch.insert(last_key, Value::String(e.event_time.to_rfc3339()));
        }
        let first_key = format!("events.{safe}.first_at");
        let prev_first = cur(&patch, current, &first_key).as_ref().and_then(parse_ts);
        if prev_first.is_none_or(|p| e.event_time < p) {
            patch.insert(first_key, Value::String(e.event_time.to_rfc3339()));
        }
        if let Some(v) = e.numeric_value() {
            let sum_key = format!("events.{safe}.sum");
            let s = cur(&patch, current, &sum_key)
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                + v;
            patch.insert(sum_key, json_num(s));
            let high_key = format!("events.{safe}.high");
            let low_key = format!("events.{safe}.low");
            let high = cur(&patch, current, &high_key).and_then(|x| x.as_f64());
            if high.is_none_or(|h| v > h) {
                patch.insert(high_key, json_num(v));
            }
            let low = cur(&patch, current, &low_key).and_then(|x| x.as_f64());
            if low.is_none_or(|l| v < l) {
                patch.insert(low_key, json_num(v));
            }
            patch.insert(format!("events.{safe}.last_value"), json_num(v));
        }
    }
    if event_count != num(current, "event_count") {
        patch.insert("event_count".into(), json_num(event_count));
    }
    if let Some(t) = last_event {
        patch.insert("last_event_at".into(), Value::String(t.to_rfc3339()));
    }
    if let Some(t) = first_event {
        patch.insert("first_event_at".into(), Value::String(t.to_rfc3339()));
    }
    patch
}

fn cur(patch: &PropertyMap, current: &PropertyMap, k: &str) -> Option<Value> {
    patch.get(k).or_else(|| current.get(k)).cloned()
}

fn parse_ts(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
}

fn json_num(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9.0e15 {
        Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(name: &str, value: &str, t: DateTime<Utc>) -> Event {
        Event::from_input(
            EventInput {
                name: name.into(),
                value: Some(value.into()),
                ..Default::default()
            },
            Uuid::nil(),
            "u",
            None,
            vec![],
            t,
        )
    }

    #[test]
    fn validation() {
        assert!(EventInput {
            name: "purchase".into(),
            ..Default::default()
        }
        .validate(false)
        .is_ok());
        assert!(EventInput {
            name: "$authenticate".into(),
            ..Default::default()
        }
        .validate(false)
        .is_err());
        assert!(EventInput {
            name: "$authenticate".into(),
            ..Default::default()
        }
        .validate(true)
        .is_ok());
        assert!(EventInput {
            name: "bad name".into(),
            ..Default::default()
        }
        .validate(false)
        .is_err());
        assert!(EventInput {
            name: "x".repeat(200),
            ..Default::default()
        }
        .validate(false)
        .is_err());
    }

    #[test]
    fn timestamp_clamped() {
        let now = Utc::now();
        let far = EventInput {
            name: "a".into(),
            timestamp: Some(now - chrono::Duration::days(400)),
            ..Default::default()
        };
        assert_eq!(
            Event::from_input(far, Uuid::nil(), "u", None, vec![], now).event_time,
            now
        );
        let recent = now - chrono::Duration::hours(1);
        let ok = EventInput {
            name: "a".into(),
            timestamp: Some(recent),
            ..Default::default()
        };
        assert_eq!(
            Event::from_input(ok, Uuid::nil(), "u", None, vec![], now).event_time,
            recent
        );
    }

    #[test]
    fn computed_fold() {
        let now = Utc::now();
        let batch = vec![
            ev("$authenticate", "", now),
            ev("purchase", "9.99", now),
            ev("purchase", "5", now + chrono::Duration::seconds(1)),
            ev("level.up", "x", now),
        ];
        let p = fold_computed(&PropertyMap::new(), &batch);
        assert_eq!(p["session_count"], json!(1));
        assert_eq!(p["event_count"], json!(3));
        assert_eq!(p["events.purchase.count"], json!(2));
        assert!((p["events.purchase.sum"].as_f64().unwrap() - 14.99).abs() < 1e-9);
        assert_eq!(p["events.level_up.count"], json!(1));
        assert!(p.get("events.level_up.sum").is_none());
        // Second fold continues the counters.
        let mut cur = PropertyMap::new();
        for (k, v) in &p {
            cur.insert(k.clone(), v.clone());
        }
        let p2 = fold_computed(
            &cur,
            &[ev("$authenticate", "", now), ev("purchase", "1", now)],
        );
        assert_eq!(p2["session_count"], json!(2));
        assert_eq!(p2["events.purchase.count"], json!(3));
        assert_eq!(p2["event_count"], json!(4));
        assert!(
            p2.get("first_seen_at").is_none(),
            "first_seen_at is set once"
        );
    }
}
