//! Analytics v2 models: custom dashboards (widgets) and chart annotations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{flags::validate_key, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetKind {
    /// Daily series of one event (`config.event`, `config.mode = count|identities|sum`).
    Timeseries,
    /// One number: `config.metric = dau|wau|mau|new_users|sessions|revenue|arpdau|arppu|paying_share|event_count`
    /// (+ `config.event` for `event_count`).
    Number,
    /// Funnel over `config.steps` within `config.window_secs`.
    Funnel,
    /// Retention cohorts for `config.days` (e.g. `[1, 7, 30]`).
    Retention,
    /// Most frequent events (`config.limit`).
    TopEvents,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Widget {
    pub id: String,
    pub kind: WidgetKind,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub config: Value,
    /// Grid width in columns (1–4).
    #[serde(default = "default_width")]
    pub width: u8,
}

fn default_width() -> u8 {
    2
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dashboard {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub widgets: Vec<Widget>,
    /// Default filters applied to every widget: `{ "platform": "ios", "country": "DE", "app_version": "1.2" }`.
    #[serde(default)]
    pub filters: Value,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub const MAX_WIDGETS: usize = 40;

impl Dashboard {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        if self.widgets.len() > MAX_WIDGETS {
            return Err(Error::invalid(format!("at most {MAX_WIDGETS} widgets")));
        }
        let mut ids = std::collections::HashSet::new();
        for w in &self.widgets {
            if w.id.is_empty() || w.id.len() > 64 || !ids.insert(w.id.as_str()) {
                return Err(Error::invalid(format!(
                    "bad or duplicate widget id {:?}",
                    w.id
                )));
            }
            if !(1..=4).contains(&w.width) {
                return Err(Error::invalid(format!(
                    "widget {:?}: width must be 1..=4",
                    w.id
                )));
            }
            if !w.config.is_object() && !w.config.is_null() {
                return Err(Error::invalid(format!(
                    "widget {:?}: config must be an object",
                    w.id
                )));
            }
            match w.kind {
                WidgetKind::Timeseries if w.config["event"].as_str().is_none_or(str::is_empty) => {
                    return Err(Error::invalid(format!(
                        "widget {:?}: config.event required",
                        w.id
                    )))
                }
                WidgetKind::Number if w.config["metric"].as_str().is_none_or(str::is_empty) => {
                    return Err(Error::invalid(format!(
                        "widget {:?}: config.metric required",
                        w.id
                    )))
                }
                WidgetKind::Funnel if w.config["steps"].as_array().is_none_or(|s| s.is_empty()) => {
                    return Err(Error::invalid(format!(
                        "widget {:?}: config.steps required",
                        w.id
                    )))
                }
                _ => {}
            }
        }
        if !self.filters.is_object() && !self.filters.is_null() {
            return Err(Error::invalid("filters must be an object"));
        }
        Ok(())
    }
}

/// A marker on time charts: "flag X changed", "live event Y started", or an operator note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: Uuid,
    pub at: DateTime<Utc>,
    pub kind: String,
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// One funnel step: an event name, optionally filtered on a metadata field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunnelStep {
    pub name: String,
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub op: Option<String>,
    #[serde(default)]
    pub value: Value,
}

impl FunnelStep {
    pub fn matches(&self, metadata: &Value) -> bool {
        let Some(field) = self.field.as_deref().filter(|f| !f.is_empty()) else {
            return true;
        };
        let actual = metadata.get(field).unwrap_or(&Value::Null);
        match self.op.as_deref().unwrap_or("eq") {
            "eq" => loose_eq(actual, &self.value),
            "neq" => !loose_eq(actual, &self.value),
            "exists" => !actual.is_null(),
            "gt" | "gte" | "lt" | "lte" => {
                let (Some(a), Some(b)) = (num(actual), num(&self.value)) else {
                    return false;
                };
                match self.op.as_deref().unwrap_or_default() {
                    "gt" => a > b,
                    "gte" => a >= b,
                    "lt" => a < b,
                    _ => a <= b,
                }
            }
            "contains" => match (actual, &self.value) {
                (Value::String(a), Value::String(b)) => a.contains(b.as_str()),
                (Value::Array(a), b) => a.iter().any(|x| loose_eq(x, b)),
                _ => false,
            },
            _ => false,
        }
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn loose_eq(a: &Value, b: &Value) -> bool {
    if a == b {
        return true;
    }
    match (num(a), num(b)) {
        (Some(x), Some(y)) => x == y,
        _ => {
            matches!((a, b), (Value::String(x), Value::Bool(y)) if x.parse::<bool>().ok() == Some(*y))
        }
    }
}

/// Fold per-identity event rows (sorted by time) into funnel step counts with a conversion window.
/// `rows` are `(identity, step_index, time)`; an identity advances to step `i` only after step `i-1`
/// and within `window_secs` of entering the funnel.
/// How funnel steps must relate in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FunnelModel {
    /// Steps in order, any time apart (within the queried range).
    Unlimited,
    /// All steps within `window_secs` of the first step (default).
    #[default]
    Window,
    /// Each step within `window_secs` of the previous one.
    Step,
    /// All steps inside one play session.
    Session,
}

/// Funnel fold with a time model. `rows` = (identity, step index, time, session); per identity
/// the steps must be reached in order, each respecting the model's constraint. Returns how many
/// identities reached each step.
/// How conversions are counted: once per player (deepest chain) or once per entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FunnelCount {
    #[default]
    Unique,
    Total,
}

/// `static`: the cohort is players who entered (step 1) inside the range. `shifting`: players
/// who entered up to one window before the range also count, as long as they reached a step
/// inside the range — conversions "shift" into the period they happened in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FunnelCohort {
    #[default]
    Static,
    Shifting,
}

pub fn fold_funnel_model(
    rows: Vec<(Uuid, usize, DateTime<Utc>, Option<Uuid>)>,
    steps: usize,
    window_secs: i64,
    model: FunnelModel,
) -> Vec<i64> {
    fold_funnel_v2(
        rows,
        steps,
        window_secs,
        model,
        FunnelCount::Unique,
        None,
    )
}

/// Funnel fold with count mode and an optional range start for shifting cohorts: a chain whose
/// entry lies before `range_start` counts only if one of its reached steps is at/after it.
pub fn fold_funnel_v2(
    mut rows: Vec<(Uuid, usize, DateTime<Utc>, Option<Uuid>)>,
    steps: usize,
    window_secs: i64,
    model: FunnelModel,
    count: FunnelCount,
    range_start: Option<DateTime<Utc>>,
) -> Vec<i64> {
    // Same-timestamp events (one batch) are ordered by step so they can still chain.
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)).then(a.1.cmp(&b.1)));
    let mut counts = vec![0i64; steps];
    let mut i = 0;
    while i < rows.len() {
        let id = rows[i].0;
        let mut j = i;
        while j < rows.len() && rows[j].0 == id {
            j += 1;
        }
        let mine = &rows[i..j];
        // Try every occurrence of step 0 as an entry point; keep the deepest progress.
        let mut best = 0usize;
        for (k, &(_, step, entered, session)) in mine.iter().enumerate() {
            if step != 0 {
                continue;
            }
            let mut reached = 1usize;
            let mut prev = entered;
            let mut last = entered;
            for &(_, s, at, sess) in &mine[k + 1..] {
                if s != reached {
                    continue;
                }
                let ok = match model {
                    FunnelModel::Unlimited => true,
                    FunnelModel::Window => (at - entered).num_seconds() <= window_secs,
                    FunnelModel::Step => (at - prev).num_seconds() <= window_secs,
                    FunnelModel::Session => session.is_some() && sess == session,
                };
                if ok {
                    reached += 1;
                    prev = at;
                    last = at;
                    if reached == steps {
                        break;
                    }
                } else if model == FunnelModel::Window && (at - entered).num_seconds() > window_secs
                {
                    break;
                }
            }
            if range_start.is_some_and(|rs| last < rs) {
                continue;
            }
            match count {
                FunnelCount::Total => {
                    for c in counts.iter_mut().take(reached) {
                        *c += 1;
                    }
                }
                FunnelCount::Unique => {
                    best = best.max(reached);
                    if best == steps {
                        break;
                    }
                }
            }
        }
        if count == FunnelCount::Unique {
            for c in counts.iter_mut().take(best) {
                *c += 1;
            }
        }
        i = j;
    }
    counts
}

pub fn fold_funnel(
    mut rows: Vec<(Uuid, usize, DateTime<Utc>)>,
    steps: usize,
    window_secs: i64,
) -> Vec<i64> {
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
    let mut counts = vec![0i64; steps];
    let mut i = 0;
    while i < rows.len() {
        let id = rows[i].0;
        let mut reached = 0usize;
        let mut entered: Option<DateTime<Utc>> = None;
        let mut j = i;
        while j < rows.len() && rows[j].0 == id {
            let (_, step, at) = rows[j];
            if step == reached {
                let within = entered.is_none_or(|e| (at - e).num_seconds() <= window_secs);
                if within {
                    if reached == 0 {
                        entered = Some(at);
                    }
                    reached += 1;
                    if reached == steps {
                        break;
                    }
                }
            }
            j += 1;
        }
        for c in counts.iter_mut().take(reached) {
            *c += 1;
        }
        while i < rows.len() && rows[i].0 == id {
            i += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn funnel_fold_respects_order_and_window() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        let t0 = Utc::now();
        let rows = vec![
            (a, 0, t0),
            (a, 1, t0 + chrono::Duration::seconds(10)),
            (a, 2, t0 + chrono::Duration::seconds(20)),
            (b, 1, t0), // step 2 before step 1 does not count
            (b, 0, t0 + chrono::Duration::seconds(5)),
            (b, 1, t0 + chrono::Duration::seconds(5000)), // outside window
        ];
        assert_eq!(fold_funnel(rows, 3, 3600), vec![2, 1, 1]);
    }

    #[test]
    fn step_filters() {
        let s = FunnelStep {
            name: "buy".into(),
            field: Some("sku".into()),
            op: Some("eq".into()),
            value: Value::String("gold".into()),
        };
        assert!(s.matches(&serde_json::json!({"sku": "gold"})));
        assert!(!s.matches(&serde_json::json!({"sku": "gems"})));
        let s = FunnelStep {
            name: "buy".into(),
            field: Some("usd".into()),
            op: Some("gte".into()),
            value: serde_json::json!(5),
        };
        assert!(s.matches(&serde_json::json!({"usd": "9.99"})));
    }
}

/// Dimension filter applied to event-based analytics. All fields are optional and AND-ed;
/// `experiment` + `variant` select events recorded while the identity was in that variant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Include events flagged `metadata.test = true` (excluded by default).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_test: bool,
}

impl EventFilter {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        for (k, v) in [
            ("country", &self.country),
            ("platform", &self.platform),
            ("app_version", &self.app_version),
            ("experiment", &self.experiment),
            ("variant", &self.variant),
        ] {
            if let Some(v) = v {
                if v.is_empty() || v.len() > 128 {
                    return Err(format!("filter {k} must be 1..128 characters"));
                }
            }
        }
        if self.variant.is_some() && self.experiment.is_none() {
            return Err("filter variant requires experiment".into());
        }
        Ok(())
    }
}
