//! End-to-end test for the demo seed / reset (`POST|DELETE /admin/v1/data/demo`).
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-demo";

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
async fn demo_seed_is_idempotent_and_reset_removes_everything() {
    let Some((base, c, _state)) = start().await else {
        eprintln!("skipping: test database not configured");
        return;
    };
    // start clean in case a previous run left demo data behind
    let _ = call(
        &c,
        reqwest::Method::DELETE,
        format!("{base}/admin/v1/data/demo"),
        None,
    )
    .await;

    let (st, b) = call(
        &c,
        reqwest::Method::POST,
        format!("{base}/admin/v1/data/demo"),
        Some(json!({ "players": 25, "days": 3, "seed": 7 })),
    )
    .await;
    assert_eq!(st, 200, "{b}");
    assert!(
        b["config"]["errors"]
            .as_array()
            .map(|e| e.is_empty())
            .unwrap_or(true),
        "{b}"
    );
    assert_eq!(b["sample"]["players"], 25, "{b}");
    assert!(b["sample"]["events"].as_u64().unwrap_or(0) > 0, "{b}");

    for (path, key) in [
        ("flags", "demo_shop_v2"),
        ("audiences", "demo_whales"),
        ("experiments", "demo_starter_pack_pricing"),
        ("live-events", "demo_weekend_sale"),
    ] {
        let (st, v) = call(
            &c,
            reqwest::Method::GET,
            format!("{base}/admin/v1/{path}/{key}"),
            None,
        )
        .await;
        assert_eq!(st, 200, "{path}/{key}: {v}");
    }

    // second seed: config merged, players not duplicated
    let (st, b2) = call(
        &c,
        reqwest::Method::POST,
        format!("{base}/admin/v1/data/demo"),
        Some(json!({ "players": 25, "days": 3, "seed": 7 })),
    )
    .await;
    assert_eq!(st, 200, "{b2}");
    assert_eq!(b2["sample"]["players"], 25, "{b2}");
    assert!(b2["sample"]["skipped"].is_string(), "{b2}");

    let (st, r) = call(
        &c,
        reqwest::Method::DELETE,
        format!("{base}/admin/v1/data/demo"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["identities_deleted"], 25, "{r}");
    assert!(r["config_removed"].as_array().unwrap().len() >= 7, "{r}");
    for (path, key) in [
        ("flags", "demo_shop_v2"),
        ("audiences", "demo_eu"),
        ("live-events", "demo_weekend_sale"),
    ] {
        let (st, _) = call(
            &c,
            reqwest::Method::GET,
            format!("{base}/admin/v1/{path}/{key}"),
            None,
        )
        .await;
        assert_eq!(st, 404, "{path}/{key} should be gone");
    }
    let (_, r2) = call(
        &c,
        reqwest::Method::DELETE,
        format!("{base}/admin/v1/data/demo"),
        None,
    )
    .await;
    assert_eq!(r2["identities_deleted"], 0, "{r2}");
}
