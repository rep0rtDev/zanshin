//! Metrics catalog: named measurements derived from events, attached to experiments as goal
//! (primary success) or monitor (guardrail) metrics.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{flags::validate_key, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    #[default]
    Goal,
    Monitor,
}

impl MetricKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MetricKind::Goal => "goal",
            MetricKind::Monitor => "monitor",
        }
    }
}

impl std::str::FromStr for MetricKind {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "goal" => Ok(MetricKind::Goal),
            "monitor" => Ok(MetricKind::Monitor),
            other => Err(Error::invalid(format!("unknown metric kind {other:?}"))),
        }
    }
}

/// How events are folded into one number per variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggregation {
    /// Number of events.
    #[default]
    Count,
    /// Distinct players that sent the event.
    Unique,
    /// Sum of the numeric field.
    Sum,
    /// Sum / count.
    Avg,
    /// Distinct players with the event / players assigned (0..1).
    Conversion,
}

impl Aggregation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Aggregation::Count => "count",
            Aggregation::Unique => "unique",
            Aggregation::Sum => "sum",
            Aggregation::Avg => "avg",
            Aggregation::Conversion => "conversion",
        }
    }

    /// Fold raw per-variant totals into the metric value.
    pub fn apply(&self, count: i64, identities: i64, sum: f64, assigned: i64) -> f64 {
        match self {
            Aggregation::Count => count as f64,
            Aggregation::Unique => identities as f64,
            Aggregation::Sum => sum,
            Aggregation::Avg => {
                if count == 0 {
                    0.0
                } else {
                    sum / count as f64
                }
            }
            Aggregation::Conversion => {
                if assigned == 0 {
                    0.0
                } else {
                    identities as f64 / assigned as f64
                }
            }
        }
    }
}

impl std::str::FromStr for Aggregation {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "count" => Ok(Aggregation::Count),
            "unique" => Ok(Aggregation::Unique),
            "sum" => Ok(Aggregation::Sum),
            "avg" => Ok(Aggregation::Avg),
            "conversion" => Ok(Aggregation::Conversion),
            other => Err(Error::invalid(format!("unknown aggregation {other:?}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kind: MetricKind,
    pub event_name: String,
    #[serde(default)]
    pub aggregation: Aggregation,
    /// Numeric source for sum/avg: empty or `value` = the event value, otherwise a metadata key.
    #[serde(default)]
    pub field: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Metric {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        if self.event_name.is_empty() || self.event_name.len() > 128 {
            return Err(Error::invalid("event_name must be 1..128 chars"));
        }
        if self.field.len() > 128 || self.field.contains(['\'', '"', '\\']) {
            return Err(Error::invalid("field must be a plain metadata key"));
        }
        if matches!(
            self.aggregation,
            Aggregation::Count | Aggregation::Unique | Aggregation::Conversion
        ) && !self.field.is_empty()
        {
            return Err(Error::invalid(format!(
                "field is only used with sum/avg, not {}",
                self.aggregation.as_str()
            )));
        }
        Ok(())
    }

    /// Metadata key to read the number from, if not the event value.
    pub fn metadata_field(&self) -> Option<&str> {
        match self.field.as_str() {
            "" | "value" => None,
            f => Some(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregations_fold() {
        assert_eq!(Aggregation::Avg.apply(4, 2, 10.0, 100), 2.5);
        assert_eq!(Aggregation::Conversion.apply(4, 25, 0.0, 100), 0.25);
        assert_eq!(Aggregation::Avg.apply(0, 0, 0.0, 0), 0.0);
        assert_eq!(Aggregation::Unique.apply(4, 3, 0.0, 0), 3.0);
    }
}
