//! Targeting conditions over flattened identity properties.
//!
//! ```json
//! { "all": [
//!     { "prop": "default.country", "op": "in", "value": ["DE", "AT"] },
//!     { "prop": "custom.level", "op": "gte", "value": 10 },
//!     { "any": [ { "prop": "default.app_version", "op": "semver_gte", "value": "1.4.0" },
//!                { "prop": "custom.beta", "op": "eq", "value": true } ] },
//!     { "not": { "prop": "custom.banned", "op": "exists" } }
//! ] }
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{properties::PropertyMap, Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Condition {
    All { all: Vec<Condition> },
    Any { any: Vec<Condition> },
    Not { not: Box<Condition> },
    Cmp(Comparison),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    /// Flattened property path, e.g. `default.country`, `custom.level`, `computed.sessions`.
    pub prop: String,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
    NotIn,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    Matches,
    Exists,
    NotExists,
    SemverEq,
    SemverGt,
    SemverGte,
    SemverLt,
    SemverLte,
    /// Timestamp property is within the last `value` seconds.
    WithinLast,
    /// Timestamp property is older than `value` seconds.
    OlderThan,
}

/// Evaluation context: flattened properties plus the subject id (available as `identity.id`
/// and `identity.external_id`).
pub struct Context<'a> {
    pub props: &'a PropertyMap,
}

impl Condition {
    /// Always-true condition.
    pub fn always() -> Self {
        Condition::All { all: vec![] }
    }

    pub fn matches(&self, ctx: &Context<'_>) -> bool {
        match self {
            Condition::All { all } => all.iter().all(|c| c.matches(ctx)),
            Condition::Any { any } => any.iter().any(|c| c.matches(ctx)),
            Condition::Not { not } => !not.matches(ctx),
            Condition::Cmp(c) => c.matches(ctx.props.get(&c.prop)),
        }
    }

    /// Reject malformed rules early (bad regexes, missing values, absurd nesting).
    pub fn validate(&self) -> Result<()> {
        self.validate_depth(0)
    }

    fn validate_depth(&self, depth: usize) -> Result<()> {
        if depth > 16 {
            return Err(Error::invalid("condition nesting too deep"));
        }
        match self {
            Condition::All { all } => all.iter().try_for_each(|c| c.validate_depth(depth + 1)),
            Condition::Any { any } => any.iter().try_for_each(|c| c.validate_depth(depth + 1)),
            Condition::Not { not } => not.validate_depth(depth + 1),
            Condition::Cmp(c) => c.validate(),
        }
    }
}

impl Comparison {
    fn validate(&self) -> Result<()> {
        if self.prop.is_empty() || self.prop.len() > 256 {
            return Err(Error::invalid("condition prop must be 1..256 chars"));
        }
        match self.op {
            Op::Exists | Op::NotExists => Ok(()),
            Op::In | Op::NotIn => {
                if !self.value.is_array() {
                    return Err(Error::invalid(format!(
                        "{}: `in` needs an array value",
                        self.prop
                    )));
                }
                Ok(())
            }
            Op::Matches => {
                let s = self.value.as_str().ok_or_else(|| {
                    Error::invalid(format!("{}: `matches` needs a string pattern", self.prop))
                })?;
                if s.len() > 512 {
                    return Err(Error::invalid("regex too long"));
                }
                regex::Regex::new(s)
                    .map_err(|e| Error::invalid(format!("{}: invalid regex: {e}", self.prop)))?;
                Ok(())
            }
            Op::WithinLast | Op::OlderThan => {
                if !self.value.as_f64().is_some_and(|v| v >= 0.0) {
                    return Err(Error::invalid(format!(
                        "{}: time ops need a non-negative number of seconds",
                        self.prop
                    )));
                }
                Ok(())
            }
            Op::SemverEq | Op::SemverGt | Op::SemverGte | Op::SemverLt | Op::SemverLte => {
                let s = self.value.as_str().ok_or_else(|| {
                    Error::invalid(format!("{}: semver ops need a string value", self.prop))
                })?;
                parse_semver(s).ok_or_else(|| {
                    Error::invalid(format!("{}: invalid semver {s:?}", self.prop))
                })?;
                Ok(())
            }
            _ => {
                if self.value.is_null() {
                    return Err(Error::invalid(format!("{}: value required", self.prop)));
                }
                Ok(())
            }
        }
    }

    pub fn matches(&self, actual: Option<&Value>) -> bool {
        match self.op {
            Op::Exists => return actual.is_some_and(|v| !v.is_null()),
            Op::NotExists => return !actual.is_some_and(|v| !v.is_null()),
            _ => {}
        }
        let Some(actual) = actual else {
            return matches!(self.op, Op::Neq | Op::NotIn | Op::NotContains);
        };
        match self.op {
            Op::Eq => loose_eq(actual, &self.value),
            Op::Neq => !loose_eq(actual, &self.value),
            Op::Gt => cmp(actual, &self.value).is_some_and(|o| o.is_gt()),
            Op::Gte => cmp(actual, &self.value).is_some_and(|o| o.is_ge()),
            Op::Lt => cmp(actual, &self.value).is_some_and(|o| o.is_lt()),
            Op::Lte => cmp(actual, &self.value).is_some_and(|o| o.is_le()),
            Op::In => self
                .value
                .as_array()
                .is_some_and(|arr| arr.iter().any(|v| loose_eq(actual, v))),
            Op::NotIn => !self
                .value
                .as_array()
                .is_some_and(|arr| arr.iter().any(|v| loose_eq(actual, v))),
            Op::Contains => contains(actual, &self.value),
            Op::NotContains => !contains(actual, &self.value),
            Op::StartsWith => str_pair(actual, &self.value).is_some_and(|(a, b)| a.starts_with(b)),
            Op::EndsWith => str_pair(actual, &self.value).is_some_and(|(a, b)| a.ends_with(b)),
            Op::Matches => str_pair(actual, &self.value).is_some_and(|(a, pat)| {
                regex::Regex::new(pat)
                    .map(|re| re.is_match(a))
                    .unwrap_or(false)
            }),
            Op::SemverEq => semver_cmp(actual, &self.value).is_some_and(|o| o.is_eq()),
            Op::SemverGt => semver_cmp(actual, &self.value).is_some_and(|o| o.is_gt()),
            Op::SemverGte => semver_cmp(actual, &self.value).is_some_and(|o| o.is_ge()),
            Op::SemverLt => semver_cmp(actual, &self.value).is_some_and(|o| o.is_lt()),
            Op::SemverLte => semver_cmp(actual, &self.value).is_some_and(|o| o.is_le()),
            Op::WithinLast => age_secs(actual).is_some_and(|age| {
                self.value
                    .as_f64()
                    .is_some_and(|secs| age >= 0.0 && age <= secs)
            }),
            Op::OlderThan => age_secs(actual)
                .is_some_and(|age| self.value.as_f64().is_some_and(|secs| age > secs)),
            Op::Exists | Op::NotExists => unreachable!(),
        }
    }
}

/// Seconds elapsed since a timestamp property (RFC 3339 string or unix seconds / millis).
fn age_secs(actual: &Value) -> Option<f64> {
    let now = chrono::Utc::now();
    let ts = match actual {
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&chrono::Utc)),
        Value::Number(n) => {
            let v = n.as_f64()?;
            let secs = if v > 1e11 { v / 1000.0 } else { v };
            chrono::DateTime::from_timestamp(secs as i64, 0)
        }
        _ => None,
    }?;
    Some((now - ts).num_milliseconds() as f64 / 1000.0)
}

/// Equality that tolerates `"5" == 5` and `"true" == true`, which is what JSON-ish clients send.
fn loose_eq(a: &Value, b: &Value) -> bool {
    if a == b {
        return true;
    }
    match (a, b) {
        (Value::String(s), Value::Number(n)) | (Value::Number(n), Value::String(s)) => {
            s.parse::<f64>().ok() == n.as_f64()
        }
        (Value::String(s), Value::Bool(x)) | (Value::Bool(x), Value::String(s)) => {
            s.parse::<bool>().ok() == Some(*x)
        }
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => false,
    }
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::String(x), Value::String(y)) if as_f64(a).is_none() || as_f64(b).is_none() => {
            Some(x.cmp(y))
        }
        _ => as_f64(a)?.partial_cmp(&as_f64(b)?),
    }
}

fn contains(actual: &Value, needle: &Value) -> bool {
    match actual {
        Value::String(s) => needle.as_str().is_some_and(|n| s.contains(n)),
        Value::Array(arr) => arr.iter().any(|v| loose_eq(v, needle)),
        Value::Object(m) => needle.as_str().is_some_and(|n| m.contains_key(n)),
        _ => false,
    }
}

fn str_pair<'a>(a: &'a Value, b: &'a Value) -> Option<(&'a str, &'a str)> {
    Some((a.as_str()?, b.as_str()?))
}

fn parse_semver(s: &str) -> Option<semver::Version> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    if let Ok(v) = semver::Version::parse(s) {
        return Some(v);
    }
    // Tolerate "1.2" and "1" by padding missing components.
    let mut parts: Vec<&str> = s.split('.').collect();
    while parts.len() < 3 {
        parts.push("0");
    }
    semver::Version::parse(&parts.join(".")).ok()
}

fn semver_cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    let (a, b) = str_pair(a, b)?;
    Some(parse_semver(a)?.cmp(&parse_semver(b)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn props() -> PropertyMap {
        let mut m = PropertyMap::new();
        m.insert("default.country".into(), json!("DE"));
        m.insert("default.app_version".into(), json!("1.4.2"));
        m.insert("custom.level".into(), json!(12));
        m.insert("custom.tags".into(), json!(["beta", "vip"]));
        m.insert("custom.beta".into(), json!("true"));
        m
    }

    fn eval(cond: Value) -> bool {
        let c: Condition = serde_json::from_value(cond).unwrap();
        c.validate().unwrap();
        c.matches(&Context { props: &props() })
    }

    #[test]
    fn comparisons() {
        assert!(eval(
            json!({"prop":"default.country","op":"eq","value":"DE"})
        ));
        assert!(eval(
            json!({"prop":"default.country","op":"in","value":["AT","DE"]})
        ));
        assert!(!eval(
            json!({"prop":"default.country","op":"not_in","value":["AT","DE"]})
        ));
        assert!(eval(json!({"prop":"custom.level","op":"gte","value":12})));
        assert!(!eval(json!({"prop":"custom.level","op":"gt","value":12})));
        assert!(eval(json!({"prop":"custom.level","op":"lt","value":"20"})));
        assert!(eval(
            json!({"prop":"custom.tags","op":"contains","value":"vip"})
        ));
        assert!(eval(json!({"prop":"custom.beta","op":"eq","value":true})));
        assert!(eval(
            json!({"prop":"default.country","op":"matches","value":"^D"})
        ));
        assert!(eval(
            json!({"prop":"default.country","op":"starts_with","value":"D"})
        ));
        assert!(eval(json!({"prop":"custom.missing","op":"not_exists"})));
        assert!(eval(json!({"prop":"custom.missing","op":"neq","value":1})));
        assert!(!eval(json!({"prop":"custom.missing","op":"eq","value":1})));
    }

    #[test]
    fn semver() {
        assert!(eval(
            json!({"prop":"default.app_version","op":"semver_gte","value":"1.4"})
        ));
        assert!(eval(
            json!({"prop":"default.app_version","op":"semver_lt","value":"v2.0.0"})
        ));
        assert!(!eval(
            json!({"prop":"default.app_version","op":"semver_gt","value":"1.4.2"})
        ));
        assert!(eval(
            json!({"prop":"default.app_version","op":"semver_eq","value":"1.4.2"})
        ));
    }

    #[test]
    fn boolean_composition() {
        assert!(eval(
            json!({"all":[{"prop":"default.country","op":"eq","value":"DE"},{"any":[{"prop":"custom.level","op":"gt","value":100},{"prop":"custom.tags","op":"contains","value":"beta"}]},{"not":{"prop":"custom.banned","op":"exists"}}]})
        ));
        assert!(eval(json!({"all":[]})));
        assert!(!eval(json!({"any":[]})));
    }

    #[test]
    fn validation() {
        let bad: Condition =
            serde_json::from_value(json!({"prop":"x","op":"matches","value":"("})).unwrap();
        assert!(bad.validate().is_err());
        let bad: Condition =
            serde_json::from_value(json!({"prop":"x","op":"in","value":"DE"})).unwrap();
        assert!(bad.validate().is_err());
        let bad: Condition =
            serde_json::from_value(json!({"prop":"x","op":"semver_gt","value":"abc"})).unwrap();
        assert!(bad.validate().is_err());
    }
}
