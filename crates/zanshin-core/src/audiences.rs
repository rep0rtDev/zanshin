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
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
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
        Ok(())
    }

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
