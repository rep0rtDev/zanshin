//! End-to-end test for the Day-1 onboarding checklist (`GET /admin/v1/onboarding`).
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-onboarding";

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
async fn onboarding_checklist_reflects_real_data() {
    let Some((base, c, _state)) = start().await else {
        eprintln!("skipping: test database not configured");
        return;
    };
    let (st, body) = call(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/onboarding"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{body}");
    let steps = body["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 8);
    assert_eq!(body["total"], 8);
    assert!(body["done"].as_u64().unwrap() <= 8);
    for s in steps {
        for k in ["id", "title", "href", "hint"] {
            assert!(s[k].is_string(), "{k} missing in {s}");
        }
        assert!(s["done"].is_boolean());
    }
    // the backups step is env-gated and never auto-completes
    let backups = steps.iter().find(|s| s["id"] == "backups").unwrap();
    assert_eq!(backups["done"], false);

    let key = format!("onb_{}", uuid::Uuid::new_v4().simple());
    let (st, b) = call(
        &c,
        reqwest::Method::POST,
        format!("{base}/admin/v1/flags"),
        Some(json!({ "key": key, "value_type": "bool", "default_value": true })),
    )
    .await;
    assert!(st == 200 || st == 201, "{b}");
    let (_, body) = call(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/onboarding"),
        None,
    )
    .await;
    let flag_step = body["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "first_flag")
        .unwrap()
        .clone();
    assert_eq!(flag_step["done"], true, "{body}");
}
