//! Production-readiness behaviour: deep readiness, request metrics, cross-request event
//! idempotency, cluster job locks and file-backed secrets. Needs ZANSHIN_TEST_DATABASE_URL and
//! ZANSHIN_TEST_REDIS_URL like the other e2e suites.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};
use zanshin_store::locks::Lock;

const ADMIN_KEY: &str = "test-bootstrap-admin-key-000";

async fn start() -> Option<(String, reqwest::Client, AppState)> {
    let db = std::env::var("ZANSHIN_TEST_DATABASE_URL").ok()?;
    let redis = std::env::var("ZANSHIN_TEST_REDIS_URL").ok()?;
    let secret_file = std::env::temp_dir().join(format!("zanshin-issuer-{}", std::process::id()));
    std::fs::write(&secret_file, "issuer-from-file\n").unwrap();
    std::env::set_var("ZANSHIN_FILE__AUTH__ISSUER", &secret_file);
    std::env::set_var("ZANSHIN__DATABASE__URL", db);
    std::env::set_var("ZANSHIN__REDIS__URL", redis);
    std::env::set_var(
        "ZANSHIN__AUTH__JWT_SECRET",
        "test-secret-test-secret-test-secret-0123",
    );
    std::env::set_var("ZANSHIN__ADMIN__BOOTSTRAP_KEY", ADMIN_KEY);
    std::env::set_var("ZANSHIN__SERVER__HTTP_ADDR", "127.0.0.1:0");
    std::env::set_var("ZANSHIN__PUSH__PROVIDER", "log");
    std::env::set_var("ZANSHIN__RETENTION__EVENTS", "1d");
    let cfg = Config::load(None).expect("config");
    assert_eq!(
        cfg.auth.issuer, "issuer-from-file",
        "ZANSHIN_FILE__ secret not applied"
    );
    assert_eq!(cfg.retention.events.as_secs(), 86_400);
    let state = AppState::build(Arc::new(cfg)).await.expect("state");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let s = state.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            http::router(s).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Some((format!("http://{addr}"), reqwest::Client::new(), state))
}

#[tokio::test]
async fn readiness_metrics_idempotency_and_locks() {
    let Some((base, http, state)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };

    // Deep readiness reports every backend and the pool.
    let r = http.get(format!("{base}/readyz")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let ready: Value = r.json().await.unwrap();
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["components"]["postgres"]["ok"], true);
    assert_eq!(ready["components"]["redis"]["ok"], true);
    assert!(ready["components"]["pool"]["size"].as_u64().is_some());

    // Per-route metrics exist after the first request.
    let text = http
        .get(format!("{base}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(text.contains("zanshin_http_requests_total"), "{text}");
    assert!(text.contains("route=\"/readyz\""), "{text}");

    // Same client event id in two separate requests is accepted once.
    let created: Value = http
        .post(format!("{base}/admin/v1/api-keys"))
        .header("x-api-key", ADMIN_KEY)
        .json(&json!({"name": "prod-client", "kind": "client"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = created["secret"].as_str().unwrap().to_string();
    let sess: Value = http
        .post(format!("{base}/v1/authenticate"))
        .header("x-api-key", &key)
        .json(&json!({"id": format!("prod-{}", uuid::Uuid::new_v4())}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = sess["token"].as_str().unwrap();
    let id = uuid::Uuid::new_v4();
    let send = |body: Value| {
        http.post(format!("{base}/v1/events"))
            .bearer_auth(token)
            .json(&body)
            .send()
    };
    let first: Value = send(json!({"events": [{"name": "purchase", "value": "1", "id": id}]}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second: Value = send(json!({"events": [{"name": "purchase", "value": "1", "id": id}]}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["accepted"], 1, "{first}");
    assert_eq!(second["accepted"], 0, "{second}");

    // Cluster job lock: a job running on one "replica" is skipped on another.
    let locks = state.locks.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let holder = tokio::spawn(async move {
        locks
            .try_run("it-job", || async move {
                let _ = started_tx.send(());
                let _ = release_rx.await;
            })
            .await
            .unwrap()
    });
    started_rx.await.unwrap();
    assert!(state.locks.is_held("it-job").await.unwrap());
    let skipped = state.locks.try_run("it-job", || async { 1 }).await.unwrap();
    assert_eq!(skipped, Lock::Skipped);
    let _ = release_tx.send(());
    assert_eq!(holder.await.unwrap(), Lock::Ran(()));
    let ran = state.locks.try_run("it-job", || async { 1 }).await.unwrap();
    assert_eq!(ran, Lock::Ran(1));

    // Draining flips readiness to 503 without closing the listener.
    state.begin_drain();
    let r = http.get(format!("{base}/readyz")).send().await.unwrap();
    assert_eq!(r.status(), 503);
    let r = http.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(r.status(), 200);
}
