//! Analytics v2: game-metrics overview, retention cohorts, funnels, custom dashboards, annotations.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use zanshin_core::{
    analytics::{
        fold_funnel_model, Annotation, Dashboard, EventFilter, FunnelModel, FunnelStep, Widget,
        WidgetKind,
    },
    flags::validate_key,
    Error, Result,
};
use zanshin_store::analytics::{
    AdSpendRepo, AdSpendRow, AnnotationRepo, DashboardRepo, PlayerMetricsRepo,
};

use super::{
    bus::ChangeBus, events::EventService, flags::normalize_tags, taxonomy::TaxonomyService,
};

const FUNNEL_ROW_CAP: i64 = 200_000;
const COHORT_CAP: i64 = 50_000;
const DEFAULT_REVENUE_EVENTS: &[&str] = &["purchase", "revenue", "iap"];

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DashboardPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub widgets: Option<Vec<Widget>>,
    #[serde(default)]
    pub filters: Option<Value>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FunnelQuery {
    pub steps: Vec<FunnelStep>,
    #[serde(default)]
    pub window_secs: Option<i64>,
    #[serde(default)]
    pub from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub to: Option<DateTime<Utc>>,
    /// `unlimited` | `window` (default) | `step` | `session`.
    #[serde(default)]
    pub model: FunnelModel,
    #[serde(default)]
    pub filter: EventFilter,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExploreQuery {
    pub event: String,
    #[serde(default)]
    pub from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub to: Option<DateTime<Utc>>,
    /// `day` (default) or `hour`.
    #[serde(default)]
    pub granularity: Option<String>,
    /// `country` | `platform` | `app_version` | `experiment:<key>` | `meta:<field>`.
    #[serde(default)]
    pub group_by: Option<String>,
    /// metadata field summed into `field_sum` (default `amount`).
    #[serde(default)]
    pub sum_field: Option<String>,
    #[serde(default)]
    pub filter: EventFilter,
}

#[derive(Clone)]
pub struct AnalyticsService {
    events: EventService,
    players: PlayerMetricsRepo,
    dashboards: DashboardRepo,
    annotations: AnnotationRepo,
    taxonomy: TaxonomyService,
    ad_spend: AdSpendRepo,
}

pub fn window(
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    default_days: i64,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let to = to.unwrap_or_else(Utc::now);
    let from = from.unwrap_or(to - Duration::days(default_days));
    if from >= to {
        return Err(Error::invalid("from must be before to"));
    }
    if to - from > Duration::days(400) {
        return Err(Error::invalid("range must be at most 400 days"));
    }
    Ok((from, to))
}

fn day_of(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d").to_string()
}

impl AnalyticsService {
    pub fn new(
        events: EventService,
        players: PlayerMetricsRepo,
        dashboards: DashboardRepo,
        annotations: AnnotationRepo,
        taxonomy: TaxonomyService,
        ad_spend: AdSpendRepo,
        bus: ChangeBus,
    ) -> Self {
        let svc = Self {
            events,
            players,
            dashboards,
            annotations,
            taxonomy,
            ad_spend,
        };
        let repo = svc.annotations.clone();
        let mut rx = bus.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(c) if c.identity_id.is_none() => {
                        let kind = format!("{:?}", c.kind).to_lowercase();
                        let kind = if kind == "liveevent" {
                            "live_event".to_string()
                        } else {
                            kind
                        };
                        let title = format!(
                            "{} {} {}",
                            kind.replace('_', " "),
                            c.key,
                            if c.deleted { "deleted" } else { "updated" }
                        );
                        let _ = repo.insert(Utc::now(), &kind, &c.key, &title, "").await;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        svc
    }

    /// Event names that carry revenue: taxonomy definitions typed `revenue` plus the conventional names.
    pub async fn revenue_names(&self) -> Vec<String> {
        let mut names: Vec<String> = DEFAULT_REVENUE_EVENTS
            .iter()
            .map(|s| s.to_string())
            .collect();
        if let Ok(defs) = self.taxonomy.definitions().await {
            for d in defs.values().filter(|d| d.value_type == "revenue") {
                if !names.contains(&d.name) {
                    names.push(d.name.clone());
                }
            }
        }
        names
    }

    pub async fn overview(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        filter: &EventFilter,
    ) -> Result<Value> {
        filter.validate().map_err(Error::invalid)?;
        let revenue_names = self.revenue_names().await;
        let (active, new_users, sessions, revenue, wau, mau, total) = tokio::try_join!(
            self.events.active_per_day(from, to, filter),
            self.players.new_identities(from, to),
            self.players.sessions(from, to),
            self.events
                .revenue_per_day(&revenue_names, from, to, filter),
            self.events
                .active_in_range(to - Duration::days(7), to, filter),
            self.events
                .active_in_range(to - Duration::days(30), to, filter),
            self.players.total_identities(),
        )?;
        let mut days: BTreeMap<String, Value> = BTreeMap::new();
        let mut d = from.date_naive();
        while d <= to.date_naive() {
            let day = d.format("%Y-%m-%d").to_string();
            days.insert(day.clone(), json!({"day": day, "dau": 0, "new_users": 0, "sessions": 0, "avg_session_secs": 0.0, "revenue": 0.0, "payers": 0, "purchases": 0}));
            d += Duration::days(1);
        }
        for (day, n) in &active {
            if let Some(v) = days.get_mut(day) {
                v["dau"] = json!(n);
            }
        }
        for r in &new_users {
            if let Some(v) = days.get_mut(&r.day) {
                v["new_users"] = json!(r.count);
            }
        }
        for r in &sessions {
            if let Some(v) = days.get_mut(&r.day) {
                v["sessions"] = json!(r.sessions);
                v["avg_session_secs"] = json!(r.avg_secs);
            }
        }
        for (day, amount, payers, purchases) in &revenue {
            if let Some(v) = days.get_mut(day) {
                v["revenue"] = json!(amount);
                v["payers"] = json!(payers);
                v["purchases"] = json!(purchases);
            }
        }
        let series: Vec<Value> = days.into_values().collect();
        let active_days: i64 = active.iter().map(|(_, n)| n).sum();
        let dau_latest = active.last().map(|(_, n)| *n).unwrap_or(0);
        let dau_avg = if active.is_empty() {
            0.0
        } else {
            active_days as f64 / active.len() as f64
        };
        let new_total: i64 = new_users.iter().map(|r| r.count).sum();
        let sessions_total: i64 = sessions.iter().map(|r| r.sessions).sum();
        let avg_session = if sessions_total > 0 {
            sessions
                .iter()
                .map(|r| r.avg_secs * r.sessions as f64)
                .sum::<f64>()
                / sessions_total as f64
        } else {
            0.0
        };
        let revenue_total: f64 = revenue.iter().map(|r| r.1).sum();
        let payer_days: i64 = revenue.iter().map(|r| r.2).sum();
        let purchases: i64 = revenue.iter().map(|r| r.3).sum();
        let ratio = |a: f64, b: f64| if b > 0.0 { a / b } else { 0.0 };
        Ok(json!({
            "from": from, "to": to, "backend": self.events.backend(), "filter": filter,
            "revenue_events": revenue_names,
            "totals": {
                "dau": dau_latest, "dau_avg": dau_avg, "wau": wau, "mau": mau, "total_players": total,
                "new_users": new_total, "sessions": sessions_total, "avg_session_secs": avg_session,
                "revenue": revenue_total, "purchases": purchases, "paying_player_days": payer_days,
                "arpdau": ratio(revenue_total, active_days as f64),
                "arppu": ratio(revenue_total, payer_days as f64),
                "paying_share": ratio(payer_days as f64, active_days as f64),
                "stickiness": ratio(dau_avg, mau as f64),
            },
            "series": series,
        }))
    }

    pub async fn retention(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        days: &[i64],
        filter: &EventFilter,
        cumulative: bool,
    ) -> Result<Value> {
        filter.validate().map_err(Error::invalid)?;
        let days: Vec<i64> = {
            let mut d: Vec<i64> = days
                .iter()
                .copied()
                .filter(|d| (1..=90).contains(d))
                .collect();
            d.sort_unstable();
            d.dedup();
            if d.is_empty() {
                vec![1, 3, 7, 14, 30]
            } else {
                d
            }
        };
        let cohorts = self
            .players
            .cohorts_filtered(
                from,
                to,
                filter.platform.as_deref(),
                filter.country.as_deref(),
                COHORT_CAP,
            )
            .await?;
        if cohorts.is_empty() {
            return Ok(
                json!({ "from": from, "to": to, "days": days, "cumulative": cumulative, "cohorts": [], "installs": 0 }),
            );
        }
        let ids: Vec<Uuid> = cohorts.iter().map(|(id, _)| *id).collect();
        let installs = ids.len();
        let horizon = to + Duration::days(*days.last().unwrap_or(&30) + 1);
        let active = self.events.active_days(&ids, from, horizon, filter).await?;
        let mut active_set: HashSet<(Uuid, String)> = HashSet::with_capacity(active.len());
        for (id, day) in active {
            active_set.insert((id, day));
        }
        let mut by_cohort: BTreeMap<String, Vec<Uuid>> = BTreeMap::new();
        for (id, day) in cohorts {
            by_cohort.entry(day).or_default().push(id);
        }
        let now_day = day_of(Utc::now());
        let out: Vec<Value> = by_cohort
            .into_iter()
            .map(|(cohort_day, ids)| {
                let base = chrono::NaiveDate::parse_from_str(&cohort_day, "%Y-%m-%d").ok();
                let mut retained = serde_json::Map::new();
                for d in &days {
                    let Some(base) = base else { continue };
                    let target = (base + Duration::days(*d)).format("%Y-%m-%d").to_string();
                    if target > now_day {
                        retained.insert(format!("d{d}"), Value::Null);
                        continue;
                    }
                    let n = if cumulative {
                        // Returned on any day 1..=d after install.
                        ids.iter()
                            .filter(|id| {
                                (1..=*d).any(|k| {
                                    let day =
                                        (base + Duration::days(k)).format("%Y-%m-%d").to_string();
                                    active_set.contains(&(**id, day))
                                })
                            })
                            .count()
                    } else {
                        ids.iter()
                            .filter(|id| active_set.contains(&(**id, target.clone())))
                            .count()
                    };
                    retained.insert(
                        format!("d{d}"),
                        json!({ "count": n, "rate": n as f64 / ids.len() as f64 }),
                    );
                }
                json!({ "cohort": cohort_day, "size": ids.len(), "retained": retained })
            })
            .collect();
        Ok(
            json!({ "from": from, "to": to, "days": days, "cumulative": cumulative, "installs": installs, "cohorts": out }),
        )
    }

    pub async fn funnel(&self, q: &FunnelQuery) -> Result<Value> {
        if q.steps.is_empty() || q.steps.len() > 20 {
            return Err(Error::invalid("steps must be 1..=20"));
        }
        let (from, to) = window(q.from, q.to, 30)?;
        let window_secs = q.window_secs.unwrap_or(7 * 86_400).clamp(1, 400 * 86_400);
        let mut names: Vec<String> = q.steps.iter().map(|s| s.name.clone()).collect();
        names.sort();
        names.dedup();
        q.filter.validate().map_err(Error::invalid)?;
        let rows = self
            .events
            .funnel_rows(&names, from, to, FUNNEL_ROW_CAP, &q.filter)
            .await?;
        let truncated = rows.len() as i64 >= FUNNEL_ROW_CAP;
        let mut folded = Vec::with_capacity(rows.len());
        for (id, name, at, meta, session) in &rows {
            for (i, step) in q.steps.iter().enumerate() {
                if step.name == *name && step.matches(meta) {
                    folded.push((*id, i, *at, *session));
                }
            }
        }
        let counts = fold_funnel_model(folded, q.steps.len(), window_secs, q.model);
        let first = counts.first().copied().unwrap_or(0);
        let steps: Vec<Value> = q
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let n = counts[i];
                let prev = if i == 0 { n } else { counts[i - 1] };
                json!({
                    "name": s.name, "field": s.field, "op": s.op, "value": s.value, "count": n,
                    "conversion": if first > 0 { n as f64 / first as f64 } else { 0.0 },
                    "step_conversion": if prev > 0 { n as f64 / prev as f64 } else { 0.0 },
                    "drop": prev - n,
                })
            })
            .collect();
        Ok(
            json!({ "from": from, "to": to, "window_secs": window_secs, "model": q.model, "filter": q.filter, "entered": first, "steps": steps, "truncated": truncated }),
        )
    }

    /// Ad-hoc event explorer: buckets × optional group with count / identities / sums.
    pub async fn explore(&self, q: &ExploreQuery) -> Result<Value> {
        q.filter.validate().map_err(Error::invalid)?;
        let (from, to) = window(q.from, q.to, 30)?;
        let g = match q.granularity.as_deref() {
            Some("hour") => zanshin_store::events::Granularity::Hour,
            _ => zanshin_store::events::Granularity::Day,
        };
        let rows = self
            .events
            .explore(
                &q.event,
                from,
                to,
                g,
                q.group_by.as_deref(),
                q.sum_field.as_deref(),
                &q.filter,
                5_000,
            )
            .await?;
        let mut groups: BTreeMap<String, (i64, i64, f64, f64)> = BTreeMap::new();
        for (_, grp, count, identities, sum, field_sum) in &rows {
            let e = groups.entry(grp.clone()).or_default();
            e.0 += count;
            e.1 += identities;
            e.2 += sum;
            e.3 += field_sum;
        }
        let mut groups: Vec<Value> = groups
            .into_iter()
            .map(|(g, (c, i, s, f))| json!({ "group": g, "count": c, "identities": i, "sum": s, "field_sum": f }))
            .collect();
        groups.sort_by(|a, b| b["count"].as_i64().cmp(&a["count"].as_i64()));
        let series: Vec<Value> = rows
            .iter()
            .map(|(b, grp, count, identities, sum, field_sum)| json!({ "bucket": b, "group": grp, "count": count, "identities": identities, "sum": sum, "field_sum": field_sum }))
            .collect();
        Ok(json!({
            "event": q.event, "from": from, "to": to, "granularity": if matches!(g, zanshin_store::events::Granularity::Hour) { "hour" } else { "day" },
            "group_by": q.group_by, "sum_field": q.sum_field.clone().unwrap_or_else(|| "amount".into()), "filter": q.filter,
            "total": { "count": rows.iter().map(|r| r.2).sum::<i64>(), "sum": rows.iter().map(|r| r.4).sum::<f64>(), "field_sum": rows.iter().map(|r| r.5).sum::<f64>() },
            "groups": groups, "series": series,
        }))
    }

    /// Players and events per country (ISO-3166 alpha-2 from the identity's default properties).
    pub async fn map(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        filter: &EventFilter,
    ) -> Result<Value> {
        filter.validate().map_err(Error::invalid)?;
        let rows = self.events.by_country(from, to, filter).await?;
        let total: i64 = rows.iter().map(|r| r.1).sum();
        let countries: Vec<Value> = rows
            .iter()
            .map(|(c, ids, ev)| json!({ "country": if c.is_empty() { "unknown" } else { c.as_str() }, "players": ids, "events": ev, "share": if total > 0 { *ids as f64 / total as f64 } else { 0.0 } }))
            .collect();
        Ok(
            json!({ "from": from, "to": to, "filter": filter, "total_players": total, "countries": countries }),
        )
    }

    /// Session-length metrics from play sessions.
    pub async fn playtime(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        filter: &EventFilter,
    ) -> Result<Value> {
        let (days, buckets) = tokio::try_join!(
            self.players.playtime(
                from,
                to,
                filter.platform.as_deref(),
                filter.app_version.as_deref()
            ),
            self.players.duration_buckets(from, to),
        )?;
        let sessions: i64 = days.iter().map(|d| d.sessions).sum();
        let total_secs: i64 = days.iter().map(|d| d.total_secs).sum();
        let players: i64 = days.iter().map(|d| d.players).sum();
        let order = ["<1m", "1-5m", "5-15m", "15-30m", "30-60m", ">60m"];
        let mut hist: Vec<Value> = order
            .iter()
            .map(|b| json!({ "bucket": b, "sessions": buckets.iter().find(|(k, _)| k == b).map(|(_, n)| *n).unwrap_or(0) }))
            .collect();
        hist.retain(|_| true);
        Ok(json!({
            "from": from, "to": to,
            "totals": {
                "sessions": sessions, "total_secs": total_secs,
                "avg_session_secs": if sessions > 0 { total_secs as f64 / sessions as f64 } else { 0.0 },
                "sessions_per_player_day": if players > 0 { sessions as f64 / players as f64 } else { 0.0 },
                "playtime_per_player_day_secs": if players > 0 { total_secs as f64 / players as f64 } else { 0.0 },
            },
            "series": days, "histogram": hist,
        }))
    }

    /// Install cohorts × cumulative revenue at day offsets, joined with advertising spend:
    /// installs, spend, CPI, LTV(d) = revenue per install, RoAS(d) = revenue / spend.
    pub async fn ltv(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        days: &[i64],
        filter: &EventFilter,
    ) -> Result<Value> {
        filter.validate().map_err(Error::invalid)?;
        let mut days: Vec<i64> = days
            .iter()
            .copied()
            .filter(|d| (0..=180).contains(d))
            .collect();
        days.sort_unstable();
        days.dedup();
        if days.is_empty() {
            days = vec![1, 3, 7, 14, 30];
        }
        let revenue_names = self.revenue_names().await;
        let cohorts = self
            .players
            .cohorts_filtered(
                from,
                to,
                filter.platform.as_deref(),
                filter.country.as_deref(),
                COHORT_CAP,
            )
            .await?;
        let ids: Vec<Uuid> = cohorts.iter().map(|(id, _)| *id).collect();
        let horizon = to + Duration::days(*days.last().unwrap_or(&30) + 1);
        let (revenue, spend) = tokio::try_join!(
            self.events
                .revenue_by_identity(&ids, &revenue_names, from, horizon),
            self.ad_spend.list(from, to),
        )?;
        let mut rev_by_id: std::collections::HashMap<Uuid, Vec<(String, f64)>> =
            std::collections::HashMap::new();
        for (id, day, amount) in revenue {
            rev_by_id.entry(id).or_default().push((day, amount));
        }
        let mut by_cohort: BTreeMap<String, Vec<Uuid>> = BTreeMap::new();
        for (id, day) in cohorts {
            by_cohort.entry(day).or_default().push(id);
        }
        let mut spend_by_day: BTreeMap<String, (f64, i64)> = BTreeMap::new();
        for r in &spend {
            if filter
                .platform
                .as_deref()
                .is_some_and(|p| !r.platform.is_empty() && r.platform != p)
                || filter
                    .country
                    .as_deref()
                    .is_some_and(|c| !r.country.is_empty() && r.country != c)
            {
                continue;
            }
            let e = spend_by_day
                .entry(r.day.format("%Y-%m-%d").to_string())
                .or_default();
            e.0 += r.spend;
            e.1 += r.installs as i64;
        }
        let now_day = day_of(Utc::now());
        let mut total_installs = 0i64;
        let mut total_spend = 0f64;
        let mut total_rev: BTreeMap<i64, f64> = BTreeMap::new();
        let mut rows: Vec<Value> = Vec::new();
        let mut all_days: std::collections::BTreeSet<String> = by_cohort.keys().cloned().collect();
        all_days.extend(spend_by_day.keys().cloned());
        for cohort_day in all_days {
            let ids = by_cohort.get(&cohort_day).cloned().unwrap_or_default();
            let (sp, reported_installs) =
                spend_by_day.get(&cohort_day).copied().unwrap_or((0.0, 0));
            let installs = ids.len() as i64;
            let base = chrono::NaiveDate::parse_from_str(&cohort_day, "%Y-%m-%d").ok();
            let mut at = serde_json::Map::new();
            for d in &days {
                let Some(base) = base else { continue };
                let cutoff = (base + Duration::days(*d)).format("%Y-%m-%d").to_string();
                if cutoff > now_day {
                    at.insert(format!("d{d}"), Value::Null);
                    continue;
                }
                let rev: f64 = ids
                    .iter()
                    .filter_map(|id| rev_by_id.get(id))
                    .flat_map(|v| v.iter().filter(|(day, _)| *day <= cutoff).map(|(_, a)| *a))
                    .sum();
                *total_rev.entry(*d).or_default() += rev;
                at.insert(format!("d{d}"), json!({
                    "revenue": rev,
                    "ltv": if installs > 0 { rev / installs as f64 } else { 0.0 },
                    "roas": if sp > 0.0 { rev / sp } else { Value::Null.as_f64().unwrap_or(0.0) },
                }));
            }
            total_installs += installs;
            total_spend += sp;
            rows.push(json!({
                "cohort": cohort_day, "installs": installs, "reported_installs": reported_installs, "spend": sp,
                "cpi": if installs > 0 && sp > 0.0 { sp / installs as f64 } else { 0.0 },
                "at": at,
            }));
        }
        let totals_at: serde_json::Map<String, Value> = days
            .iter()
            .map(|d| {
                let rev = total_rev.get(d).copied().unwrap_or(0.0);
                (format!("d{d}"), json!({ "revenue": rev, "ltv": if total_installs > 0 { rev / total_installs as f64 } else { 0.0 }, "roas": if total_spend > 0.0 { rev / total_spend } else { 0.0 } }))
            })
            .collect();
        Ok(json!({
            "from": from, "to": to, "days": days, "filter": filter, "revenue_events": revenue_names,
            "totals": { "installs": total_installs, "spend": total_spend, "cpi": if total_installs > 0 && total_spend > 0.0 { total_spend / total_installs as f64 } else { 0.0 }, "at": totals_at },
            "cohorts": rows,
        }))
    }

    pub async fn ad_spend(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<AdSpendRow>> {
        self.ad_spend.list(from, to).await
    }

    /// Upsert spend rows (JSON array, or CSV `day,platform,country,campaign,spend,installs[,currency]`).
    pub async fn import_ad_spend(&self, rows: Vec<AdSpendRow>) -> Result<u64> {
        if rows.len() > 50_000 {
            return Err(Error::invalid("at most 50000 rows per import"));
        }
        for r in &rows {
            if r.spend < 0.0
                || r.installs < 0
                || r.platform.len() > 64
                || r.country.len() > 8
                || r.campaign.len() > 128
            {
                return Err(Error::invalid("invalid ad spend row"));
            }
        }
        self.ad_spend.upsert_many(&rows).await
    }

    pub async fn delete_ad_spend(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<u64> {
        self.ad_spend.delete(from, to).await
    }

    pub fn parse_ad_spend_csv(text: &str) -> Result<Vec<AdSpendRow>> {
        let mut out = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || (i == 0 && line.to_lowercase().starts_with("day")) {
                continue;
            }
            let cols: Vec<&str> = line.split(',').map(str::trim).collect();
            if cols.len() < 5 {
                return Err(Error::invalid(format!(
                    "line {}: expected day,platform,country,campaign,spend[,installs[,currency]]",
                    i + 1
                )));
            }
            let day = chrono::NaiveDate::parse_from_str(cols[0], "%Y-%m-%d")
                .map_err(|_| Error::invalid(format!("line {}: bad date {:?}", i + 1, cols[0])))?;
            out.push(AdSpendRow {
                day,
                platform: cols[1].to_string(),
                country: cols[2].to_uppercase(),
                campaign: cols[3].to_string(),
                spend: cols[4]
                    .parse()
                    .map_err(|_| Error::invalid(format!("line {}: bad spend", i + 1)))?,
                installs: cols.get(5).and_then(|s| s.parse().ok()).unwrap_or(0),
                currency: cols
                    .get(6)
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "USD".into()),
            });
        }
        Ok(out)
    }

    /// Operational error summary: rejected events (taxonomy validation) and failed deliveries.
    pub async fn errors(&self, since: DateTime<Utc>) -> Result<Value> {
        let rejected = self.taxonomy.rejected(500).await?;
        let failures = self.players.failures_since(since).await?;
        let mut by_reason: BTreeMap<String, i64> = BTreeMap::new();
        for r in &rejected {
            *by_reason.entry(r.reason.clone()).or_default() += 1;
        }
        Ok(json!({
            "since": since,
            "rejected_events": { "count": rejected.len(), "by_reason": by_reason, "recent": rejected.iter().take(20).collect::<Vec<_>>() },
            "failures": failures.iter().map(|(k, n)| json!({ "source": k, "count": n })).collect::<Vec<_>>(),
        }))
    }

    pub fn players(&self) -> &PlayerMetricsRepo {
        &self.players
    }

    pub fn event_service(&self) -> &EventService {
        &self.events
    }

    // ---- annotations

    pub async fn annotations(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<Annotation>> {
        self.annotations.list(from, to, 500).await
    }

    pub async fn annotate(
        &self,
        at: Option<DateTime<Utc>>,
        title: &str,
        body: &str,
    ) -> Result<Annotation> {
        if title.trim().is_empty() || title.len() > 200 {
            return Err(Error::invalid("title must be 1..200 chars"));
        }
        self.annotations
            .insert(at.unwrap_or_else(Utc::now), "note", "", title, body)
            .await
    }

    pub async fn delete_annotation(&self, id: Uuid) -> Result<()> {
        self.annotations.delete(id).await
    }

    pub async fn purge_annotations(&self, keep: Duration) -> Result<u64> {
        self.annotations.purge(Utc::now() - keep).await
    }

    // ---- dashboards

    pub async fn list_dashboards(&self) -> Result<Vec<Dashboard>> {
        self.dashboards.list().await
    }

    pub async fn get_dashboard(&self, key: &str) -> Result<Dashboard> {
        self.dashboards.get(key).await
    }

    pub async fn create_dashboard(&self, key: &str, p: DashboardPatch) -> Result<Dashboard> {
        validate_key(key)?;
        let now = Utc::now();
        let d = Dashboard {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            description: p.description.unwrap_or_default(),
            widgets: p.widgets.unwrap_or_default(),
            filters: p.filters.unwrap_or_else(|| json!({})),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        d.validate()?;
        self.dashboards.create(&d).await
    }

    pub async fn update_dashboard(&self, key: &str, p: DashboardPatch) -> Result<Dashboard> {
        let mut d = self.dashboards.get(key).await?;
        if let Some(v) = p.name {
            d.name = v;
        }
        if let Some(v) = p.description {
            d.description = v;
        }
        if let Some(v) = p.widgets {
            d.widgets = v;
        }
        if let Some(v) = p.filters {
            d.filters = v;
        }
        if let Some(v) = p.tags {
            d.tags = normalize_tags(v)?;
        }
        d.validate()?;
        self.dashboards.update(&d).await
    }

    pub async fn delete_dashboard(&self, key: &str) -> Result<()> {
        self.dashboards.delete(key).await
    }

    /// Evaluate every widget of a dashboard for the window.
    pub async fn dashboard_data(
        &self,
        key: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Value> {
        let d = self.dashboards.get(key).await?;
        let filter: EventFilter = serde_json::from_value(d.filters.clone()).unwrap_or_default();
        let mut overview: Option<Value> = None;
        let mut out: Vec<Value> = Vec::with_capacity(d.widgets.len());
        for w in &d.widgets {
            let data = match w.kind {
                WidgetKind::Timeseries => {
                    let event = w.config["event"].as_str().unwrap_or_default();
                    self.events
                        .timeseries(event, from, to, zanshin_store::events::Granularity::Day)
                        .await
                        .map(|b| json!(b))
                }
                WidgetKind::Number => {
                    let metric = w.config["metric"].as_str().unwrap_or_default();
                    if metric == "event_count" {
                        let event = w.config["event"].as_str().unwrap_or_default();
                        self.events.timeseries(event, from, to, zanshin_store::events::Granularity::Day).await.map(|b| {
                            json!({ "value": b.iter().map(|x| x.count).sum::<i64>(), "identities": b.iter().map(|x| x.identities).sum::<i64>() })
                        })
                    } else {
                        if overview.is_none() {
                            overview = Some(self.overview(from, to, &filter).await?);
                        }
                        let totals = &overview.as_ref().unwrap()["totals"];
                        Ok(json!({ "value": totals.get(metric).cloned().unwrap_or(Value::Null) }))
                    }
                }
                WidgetKind::Funnel => {
                    let steps: Vec<FunnelStep> =
                        serde_json::from_value(w.config["steps"].clone()).unwrap_or_default();
                    let q = FunnelQuery {
                        steps,
                        window_secs: w.config["window_secs"].as_i64(),
                        from: Some(from),
                        to: Some(to),
                        model: serde_json::from_value(w.config["model"].clone())
                            .unwrap_or_default(),
                        filter: filter.clone(),
                    };
                    self.funnel(&q).await
                }
                WidgetKind::Retention => {
                    let days: Vec<i64> = w.config["days"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
                        .unwrap_or_default();
                    self.retention(
                        from,
                        to,
                        &days,
                        &filter,
                        w.config["cumulative"].as_bool().unwrap_or(false),
                    )
                    .await
                }
                WidgetKind::TopEvents => {
                    let limit = w.config["limit"].as_i64().unwrap_or(10);
                    self.events
                        .top_names(from, to, limit)
                        .await
                        .map(|n| json!(n))
                }
            };
            out.push(match data {
                Ok(v) => json!({ "id": w.id, "kind": w.kind, "title": w.title, "width": w.width, "data": v }),
                Err(e) => json!({ "id": w.id, "kind": w.kind, "title": w.title, "width": w.width, "error": e.to_string() }),
            });
        }
        Ok(json!({ "dashboard": d.key, "from": from, "to": to, "widgets": out }))
    }
}
