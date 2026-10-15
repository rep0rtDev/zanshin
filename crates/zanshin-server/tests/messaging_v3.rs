//! End-to-end tests for messaging v3 (frequency caps, quiet hours, copy variants, rich push, email
//! integrations) and journeys v3 (split, goal, exit criteria).
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-msgv3";

async fn start() -> Option<(String, reqwest::Client, AppState)> {
    let db = std::env::var("ZANSHIN_TEST_DATABASE_URL").ok()?;
    let redis = std::env::var("ZANSHIN_TEST_REDIS_URL").ok()?;
    std::env::set_var("ZANSHIN__DATABASE__URL", db);
    std::env::set_var("ZANSHIN__REDIS__URL", redis);
    std::env::set_var(
        "ZANSHIN__AUTH__JWT_SECRET",
        "test-secret-test-secret-test-secret-0123",
    );
    std::env::set_var("ZANSHIN__ADMIN__BOOTSTRAP_KEY", ADMIN_KEY);
    std::env::set_var("ZANSHIN__SERVER__HTTP_ADDR", "127.0.0.1:0");
    std::env::set_var("ZANSHIN__PUSH__PROVIDER", "log");
    let cfg = Config::load(None).expect("config");
    let state = AppState::build(Arc::new(cfg)).await.expect("state");
    let handle = state.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            http::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Some((format!("http://{addr}"), reqwest::Client::new(), handle))
}

async fn call(
    c: &reqwest::Client,
    m: reqwest::Method,
    url: String,
    body: Option<Value>,
) -> (u16, Value) {
    let mut r = c.request(m, url).bearer_auth(ADMIN_KEY);
    if let Some(b) = body {
        r = r.json(&b);
    }
    let resp = r.send().await.unwrap();
    (
        resp.status().as_u16(),
        resp.json::<Value>().await.unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn messaging_v3_flow() {
    let Some((base, c, _state)) = start().await else {
        return;
    };
    use reqwest::Method;
    let (get, post) = (Method::GET, Method::POST);
    let sfx = uuid::Uuid::now_v7().simple().to_string();
    let admin = |p: &str| format!("{base}/admin/v1{p}");

    // --- campaign validation -------------------------------------------------------------------
    let key = format!("m3_{sfx}");
    let base_msg = json!({
        "key": key, "name": "v3", "title": "Hi", "body": "there", "enabled": true,
    });
    let bad = |patch: Value| {
        let mut m = base_msg.clone();
        for (k, v) in patch.as_object().unwrap() {
            m[k] = v.clone();
        }
        m
    };
    for (what, patch) in [
        (
            "cap max 0",
            json!({ "frequency_cap": { "max": 0, "per_secs": 3600 } }),
        ),
        (
            "quiet hour 24",
            json!({ "quiet_hours": { "start_hour": 22, "end_hour": 24 } }),
        ),
        (
            "variant weight 0",
            json!({ "variants": [{ "key": "a", "weight": 0 }, { "key": "b", "weight": 1 }] }),
        ),
        (
            "duplicate variant",
            json!({ "variants": [{ "key": "a", "weight": 1 }, { "key": "a", "weight": 1 }] }),
        ),
        (
            "bad action",
            json!({ "push_options": { "actions": [{ "id": "", "title": "x" }] } }),
        ),
    ] {
        let (st, _) = call(&c, post.clone(), admin("/messages"), Some(bad(patch))).await;
        assert_eq!(st, 400, "{what} must be rejected");
    }
    let (st, m) = call(
        &c,
        post.clone(),
        admin("/messages"),
        Some(bad(json!({
            "frequency_cap": { "max": 1, "per_secs": 86400 },
            "quiet_hours": { "start_hour": 22, "end_hour": 8, "timezone_prop": "default.timezone" },
            "variants": [
                { "key": "a", "weight": 70, "title": "A title" },
                { "key": "b", "weight": 30, "title": "B title" }
            ],
            "push_options": { "sound": "ping", "badge": 1, "deep_link": "game://shop",
                              "actions": [{ "id": "open", "title": "Open", "url": "game://shop" }] }
        }))),
    )
    .await;
    assert_eq!(st, 201, "{m}");
    assert_eq!(m["frequency_cap"]["max"], 1);
    assert_eq!(m["quiet_hours"]["end_hour"], 8);
    assert_eq!(m["variants"].as_array().map(Vec::len), Some(2));
    assert_eq!(m["push_options"]["actions"][0]["id"], "open");

    // variants / deferrals endpoints answer for a fresh campaign
    let (st, v) = call(
        &c,
        get.clone(),
        admin(&format!("/messages/{key}/variants")),
        None,
    )
    .await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["items"].as_array().map(Vec::len), Some(2));
    assert_eq!(v["total_sent"], 0);
    let (st, d) = call(
        &c,
        get.clone(),
        admin(&format!("/messages/{key}/deferrals")),
        None,
    )
    .await;
    assert_eq!(st, 200, "{d}");
    assert_eq!(d["items"].as_array().map(Vec::len), Some(0));

    // sticky weighted variant assignment
    let campaign: zanshin_core::messages::MessageCampaign = serde_json::from_value(m).unwrap();
    let mut a = 0;
    for i in 0..1000 {
        let ext = format!("player-{i}");
        let first = campaign.variant_for(&ext).map(|v| v.key.clone());
        assert_eq!(first, campaign.variant_for(&ext).map(|v| v.key.clone()));
        if first.as_deref() == Some("a") {
            a += 1;
        }
    }
    assert!(
        (600..=800).contains(&a),
        "weighted split off: {a}/1000 in a"
    );

    // --- email integrations ----------------------------------------------------------------------
    let (st, _) = call(
        &c,
        post.clone(),
        admin("/integrations"),
        Some(json!({ "key": format!("smtp_bad_{sfx}"), "name": "x", "kind": "smtp", "config": { "from": "a@b.c" } })),
    )
    .await;
    assert_eq!(st, 400, "smtp without host must be rejected");
    let (st, i) = call(
        &c,
        post.clone(),
        admin("/integrations"),
        Some(
            json!({ "key": format!("sg_{sfx}"), "name": "SendGrid", "kind": "sendgrid",
                     "config": { "api_key": "SG.secret", "from": "noreply@example.com" } }),
        ),
    )
    .await;
    assert_eq!(st, 201, "{i}");
    assert_eq!(i["config"]["api_key"], "••••", "api key must be redacted");
    let (st, i) = call(
        &c,
        post.clone(),
        admin("/integrations"),
        Some(json!({ "key": format!("ses_{sfx}"), "name": "SES", "kind": "ses",
                     "config": { "region": "eu-west-1", "access_key": "AKIA", "secret_key": "s3cret", "from": "noreply@example.com" } })),
    )
    .await;
    assert_eq!(st, 201, "{i}");
    assert_eq!(i["config"]["secret_key"], "••••");

    // --- journeys v3 -------------------------------------------------------------------------------
    let jk = format!("j3_{sfx}");
    let steps = json!([
        { "id": "split", "kind": "split", "branches": [{ "step": "a", "weight": 50 }, { "step": "b", "weight": 50 }] },
        { "id": "a", "kind": "track", "name": "j3_a", "value": "", "metadata": {} },
        { "id": "end_a", "kind": "exit" },
        { "id": "b", "kind": "track", "name": "j3_b", "value": "", "metadata": {} }
    ]);
    let (st, _) = call(
        &c,
        post.clone(),
        admin("/journeys"),
        Some(json!({ "key": jk, "name": "v3", "trigger": { "kind": "manual" },
                     "steps": [{ "id": "split", "kind": "split", "branches": [{ "step": "nope", "weight": 1 }] }] })),
    )
    .await;
    assert_eq!(st, 400, "split to unknown step must be rejected");
    let (st, j) = call(
        &c,
        post.clone(),
        admin("/journeys"),
        Some(
            json!({ "key": jk, "name": "v3", "trigger": { "kind": "manual" }, "steps": steps,
                     "goal": { "event": "j3_goal", "within_secs": 3600 },
                     "exit_when": { "events": ["j3_quit"], "on_goal": true } }),
        ),
    )
    .await;
    assert_eq!(st, 201, "{j}");
    assert_eq!(j["goal"]["event"], "j3_goal");
    assert_eq!(j["exit_when"]["on_goal"], true);
    let (st, r) = call(
        &c,
        get.clone(),
        admin(&format!("/journeys/{jk}/runs")),
        None,
    )
    .await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["conversions"]["runs"], 0);
    assert!(r["conversions"]["by_branch"].is_array());
}
