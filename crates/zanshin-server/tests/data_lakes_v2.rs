//! End-to-end tests for data lakes v2: sink validation (Snowflake / Redshift / Parquet /
//! partitioning) and notification channels (CRUD, test send, config-change fan-out).
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-datalakes2";

async fn start() -> Option<(String, reqwest::Client)> {
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
    Some((format!("http://{addr}"), reqwest::Client::new()))
}

async fn call(
    c: &reqwest::Client,
    key: &str,
    m: reqwest::Method,
    url: String,
    body: Option<Value>,
) -> (u16, Value) {
    let mut r = c.request(m, url).bearer_auth(key);
    if let Some(b) = body {
        r = r.json(&b);
    }
    let resp = r.send().await.unwrap();
    (
        resp.status().as_u16(),
        resp.json::<Value>().await.unwrap_or(Value::Null),
    )
}

/// Tiny HTTP receiver standing in for a Slack / Discord / webhook endpoint.
async fn receiver() -> (String, Arc<tokio::sync::Mutex<Vec<Value>>>) {
    use axum::{routing::post, Router};
    let got: Arc<tokio::sync::Mutex<Vec<Value>>> = Default::default();
    let g = got.clone();
    let app = Router::new().route(
        "/{*rest}",
        post(move |axum::Json(v): axum::Json<Value>| {
            let g = g.clone();
            async move {
                g.lock().await.push(v);
                "ok"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), got)
}

#[tokio::test]
async fn data_lakes_v2_flow() {
    let Some((base, c)) = start().await else {
        return;
    };
    let sfx = uuid::Uuid::now_v7().simple().to_string();
    let adm = |m: reqwest::Method, p: String, b: Option<Value>| {
        call(&c, ADMIN_KEY, m, format!("{base}{p}"), b)
    };

    // --- sink validation ----------------------------------------------------------------------
    let bad = |kind: &str, cfg: Value, format: &str| {
        adm(
            reqwest::Method::POST,
            "/admin/v1/export-sinks".into(),
            Some(
                json!({"key": format!("sink_{sfx}"), "kind": kind, "config": cfg, "format": format}),
            ),
        )
    };
    let s3 = json!({"bucket": "b", "region": "eu-west-1", "access_key": "a", "secret_key": "s"});
    let (st, b) = bad("snowflake", s3.clone(), "jsonl").await;
    assert_eq!(st, 400, "snowflake needs account/host/...: {b}");
    let (st, b) = bad("redshift", json!({"bucket": "b", "region": "r", "access_key": "a", "secret_key": "s", "database": "d", "table": "t", "iam_role": "arn"}), "jsonl").await;
    assert_eq!(st, 400, "redshift needs cluster_id xor workgroup: {b}");
    let (st, b) = bad("http", json!({"url": "https://x.example"}), "parquet").await;
    assert_eq!(st, 400, "parquet only for S3-backed sinks: {b}");
    let mut part = s3.clone();
    part["partition"] = json!("weird");
    let (st, b) = bad("s3", part, "jsonl").await;
    assert_eq!(st, 400, "{b}");
    let mut ok = s3.clone();
    ok["partition"] = json!("event");
    let (st, b) = bad("s3", ok, "parquet").await;
    assert_eq!(st, 201, "{b}");
    assert_eq!(b["format"], json!("parquet"));
    let mut rs = s3.clone();
    for (k, v) in [
        ("database", "d"),
        ("table", "t"),
        ("iam_role", "arn:aws:iam::1:role/x"),
        ("workgroup", "wg"),
    ] {
        rs[k] = json!(v);
    }
    let (st, b) = adm(
        reqwest::Method::POST,
        "/admin/v1/export-sinks".into(),
        Some(
            json!({"key": format!("rs_{sfx}"), "kind": "redshift", "config": rs, "format": "csv"}),
        ),
    )
    .await;
    assert_eq!(st, 201, "{b}");
    assert_eq!(b["kind"], json!("redshift"));

    // --- notification channels ----------------------------------------------------------------
    let (rx_url, got) = receiver().await;
    let (st, b) = adm(
        reqwest::Method::GET,
        "/admin/v1/notification-channels/topics".into(),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert!(b["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t == "export.failed"));
    let (st, b) = adm(
        reqwest::Method::POST,
        "/admin/v1/notification-channels".into(),
        Some(json!({"key": format!("slack_{sfx}"), "kind": "slack", "url": rx_url})),
    )
    .await;
    assert_eq!(st, 400, "slack needs hooks.slack.com url: {b}");
    let (st, b) = adm(
        reqwest::Method::POST,
        "/admin/v1/notification-channels".into(),
        Some(json!({"key": format!("hook_{sfx}"), "kind": "webhook", "url": format!("{rx_url}/notify"),
                    "topics": ["config.changed", "test"], "kinds": ["flags"]})),
    )
    .await;
    assert_eq!(st, 201, "{b}");
    let ch_key = b["key"].as_str().unwrap().to_string();
    let (st, b) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/notification-channels/{ch_key}/test"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["ok"], json!(true), "{b}");
    assert_eq!(b["status"], json!(200));

    // a flag change fans out as config.changed (kind flags); an audience change is filtered out.
    let flag_key = format!("nf_{sfx}");
    let (st, b) = adm(
        reqwest::Method::POST,
        "/admin/v1/flags".into(),
        Some(json!({"key": flag_key, "value_type": "string", "default_value": "1"})),
    )
    .await;
    assert!(st < 300, "{b}");
    let (st, b) = adm(
        reqwest::Method::POST,
        "/admin/v1/audiences".into(),
        Some(json!({"key": format!("na_{sfx}"), "name": "x", "expression": format!("custom.cohort == \"{sfx}\"")})),
    )
    .await;
    assert!(st < 300, "{b}");
    let mut seen = Vec::new();
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        seen = got.lock().await.clone();
        if seen.iter().any(|v| v["topic"] == json!("config.changed")) {
            break;
        }
    }
    let cfg: Vec<&Value> = seen
        .iter()
        .filter(|v| v["topic"] == json!("config.changed"))
        .collect();
    assert_eq!(cfg.len(), 1, "{seen:?}");
    assert_eq!(cfg[0]["kind"], json!("flags"));
    assert!(
        cfg[0]["text"].as_str().unwrap().starts_with("flags"),
        "{}",
        cfg[0]
    );
    assert_eq!(cfg[0]["fields"][1]["value"], json!("/admin/v1/flags"));
    assert!(seen.iter().any(|v| v["topic"] == json!("test")));

    // list masks nothing for webhook kind; update + disable + delete
    let (st, b) = adm(
        reqwest::Method::PUT,
        format!("/admin/v1/notification-channels/{ch_key}"),
        Some(json!({"enabled": false, "topics": ["*"]})),
    )
    .await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["enabled"], json!(false));
    assert_eq!(b["last_status"], json!(200));
    let (_, l) = adm(
        reqwest::Method::GET,
        "/admin/v1/notification-channels".into(),
        None,
    )
    .await;
    assert!(l["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["key"] == json!(ch_key)));

    // cleanup
    for p in [
        format!("/admin/v1/notification-channels/{ch_key}"),
        format!("/admin/v1/export-sinks/sink_{sfx}"),
        format!("/admin/v1/export-sinks/rs_{sfx}"),
        format!("/admin/v1/flags/{flag_key}"),
        format!("/admin/v1/audiences/na_{sfx}"),
    ] {
        let (st, b) = adm(reqwest::Method::DELETE, p.clone(), None).await;
        assert!(st < 300, "{p}: {b}");
    }
}
