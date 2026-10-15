//! Metrics catalog + evaluation of goal / monitor metrics per experiment variant.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use zanshin_core::{
    experiments::Experiment,
    flags::validate_key,
    metrics::{Aggregation, Metric, MetricKind},
    Error, Result,
};
use zanshin_store::{experiments::VariantStats, metrics::MetricRepo};

use super::{events::EventService, flags::normalize_tags};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MetricPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub kind: Option<MetricKind>,
    #[serde(default)]
    pub event_name: Option<String>,
    #[serde(default)]
    pub aggregation: Option<Aggregation>,
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct MetricService {
    repo: MetricRepo,
    events: EventService,
}

impl MetricService {
    pub fn new(repo: MetricRepo, events: EventService) -> Self {
        Self { repo, events }
    }

    pub async fn list(&self) -> Result<Vec<Metric>> {
        self.repo.list().await
    }

    pub async fn get(&self, key: &str) -> Result<Metric> {
        self.repo.get(key).await
    }

    pub async fn used_by(&self, key: &str) -> Result<Vec<String>> {
        self.repo.used_by(key).await
    }

    pub async fn create(&self, key: &str, p: MetricPatch) -> Result<Metric> {
        validate_key(key)?;
        let now = chrono::Utc::now();
        let m = Metric {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            description: p.description.unwrap_or_default(),
            kind: p.kind.unwrap_or_default(),
            event_name: p
                .event_name
                .ok_or_else(|| Error::invalid("event_name is required"))?,
            aggregation: p.aggregation.unwrap_or_default(),
            field: p.field.unwrap_or_default(),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        m.validate()?;
        self.repo.create(&m).await
    }

    pub async fn update(&self, key: &str, p: MetricPatch) -> Result<Metric> {
        let mut m = self.repo.get(key).await?;
        if let Some(v) = p.name {
            m.name = v;
        }
        if let Some(v) = p.description {
            m.description = v;
        }
        if let Some(v) = p.kind {
            m.kind = v;
        }
        if let Some(v) = p.event_name {
            m.event_name = v;
        }
        if let Some(v) = p.aggregation {
            m.aggregation = v;
        }
        if let Some(v) = p.field {
            m.field = v;
        }
        if let Some(v) = p.tags {
            m.tags = normalize_tags(v)?;
        }
        m.validate()?;
        self.repo.update(&m).await
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.repo.delete(key).await
    }

    /// Evaluate every catalog metric attached to the experiment: one value per variant plus lift
    /// against the control variant. Unknown metric keys are reported, not fatal.
    pub async fn for_experiment(
        &self,
        exp: &Experiment,
        stats: &[VariantStats],
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Vec<Value> {
        let assigned: HashMap<&str, i64> = stats
            .iter()
            .map(|s| (s.variant.as_str(), s.assigned))
            .collect();
        let mut out = Vec::new();
        for key in &exp.metrics {
            let metric = match self.repo.get(key).await {
                Ok(m) => m,
                Err(e) => {
                    out.push(json!({"metric": key, "error": e.to_string()}));
                    continue;
                }
            };
            let totals = match self
                .events
                .experiment_metric_field(
                    &exp.key,
                    &metric.event_name,
                    metric.metadata_field(),
                    from,
                    to,
                )
                .await
            {
                Ok(t) => t,
                Err(e) => {
                    out.push(json!({"metric": key, "error": e.to_string()}));
                    continue;
                }
            };
            let value_of = |variant: &str| -> f64 {
                let t = totals.iter().find(|t| t.variant == variant);
                metric.aggregation.apply(
                    t.map(|t| t.count).unwrap_or(0),
                    t.map(|t| t.identities).unwrap_or(0),
                    t.map(|t| t.sum).unwrap_or(0.0),
                    assigned.get(variant).copied().unwrap_or(0),
                )
            };
            let control = exp.control.as_deref().map(value_of);
            let is_conv = matches!(
                metric.aggregation,
                zanshin_core::metrics::Aggregation::Conversion
            );
            let ident_of = |variant: &str| -> i64 {
                totals
                    .iter()
                    .find(|t| t.variant == variant)
                    .map(|t| t.identities)
                    .unwrap_or(0)
            };
            let control_pair = exp
                .control
                .as_deref()
                .map(|c| (ident_of(c), assigned.get(c).copied().unwrap_or(0)));
            let variants: Vec<Value> = exp
                .variants
                .iter()
                .map(|v| {
                    let value = value_of(&v.key);
                    let lift = match control {
                        Some(c) if c != 0.0 && exp.control.as_deref() != Some(&v.key) => {
                            Some((value - c) / c)
                        }
                        _ => None,
                    };
                    let mut row = json!({"variant": v.key, "value": value, "lift": lift});
                    if is_conv {
                        let ctrl = if exp.control.as_deref() == Some(&v.key) {
                            None
                        } else {
                            control_pair
                        };
                        row["stats"] = json!(zanshin_core::stats::conversion(
                            ident_of(&v.key),
                            assigned.get(v.key.as_str()).copied().unwrap_or(0),
                            ctrl,
                        ));
                        if let Some(c) = ctrl {
                            row["sequential"] = json!(zanshin_core::stats::sequential(
                                ident_of(&v.key),
                                assigned.get(v.key.as_str()).copied().unwrap_or(0),
                                c,
                                None,
                            ));
                        }
                    }
                    row
                })
                .collect();
            out.push(json!({
                "metric": metric.key, "name": metric.name, "kind": metric.kind, "event_name": metric.event_name,
                "aggregation": metric.aggregation, "variants": variants,
            }));
        }
        out
    }

    /// CUPED: per-player outcomes on each count / sum / avg metric, adjusted with the same player's
    /// pre-experiment value of the metric as covariate. Returns one entry per eligible metric with
    /// `theta`, `variance_reduction` and raw vs adjusted means per variant.
    pub async fn cuped_for_experiment(
        &self,
        exp: &Experiment,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        pre_days: i64,
    ) -> Vec<Value> {
        use zanshin_core::metrics::Aggregation;
        const MAX_IDENTITIES: i64 = 50_000;
        let pre_from = from - chrono::Duration::days(pre_days.clamp(1, 90));
        let mut out = Vec::new();
        for key in &exp.metrics {
            let Ok(metric) = self.repo.get(key).await else {
                continue;
            };
            if !matches!(
                metric.aggregation,
                Aggregation::Count | Aggregation::Sum | Aggregation::Avg
            ) {
                continue;
            }
            let post = match self
                .events
                .experiment_metric_by_identity(
                    &exp.key,
                    &metric.event_name,
                    metric.metadata_field(),
                    from,
                    to,
                    MAX_IDENTITIES,
                )
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    out.push(json!({"metric": key, "error": e.to_string()}));
                    continue;
                }
            };
            if post.len() < 10 {
                out.push(json!({"metric": key, "error": "not enough players with the event yet (need 10)"}));
                continue;
            }
            let ids: Vec<uuid::Uuid> = post.iter().map(|p| p.identity_id).collect();
            let pre = match self
                .events
                .metric_by_identity(
                    &metric.event_name,
                    metric.metadata_field(),
                    &ids,
                    pre_from,
                    from,
                )
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    out.push(json!({"metric": key, "error": e.to_string()}));
                    continue;
                }
            };
            let pre_by_id: HashMap<uuid::Uuid, f64> = pre
                .into_iter()
                .map(|p| {
                    (
                        p.identity_id,
                        metric_value(&metric.aggregation, p.count, p.sum),
                    )
                })
                .collect();
            let outcomes: Vec<f64> = post
                .iter()
                .map(|p| metric_value(&metric.aggregation, p.count, p.sum))
                .collect();
            let covariates: Vec<f64> = post
                .iter()
                .map(|p| pre_by_id.get(&p.identity_id).copied().unwrap_or(0.0))
                .collect();
            let (theta, adjusted, reduction) = zanshin_core::stats::cuped(&outcomes, &covariates);
            let mut per: HashMap<&str, (f64, f64, usize)> = HashMap::new();
            for (i, p) in post.iter().enumerate() {
                let e = per.entry(p.variant.as_str()).or_insert((0.0, 0.0, 0));
                e.0 += outcomes[i];
                e.1 += adjusted[i];
                e.2 += 1;
            }
            let ctrl = exp
                .control
                .as_deref()
                .and_then(|c| per.get(c))
                .map(|(_, a, n)| a / *n as f64);
            let variants: Vec<Value> = exp
                .variants
                .iter()
                .map(|v| {
                    let (raw, adj, n) = per.get(v.key.as_str()).copied().unwrap_or((0.0, 0.0, 0));
                    let (mean, adjusted_mean) = if n > 0 {
                        (raw / n as f64, adj / n as f64)
                    } else {
                        (0.0, 0.0)
                    };
                    let lift = match ctrl {
                        Some(c) if c != 0.0 && exp.control.as_deref() != Some(&v.key) => {
                            Some((adjusted_mean - c) / c)
                        }
                        _ => None,
                    };
                    json!({"variant": v.key, "players": n, "mean": mean, "adjusted_mean": adjusted_mean, "adjusted_lift": lift})
                })
                .collect();
            out.push(json!({
                "metric": metric.key, "name": metric.name, "aggregation": metric.aggregation,
                "theta": theta, "variance_reduction": reduction, "players": post.len(),
                "pre_window": { "from": pre_from, "to": from }, "variants": variants,
            }));
        }
        out
    }

    /// Monitor metrics of a live event over a window, per variant (`default` without variants),
    /// read from the `live:<key>` event tag. Conversion is reported per unique participant seen.
    pub async fn for_live_event(
        &self,
        ev: &zanshin_core::live_events::LiveEvent,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Vec<Value> {
        let tag = ev.tag(None).0;
        let mut variants: Vec<String> = ev.variants.iter().map(|v| v.key.clone()).collect();
        if variants.is_empty() {
            variants.push(zanshin_core::live_events::DEFAULT_VARIANT.to_string());
        }
        let mut out = Vec::new();
        for key in &ev.metrics {
            let metric = match self.repo.get(key).await {
                Ok(m) => m,
                Err(e) => {
                    out.push(json!({"metric": key, "error": e.to_string()}));
                    continue;
                }
            };
            let totals = match self
                .events
                .experiment_metric_field(
                    &tag,
                    &metric.event_name,
                    metric.metadata_field(),
                    from,
                    to,
                )
                .await
            {
                Ok(t) => t,
                Err(e) => {
                    out.push(json!({"metric": key, "error": e.to_string()}));
                    continue;
                }
            };
            let rows: Vec<Value> = variants
                .iter()
                .map(|v| {
                    let t = totals.iter().find(|t| &t.variant == v);
                    let (count, identities, sum) = t
                        .map(|t| (t.count, t.identities, t.sum))
                        .unwrap_or((0, 0, 0.0));
                    json!({
                        "variant": v,
                        "value": metric.aggregation.apply(count, identities, sum, identities),
                        "count": count, "identities": identities, "sum": sum,
                    })
                })
                .collect();
            out.push(json!({
                "metric": metric.key, "name": metric.name, "kind": metric.kind, "event_name": metric.event_name,
                "aggregation": metric.aggregation, "variants": rows,
            }));
        }
        out
    }

    /// Daily series of one live-event metric per variant.
    pub async fn live_series(
        &self,
        ev: &zanshin_core::live_events::LiveEvent,
        metric_key: &str,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Value> {
        let metric = self.repo.get(metric_key).await?;
        let rows = self
            .events
            .experiment_metric_series(
                &ev.tag(None).0,
                &metric.event_name,
                metric.metadata_field(),
                from,
                to,
            )
            .await?;
        let points: Vec<Value> = rows
            .iter()
            .map(|r| json!({"variant": r.variant, "day": r.day, "value": metric.aggregation.apply(r.count, r.identities, r.sum, r.identities), "count": r.count, "identities": r.identities, "sum": r.sum}))
            .collect();
        Ok(json!({"metric": metric.key, "aggregation": metric.aggregation, "points": points}))
    }

    /// Daily series of one metric per variant.
    pub async fn series(
        &self,
        exp: &Experiment,
        metric_key: &str,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Value> {
        let metric = self.repo.get(metric_key).await?;
        let rows = self
            .events
            .experiment_metric_series(
                &exp.key,
                &metric.event_name,
                metric.metadata_field(),
                from,
                to,
            )
            .await?;
        let points: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({"variant": r.variant, "day": r.day, "value": metric.aggregation.apply(r.count, r.identities, r.sum, 0), "count": r.count, "identities": r.identities, "sum": r.sum})
            })
            .collect();
        Ok(json!({"metric": metric.key, "aggregation": metric.aggregation, "points": points}))
    }
}

/// A single player's value of a metric from their event count and summed field.
fn metric_value(agg: &zanshin_core::metrics::Aggregation, count: i64, sum: f64) -> f64 {
    match agg {
        zanshin_core::metrics::Aggregation::Count => count as f64,
        zanshin_core::metrics::Aggregation::Sum => sum,
        zanshin_core::metrics::Aggregation::Avg if count > 0 => sum / count as f64,
        _ => 0.0,
    }
}
