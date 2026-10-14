//! Audiences: named, rule-based segments of identities, optionally sampled by percentage.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    flags::validate_key,
    hash,
    properties::PropertyMap,
    rules::{Condition, Context},
    Error, Result,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Audience {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "Condition::always")]
    pub condition: Condition,
    /// Sample this share (0..=100) of identities matching the condition.
    #[serde(default = "default_percentage")]
    pub percentage: f64,
    pub enabled: bool,
    #[serde(default)]
    pub salt: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// `dynamic` (rule-based) or `managed` (membership pushed from outside / curated by hand).
    #[serde(default)]
    pub kind: AudienceKind,
    /// `live` evaluates the rule on every request; `scheduled` serves stored membership refreshed
    /// by `schedule_cron` / explicit recompute.
    #[serde(default)]
    pub recompute: Recompute,
    #[serde(default)]
    pub schedule_cron: String,
    /// Free-form origin for managed audiences (e.g. "crm", "nakama:guild_leaders").
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copied_from: Option<String>,
    #[serde(default)]
    pub last_computed_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub member_count: i64,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudienceKind {
    #[default]
    Dynamic,
    Managed,
}

impl AudienceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AudienceKind::Dynamic => "dynamic",
            AudienceKind::Managed => "managed",
        }
    }
}

impl std::str::FromStr for AudienceKind {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "dynamic" => Ok(AudienceKind::Dynamic),
            "managed" => Ok(AudienceKind::Managed),
            other => Err(Error::invalid(format!("unknown audience kind {other:?}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recompute {
    #[default]
    Live,
    Scheduled,
}

impl Recompute {
    pub fn as_str(&self) -> &'static str {
        match self {
            Recompute::Live => "live",
            Recompute::Scheduled => "scheduled",
        }
    }
}

impl std::str::FromStr for Recompute {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "live" => Ok(Recompute::Live),
            "scheduled" => Ok(Recompute::Scheduled),
            other => Err(Error::invalid(format!("unknown recompute mode {other:?}"))),
        }
    }
}

fn default_percentage() -> f64 {
    100.0
}

impl Audience {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        self.condition.validate()?;
        if !(0.0..=100.0).contains(&self.percentage) || self.percentage.is_nan() {
            return Err(Error::invalid("percentage must be within 0..=100"));
        }
        if self.recompute == Recompute::Scheduled && !self.schedule_cron.is_empty() {
            self.schedule_cron
                .parse::<cron::Schedule>()
                .map_err(|e| Error::invalid(format!("schedule_cron: {e}")))?;
        }
        Ok(())
    }

    /// Whether the rule (and sampling) admits the subject. Include / exclude lists and stored
    /// membership for `managed` / `scheduled` audiences are layered on top by the service.
    pub fn contains(&self, subject: &str, props: &PropertyMap) -> bool {
        if !self.enabled {
            return false;
        }
        if !self.condition.matches(&Context { props }) {
            return false;
        }
        hash::in_rollout(
            &format!("audience:{}", self.key),
            &self.salt,
            subject,
            self.percentage,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn aud(cond: serde_json::Value, pct: f64) -> Audience {
        Audience {
            id: Uuid::nil(),
            key: "beta".into(),
            name: String::new(),
            description: String::new(),
            condition: serde_json::from_value(cond).unwrap(),
            percentage: pct,
            enabled: true,
            salt: String::new(),
            tags: vec![],
            kind: AudienceKind::Dynamic,
            recompute: Recompute::Live,
            schedule_cron: String::new(),
            source: String::new(),
            copied_from: None,
            last_computed_at: None,
            member_count: 0,
            version: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn membership() {
        let a = aud(json!({"prop":"custom.beta","op":"eq","value":true}), 100.0);
        a.validate().unwrap();
        let mut p = PropertyMap::new();
        p.insert("custom.beta".into(), json!(true));
        assert!(a.contains("u", &p));
        p.insert("custom.beta".into(), json!(false));
        assert!(!a.contains("u", &p));
        let mut disabled = aud(json!({"all":[]}), 100.0);
        disabled.enabled = false;
        assert!(!disabled.contains("u", &p));
    }

    #[test]
    fn sampling() {
        let a = aud(json!({"all":[]}), 10.0);
        let p = PropertyMap::new();
        let n = (0..5000)
            .filter(|i| a.contains(&format!("u{i}"), &p))
            .count();
        assert!((400..600).contains(&n), "{n}");
    }
}
