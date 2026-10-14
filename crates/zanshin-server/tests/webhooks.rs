//! End-to-end tests for outbound webhooks: matching, signing, batching, retries, delivery log.
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-webhooks";

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

#[derive(Clone)]
struct Hit {
    event: String,
    signature: String,
    timestamp: String,
    body: Value,
    raw: Vec<u8>,
}

type Hits = std::sync::Arc<tokio::sync::Mutex<Vec<Hit>>>;

async fn receiver(status: u16) -> (String, Hits) {
    use axum::{body::Bytes, extract::State, http::HeaderMap, routing::post, Router};
    let hits: Hits = Default::default();
    let app = Router::new()
        .route(
            "/hook",
            post(
                move |State(h): State<Hits>, headers: HeaderMap, body: Bytes| async move {
                    let hdr = |n: &str| {
                        headers
                            .get(n)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_string()
                    };
                    h.lock().await.push(Hit {
                        event: hdr("x-zanshin-event"),
                        signature: hdr("x-zanshin-signature"),
                        timestamp: hdr("x-zanshin-timestamp"),
                        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                        raw: body.to_vec(),
                    });
                    (axum::http::StatusCode::from_u16(status).unwrap(), "ok")
                },
            ),
        )
        .with_state(hits.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/hook"), hits)
}

fn verify(secret: &str, hit: &Hit) -> bool {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(hit.timestamp.as_bytes());
    mac.update(b".");
    mac.update(&hit.raw);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    hit.signature == format!("sha256={hex}")
}

async fn wait_for(hits: &Hits, pred: impl Fn(&Hit) -> bool) -> Option<Hit> {
    for _ in 0..60 {
        if let Some(h) = hits.lock().await.iter().find(|h| pred(h)).cloned() {
            return Some(h);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    None
}

fn has_type(h: &Hit, t: &str) -> bool {
    h.body["events"]
        .as_array()
        .is_some_and(|evs| evs.iter().any(|e| e["type"] == t))
}

#[tokio::test]
async fn webhooks_flow() {
    let Some((base, c)) = start().await else {
        return;
    };
    let sfx = uuid::Uuid::now_v7().simple().to_string();
    let adm = |m: reqwest::Method, p: String, b: Option<Value>| {
        call(&c, ADMIN_KEY, m, format!("{base}{p}"), b)
    };
    let (_, k) = adm(
        reqwest::Method::POST,
        "/admin/v1/api-keys".into(),
        Some(json!({"name": "c", "kind": "client"})),
    )
    .await;
    let ck = k["secret"].as_str().unwrap().to_string();

    let (_, types) = adm(
        reqwest::Method::GET,
        "/admin/v1/webhooks/event-types".into(),
        None,
    )
    .await;
    assert!(types["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t == "flag.updated"));

    // Subscription with filters + HMAC.
    let (url, hits) = receiver(200).await;
    let wkey = format!("hook_{sfx}");
    let (st, bad) = adm(
        reqwest::Method::POST,
        "/admin/v1/webhooks".into(),
        Some(json!({"key": wkey, "url": url, "events": ["flag/updated"]})),
    )
    .await;
    assert_eq!(st, 400, "bad pattern: {bad}");
    let (st, w) = adm(reqwest::Method::POST, "/admin/v1/webhooks".into(), Some(json!({
        "key": wkey, "url": url, "secret": "s3cret", "events": ["flag.*", "identity.created", "event.purchase", "webhook.test"]
    }))).await;
    assert_eq!(st, 201, "{w}");
    assert_eq!(w["secret"], "••••");

    // Test send is synchronous and signed.
    let (st, d) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/webhooks/{wkey}/test"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{d}");
    assert_eq!(d["status"], "sent", "{d}");
    assert_eq!(d["attempts"], 1);
    let h = wait_for(&hits, |h| h.event == "webhook.test")
        .await
        .expect("test hit");
    assert!(verify("s3cret", &h), "signature must verify");
    assert_eq!(h.body["webhook"], wkey);
    assert_eq!(h.body["delivery_id"], d["id"]);

    // Config change → flag.updated via the change bus.
    let fkey = format!("f_{sfx}");
    let (st, _) = adm(
        reqwest::Method::POST,
        "/admin/v1/flags".into(),
        Some(json!({"key": fkey, "value_type": "bool", "default_value": false})),
    )
    .await;
    assert_eq!(st, 201);
    let h = wait_for(&hits, |h| has_type(h, "flag.updated"))
        .await
        .expect("flag.updated");
    assert_eq!(h.body["events"][0]["data"]["key"], fkey);
    assert_eq!(h.event, "flag.updated");

    // Player lifecycle + custom events; non-matching events are not delivered.
    let ext = format!("p_{sfx}");
    let s: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .json(&json!({"id": ext}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tok = s["token"].as_str().unwrap().to_string();
    let h = wait_for(&hits, |h| has_type(h, "identity.created"))
        .await
        .expect("identity.created");
    assert_eq!(h.body["events"][0]["data"]["external_id"], ext);
    let (st, _) = call(
        &c,
        &tok,
        reqwest::Method::POST,
        format!("{base}/v1/events"),
        Some(json!({"events": [{"name": "login"}, {"name": "purchase", "value": "9.99"}]})),
    )
    .await;
    assert!(st == 200 || st == 202);
    let h = wait_for(&hits, |h| has_type(h, "event.purchase"))
        .await
        .expect("event.purchase");
    assert_eq!(h.body["events"][0]["data"]["value"], "9.99");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        hits.lock()
            .await
            .iter()
            .all(|h| !has_type(h, "event.login")),
        "login is not subscribed"
    );

    let (st, l) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/webhooks/{wkey}/deliveries"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{l}");
    assert!(
        l["stats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["status"] == "sent" && s["count"].as_i64().unwrap() >= 4),
        "{l}"
    );
    assert!(l["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|d| d["status"] == "sent"));

    // Failing endpoint → failed with backoff, manual retry, dead after max attempts.
    let (bad_url, bad_hits) = receiver(500).await;
    let bkey = format!("bad_{sfx}");
    let (st, _) = adm(
        reqwest::Method::POST,
        "/admin/v1/webhooks".into(),
        Some(json!({"key": bkey, "url": bad_url, "events": ["webhook.test"], "max_attempts": 2})),
    )
    .await;
    assert_eq!(st, 201);
    let (st, d) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/webhooks/{bkey}/test"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{d}");
    assert_eq!(d["status"], "failed", "{d}");
    assert_eq!(d["last_status"], 500);
    assert!(d["next_attempt_at"].is_string());
    assert_eq!(bad_hits.lock().await.len(), 1);
    let did = d["id"].as_str().unwrap();
    let (st, r) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/webhooks/{bkey}/deliveries/{did}/retry"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["status"], "failed");
    assert_eq!(r["attempts"], 1, "manual retry restarts the attempt budget");
    assert_eq!(bad_hits.lock().await.len(), 2);
    let (st, l) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/webhooks/{bkey}/deliveries?status=failed"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(l["items"].as_array().unwrap().len(), 1);
    // Disabled webhooks are skipped by the matcher.
    let (st, _) = adm(
        reqwest::Method::PUT,
        format!("/admin/v1/webhooks/{bkey}"),
        Some(json!({"enabled": false, "secret": "••••"})),
    )
    .await;
    assert_eq!(st, 200);

    // Batching: two events, one request.
    let (burl, bhits) = receiver(200).await;
    let batch_key = format!("batch_{sfx}");
    let (st, _) = adm(reqwest::Method::POST, "/admin/v1/webhooks".into(), Some(json!({"key": batch_key, "url": burl, "events": ["event.batchy"], "batch_size": 2, "batch_window_secs": 60}))).await;
    assert_eq!(st, 201);
    let (st, _) = call(
        &c,
        &tok,
        reqwest::Method::POST,
        format!("{base}/v1/events"),
        Some(
            json!({"events": [{"name": "batchy", "value": "1"}, {"name": "batchy", "value": "2"}]}),
        ),
    )
    .await;
    assert!(st == 200 || st == 202);
    let h = wait_for(&bhits, |h| h.event == "batch")
        .await
        .expect("batched delivery");
    assert_eq!(h.body["events"].as_array().unwrap().len(), 2);
    assert_eq!(bhits.lock().await.len(), 1);

    let (st, _) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/webhooks/{batch_key}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let (st, _) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/webhooks/{batch_key}"),
        None,
    )
    .await;
    assert_eq!(st, 404);
    for key in [&wkey, &bkey] {
        let (st, _) = adm(
            reqwest::Method::DELETE,
            format!("/admin/v1/webhooks/{key}"),
            None,
        )
        .await;
        assert_eq!(st, 200);
    }
}
