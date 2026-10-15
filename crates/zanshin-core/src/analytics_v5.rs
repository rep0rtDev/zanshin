//! Analytics v5: read-only SQL explorer guard, saved queries, scheduled reports, metric alerts
//! and path analysis folding. Pure data + logic; storage and delivery live in the server.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{flags::validate_key, Error, Result};

pub const MAX_SQL_LEN: usize = 20_000;
pub const MAX_SQL_ROWS: i64 = 10_000;

/// Accepts a single `SELECT` / `WITH … SELECT` statement and rejects anything that could write,
/// change settings or read files. Returns the trimmed statement without a trailing `;`.
pub fn validate_readonly_sql(sql: &str) -> Result<String> {
    let s = sql.trim().trim_end_matches(';').trim();
    if s.is_empty() {
        return Err(Error::invalid("sql is empty"));
    }
    if s.len() > MAX_SQL_LEN {
        return Err(Error::invalid(format!(
            "sql longer than {MAX_SQL_LEN} chars"
        )));
    }
    if s.contains(';') {
        return Err(Error::invalid("only a single statement is allowed"));
    }
    if s.contains("--") || s.contains("/*") || s.contains("#!") {
        return Err(Error::invalid("comments are not allowed"));
    }
    let lower = s.to_ascii_lowercase();
    let first = lower.split_whitespace().next().unwrap_or("");
    if first != "select"
        && first != "with"
        && first != "show"
        && first != "describe"
        && first != "explain"
    {
        return Err(Error::invalid(
            "only SELECT / WITH / SHOW / DESCRIBE / EXPLAIN are allowed",
        ));
    }
    const BANNED: &[&str] = &[
        "insert",
        "update",
        "delete",
        "alter",
        "drop",
        "truncate",
        "create",
        "attach",
        "detach",
        "rename",
        "optimize",
        "system",
        "grant",
        "revoke",
        "kill",
        "set",
        "use",
        "exchange",
        "move",
        "outfile",
        "infile",
        "url(",
        "file(",
        "s3(",
        "remote(",
        "remotesecure(",
        "mysql(",
        "postgresql(",
        "odbc(",
        "jdbc(",
        "hdfs(",
        "input(",
        "executable(",
        "format(",
        "settings",
    ];
    let words: Vec<&str> = lower
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '('))
        .filter(|w| !w.is_empty())
        .collect();
    for w in &words {
        let w = w.trim_start_matches('(');
        for b in BANNED {
            let hit = if b.ends_with('(') {
                w.starts_with(b) || w == b.trim_end_matches('(')
            } else {
                w == *b
            };
            if hit {
                return Err(Error::invalid(format!(
                    "`{}` is not allowed in the explorer",
                    b.trim_end_matches('(')
                )));
            }
        }
    }
    Ok(s.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedQuery {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub sql: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SavedQuery {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        if self.name.len() > 128 || self.description.len() > 2000 {
            return Err(Error::invalid("name / description too long"));
        }
        validate_readonly_sql(&self.sql).map(|_| ())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReportEmail {
    /// Key of an `smtp` / `ses` / `sendgrid` / `resend` integration.
    pub integration: String,
    pub to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportSchedule {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Saved query to run; its first rows (up to `max_rows`) go into the report.
    pub query_key: String,
    pub cron: String,
    #[serde(default = "default_tz")]
    pub timezone: String,
    /// Notification channel keys (Slack / Discord / webhook). Empty = every channel subscribed to `report`.
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub email: Option<ReportEmail>,
    #[serde(default = "default_rows")]
    pub max_rows: i64,
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_status: String,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}
fn default_tz() -> String {
    "UTC".into()
}
fn default_rows() -> i64 {
    20
}

impl ReportSchedule {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        validate_key(&self.query_key)?;
        self.cron
            .parse::<cron::Schedule>()
            .map_err(|e| Error::invalid(format!("cron: {e}")))?;
        self.timezone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| Error::invalid(format!("unknown timezone {:?}", self.timezone)))?;
        if !(1..=200).contains(&self.max_rows) {
            return Err(Error::invalid("max_rows must be 1..=200"));
        }
        if let Some(e) = &self.email {
            validate_key(&e.integration)?;
            if e.to.is_empty() || e.to.len() > 20 || e.to.iter().any(|t| !t.contains('@')) {
                return Err(Error::invalid("email.to must be 1..=20 addresses"));
            }
        }
        Ok(())
    }

    /// Next fire time strictly after `after`, in the schedule's timezone.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let tz: chrono_tz::Tz = self.timezone.parse().ok()?;
        let s = self.cron.parse::<cron::Schedule>().ok()?;
        s.after(&after.with_timezone(&tz))
            .next()
            .map(|t| t.with_timezone(&Utc))
    }

    /// Due when the last cron slot before `now` is later than `last_run_at` (or never ran).
    pub fn due(&self, now: DateTime<Utc>) -> bool {
        if !self.enabled {
            return false;
        }
        let since = self.last_run_at.unwrap_or(self.created_at).min(now);
        self.next_after(since).is_some_and(|t| t <= now)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertAggregation {
    #[default]
    Count,
    Unique,
    Sum,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AlertCondition {
    Above {
        threshold: f64,
    },
    Below {
        threshold: f64,
    },
    /// Current window deviates from the mean of the previous `baseline_windows` windows by more
    /// than `zscore` standard deviations (and by at least `min_delta`).
    Anomaly {
        #[serde(default = "default_z")]
        zscore: f64,
        #[serde(default = "default_bw")]
        baseline_windows: u32,
        #[serde(default)]
        min_delta: f64,
    },
}

fn default_z() -> f64 {
    3.0
}
fn default_bw() -> u32 {
    7
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricAlert {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub event: String,
    #[serde(default)]
    pub aggregation: AlertAggregation,
    #[serde(default)]
    pub field: String,
    pub condition: AlertCondition,
    /// Length of the evaluated window in seconds (ends now).
    pub window_secs: i64,
    #[serde(default = "default_cooldown")]
    pub cooldown_secs: i64,
    #[serde(default)]
    pub last_value: Option<f64>,
    #[serde(default)]
    pub last_checked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_fired_at: Option<DateTime<Utc>>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_cooldown() -> i64 {
    3600
}

impl MetricAlert {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        if self.event.is_empty() || self.event.len() > 128 {
            return Err(Error::invalid("event is required"));
        }
        if self.aggregation == AlertAggregation::Sum && self.field.is_empty() {
            return Err(Error::invalid("sum needs a metadata field"));
        }
        if !(60..=30 * 86_400).contains(&self.window_secs) {
            return Err(Error::invalid("window_secs must be 60..=2592000"));
        }
        if !(0..=30 * 86_400).contains(&self.cooldown_secs) {
            return Err(Error::invalid("cooldown_secs must be 0..=2592000"));
        }
        match &self.condition {
            AlertCondition::Above { threshold } | AlertCondition::Below { threshold } => {
                if !threshold.is_finite() {
                    return Err(Error::invalid("threshold must be finite"));
                }
            }
            AlertCondition::Anomaly {
                zscore,
                baseline_windows,
                min_delta,
            } => {
                if !(*zscore > 0.0 && zscore.is_finite()) {
                    return Err(Error::invalid("zscore must be > 0"));
                }
                if !(3..=60).contains(baseline_windows) {
                    return Err(Error::invalid("baseline_windows must be 3..=60"));
                }
                if !(*min_delta >= 0.0 && min_delta.is_finite()) {
                    return Err(Error::invalid("min_delta must be >= 0"));
                }
            }
        }
        Ok(())
    }

    pub fn baseline_windows(&self) -> u32 {
        match &self.condition {
            AlertCondition::Anomaly {
                baseline_windows, ..
            } => *baseline_windows,
            _ => 0,
        }
    }

    pub fn in_cooldown(&self, now: DateTime<Utc>) -> bool {
        self.last_fired_at
            .is_some_and(|t| (now - t).num_seconds() < self.cooldown_secs)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AlertFire {
    pub value: f64,
    pub baseline: Option<f64>,
    pub deviation: Option<f64>,
    pub text: String,
}

/// Decides whether the alert fires given the current window value and the preceding windows
/// (oldest first) used as the anomaly baseline.
pub fn evaluate_alert(a: &MetricAlert, current: f64, history: &[f64]) -> Option<AlertFire> {
    match &a.condition {
        AlertCondition::Above { threshold } => (current > *threshold).then(|| AlertFire {
            value: current,
            baseline: None,
            deviation: None,
            text: format!("{} = {current:.2} is above {threshold:.2}", a.event),
        }),
        AlertCondition::Below { threshold } => (current < *threshold).then(|| AlertFire {
            value: current,
            baseline: None,
            deviation: None,
            text: format!("{} = {current:.2} is below {threshold:.2}", a.event),
        }),
        AlertCondition::Anomaly {
            zscore,
            baseline_windows,
            min_delta,
        } => {
            if history.len() < (*baseline_windows as usize).min(3) {
                return None;
            }
            let n = history.len() as f64;
            let mean = history.iter().sum::<f64>() / n;
            let var = history.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
            let sd = var.sqrt().max(mean.abs().sqrt().max(1.0) * 0.05);
            let z = (current - mean) / sd;
            if z.abs() >= *zscore && (current - mean).abs() >= *min_delta {
                Some(AlertFire {
                    value: current,
                    baseline: Some(mean),
                    deviation: Some(z),
                    text: format!(
                        "{} = {current:.2}, baseline {mean:.2} over {} windows (z = {z:+.1})",
                        a.event,
                        history.len()
                    ),
                })
            } else {
                None
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathDirection {
    #[default]
    After,
    Before,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PathReport {
    /// Anchor occurrences considered.
    pub anchors: i64,
    /// For every depth (1-based distance from the anchor) the top event names with counts;
    /// `$end` marks players with no further event.
    pub levels: Vec<Vec<(String, i64)>>,
    /// Most common full sequences (anchor excluded) with counts.
    pub sequences: Vec<(Vec<String>, i64)>,
}

/// Folds `(identity, name, unix_ms)` rows into the paths players take right after (or before)
/// `anchor`, up to `depth` steps; consecutive steps farther apart than `max_gap_secs` end the
/// path. One path per anchor occurrence; repeated anchors inside a path are kept as steps.
pub fn fold_paths(
    rows: Vec<(Uuid, String, i64)>,
    anchor: &str,
    direction: PathDirection,
    depth: usize,
    max_gap_secs: i64,
    top: usize,
) -> PathReport {
    let depth = depth.clamp(1, 10);
    let mut by_id: HashMap<Uuid, Vec<(String, i64)>> = HashMap::new();
    for (id, name, ts) in rows {
        by_id.entry(id).or_default().push((name, ts));
    }
    let mut levels: Vec<HashMap<String, i64>> = vec![HashMap::new(); depth];
    let mut seqs: HashMap<Vec<String>, i64> = HashMap::new();
    let mut anchors = 0i64;
    for evs in by_id.values_mut() {
        evs.sort_by_key(|(_, ts)| *ts);
        if direction == PathDirection::Before {
            evs.reverse();
        }
        let mut i = 0;
        while i < evs.len() {
            if evs[i].0 != anchor {
                i += 1;
                continue;
            }
            anchors += 1;
            let mut seq = Vec::with_capacity(depth);
            let mut prev_ts = evs[i].1;
            let mut j = i + 1;
            while seq.len() < depth {
                let Some((name, ts)) = evs.get(j) else {
                    seq.push("$end".to_string());
                    break;
                };
                if (ts - prev_ts).abs() > max_gap_secs * 1000 {
                    seq.push("$end".to_string());
                    break;
                }
                seq.push(name.clone());
                prev_ts = *ts;
                j += 1;
            }
            for (d, name) in seq.iter().enumerate() {
                *levels[d].entry(name.clone()).or_default() += 1;
            }
            *seqs.entry(seq).or_default() += 1;
            // continue scanning after this anchor; the next anchor starts its own path
            i += 1;
        }
    }
    let mut out = PathReport {
        anchors,
        ..Default::default()
    };
    for l in levels {
        let mut v: Vec<(String, i64)> = l.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.truncate(top);
        out.levels.push(v);
    }
    let mut v: Vec<(Vec<String>, i64)> = seqs.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(top);
    out.sequences = v;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_guard() {
        assert!(validate_readonly_sql("SELECT name, count() FROM events GROUP BY name;").is_ok());
        assert!(validate_readonly_sql("with t as (select 1) select * from t").is_ok());
        for bad in [
            "",
            "DROP TABLE events",
            "SELECT 1; DROP TABLE events",
            "select * from events -- x",
            "INSERT INTO events SELECT 1",
            "select * from url('http://x', CSV)",
            "select 1 into outfile '/tmp/x'",
            "select 1 settings max_threads = 1",
        ] {
            assert!(validate_readonly_sql(bad).is_err(), "{bad}");
        }
    }

    fn alert(cond: AlertCondition) -> MetricAlert {
        MetricAlert {
            id: Uuid::nil(),
            key: "a".into(),
            name: String::new(),
            enabled: true,
            event: "purchase".into(),
            aggregation: AlertAggregation::Count,
            field: String::new(),
            condition: cond,
            window_secs: 3600,
            cooldown_secs: 0,
            last_value: None,
            last_checked_at: None,
            last_fired_at: None,
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn thresholds_and_anomaly() {
        assert!(
            evaluate_alert(&alert(AlertCondition::Above { threshold: 10.0 }), 11.0, &[]).is_some()
        );
        assert!(
            evaluate_alert(&alert(AlertCondition::Above { threshold: 10.0 }), 10.0, &[]).is_none()
        );
        assert!(
            evaluate_alert(&alert(AlertCondition::Below { threshold: 10.0 }), 9.0, &[]).is_some()
        );
        let a = alert(AlertCondition::Anomaly {
            zscore: 3.0,
            baseline_windows: 7,
            min_delta: 0.0,
        });
        let hist = [100.0, 102.0, 98.0, 101.0, 99.0, 100.0, 103.0];
        assert!(evaluate_alert(&a, 101.0, &hist).is_none());
        let f = evaluate_alert(&a, 10.0, &hist).expect("drop fires");
        assert!(f.deviation.unwrap() < -3.0);
        assert!(
            evaluate_alert(&a, 10.0, &hist[..2]).is_none(),
            "too little history"
        );
    }

    #[test]
    fn paths_fold() {
        let p = Uuid::now_v7();
        let q = Uuid::now_v7();
        let rows = vec![
            (p, "start".into(), 0),
            (p, "shop".into(), 1000),
            (p, "buy".into(), 2000),
            (q, "start".into(), 0),
            (q, "shop".into(), 1000),
            (q, "quit".into(), 999_000_000), // beyond gap
        ];
        let r = fold_paths(rows, "start", PathDirection::After, 3, 3600, 10);
        assert_eq!(r.anchors, 2);
        assert_eq!(r.levels[0], vec![("shop".to_string(), 2)]);
        assert_eq!(r.levels[1].len(), 2);
        assert!(r
            .sequences
            .iter()
            .any(|(s, n)| s == &vec!["shop".to_string(), "$end".to_string()] && *n == 1));
        assert!(r.sequences.iter().any(|(s, n)| s
            == &vec!["shop".to_string(), "buy".to_string(), "$end".to_string()]
            && *n == 1));
    }

    #[test]
    fn report_due() {
        let now = Utc::now();
        let r = ReportSchedule {
            id: Uuid::nil(),
            key: "r".into(),
            name: String::new(),
            enabled: true,
            query_key: "q".into(),
            cron: "0 0 * * * *".into(),
            timezone: "UTC".into(),
            channels: vec![],
            email: None,
            max_rows: 20,
            last_run_at: Some(now - chrono::Duration::hours(2)),
            last_status: String::new(),
            version: 1,
            created_at: now - chrono::Duration::days(1),
            updated_at: now,
        };
        assert!(r.validate().is_ok());
        assert!(r.due(now));
        let fresh = ReportSchedule {
            last_run_at: Some(now),
            ..r.clone()
        };
        assert!(!fresh.due(now));
    }
}
