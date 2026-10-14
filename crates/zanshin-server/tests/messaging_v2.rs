//! End-to-end tests for messaging v2: templates, schedules, integrations, delivery log, inbox delete.
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-msgv2";

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

struct Hit {
    signature: String,
    timestamp: String,
    body: Vec<u8>,
}

async fn webhook_receiver() -> (String, std::sync::Arc<tokio::sync::Mutex<Vec<Hit>>>) {
    use axum::{body::Bytes, extract::State, http::HeaderMap, routing::post, Router};
    let hits = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<Hit>::new()));
    let app = Router::new()
        .route(
            "/hook",
            post(
                |State(h): State<std::sync::Arc<tokio::sync::Mutex<Vec<Hit>>>>,
                 headers: HeaderMap,
                 body: Bytes| async move {
                    h.lock().await.push(Hit {
                        signature: headers
                            .get("x-zanshin-signature")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_string(),
                        timestamp: headers
                            .get("x-zanshin-timestamp")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_string(),
                        body: body.to_vec(),
                    });
                    "ok"
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
    mac.update(&hit.body);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    hit.signature == format!("sha256={hex}")
}

#[tokio::test]
async fn messaging_v2_flow() {
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
    let ext = format!("ana_{sfx}");
    let s: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .json(&json!({"id": ext, "custom": {"name": "Ana", "locale": "de"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tok = s["token"].as_str().unwrap().to_string();
    let inbox = |tok: String| {
        let c = c.clone();
        let base = base.clone();
        async move {
            let (_, v) = call(
                &c,
                &tok,
                reqwest::Method::GET,
                format!("{base}/v1/messages"),
                None,
            )
            .await;
            v["messages"].as_array().cloned().unwrap_or_default()
        }
    };

    // Templates: validation, locale fallback, preview, test send.
    let tkey = format!("welcome_{sfx}");
    let (st, bad) = adm(
        reqwest::Method::POST,
        "/admin/v1/templates".into(),
        Some(json!({"key": tkey, "default_locale": "en", "translations": {"de": {"title": "x"}}})),
    )
    .await;
    assert_eq!(st, 400, "missing default locale: {bad}");
    let (st, t) = adm(reqwest::Method::POST, "/admin/v1/templates".into(), Some(json!({
        "key": tkey, "name": "Welcome", "format": "plaintext", "default_locale": "en",
        "translations": {"en": {"title": "Hi {{custom.name}}", "body": "en body"}, "de": {"title": "Hallo {{custom.name}}", "body": "de body"}},
        "payload": {"screen": "home"}
    }))).await;
    assert_eq!(st, 201, "{t}");
    let (st, r) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/templates/{tkey}/render"),
        Some(json!({"locale": "de-AT"})),
    )
    .await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["rendered"]["locale"], "de");
    let (_, r) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/templates/{tkey}/render"),
        Some(json!({"locale": "fr"})),
    )
    .await;
    assert_eq!(r["rendered"]["locale"], "en");
    let (st, r) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/templates/{tkey}/test"),
        Some(json!({"external_id": ext})),
    )
    .await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(
        r["rendered"]["title"], "Hallo Ana",
        "player locale from custom.locale"
    );
    let msgs = inbox(tok.clone()).await;
    let test_msg = msgs
        .iter()
        .find(|m| m["key"] == format!("template:{tkey}"))
        .unwrap_or_else(|| panic!("test send in inbox: {msgs:?}"));
    assert_eq!(test_msg["title"], "Hallo Ana");

    // Webhook integration with HMAC.
    let (url, hits) = webhook_receiver().await;
    let ikey = format!("hook_{sfx}");
    let (st, i) = adm(
        reqwest::Method::POST,
        "/admin/v1/integrations".into(),
        Some(json!({"key": ikey, "kind": "webhook", "config": {"url": url, "secret": "s3cret"}})),
    )
    .await;
    assert_eq!(st, 201, "{i}");
    assert_eq!(i["config"]["secret"], "••••", "secret masked");
    let (st, o) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/integrations/{ikey}/test"),
        Some(json!({"external_id": ext})),
    )
    .await;
    assert_eq!(st, 200, "{o}");
    assert_eq!(o["status"], "sent", "{o}");
    {
        let h = hits.lock().await;
        assert_eq!(h.len(), 1);
        assert!(verify("s3cret", &h[0]), "signature must verify");
        let body: Value = serde_json::from_slice(&h[0].body).unwrap();
        assert_eq!(body["external_id"], ext);
    }
    // Sending the mask back keeps the secret.
    let (st, _) = adm(
        reqwest::Method::PUT,
        format!("/admin/v1/integrations/{ikey}"),
        Some(json!({"config": {"url": url, "secret": "••••"}})),
    )
    .await;
    assert_eq!(st, 200);

    // Campaign rendered from the template, delivered to inbox + webhook, logged per channel.
    let mkey = format!("promo_{sfx}");
    let (st, bad) = adm(
        reqwest::Method::POST,
        "/admin/v1/messages".into(),
        Some(json!({"key": format!("bad_{sfx}"), "title": "x", "channels": ["inbox", "nope"]})),
    )
    .await;
    assert_eq!(st, 400, "unknown channel: {bad}");
    let (st, m) = adm(reqwest::Method::POST, "/admin/v1/messages".into(), Some(json!({"key": mkey, "title": "inline", "template_key": tkey, "channels": ["inbox", ikey], "enabled": true}))).await;
    assert_eq!(st, 201, "{m}");
    let msgs = inbox(tok.clone()).await;
    let promo = msgs
        .iter()
        .find(|m| m["key"] == mkey)
        .expect("campaign delivered");
    assert_eq!(
        promo["title"], "Hallo Ana",
        "template wins over inline title"
    );
    assert_eq!(
        promo["payload"]["screen"], "home",
        "template payload used when campaign payload is null"
    );
    assert_eq!(
        hits.lock().await.len(),
        2,
        "webhook called for the campaign"
    );
    let (st, d) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/messages/{mkey}/deliveries"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{d}");
    let stats = d["stats"].as_array().unwrap();
    assert!(
        stats
            .iter()
            .any(|s| s["channel"] == "inbox" && s["status"] == "sent" && s["count"] == 1),
        "{d}"
    );
    assert!(
        stats
            .iter()
            .any(|s| s["channel"] == ikey && s["status"] == "sent" && s["count"] == 1),
        "{d}"
    );
    assert_eq!(d["items"].as_array().unwrap().len(), 2);
    let (st, _) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/templates/{tkey}"),
        None,
    )
    .await;
    assert_eq!(st, 409, "template in use");

    // Cron schedule: one delivery per occurrence, not per fetch.
    let ckey = format!("daily_{sfx}");
    let (st, m) = adm(reqwest::Method::POST, "/admin/v1/messages".into(), Some(json!({"key": ckey, "title": "tick", "schedule": {"kind": "cron", "cron": "0 * * * * *"}, "enabled": true}))).await;
    assert_eq!(st, 201, "{m}");
    let n1 = inbox(tok.clone())
        .await
        .iter()
        .filter(|m| m["key"] == ckey)
        .count();
    let n2 = inbox(tok.clone())
        .await
        .iter()
        .filter(|m| m["key"] == ckey)
        .count();
    assert_eq!((n1, n2), (1, 1));
    let (st, bad) = adm(reqwest::Method::POST, "/admin/v1/messages".into(), Some(json!({"key": format!("badcron_{sfx}"), "title": "x", "schedule": {"kind": "cron", "cron": "nope"}}))).await;
    assert_eq!(st, 400, "{bad}");

    // Live-event schedule: delivered while the event's occurrence window is open.
    let ekey = format!("rush_{sfx}");
    let starts = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let (st, e) = adm(
        reqwest::Method::POST,
        "/admin/v1/live-events".into(),
        Some(json!({"key": ekey, "starts_at": starts, "duration_secs": 7200, "enabled": true})),
    )
    .await;
    assert_eq!(st, 201, "{e}");
    let lkey = format!("rush_msg_{sfx}");
    let (st, m) = adm(reqwest::Method::POST, "/admin/v1/messages".into(), Some(json!({"key": lkey, "title": "Rush is on", "schedule": {"kind": "live_event", "live_event": ekey, "anchor": "start"}, "enabled": true}))).await;
    assert_eq!(st, 201, "{m}");
    let msgs = inbox(tok.clone()).await;
    let rush: Vec<_> = msgs.iter().filter(|m| m["key"] == lkey).collect();
    assert_eq!(rush.len(), 1, "{msgs:?}");
    assert_eq!(
        inbox(tok.clone())
            .await
            .iter()
            .filter(|m| m["key"] == lkey)
            .count(),
        1
    );
    let (st, bad) = adm(reqwest::Method::POST, "/admin/v1/messages".into(), Some(json!({"key": format!("badev_{sfx}"), "title": "x", "schedule": {"kind": "live_event", "live_event": "missing_event"}}))).await;
    assert_eq!(st, 400, "{bad}");

    // Inbox delete (player + admin).
    let rush_id = rush[0]["id"].as_str().unwrap().to_string();
    let (st, _) = call(
        &c,
        &tok,
        reqwest::Method::DELETE,
        format!("{base}/v1/inbox/{rush_id}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert!(inbox(tok.clone()).await.iter().all(|m| m["id"] != rush_id));
    let identity_id = test_msg["identity_id"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| r["identity_id"].as_str().unwrap().to_string());
    let promo_id = promo["id"].as_str().unwrap();
    let (st, _) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/identities/{identity_id}/inbox/{promo_id}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert!(inbox(tok.clone()).await.iter().all(|m| m["id"] != promo_id));
}
