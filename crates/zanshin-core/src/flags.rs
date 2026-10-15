//! Feature flags: typed values, ordered targeting rules, percentage rollouts and variants.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    hash,
    properties::PropertyMap,
    rules::{Condition, Context},
    Error, Result,
};

pub const MAX_RULES: usize = 64;
pub const MAX_VARIANTS: usize = 32;
pub const MAX_VALUE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Bool,
    String,
    Number,
    Json,
}

impl ValueType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ValueType::Bool => "bool",
            ValueType::String => "string",
            ValueType::Number => "number",
            ValueType::Json => "json",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "bool" => Ok(ValueType::Bool),
            "string" => Ok(ValueType::String),
            "number" => Ok(ValueType::Number),
            "json" => Ok(ValueType::Json),
            other => Err(Error::invalid(format!("unknown value type {other:?}"))),
        }
    }

    pub fn check(&self, v: &Value) -> Result<()> {
        let ok = match self {
            ValueType::Bool => v.is_boolean(),
            ValueType::String => v.is_string(),
            ValueType::Number => v.is_number(),
            ValueType::Json => true,
        };
        if !ok {
            return Err(Error::invalid(format!(
                "value {v} is not of type {}",
                self.as_str()
            )));
        }
        if serde_json::to_vec(v).map(|b| b.len()).unwrap_or(usize::MAX) > MAX_VALUE_BYTES {
            return Err(Error::invalid("flag value too large"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub key: String,
    pub value: Value,
    /// Relative weight; weights need not sum to 100.
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Targeting condition; omitted = matches everyone.
    #[serde(default = "Condition::always")]
    pub condition: Condition,
    /// Audience keys the identity must belong to (all of them). Resolved by the caller.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audiences: Vec<String>,
    /// Percentage of matching identities the rule applies to (0..=100).
    #[serde(default = "default_percentage")]
    pub percentage: f64,
    /// Fixed value served by this rule; mutually exclusive with `variants`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// Weighted variants served by this rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Variant>,
}

fn default_percentage() -> f64 {
    100.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Flag {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub value_type: ValueType,
    pub default_value: Value,
    pub enabled: bool,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Hash salt; changing it reshuffles every rollout bucket of this flag.
    #[serde(default)]
    pub salt: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Other flags that must resolve to the expected value for this flag's rules to apply;
    /// otherwise the default is served (`Reason::Prerequisite`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prerequisites: Vec<Prerequisite>,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A dependency on another flag: it must be enabled and, when `value` is given, resolve to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prerequisite {
    pub flag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// Why a particular value was served; returned to admins for debugging and to clients as `reason`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Reason {
    Disabled,
    Default,
    Override,
    Rule {
        rule_id: String,
    },
    Variant {
        rule_id: String,
        variant: String,
    },
    Experiment {
        experiment: String,
        variant: String,
    },
    LiveEvent {
        live_event: String,
    },
    /// A prerequisite flag did not resolve to its expected value; the default was served.
    Prerequisite {
        flag: String,
    },
}

/// A value fixed for this identity before rules are consulted.
#[derive(Debug, Clone, PartialEq)]
pub enum Pinned<'a> {
    Override(&'a Value),
    LiveEvent {
        live_event: &'a str,
        value: &'a Value,
    },
    Experiment {
        experiment: &'a str,
        variant: &'a str,
        value: &'a Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evaluation {
    pub key: String,
    pub value: Value,
    pub value_type: ValueType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub reason: Reason,
    pub version: i64,
}

pub fn validate_key(key: &str) -> Result<()> {
    let ok = !key.is_empty()
        && key.len() <= 128
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "invalid flag key {key:?} (use [A-Za-z_][A-Za-z0-9_.-]*, max 128)"
        )))
    }
}

impl Flag {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        self.value_type.check(&self.default_value)?;
        if self.rules.len() > MAX_RULES {
            return Err(Error::invalid(format!("too many rules (max {MAX_RULES})")));
        }
        let mut seen = std::collections::HashSet::new();
        for r in &self.rules {
            if r.id.is_empty() || r.id.len() > 64 || !seen.insert(&r.id) {
                return Err(Error::invalid(format!(
                    "rule ids must be unique and 1..64 chars ({:?})",
                    r.id
                )));
            }
            r.condition.validate()?;
            if !(0.0..=100.0).contains(&r.percentage) || r.percentage.is_nan() {
                return Err(Error::invalid(format!(
                    "rule {}: percentage must be within 0..=100",
                    r.id
                )));
            }
            match (&r.value, r.variants.is_empty()) {
                (Some(v), true) => self.value_type.check(v)?,
                (None, false) => {
                    if r.variants.len() > MAX_VARIANTS {
                        return Err(Error::invalid(format!("rule {}: too many variants", r.id)));
                    }
                    let mut keys = std::collections::HashSet::new();
                    for v in &r.variants {
                        if v.key.is_empty() || v.key.len() > 64 || !keys.insert(&v.key) {
                            return Err(Error::invalid(format!(
                                "rule {}: variant keys must be unique and 1..64 chars",
                                r.id
                            )));
                        }
                        if v.weight < 0.0 || v.weight.is_nan() {
                            return Err(Error::invalid(format!(
                                "rule {}: variant {} has a negative weight",
                                r.id, v.key
                            )));
                        }
                        self.value_type.check(&v.value)?;
                    }
                    if r.variants.iter().all(|v| v.weight <= 0.0) {
                        return Err(Error::invalid(format!(
                            "rule {}: at least one variant needs a positive weight",
                            r.id
                        )));
                    }
                }
                (Some(_), false) => {
                    return Err(Error::invalid(format!(
                        "rule {}: set either value or variants, not both",
                        r.id
                    )))
                }
                (None, true) => {
                    return Err(Error::invalid(format!(
                        "rule {}: needs a value or variants",
                        r.id
                    )))
                }
            }
        }
        for p in &self.prerequisites {
            validate_key(&p.flag)?;
            if p.flag == self.key {
                return Err(Error::invalid("a flag cannot be its own prerequisite"));
            }
        }
        if self.prerequisites.len() > 16 {
            return Err(Error::invalid("too many prerequisites (max 16)"));
        }
        Ok(())
    }

    /// Evaluate for a subject. `props` is the flattened property map; `audiences` is the set of
    /// audience keys the subject belongs to; `pinned` (override or experiment variant) wins over rules.
    pub fn evaluate(
        &self,
        subject: &str,
        props: &PropertyMap,
        audiences: &dyn Fn(&str) -> bool,
        pinned: Option<Pinned<'_>>,
    ) -> Evaluation {
        let mut ev = Evaluation {
            key: self.key.clone(),
            value: self.default_value.clone(),
            value_type: self.value_type,
            variant: None,
            reason: Reason::Default,
            version: self.version,
        };
        if !self.enabled {
            ev.reason = Reason::Disabled;
            return ev;
        }
        match pinned {
            Some(Pinned::Override(v)) => {
                ev.value = v.clone();
                ev.reason = Reason::Override;
                return ev;
            }
            Some(Pinned::Experiment {
                experiment,
                variant,
                value,
            }) => {
                ev.value = value.clone();
                ev.variant = Some(variant.to_string());
                ev.reason = Reason::Experiment {
                    experiment: experiment.to_string(),
                    variant: variant.to_string(),
                };
                return ev;
            }
            Some(Pinned::LiveEvent { live_event, value }) => {
                ev.value = value.clone();
                ev.reason = Reason::LiveEvent {
                    live_event: live_event.to_string(),
                };
                return ev;
            }
            None => {}
        }
        let ctx = Context { props };
        for rule in &self.rules {
            if !rule.condition.matches(&ctx) {
                continue;
            }
            if !rule.audiences.iter().all(|a| audiences(a)) {
                continue;
            }
            let ns = format!("flag:{}", self.key);
            let rollout_salt = format!("{}:{}", self.salt, rule.id);
            if !hash::in_rollout(&ns, &rollout_salt, subject, rule.percentage) {
                continue;
            }
            if let Some(v) = &rule.value {
                ev.value = v.clone();
                ev.reason = Reason::Rule {
                    rule_id: rule.id.clone(),
                };
                return ev;
            }
            let weights: Vec<f64> = rule.variants.iter().map(|v| v.weight).collect();
            let variant_salt = format!("{}:{}:variant", self.salt, rule.id);
            if let Some(i) = hash::weighted_pick(&ns, &variant_salt, subject, &weights) {
                let v = &rule.variants[i];
                ev.value = v.value.clone();
                ev.variant = Some(v.key.clone());
                ev.reason = Reason::Variant {
                    rule_id: rule.id.clone(),
                    variant: v.key.clone(),
                };
                return ev;
            }
        }
        ev
    }
}

pub fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn flag(rules: Value) -> Flag {
        Flag {
            id: Uuid::nil(),
            key: "shop_v2".into(),
            name: String::new(),
            description: String::new(),
            value_type: ValueType::Bool,
            default_value: json!(false),
            enabled: true,
            rules: serde_json::from_value(rules).unwrap(),
            salt: String::new(),
            tags: vec![],
            prerequisites: vec![],
            version: 3,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn props(country: &str, level: i64) -> PropertyMap {
        let mut m = PropertyMap::new();
        m.insert("default.country".into(), json!(country));
        m.insert("custom.level".into(), json!(level));
        m
    }

    #[test]
    fn rule_order_and_default() {
        let f = flag(json!([
            {"id":"de","condition":{"prop":"default.country","op":"eq","value":"DE"},"value":true},
            {"id":"high","condition":{"prop":"custom.level","op":"gte","value":50},"value":true}
        ]));
        f.validate().unwrap();
        let none = |_: &str| false;
        assert_eq!(
            f.evaluate("u1", &props("DE", 1), &none, None).reason,
            Reason::Rule {
                rule_id: "de".into()
            }
        );
        assert_eq!(
            f.evaluate("u1", &props("FR", 99), &none, None).reason,
            Reason::Rule {
                rule_id: "high".into()
            }
        );
        let ev = f.evaluate("u1", &props("FR", 1), &none, None);
        assert_eq!(ev.reason, Reason::Default);
        assert_eq!(ev.value, json!(false));
        assert_eq!(ev.version, 3);
    }

    #[test]
    fn disabled_and_override() {
        let mut f = flag(json!([{"id":"all","value":true}]));
        let none = |_: &str| false;
        assert_eq!(
            f.evaluate(
                "u",
                &props("DE", 1),
                &none,
                Some(Pinned::Override(&json!(false)))
            )
            .reason,
            Reason::Override
        );
        f.enabled = false;
        let ev = f.evaluate(
            "u",
            &props("DE", 1),
            &none,
            Some(Pinned::Override(&json!(true))),
        );
        assert_eq!(ev.reason, Reason::Disabled);
        assert_eq!(ev.value, json!(false));
    }

    #[test]
    fn rollout_percentage_and_audience_gate() {
        let f = flag(json!([{"id":"half","percentage":50,"value":true}]));
        let none = |_: &str| false;
        let hits = (0..4000)
            .filter(|i| {
                f.evaluate(&format!("u{i}"), &props("DE", 1), &none, None)
                    .value
                    == json!(true)
            })
            .count();
        assert!((1800..2200).contains(&hits), "hits {hits}");

        let f = flag(json!([{"id":"beta","audiences":["beta"],"value":true}]));
        assert_eq!(
            f.evaluate("u", &props("DE", 1), &none, None).reason,
            Reason::Default
        );
        let beta = |a: &str| a == "beta";
        assert_eq!(
            f.evaluate("u", &props("DE", 1), &beta, None).reason,
            Reason::Rule {
                rule_id: "beta".into()
            }
        );
    }

    #[test]
    fn variants_are_sticky() {
        let f = flag(
            json!([{"id":"ab","variants":[{"key":"a","value":true,"weight":50},{"key":"b","value":false,"weight":50}]}]),
        );
        f.validate().unwrap();
        let none = |_: &str| false;
        let first = f.evaluate("player", &props("DE", 1), &none, None);
        for _ in 0..10 {
            assert_eq!(
                f.evaluate("player", &props("DE", 1), &none, None).variant,
                first.variant
            );
        }
        assert!(matches!(first.reason, Reason::Variant { .. }));
    }

    #[test]
    fn validation_catches_type_and_shape_errors() {
        let mut f = flag(json!([{"id":"x","value":"not-a-bool"}]));
        assert!(f.validate().is_err());
        f = flag(json!([{"id":"x","value":true,"variants":[{"key":"a","value":true,"weight":1}]}]));
        assert!(f.validate().is_err());
        f = flag(json!([{"id":"x","percentage":150,"value":true}]));
        assert!(f.validate().is_err());
        f = flag(json!([{"id":"x","value":true},{"id":"x","value":false}]));
        assert!(f.validate().is_err());
        assert!(validate_key("9bad").is_err());
        assert!(validate_key("good.key-1").is_ok());
    }
}

/// One leaf-level difference between two flag values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueDiff {
    /// JSON pointer-ish path (`/` separated), empty for scalar roots.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Value>,
}

/// Serialized size of a value as the client receives it.
pub fn payload_bytes(v: &Value) -> usize {
    serde_json::to_vec(v).map(|b| b.len()).unwrap_or(0)
}

/// Leaf-level diff of two JSON values (objects recurse, everything else compares whole).
pub fn diff_values(a: &Value, b: &Value) -> Vec<ValueDiff> {
    fn walk(path: &str, a: Option<&Value>, b: Option<&Value>, out: &mut Vec<ValueDiff>) {
        match (a, b) {
            (Some(Value::Object(x)), Some(Value::Object(y))) => {
                let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
                keys.sort();
                keys.dedup();
                for k in keys {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}/{k}")
                    };
                    walk(&p, x.get(k), y.get(k), out);
                }
            }
            (x, y) if x == y => {}
            (x, y) => out.push(ValueDiff {
                path: path.to_string(),
                from: x.cloned(),
                to: y.cloned(),
            }),
        }
    }
    let mut out = Vec::new();
    walk("", Some(a), Some(b), &mut out);
    out
}

impl Flag {
    /// Resolve a value selector used by the diff API: `default`, `rule:<id>`, `variant:<key>`
    /// (first rule carrying that variant) or `rule:<id>:variant:<key>`.
    pub fn named_value(&self, selector: &str) -> Option<&Value> {
        if selector == "default" {
            return Some(&self.default_value);
        }
        if let Some(rest) = selector.strip_prefix("variant:") {
            return self
                .rules
                .iter()
                .flat_map(|r| r.variants.iter())
                .find(|v| v.key == rest)
                .map(|v| &v.value);
        }
        let rest = selector.strip_prefix("rule:")?;
        let (rule_id, variant) = match rest.split_once(":variant:") {
            Some((r, v)) => (r, Some(v)),
            None => (rest, None),
        };
        let rule = self.rules.iter().find(|r| r.id == rule_id)?;
        match variant {
            Some(vk) => rule.variants.iter().find(|v| v.key == vk).map(|v| &v.value),
            None => rule.value.as_ref().or(Some(&self.default_value)),
        }
    }

    /// Every selectable value of the flag with its payload size (for the console's size column).
    pub fn payload_sizes(&self) -> Vec<(String, usize)> {
        let mut out = vec![("default".to_string(), payload_bytes(&self.default_value))];
        for r in &self.rules {
            if let Some(v) = &r.value {
                out.push((format!("rule:{}", r.id), payload_bytes(v)));
            }
            for v in &r.variants {
                out.push((
                    format!("rule:{}:variant:{}", r.id, v.key),
                    payload_bytes(&v.value),
                ));
            }
        }
        out
    }
}
