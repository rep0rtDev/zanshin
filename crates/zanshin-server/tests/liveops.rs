//! End-to-end tests for live events v2 (join / sticky / participation / flag overrides / QA time)
//! and the flag explain chain. Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-liveops";

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

fn find<'a>(list: &'a Value, key: &str) -> Option<&'a Value> {
    list["live_events"]
        .as_array()?
        .iter()
        .find(|e| e["key"] == key)
}

#[tokio::test]
async fn live_events_v2_and_flag_explain() {
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
    let xid = format!("p_{sfx}");
    let sess: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .json(&json!({"id": xid, "custom": {"vip": true}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tok = sess["token"].as_str().unwrap().to_string();
    let iid = sess["identity"]["id"]
        .as_str()
        .or(sess["identity_id"].as_str())
        .unwrap()
        .to_string();
    let now = chrono::Utc::now();
    let cli = |m: reqwest::Method, p: String, b: Option<Value>| {
        call(&c, &tok, m, format!("{base}{p}"), b)
    };

    // Flag with a rule; the live event overrides it for participants.
    let fkey = format!("boost_{sfx}");
    let (st, _) = adm(reqwest::Method::POST, "/admin/v1/flags".into(), Some(json!({
        "key": fkey, "value_type": "number", "default_value": 1,
        "rules": [{"id": "vip", "condition": {"prop": "custom.vip", "op": "eq", "value": true}, "value": 2}]
    }))).await;
    assert_eq!(st, 201);
    let (_, f) = cli(
        reqwest::Method::GET,
        format!("/v1/flags?names={fkey}"),
        None,
    )
    .await;
    assert_eq!(f["flags"][0]["value"], 2);
    assert_eq!(f["flags"][0]["reason"]["kind"], "rule");

    // Explicit-join event with a 2-second participation timer, sticky, open now.
    let ekey = format!("raid_{sfx}");
    let (st, e) = adm(reqwest::Method::POST, "/admin/v1/live-events".into(), Some(json!({
        "key": ekey, "name": "Raid", "starts_at": (now - chrono::Duration::minutes(1)).to_rfc3339(),
        "duration_secs": 3600, "payload": {"boss": "dragon"},
        "join_mode": "explicit", "sticky": true, "participation_secs": 2,
        "flag_overrides": {fkey.clone(): 10}
    }))).await;
    assert_eq!(st, 201, "{e}");
    assert_eq!(e["join_mode"], "explicit");
    let (_, l) = cli(
        reqwest::Method::GET,
        "/v1/live-events?upcoming=true".into(),
        None,
    )
    .await;
    let view = find(&l, &ekey).expect("targeted event listed");
    assert_eq!(
        view["active"], false,
        "explicit event inactive before join: {view}"
    );
    assert_eq!(view["join_required"], true);
    let (_, f) = cli(
        reqwest::Method::GET,
        format!("/v1/flags?names={fkey}"),
        None,
    )
    .await;
    assert_eq!(f["flags"][0]["value"], 2, "no override before join");

    // Join → active, timer set, flag overridden with reason live_event, explain shows the layer.
    let (st, j) = cli(
        reqwest::Method::POST,
        format!("/v1/live-events/{ekey}/join"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{j}");
    assert_eq!(j["active"], true);
    assert!(j["joined_at"].is_string() && j["participation_ends_at"].is_string());
    let (_, f) = cli(
        reqwest::Method::GET,
        format!("/v1/flags?names={fkey}"),
        None,
    )
    .await;
    assert_eq!(f["flags"][0]["value"], 10, "{f}");
    assert_eq!(f["flags"][0]["reason"]["kind"], "live_event");
    let (st, ex) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/flags/{fkey}/explain/{iid}"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{ex}");
    assert_eq!(ex["live_event"]["live_event"], ekey);
    assert_eq!(ex["rules"][0]["condition_matches"], true);
    assert_eq!(ex["result"]["value"], 10);
    let (_, p) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/live-events/{ekey}/participants"),
        None,
    )
    .await;
    assert_eq!(p["items"][0]["external_id"], xid);
    let (_, tl) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/live-events/{ekey}/timeline"),
        None,
    )
    .await;
    assert_eq!(tl["participants"]["active"], 1);
    assert!(!tl["occurrences"].as_array().unwrap().is_empty());
    assert_eq!(tl["joins"][0]["count"], 1);

    // Participation timer expires → inactive again, override gone; leave is recorded.
    tokio::time::sleep(std::time::Duration::from_millis(2300)).await;
    let (_, l) = cli(
        reqwest::Method::GET,
        "/v1/live-events?upcoming=true".into(),
        None,
    )
    .await;
    assert_eq!(find(&l, &ekey).unwrap()["active"], false, "timer expired");
    let (_, f) = cli(
        reqwest::Method::GET,
        format!("/v1/flags?names={fkey}"),
        None,
    )
    .await;
    assert_eq!(f["flags"][0]["value"], 2);
    let (st, lv) = cli(
        reqwest::Method::POST,
        format!("/v1/live-events/{ekey}/leave"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(lv["left"], true);
    let (_, p) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/live-events/{ekey}/participants"),
        None,
    )
    .await;
    assert!(p["items"][0]["left_at"].is_string());
    let (st, _) = cli(
        reqwest::Method::POST,
        format!("/v1/live-events/{ekey}/leave"),
        None,
    )
    .await;
    assert_eq!(st, 200);

    // Sticky: re-join (timer resets), then shrink the window so it is closed — still active.
    let (st, _) = cli(
        reqwest::Method::POST,
        format!("/v1/live-events/{ekey}/join"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let (st, _) = adm(
        reqwest::Method::PUT,
        format!("/admin/v1/live-events/{ekey}"),
        Some(json!({"participation_secs": 3600})),
    )
    .await;
    assert_eq!(st, 200);
    let (st, _) = adm(reqwest::Method::PUT, format!("/admin/v1/live-events/{ekey}"), Some(json!({"starts_at": (now - chrono::Duration::hours(3)).to_rfc3339(), "duration_secs": 60}))).await;
    assert_eq!(st, 200);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (_, l) = cli(reqwest::Method::GET, "/v1/live-events".into(), None).await;
    let v = find(&l, &ekey).expect("sticky participant keeps closed event");
    assert_eq!(v["active"], true, "{v}");
    assert!(v["current"].is_null());

    // Joining a closed event is rejected for a fresh player.
    let s2: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .json(&json!({"id": format!("q_{sfx}")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tok2 = s2["token"].as_str().unwrap().to_string();
    let iid2 = s2["identity"]["id"]
        .as_str()
        .or(s2["identity_id"].as_str())
        .unwrap()
        .to_string();
    let (st, _) = call(
        &c,
        &tok2,
        reqwest::Method::POST,
        format!("{base}/v1/live-events/{ekey}/join"),
        None,
    )
    .await;
    assert_eq!(st, 400);

    // QA time travel: a future auto event is active only for the time-travelling identity.
    let fut = format!("xmas_{sfx}");
    let (st, _) = adm(reqwest::Method::POST, "/admin/v1/live-events".into(), Some(json!({
        "key": fut, "starts_at": (now + chrono::Duration::days(30)).to_rfc3339(), "duration_secs": 86400, "payload": {}
    }))).await;
    assert_eq!(st, 201);
    let (_, l) = call(
        &c,
        &tok2,
        reqwest::Method::GET,
        format!("{base}/v1/live-events"),
        None,
    )
    .await;
    assert!(find(&l, &fut).is_none());
    let (st, q) = adm(reqwest::Method::PUT, format!("/admin/v1/identities/{iid2}/qa-time"), Some(json!({"at": (now + chrono::Duration::days(30) + chrono::Duration::hours(1)).to_rfc3339()}))).await;
    assert_eq!(st, 200, "{q}");
    let (_, l) = call(
        &c,
        &tok2,
        reqwest::Method::GET,
        format!("{base}/v1/live-events"),
        None,
    )
    .await;
    assert_eq!(find(&l, &fut).unwrap()["active"], true, "{l}");
    let (_, l) = cli(reqwest::Method::GET, "/v1/live-events".into(), None).await;
    assert!(find(&l, &fut).is_none(), "other identity unaffected");
    let (_, g) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/identities/{iid2}/qa-time"),
        None,
    )
    .await;
    assert!(g["qa_time"].is_string());
    let (st, _) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/identities/{iid2}/qa-time"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let (_, l) = call(
        &c,
        &tok2,
        reqwest::Method::GET,
        format!("{base}/v1/live-events"),
        None,
    )
    .await;
    assert!(find(&l, &fut).is_none());
}
