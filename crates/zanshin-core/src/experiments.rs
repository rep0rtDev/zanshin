//! A/B experiments: targeted, allocated by percentage, variants picked deterministically and
//! then pinned per identity (sticky assignment lives in storage).

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

pub const MAX_VARIANTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Being designed; nobody is assigned.
    Draft,
    /// Assigning and serving.
    Running,
    /// Existing assignments are served, nobody new is assigned.
    Paused,
    /// Finished; nothing is served.
    Stopped,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Draft => "draft",
            Status::Running => "running",
            Status::Paused => "paused",
            Status::Stopped => "stopped",
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "draft" => Ok(Status::Draft),
            "running" => Ok(Status::Running),
            "paused" => Ok(Status::Paused),
            "stopped" => Ok(Status::Stopped),
            other => Err(Error::invalid(format!(
                "unknown experiment status {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperimentVariant {
    pub key: String,
    #[serde(default)]
    pub name: String,
    /// Payload served to the client (and, when `flag_key` is set, the value of that flag).
    pub value: Value,
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Experiment {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub hypothesis: String,
    pub status: Status,
    #[serde(default = "Condition::always")]
    pub condition: Condition,
    /// Audience keys the identity must belong to (all of them).
    #[serde(default)]
    pub audiences: Vec<String>,
    /// Share of eligible identities to enrol (0..=100).
    #[serde(default = "default_percentage")]
    pub percentage: f64,
    pub variants: Vec<ExperimentVariant>,
    /// Variant key treated as control in results.
    #[serde(default)]
    pub control: Option<String>,
    /// Feature flag whose value the assigned variant overrides.
    #[serde(default)]
    pub flag_key: Option<String>,
    #[serde(default)]
    pub salt: String,
    #[serde(default)]
    pub starts_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub ends_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// When several running experiments pin the same flag, the highest priority wins
    /// (ties: earliest start). Lower-priority experiments still enrol and track metrics.
    #[serde(default)]
    pub priority: i32,
    /// What the bucket hash is keyed on: `identity` or `property:<flat path>` (e.g. `custom.clan_id`).
    #[serde(default = "default_split_key")]
    pub split_key: String,
    /// Stop enrolling once this many players are assigned.
    #[serde(default)]
    pub max_participants: Option<i64>,
    /// Stop enrolling after this instant (assigned players keep being served).
    #[serde(default)]
    pub enrol_until: Option<chrono::DateTime<chrono::Utc>>,
    /// Locked experiments only accept status / lock changes.
    #[serde(default)]
    pub locked: bool,
    /// Keys from the metrics catalog (goal + monitor) evaluated in results.
    #[serde(default)]
    pub metrics: Vec<String>,
    #[serde(default)]
    pub copied_from: Option<String>,
    /// Previous experiment in a phased sequence.
    #[serde(default)]
    pub phase_of: Option<String>,
    #[serde(default)]
    pub interrupted_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub interrupt_reason: String,
    /// Variant declared the winner when the experiment was concluded.
    #[serde(default)]
    pub winner: Option<String>,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub fn default_split_key() -> String {
    "identity".into()
}

fn default_percentage() -> f64 {
    100.0
}

impl Experiment {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        self.condition.validate()?;
        if !(0.0..=100.0).contains(&self.percentage) || self.percentage.is_nan() {
            return Err(Error::invalid("percentage must be within 0..=100"));
        }
        if self.variants.len() < 2 {
            return Err(Error::invalid("an experiment needs at least two variants"));
        }
        if self.variants.len() > MAX_VARIANTS {
            return Err(Error::invalid(format!(
                "too many variants (max {MAX_VARIANTS})"
            )));
        }
        let mut keys = std::collections::HashSet::new();
        for v in &self.variants {
            if v.key.is_empty() || v.key.len() > 64 || !keys.insert(&v.key) {
                return Err(Error::invalid(
                    "variant keys must be unique and 1..64 chars",
                ));
            }
            if v.weight < 0.0 || v.weight.is_nan() {
                return Err(Error::invalid(format!(
                    "variant {} has a negative weight",
                    v.key
                )));
            }
        }
        if self.variants.iter().all(|v| v.weight <= 0.0) {
            return Err(Error::invalid(
                "at least one variant needs a positive weight",
            ));
        }
        if let Some(c) = &self.control {
            if !keys.contains(c) {
                return Err(Error::invalid(format!("control {c:?} is not a variant")));
            }
        }
        if let Some(f) = &self.flag_key {
            validate_key(f)?;
        }
        if let (Some(s), Some(e)) = (self.starts_at, self.ends_at) {
            if e <= s {
                return Err(Error::invalid("ends_at must be after starts_at"));
            }
        }
        if self.split_key != "identity"
            && !self
                .split_key
                .strip_prefix("property:")
                .is_some_and(|p| !p.is_empty() && p.len() <= 128)
        {
            return Err(Error::invalid(
                "split_key must be \"identity\" or \"property:<path>\"",
            ));
        }
        if self.max_participants.is_some_and(|m| m <= 0) {
            return Err(Error::invalid("max_participants must be positive"));
        }
        if let Some(w) = &self.winner {
            if !keys.contains(w) {
                return Err(Error::invalid(format!("winner {w:?} is not a variant")));
            }
        }
        for m in &self.metrics {
            validate_key(m)?;
        }
        Ok(())
    }

    /// The string the bucket hash is keyed on for this player.
    pub fn split_subject<'a>(
        &self,
        external_id: &'a str,
        props: &'a PropertyMap,
    ) -> std::borrow::Cow<'a, str> {
        match self.split_key.strip_prefix("property:") {
            Some(path) => match props.get(path) {
                Some(Value::String(s)) if !s.is_empty() => std::borrow::Cow::Borrowed(s.as_str()),
                Some(Value::Null) | None => std::borrow::Cow::Borrowed(external_id),
                Some(v) => std::borrow::Cow::Owned(v.to_string()),
            },
            None => std::borrow::Cow::Borrowed(external_id),
        }
    }

    /// Is the experiment serving assignments right now?
    pub fn is_live(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        matches!(self.status, Status::Running | Status::Paused) && self.in_window(now)
    }

    /// May new identities be enrolled right now?
    pub fn is_enrolling(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.status == Status::Running
            && self.in_window(now)
            && self.enrol_until.is_none_or(|u| now < u)
    }

    fn in_window(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.starts_at.is_none_or(|s| now >= s) && self.ends_at.is_none_or(|e| now < e)
    }

    pub fn variant(&self, key: &str) -> Option<&ExperimentVariant> {
        self.variants.iter().find(|v| v.key == key)
    }

    /// Decide whether a not-yet-assigned subject is enrolled and, if so, into which variant.
    pub fn allocate(
        &self,
        subject: &str,
        props: &PropertyMap,
        audiences: &dyn Fn(&str) -> bool,
    ) -> Option<&ExperimentVariant> {
        if !self.condition.matches(&Context { props }) {
            return None;
        }
        if !self.audiences.iter().all(|a| audiences(a)) {
            return None;
        }
        let subject = self.split_subject(subject, props);
        let subject = subject.as_ref();
        let ns = format!("experiment:{}", self.key);
        if !hash::in_rollout(&ns, &self.salt, subject, self.percentage) {
            return None;
        }
        let weights: Vec<f64> = self.variants.iter().map(|v| v.weight).collect();
        let i = hash::weighted_pick(&ns, &format!("{}:variant", self.salt), subject, &weights)?;
        Some(&self.variants[i])
    }
}

/// What a client sees for an experiment it is enrolled in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    pub key: String,
    pub name: String,
    pub variant: String,
    pub value: Value,
    pub version: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn exp() -> Experiment {
        Experiment {
            priority: 0,
            id: Uuid::nil(),
            key: "price_test".into(),
            name: String::new(),
            description: String::new(),
            hypothesis: String::new(),
            status: Status::Running,
            condition: serde_json::from_value(
                json!({"prop":"default.country","op":"eq","value":"DE"}),
            )
            .unwrap(),
            audiences: vec![],
            percentage: 50.0,
            variants: vec![
                ExperimentVariant {
                    key: "control".into(),
                    name: String::new(),
                    value: json!(9.99),
                    weight: 50.0,
                },
                ExperimentVariant {
                    key: "cheap".into(),
                    name: String::new(),
                    value: json!(4.99),
                    weight: 50.0,
                },
            ],
            control: Some("control".into()),
            flag_key: None,
            salt: String::new(),
            split_key: default_split_key(),
            max_participants: None,
            enrol_until: None,
            locked: false,
            metrics: vec![],
            copied_from: None,
            phase_of: None,
            interrupted_at: None,
            interrupt_reason: String::new(),
            winner: None,
            starts_at: None,
            ends_at: None,
            tags: vec![],
            version: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn allocation_respects_targeting_and_percentage() {
        let e = exp();
        e.validate().unwrap();
        let none = |_: &str| false;
        let mut fr = PropertyMap::new();
        fr.insert("default.country".into(), json!("FR"));
        assert!(e.allocate("u", &fr, &none).is_none());
        let mut de = PropertyMap::new();
        de.insert("default.country".into(), json!("DE"));
        let enrolled = (0..4000)
            .filter(|i| e.allocate(&format!("u{i}"), &de, &none).is_some())
            .count();
        assert!((1800..2200).contains(&enrolled), "{enrolled}");
        let first = e.allocate("player", &de, &none).map(|v| v.key.clone());
        assert_eq!(
            first,
            e.allocate("player", &de, &none).map(|v| v.key.clone())
        );
    }

    #[test]
    fn lifecycle() {
        let mut e = exp();
        let now = chrono::Utc::now();
        assert!(e.is_enrolling(now) && e.is_live(now));
        e.status = Status::Paused;
        assert!(!e.is_enrolling(now) && e.is_live(now));
        e.status = Status::Stopped;
        assert!(!e.is_live(now));
        e.status = Status::Running;
        e.ends_at = Some(now - chrono::Duration::hours(1));
        assert!(!e.is_live(now));
    }

    #[test]
    fn validation() {
        let mut e = exp();
        e.variants.pop();
        assert!(e.validate().is_err());
        let mut e = exp();
        e.control = Some("missing".into());
        assert!(e.validate().is_err());
    }
}
