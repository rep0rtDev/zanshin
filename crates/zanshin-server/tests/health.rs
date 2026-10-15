//! End-to-end test for the Day-1 Platform → Health overview (`GET /admin/v1/health`).
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::Value;
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-health";

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
async fn health_overview_reports_dependencies_jobs_and_queues() {
    let Some((base, c, state)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL not set");
        return;
    };
    state.jobs.started("purge");
    state
        .jobs
        .finished("purge", std::time::Duration::from_millis(3));
    state.jobs.failed("scheduled_sweep", "lock failed".into());

    let (code, v) = call(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/health"),
        None,
    )
    .await;
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["status"], "warn", "{v}");
    assert!(v["dependencies"]["ok"].as_bool().unwrap(), "{v}");
    assert!(v["dependencies"]["postgres"]["ok"].as_bool().unwrap());
    assert!(v["dependencies"]["redis"]["ok"].as_bool().unwrap());
    assert_eq!(v["process"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["process"]["draining"], false);
    assert!(v["ingest"]["queue_capacity"].as_u64().unwrap() > 0);
    assert!(v["ingest"]["queue_depth"].as_u64().is_some());

    let jobs = v["jobs"].as_array().unwrap();
    let purge = jobs
        .iter()
        .find(|j| j["name"] == "purge")
        .expect("purge job");
    assert_eq!(purge["runs"], 1);
    assert_eq!(purge["interval_secs"], 600);
    assert_eq!(purge["stale"], false);
    let sweep = jobs
        .iter()
        .find(|j| j["name"] == "scheduled_sweep")
        .expect("sweep job");
    assert_eq!(sweep["last_error"], "lock failed");
    let issues: Vec<String> = v["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i.as_str().unwrap().to_string())
        .collect();
    assert!(
        issues.iter().any(|i| i.contains("scheduled_sweep")),
        "{issues:?}"
    );

    let d = &v["webhooks"]["deliveries"];
    for k in [
        "pending",
        "retrying",
        "dead_24h",
        "sent_24h",
        "oldest_pending_secs",
    ] {
        assert!(d[k].as_i64().is_some(), "{k}: {v}");
    }
    assert!(
        v["integrations"].is_array()
            && v["exports"].is_array()
            && v["reports"].is_array()
            && v["alerts"].is_array()
    );

    // the page is read-only: no admin key → 401
    let r = c
        .get(format!("{base}/admin/v1/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);
}
