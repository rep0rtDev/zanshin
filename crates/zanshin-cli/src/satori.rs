//! Best-effort mapper from Satori JSON (client API responses or console exports) to a Zanshin
//! config bundle. Satori has no public export format, so the mapper accepts the documented
//! client-API shapes and reports every field it could not translate instead of guessing.
use serde_json::{json, Map, Value};

pub struct Report {
    pub mapped: Vec<(String, String)>,
    pub skipped: Vec<(String, String, String)>,
    pub notes: Vec<String>,
}

fn arr<'a>(src: &'a Value, keys: &[&str]) -> Vec<&'a Value> {
    for k in keys {
        if let Some(a) = src.get(k).and_then(Value::as_array) {
            return a.iter().collect();
        }
    }
    if let Some(a) = src.as_array() {
        return a.iter().collect();
    }
    vec![]
}

fn key_of(v: &Value) -> Option<String> {
    v.get("name")
        .or_else(|| v.get("key"))
        .or_else(|| v.get("id"))
        .and_then(Value::as_str)
        .map(|s| {
            s.trim()
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|s| !s.is_empty())
}

/// Satori flag values are strings; `(value_type, default_value)` for Zanshin.
fn flag_value(raw: &Value, infer: bool) -> (&'static str, Value) {
    match raw {
        Value::Bool(b) => ("bool", json!(b)),
        Value::Number(n) => ("number", json!(n)),
        Value::Object(_) | Value::Array(_) => ("json", raw.clone()),
        Value::String(s) if infer => {
            let t = s.trim();
            if t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("false") {
                return ("bool", json!(t.eq_ignore_ascii_case("true")));
            }
            if let Ok(n) = t.parse::<f64>() {
                if n.is_finite() {
                    return ("number", json!(n));
                }
            }
            if (t.starts_with('{') || t.starts_with('['))
                && serde_json::from_str::<Value>(t).is_ok()
            {
                return ("json", serde_json::from_str(t).unwrap());
            }
            ("string", json!(s))
        }
        Value::String(s) => ("string", json!(s)),
        Value::Null => ("string", json!("")),
    }
}

fn ts(v: Option<&Value>) -> Option<String> {
    let v = v?;
    if let Some(s) = v.as_str() {
        if s.parse::<i64>().is_err() {
            return Some(s.to_string());
        }
    }
    let secs = v.as_i64().or_else(|| v.as_str()?.parse().ok())?;
    if secs <= 0 {
        return None;
    }
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.to_rfc3339())
}

pub fn map(src: &Value, infer_types: bool) -> (Value, Report) {
    let mut rep = Report {
        mapped: vec![],
        skipped: vec![],
        notes: vec![],
    };
    let mut flags = vec![];
    let mut live_events = vec![];
    let mut experiments = vec![];
    let mut audiences = vec![];

    for f in arr(src, &["flags", "feature_flags"]) {
        let Some(key) = key_of(f) else {
            rep.skipped
                .push(("flag".into(), "?".into(), "no name".into()));
            continue;
        };
        let (vt, dv) = flag_value(f.get("value").unwrap_or(&Value::Null), infer_types);
        let mut e = json!({ "key": key, "name": f.get("name").cloned().unwrap_or(json!(key)),
            "value_type": vt, "default_value": dv, "enabled": true });
        if let Some(r) = f.get("change_reason") {
            e["description"] = json!(format!(
                "Imported from Satori (change_reason: {})",
                r.as_str().unwrap_or("?")
            ));
        }
        for k in ["conditions", "variants", "overrides"] {
            if f.get(k).is_some() {
                rep.skipped.push((
                    "flag".into(),
                    key.clone(),
                    format!("{k}: review and recreate as rules/variants"),
                ));
            }
        }
        rep.mapped.push(("flag".into(), key.clone()));
        flags.push(e);
    }

    for le in arr(src, &["live_events", "liveEvents", "events"]) {
        let Some(key) = key_of(le) else {
            rep.skipped
                .push(("live_event".into(), "?".into(), "no name".into()));
            continue;
        };
        let starts = ts(le
            .get("active_start_time_sec")
            .or(le.get("start_time_sec"))
            .or(le.get("start_time")));
        let mut ends = ts(le
            .get("active_end_time_sec")
            .or(le.get("end_time_sec"))
            .or(le.get("end_time")));
        if ends.is_none() {
            if let (Some(s), Some(d)) = (&starts, le.get("duration_sec").and_then(Value::as_i64)) {
                if let Ok(st) = chrono::DateTime::parse_from_rfc3339(s) {
                    ends = Some((st + chrono::Duration::seconds(d)).to_rfc3339());
                }
            }
        }
        let payload = match le.get("value") {
            Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(json!({ "value": s })),
            Some(v) => v.clone(),
            None => json!({}),
        };
        let mut e = json!({ "key": key, "name": le.get("name").cloned().unwrap_or(json!(key)),
            "payload": payload, "enabled": true,
            "description": le.get("description").cloned().unwrap_or(json!("Imported from Satori")) });
        if let Some(s) = starts {
            e["starts_at"] = json!(s);
        }
        if let Some(en) = ends {
            e["ends_at"] = json!(en);
        }
        if let Some(cron) = le
            .get("reset_cron")
            .or(le.get("reset_cron_expr"))
            .and_then(Value::as_str)
        {
            e["schedule"] = json!({ "cron": cron, "duration_sec": le.get("duration_sec").cloned().unwrap_or(Value::Null) });
            rep.notes.push(format!("live_event {key}: recurring schedule `{cron}` — verify the timezone in the console"));
        }
        rep.mapped.push(("live_event".into(), key.clone()));
        live_events.push(e);
    }

    for ex in arr(src, &["experiments"]) {
        let Some(key) = key_of(ex) else {
            rep.skipped
                .push(("experiment".into(), "?".into(), "no name".into()));
            continue;
        };
        let variants = match ex.get("variants").and_then(Value::as_array) {
            Some(vs) => vs
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let vk = key_of(v).unwrap_or_else(|| format!("v{i}"));
                    json!({ "key": vk, "value": v.get("value").cloned().unwrap_or(Value::Null),
                        "weight": v.get("weight").or(v.get("percentage")).cloned().unwrap_or(json!(1)) })
                })
                .collect::<Vec<_>>(),
            None => {
                // Satori's client API only exposes the value of the variant the player is in.
                rep.skipped.push(("experiment".into(), key.clone(), "only the player's own variant value is in the export; control/other variants must be added by hand".into()));
                vec![
                    json!({ "key": "control", "value": Value::Null, "weight": 1 }),
                    json!({ "key": "variant", "value": ex.get("value").cloned().unwrap_or(Value::Null), "weight": 1 }),
                ]
            }
        };
        experiments.push(json!({ "key": key, "name": ex.get("name").cloned().unwrap_or(json!(key)),
            "status": "draft", "control": variants.first().and_then(|v| v["key"].as_str()).unwrap_or("control"),
            "variants": variants, "description": "Imported from Satori — review before starting" }));
        rep.mapped.push(("experiment".into(), key.clone()));
    }

    for a in arr(src, &["audiences"]) {
        let Some(key) = key_of(a) else {
            rep.skipped
                .push(("audience".into(), "?".into(), "no name".into()));
            continue;
        };
        let mut e = json!({ "key": key, "name": a.get("name").cloned().unwrap_or(json!(key)),
            "description": a.get("description").cloned().unwrap_or(json!("Imported from Satori")) });
        match a.get("filter").or(a.get("expression")).or(a.get("rule")) {
            Some(Value::String(expr)) => {
                e["expression"] = json!(translate_expression(expr));
                rep.notes.push(format!(
                    "audience {key}: expression translated best effort — check it in the editor"
                ));
            }
            Some(other) => {
                e["condition"] = other.clone();
                rep.skipped.push((
                    "audience".into(),
                    key.clone(),
                    "structured filter copied verbatim; may be rejected by the server".into(),
                ));
            }
            None => rep.skipped.push((
                "audience".into(),
                key.clone(),
                "no filter — imported as an empty audience".into(),
            )),
        }
        rep.mapped.push(("audience".into(), key.clone()));
        audiences.push(e);
    }

    for k in ["identities", "events", "messages", "schedules", "metrics"] {
        if src.get(k).is_some() {
            rep.skipped.push((k.into(), "*".into(), "not part of the config bundle (identities/events: use the server API / `events import`)".into()));
        }
    }

    let mut entities = Map::new();
    if !flags.is_empty() {
        entities.insert("flags".into(), json!(flags));
    }
    if !audiences.is_empty() {
        entities.insert("audiences".into(), json!(audiences));
    }
    if !experiments.is_empty() {
        entities.insert("experiments".into(), json!(experiments));
    }
    if !live_events.is_empty() {
        entities.insert("live_events".into(), json!(live_events));
    }
    (
        json!({ "format": "zanshin-config/1", "entities": entities }),
        rep,
    )
}

/// Satori expression dialect → Zanshin expression language (best effort, textual).
fn translate_expression(expr: &str) -> String {
    let mut s = expr.to_string();
    for (from, to) in [
        ("PropertiesCustom(", "custom."),
        ("PropertiesDefault(", "default."),
        ("PropertiesComputed(", "computed."),
        ("Properties(", "default."),
        ("&&", " and "),
        ("||", " or "),
        ("!(", "not ("),
    ] {
        s = s.replace(from, to);
    }
    // `custom."level", 0)` → `custom.level`
    let re = regex::Regex::new(r#"(custom|default|computed)\.\"([A-Za-z0-9_]+)\"(,\s*[^)]*)?\)"#)
        .unwrap();
    re.replace_all(&s, "$1.$2").to_string()
}

pub fn print_report(r: &Report) {
    eprintln!("mapped {} entities:", r.mapped.len());
    for (k, key) in &r.mapped {
        eprintln!("  + {k} {key}");
    }
    if !r.skipped.is_empty() {
        eprintln!("needs attention ({}):", r.skipped.len());
        for (k, key, why) in &r.skipped {
            eprintln!("  ! {k} {key}: {why}");
        }
    }
    for n in &r.notes {
        eprintln!("  · {n}");
    }
}

impl serde::Serialize for Report {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        json!({
            "mapped": self.mapped.iter().map(|(k, key)| json!({ "kind": k, "key": key })).collect::<Vec<_>>(),
            "skipped": self.skipped.iter().map(|(k, key, why)| json!({ "kind": k, "key": key, "reason": why })).collect::<Vec<_>>(),
            "notes": self.notes,
        })
        .serialize(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_client_api_shapes() {
        let src = json!({
            "flags": [
                { "name": "Shop V2", "value": "true", "change_reason": "experiment" },
                { "name": "price", "value": "4.99" },
                { "name": "cfg", "value": "{\"a\":1}" },
                { "value": "orphan" }
            ],
            "live_events": [
                { "name": "sale", "value": "{\"discount\":20}", "active_start_time_sec": 1700000000, "duration_sec": 3600, "reset_cron": "0 0 * * 6" }
            ],
            "experiments": [ { "name": "pricing", "value": "2.99" } ],
            "audiences": [ { "name": "whales", "filter": "PropertiesCustom(\"level\", 0) > 10 && Properties(\"country\") == \"US\"" } ],
            "identities": []
        });
        let (bundle, rep) = map(&src, true);
        let flags = bundle["entities"]["flags"].as_array().unwrap();
        assert_eq!(flags.len(), 3);
        assert_eq!(flags[0]["key"], "shop_v2");
        assert_eq!(flags[0]["value_type"], "bool");
        assert_eq!(flags[0]["default_value"], true);
        assert_eq!(flags[1]["value_type"], "number");
        assert_eq!(flags[2]["value_type"], "json");
        let le = &bundle["entities"]["live_events"][0];
        assert_eq!(le["payload"]["discount"], 20);
        assert_eq!(le["starts_at"], "2023-11-14T22:13:20+00:00");
        assert_eq!(le["ends_at"], "2023-11-14T23:13:20+00:00");
        assert_eq!(le["schedule"]["cron"], "0 0 * * 6");
        let ex = &bundle["entities"]["experiments"][0];
        assert_eq!(ex["status"], "draft");
        assert_eq!(ex["variants"].as_array().unwrap().len(), 2);
        assert_eq!(
            bundle["entities"]["audiences"][0]["expression"],
            "custom.level > 10  and  default.country == \"US\""
        );
        assert_eq!(rep.mapped.len(), 6);
        assert!(rep.skipped.iter().any(|(k, _, _)| k == "identities"));
        assert!(rep
            .skipped
            .iter()
            .any(|(k, key, _)| k == "flag" && key == "?"));
    }

    #[test]
    fn strings_stay_strings_without_infer() {
        let (bundle, _) = map(
            &json!({ "flags": [{ "name": "f", "value": "true" }] }),
            false,
        );
        assert_eq!(bundle["entities"]["flags"][0]["value_type"], "string");
    }
}
