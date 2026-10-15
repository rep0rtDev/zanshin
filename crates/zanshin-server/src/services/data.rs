//! Data management: GDPR deletion / takeout, event purges, synthetic sample data.

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;
use zanshin_core::{events::EventInput, Error, Result};

use crate::state::AppState;

pub async fn gdpr_delete(state: &AppState, id: Uuid) -> Result<Value> {
    let identity = state.identity.get(id).await?;
    let mut meta = serde_json::Map::new();
    meta.insert(
        "external_id".into(),
        Value::String(identity.external_id.clone()),
    );
    state
        .events
        .system(
            &identity,
            zanshin_core::events::EV_IDENTITY_DELETE,
            "",
            Some(meta),
            None,
        )
        .await;
    state.events.delete_identity(id).await?;
    state.identity.delete(id).await?;
    Ok(json!({ "deleted": id, "external_id": identity.external_id }))
}

/// Everything stored about one player, for data-subject access requests.
pub async fn takeout(state: &AppState, id: Uuid) -> Result<Value> {
    let identity = state.identity.get(id).await?;
    let events = state.events.recent_for_identity(id, 500).await?;
    let sessions = state
        .taxonomy
        .repo
        .sessions_for_identity(id, 200)
        .await
        .unwrap_or_default();
    Ok(json!({
        "exported_at": Utc::now(),
        "identity": identity,
        "events": events,
        "sessions": sessions,
    }))
}

#[derive(Debug, Deserialize)]
pub struct PurgeRequest {
    pub before: DateTime<Utc>,
    #[serde(default)]
    pub names: Vec<String>,
}

pub async fn purge_events(state: &AppState, req: &PurgeRequest) -> Result<Value> {
    if req.before > Utc::now() - Duration::hours(1) {
        return Err(Error::invalid(
            "before must be at least one hour in the past",
        ));
    }
    let n = state.events.delete_before(req.before, &req.names).await?;
    Ok(
        json!({ "deleted": n, "backend": state.events.backend(), "async": state.events.backend() == "clickhouse" }),
    )
}

#[derive(Debug, Deserialize)]
pub struct SampleRequest {
    #[serde(default = "default_players")]
    pub players: u32,
    #[serde(default = "default_days")]
    pub days: u32,
    #[serde(default)]
    pub seed: u64,
}
fn default_players() -> u32 {
    200
}
fn default_days() -> u32 {
    14
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next() % 10_000) as f64 / 10_000.0 < p
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[(self.next() as usize) % xs.len()]
    }
}

/// Generate a deterministic population of players with sessions, progression and purchases spread over the past days.
/// Default audiences, in Satori's naming: key, name, description, expression.
pub const DEFAULT_AUDIENCES: &[(&str, &str, &str, &str)] = &[
    (
        "tier_1_countries",
        "Tier-1-Countries",
        "Players in high-ARPU markets.",
        r#"default.country in ["US", "CA", "GB", "AU", "DE", "FR", "JP", "KR"]"#,
    ),
    (
        "spenders",
        "Spenders",
        "Made at least one purchase.",
        "computed.purchases >= 1",
    ),
    (
        "whales",
        "Whales",
        "Lifetime spend of 100 USD or more.",
        "computed.spend_usd >= 100",
    ),
    (
        "non_spenders",
        "Non-Spenders",
        "Never purchased.",
        "not computed.purchases >= 1",
    ),
    (
        "new_players",
        "New-Players",
        "First seen within the last 7 days.",
        "computed.first_seen_at within_last 7d",
    ),
    (
        "lapsed_players",
        "Lapsed-Players",
        "Not seen for 7 days or more.",
        "computed.last_seen_at older_than 7d",
    ),
    (
        "ios",
        "iOS-Players",
        "Playing on iOS.",
        r#"default.platform == "ios""#,
    ),
    (
        "android",
        "Android-Players",
        "Playing on Android.",
        r#"default.platform == "android""#,
    ),
];

/// Default metrics: key, name, kind, event, aggregation, field.
pub const DEFAULT_METRICS: &[(&str, &str, &str, &str, &str, &str)] = &[
    (
        "active_users",
        "Active-Users",
        "monitor",
        "$session_start",
        "unique",
        "",
    ),
    (
        "sessions",
        "Sessions",
        "monitor",
        "$session_start",
        "count",
        "",
    ),
    (
        "revenue",
        "Revenue",
        "goal",
        "purchaseCompleted",
        "sum",
        "amount_usd",
    ),
    (
        "purchases",
        "Purchases",
        "goal",
        "purchaseCompleted",
        "count",
        "",
    ),
    (
        "paying_users",
        "Paying-Users",
        "goal",
        "purchaseCompleted",
        "unique",
        "",
    ),
    (
        "ad_impressions",
        "Ad-Impressions",
        "monitor",
        "adImpression",
        "count",
        "",
    ),
];

/// Creates whichever default audiences / metrics are missing. Existing keys are left as they are.
pub async fn seed_defaults(state: &AppState) -> Result<Value> {
    use crate::services::{audiences::AudiencePatch, metrics::MetricPatch};
    let mut audiences = Vec::new();
    for (key, name, description, expr) in DEFAULT_AUDIENCES {
        if state.audiences.get(key).await.is_ok() {
            continue;
        }
        let condition = zanshin_core::expr::parse(expr)?;
        state
            .audiences
            .create(
                key,
                AudiencePatch {
                    name: Some(name.to_string()),
                    description: Some(description.to_string()),
                    condition: Some(condition),
                    enabled: Some(true),
                    tags: Some(vec!["default".into()]),
                    ..Default::default()
                },
            )
            .await?;
        audiences.push(*key);
    }
    let mut metrics_created = Vec::new();
    for (key, name, kind, event, agg, field) in DEFAULT_METRICS {
        if state.metrics.get(key).await.is_ok() {
            continue;
        }
        let patch: MetricPatch = serde_json::from_value(json!({
            "name": name, "kind": kind, "event_name": event, "aggregation": agg,
            "field": if field.is_empty() { Value::Null } else { json!(field) },
            "tags": ["default"],
        }))?;
        state.metrics.create(key, patch).await?;
        metrics_created.push(*key);
    }
    Ok(json!({ "audiences": audiences, "metrics": metrics_created }))
}

pub async fn generate_sample(state: &AppState, req: &SampleRequest) -> Result<Value> {
    if !(1..=5000).contains(&req.players) || !(1..=90).contains(&req.days) {
        return Err(Error::invalid("players must be 1..5000 and days 1..90"));
    }
    let mut rng = Lcg(req.seed ^ 0x9e37_79b9_7f4a_7c15);
    let countries = ["US", "DE", "FR", "BR", "JP", "GB", "KR", "RU"];
    let platforms = ["ios", "android", "windows"];
    let mut events_total = 0usize;
    let mut revenue: f64 = 0.0;
    let now = Utc::now();
    for i in 0..req.players {
        let signup_day = (rng.next() % req.days as u64) as i64;
        let mut default = Map::new();
        default.insert("country".into(), json!(rng.pick(&countries)));
        default.insert("platform".into(), json!(rng.pick(&platforms)));
        default.insert(
            "app_version".into(),
            json!(format!("1.{}.0", rng.next() % 5)),
        );
        let mut custom = Map::new();
        custom.insert("sample".into(), json!(true));
        custom.insert("level".into(), json!(1));
        let identity = state
            .identity
            .upsert(&format!("sample-{}-{i}", req.seed), default, custom)
            .await?;
        let whale = rng.chance(0.05);
        let mut level = 1i64;
        let mut inputs: Vec<EventInput> = Vec::new();
        for d in (0..=signup_day).rev() {
            let retain_p = if d == signup_day {
                1.0
            } else {
                0.55 * (0.93f64).powi((signup_day - d) as i32)
            };
            if !rng.chance(retain_p) {
                continue;
            }
            let base = now - Duration::days(d) - Duration::minutes((rng.next() % 1200) as i64);
            let mut push = |name: &str,
                            value: Option<String>,
                            metadata: Option<Map<String, Value>>,
                            offset_secs: i64| {
                inputs.push(EventInput {
                    name: name.into(),
                    value,
                    metadata,
                    timestamp: Some(base + Duration::seconds(offset_secs)),
                    id: None,
                });
            };
            push("session_start", None, None, 0);
            let levels = 1 + (rng.next() % 3) as i64;
            for k in 0..levels {
                level += 1;
                push("level_up", Some(level.to_string()), None, 60 + k * 90);
            }
            if rng.chance(if whale { 0.5 } else { 0.04 }) {
                let amount = [0.99, 4.99, 9.99, 19.99, 49.99][(rng.next() % 5) as usize];
                revenue += amount;
                let mut m = Map::new();
                m.insert("amount".into(), json!(amount));
                m.insert("currency".into(), json!("USD"));
                m.insert(
                    "sku".into(),
                    json!(format!("gems_{}", (amount * 100.0) as i64)),
                );
                push("purchase", None, Some(m), 400);
            }
            push(
                "session_end",
                Some((600 + rng.next() % 1800).to_string()),
                None,
                900,
            );
        }
        events_total += inputs.len();
        for chunk in inputs.chunks(100) {
            state
                .events
                .track(&identity, chunk.to_vec(), false, None)
                .await?;
        }
    }
    Ok(
        json!({ "players": req.players, "days": req.days, "events": events_total, "revenue": (revenue * 100.0).round() / 100.0, "seed": req.seed }),
    )
}

/// Keys of the demo LiveOps configuration; everything the demo creates is prefixed `demo_`
/// and every synthetic player carries `custom.sample = true`, so a reset can find it all again.
const DEMO_PREFIX: &str = "demo_";

fn demo_bundle() -> Value {
    let now = Utc::now();
    let starts = (now - chrono::Duration::days(1)).to_rfc3339();
    let ends = (now + chrono::Duration::days(6)).to_rfc3339();
    json!({
        "format": "zanshin-config/1",
        "entities": {
            "audiences": [
                { "key": "demo_whales", "name": "Demo · Whales", "description": "Players at level 20+ (sample data)",
                  "expression": "custom.level >= 20" },
                { "key": "demo_new_players", "name": "Demo · New players", "description": "Level below 5 (sample data)",
                  "expression": "custom.level < 5" },
                { "key": "demo_eu", "name": "Demo · Europe", "description": "DE / FR / GB (sample data)",
                  "expression": "default.country in ['DE', 'FR', 'GB']" }
            ],
            "flags": [
                { "key": "demo_shop_v2", "name": "Demo · Shop v2", "value_type": "bool", "default_value": false, "enabled": true,
                  "description": "Sample flag: new shop layout, rolled out to half of the players",
                  "rollout": 50 },
                { "key": "demo_starter_pack_price", "name": "Demo · Starter pack price", "value_type": "number",
                  "default_value": 4.99, "enabled": true,
                  "description": "Sample flag: price is varied by the demo experiment" },
                { "key": "demo_welcome_text", "name": "Demo · Welcome text", "value_type": "string",
                  "default_value": "Welcome back!", "enabled": true,
                  "description": "Sample flag: greeting shown on the title screen" }
            ],
            "experiments": [
                { "key": "demo_starter_pack_pricing", "name": "Demo · Starter pack pricing", "status": "running",
                  "flag_key": "demo_starter_pack_price", "control": "a", "description": "Sample A/B test on the starter pack price",
                  "variants": [ { "key": "a", "value": 4.99, "weight": 1 }, { "key": "b", "value": 2.99, "weight": 1 } ] }
            ],
            "live_events": [
                { "key": "demo_weekend_sale", "name": "Demo · Weekend sale", "description": "Sample live event with a 20% discount",
                  "starts_at": starts, "ends_at": ends, "payload": { "discount": 20 }, "enabled": true }
            ]
        }
    })
}

#[derive(Debug, Deserialize)]
pub struct DemoRequest {
    #[serde(default = "default_demo_players")]
    pub players: u32,
    #[serde(default = "default_days")]
    pub days: u32,
    #[serde(default)]
    pub seed: u64,
    /// Skip the synthetic players/events and only create the demo configuration.
    #[serde(default)]
    pub config_only: bool,
}
fn default_demo_players() -> u32 {
    500
}

/// Seed a fresh deployment with demo configuration (flags, audiences, experiment, live event)
/// plus synthetic players and events. Idempotent: configuration is merged by key and sample
/// players are only generated when none exist yet.
pub async fn demo_seed(state: &AppState, req: &DemoRequest) -> Result<Value> {
    let config = super::config::import(
        state,
        &super::config::ImportRequest {
            bundle: demo_bundle(),
            mode: super::config::ImportMode::Merge,
            kinds: None,
            keys: None,
            dry_run: false,
        },
    )
    .await?;
    let repo = zanshin_store::identities::IdentityRepo::new(state.stores.pg.clone());
    let existing = repo.ids_where_custom("sample", &json!(true)).await?.len();
    let sample = if req.config_only {
        json!({ "skipped": "config_only" })
    } else if existing > 0 {
        json!({ "skipped": "sample players already present", "players": existing })
    } else {
        generate_sample(
            state,
            &SampleRequest {
                players: req.players,
                days: req.days,
                seed: req.seed,
            },
        )
        .await?
    };
    Ok(json!({ "config": config, "sample": sample }))
}

/// Remove everything `demo_seed` created: synthetic players (and their events, assignments,
/// inbox, …) and every `demo_*` configuration entity.
pub async fn demo_reset(state: &AppState) -> Result<Value> {
    let repo = zanshin_store::identities::IdentityRepo::new(state.stores.pg.clone());
    let ids = repo.ids_where_custom("sample", &json!(true)).await?;
    let mut identities = 0usize;
    for id in ids {
        if gdpr_delete(state, id).await.is_ok() {
            identities += 1;
        }
    }
    let mut removed = vec![];
    for kind in ["experiments", "live_events", "flags", "audiences"] {
        let keys: Vec<String> = match kind {
            "experiments" => state
                .experiments
                .list()
                .await
                .iter()
                .map(|e| e.key.clone())
                .collect(),
            "live_events" => state
                .live_events
                .list()
                .await
                .iter()
                .map(|e| e.key.clone())
                .collect(),
            "flags" => state
                .flags
                .list()
                .await
                .iter()
                .map(|f| f.key.clone())
                .collect(),
            _ => state
                .audiences
                .list()
                .await
                .iter()
                .map(|a| a.key.clone())
                .collect(),
        };
        for key in keys.into_iter().filter(|k| k.starts_with(DEMO_PREFIX)) {
            if super::config::remove(state, kind, &key).await.is_ok() {
                removed.push(json!({ "kind": kind, "key": key }));
            }
        }
    }
    Ok(json!({ "identities_deleted": identities, "config_removed": removed }))
}
