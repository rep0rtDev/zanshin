//! Identity properties: three namespaces (default / computed / custom) of JSON values.

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{Error, Result};

pub type PropertyMap = Map<String, Value>;

/// Maximum number of keys per namespace.
pub const MAX_KEYS: usize = 256;
/// Maximum serialized size of a single value in bytes.
pub const MAX_VALUE_BYTES: usize = 4096;

static KEY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_.\-]{0,127}$").unwrap());

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PropertySet {
    #[serde(default)]
    pub default: PropertyMap,
    #[serde(default)]
    pub computed: PropertyMap,
    #[serde(default)]
    pub custom: PropertyMap,
}

impl PropertySet {
    /// Flattened view used by targeting rules: `default.*`, `computed.*`, `custom.*`.
    pub fn flatten(&self) -> PropertyMap {
        let mut out = PropertyMap::new();
        for (ns, map) in [
            ("default", &self.default),
            ("computed", &self.computed),
            ("custom", &self.custom),
        ] {
            for (k, v) in map {
                out.insert(format!("{ns}.{k}"), v.clone());
            }
        }
        out
    }
}

/// Validate keys and value sizes of a property map supplied by a client.
pub fn validate(map: &PropertyMap) -> Result<()> {
    if map.len() > MAX_KEYS {
        return Err(Error::invalid(format!(
            "too many properties (max {MAX_KEYS})"
        )));
    }
    for (k, v) in map {
        if !KEY_RE.is_match(k) {
            return Err(Error::invalid(format!("invalid property key {k:?}")));
        }
        if matches!(v, Value::Object(_) | Value::Array(_)) {
            let size = serde_json::to_vec(v).map(|b| b.len()).unwrap_or(usize::MAX);
            if size > MAX_VALUE_BYTES {
                return Err(Error::invalid(format!(
                    "property {k:?} value too large (max {MAX_VALUE_BYTES} bytes)"
                )));
            }
        } else if let Value::String(s) = v {
            if s.len() > MAX_VALUE_BYTES {
                return Err(Error::invalid(format!(
                    "property {k:?} value too large (max {MAX_VALUE_BYTES} bytes)"
                )));
            }
        }
    }
    Ok(())
}

/// Merge `patch` into `base`. A JSON `null` removes the key.
pub fn merge(base: &mut PropertyMap, patch: PropertyMap) {
    for (k, v) in patch {
        if v.is_null() {
            base.remove(&k);
        } else {
            base.insert(k, v);
        }
    }
    if base.len() > MAX_KEYS {
        // Keep the newest keys deterministic: drop from the front.
        while base.len() > MAX_KEYS {
            let first = base.keys().next().cloned();
            match first {
                Some(k) => {
                    base.remove(&k);
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_removes_on_null() {
        let mut base = PropertyMap::new();
        base.insert("a".into(), json!(1));
        base.insert("b".into(), json!(2));
        let mut patch = PropertyMap::new();
        patch.insert("a".into(), Value::Null);
        patch.insert("c".into(), json!("x"));
        merge(&mut base, patch);
        assert_eq!(base.get("a"), None);
        assert_eq!(base.get("b"), Some(&json!(2)));
        assert_eq!(base.get("c"), Some(&json!("x")));
    }

    #[test]
    fn validate_rejects_bad_keys() {
        let mut m = PropertyMap::new();
        m.insert("1bad".into(), json!(1));
        assert!(validate(&m).is_err());
        let mut m = PropertyMap::new();
        m.insert("good_key.sub-1".into(), json!(1));
        assert!(validate(&m).is_ok());
    }

    #[test]
    fn flatten_namespaces() {
        let mut p = PropertySet::default();
        p.default.insert("country".into(), json!("DE"));
        p.custom.insert("vip".into(), json!(true));
        let f = p.flatten();
        assert_eq!(f.get("default.country"), Some(&json!("DE")));
        assert_eq!(f.get("custom.vip"), Some(&json!(true)));
    }
}
