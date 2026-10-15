//! End-to-end tests for analytics v5: SQL explorer guard, saved queries, reports, metric alerts,
//! path analysis, funnel cohorts and the player timeline (Postgres event store).
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-anv5";

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

#[tokio::test]
async fn analytics_v5_flow() {
    let Some((base, c)) = start().await else {
        return;
    };
    use reqwest::Method;
    let (get, post, put) = (Method::GET, Method::POST, Method::PUT);
    let sfx = uuid::Uuid::now_v7().simple().to_string();
    let adm = |m: Method, p: &str, b: Option<Value>| {
        call(&c, ADMIN_KEY, m, format!("{base}/admin/v1{p}"), b)
    };

    // --- SQL explorer (no ClickHouse in tests) -----------------------------------------------------
    let (st, b) = adm(get.clone(), "/sql", None).await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["available"], false);
    let (st, b) = adm(
        post.clone(),
        "/sql",
        Some(json!({ "sql": "DROP TABLE events" })),
    )
    .await;
    assert_eq!(st, 400, "{b}");
    assert!(
        b["message"].as_str().unwrap_or("").contains("SELECT"),
        "{b}"
    );
    let (st, b) = adm(
        post.clone(),
        "/sql",
        Some(json!({ "sql": "SELECT 1; SELECT 2" })),
    )
    .await;
    assert_eq!(st, 400, "{b}");
    let (st, b) = adm(
        post.clone(),
        "/sql",
        Some(json!({ "sql": "SELECT count() FROM events" })),
    )
    .await;
    assert_eq!(st, 400, "{b}");
    assert!(
        b["message"].as_str().unwrap_or("").contains("ClickHouse"),
        "{b}"
    );

    // --- saved queries --------------------------------------------------------------------------------
    let qk = format!("q_{sfx}");
    let (st, _) = adm(
        post.clone(),
        "/analytics/queries",
        Some(json!({ "key": qk, "sql": "ALTER TABLE events DELETE WHERE 1" })),
    )
    .await;
    assert_eq!(st, 400);
    let (st, b) = adm(post.clone(), "/analytics/queries", Some(json!({ "key": qk, "name": "Events by name", "sql": "SELECT name, count() AS n FROM events GROUP BY name" }))).await;
    assert_eq!(st, 201, "{b}");
    let (st, b) = adm(
        put.clone(),
        &format!("/analytics/queries/{qk}"),
        Some(json!({ "description": "daily" })),
    )
    .await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["description"], "daily");
    assert_eq!(b["version"], 2);
    let (st, b) = adm(get.clone(), "/analytics/queries", None).await;
    assert_eq!(st, 200);
    assert!(b["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|q| q["key"] == qk));
    let (st, _) = adm(post.clone(), &format!("/analytics/queries/{qk}/run"), None).await;
    assert_eq!(st, 400, "run needs ClickHouse");

    // --- reports --------------------------------------------------------------------------------------
    let rk = format!("r_{sfx}");
    let (st, _) = adm(
        post.clone(),
        "/analytics/reports",
        Some(json!({ "key": rk, "query_key": qk, "cron": "not a cron" })),
    )
    .await;
    assert_eq!(st, 400);
    let (st, _) = adm(
        post.clone(),
        "/analytics/reports",
        Some(json!({ "key": rk, "query_key": "missing_query", "cron": "0 0 9 * * *" })),
    )
    .await;
    assert_eq!(st, 404);
    let (st, b) = adm(
        post.clone(),
        "/analytics/reports",
        Some(json!({ "key": rk, "name": "Daily", "query_key": qk, "cron": "0 0 9 * * *", "timezone": "Europe/Berlin", "max_rows": 10 })),
    )
    .await;
    assert_eq!(st, 201, "{b}");
    assert_eq!(b["timezone"], "Europe/Berlin");
    let (st, _) = adm(post.clone(), &format!("/analytics/reports/{rk}/run"), None).await;
    assert_eq!(st, 400, "query cannot run without ClickHouse");
    let (st, b) = adm(get.clone(), &format!("/analytics/reports/{rk}"), None).await;
    assert_eq!(st, 200);
    assert!(b["last_run_at"].is_string(), "{b}");
    assert!(
        b["last_status"].as_str().unwrap().starts_with("error"),
        "{b}"
    );
    let (st, b) = adm(put.clone(), &format!("/analytics/reports/{rk}"), Some(json!({ "email": { "integration": "mail", "to": ["ops@example.com"] }, "enabled": false }))).await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["email"]["to"][0], "ops@example.com");
    assert_eq!(b["enabled"], false);

    // --- events for alerts / paths / cohorts -----------------------------------------------------
    let (_, k) = adm(
        post.clone(),
        "/api-keys",
        Some(json!({ "name": "c", "kind": "client" })),
    )
    .await;
    let ck = k["secret"].as_str().unwrap().to_string();
    let sess: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .json(&json!({ "id": format!("walker_{sfx}") }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tok = sess["token"].as_str().unwrap().to_string();
    let iid = sess["identity"]["id"].as_str().unwrap().to_string();
    let (e_start, e_shop, e_buy) = (
        format!("p_start_{sfx}"),
        format!("p_shop_{sfx}"),
        format!("p_buy_{sfx}"),
    );
    for name in [&e_start, &e_shop, &e_buy, &e_start, &e_shop] {
        let (st, b) = call(&c, &tok, post.clone(), format!("{base}/v1/events"), Some(json!({ "events": [{ "name": name, "value": "1", "metadata": { "amount": 2.5 } }] }))).await;
        assert_eq!(st, 200, "{b}");
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    }
    let mut seen = 0;
    for _ in 0..60 {
        let (_, b) = adm(
            get.clone(),
            &format!("/identities/{iid}/events?limit=50"),
            None,
        )
        .await;
        seen = b["events"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|e| !e["name"].as_str().unwrap_or("$").starts_with('$'))
                    .count()
            })
            .unwrap_or(0);
        if seen >= 5 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(seen >= 5, "events ingested: {seen}");

    // --- metric alerts ----------------------------------------------------------------------------
    let ak = format!("a_{sfx}");
    let (st, _) = adm(post.clone(), "/analytics/alerts", Some(json!({ "key": ak, "event": e_start, "condition": { "kind": "above", "threshold": 1 }, "window_secs": 10 }))).await;
    assert_eq!(st, 400, "window too short");
    let (st, b) = adm(
        post.clone(),
        "/analytics/alerts",
        Some(json!({ "key": ak, "name": "Starts spike", "event": e_start, "aggregation": "count", "condition": { "kind": "above", "threshold": 1 }, "window_secs": 3600, "cooldown_secs": 3600 })),
    )
    .await;
    assert_eq!(st, 201, "{b}");
    let (st, b) = adm(post.clone(), &format!("/analytics/alerts/{ak}/check"), None).await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["value"], 2.0, "{b}");
    assert_eq!(b["fired"], true, "{b}");
    let (st, b) = adm(post.clone(), &format!("/analytics/alerts/{ak}/check"), None).await;
    assert_eq!(st, 200);
    assert_eq!(b["fired"], false, "cooldown: {b}");
    assert_eq!(b["suppressed"], true, "{b}");
    let (st, b) = adm(get.clone(), &format!("/analytics/alerts/{ak}/fires"), None).await;
    assert_eq!(st, 200);
    assert_eq!(b["items"].as_array().map(Vec::len), Some(1), "{b}");
    let (st, b) = adm(put.clone(), &format!("/analytics/alerts/{ak}"), Some(json!({ "aggregation": "sum", "field": "amount", "condition": { "kind": "below", "threshold": 100 } }))).await;
    assert_eq!(st, 200, "{b}");
    let (st, b) = adm(
        post.clone(),
        &format!("/analytics/alerts/{ak}/check?force=true"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["value"], 5.0, "sum of amount: {b}");
    assert_eq!(b["fired"], true, "{b}");
    let (st, b) = adm(get.clone(), &format!("/analytics/alerts/{ak}"), None).await;
    assert_eq!(st, 200);
    assert_eq!(b["last_value"], 5.0, "{b}");
    assert!(b["last_fired_at"].is_string());
    let (st, _) = adm(
        put.clone(),
        &format!("/analytics/alerts/{ak}"),
        Some(json!({ "condition": { "kind": "anomaly", "zscore": 3, "baseline_windows": 2 } })),
    )
    .await;
    assert_eq!(st, 400, "baseline_windows must be >= 3");

    // --- paths ------------------------------------------------------------------------------------
    let from = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let (st, b) = adm(
        post.clone(),
        "/analytics/paths",
        Some(json!({ "event": e_start, "depth": 2, "from": from, "max_gap_secs": 600 })),
    )
    .await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["anchors"], 2, "{b}");
    assert_eq!(b["levels"][0]["items"][0]["name"], e_shop, "{b}");
    assert_eq!(b["levels"][0]["items"][0]["count"], 2, "{b}");
    let seqs = b["sequences"].as_array().unwrap();
    assert!(
        seqs.iter()
            .any(|s| s["steps"] == json!([e_shop, e_buy]) && s["count"] == 1),
        "{b}"
    );
    assert!(
        seqs.iter()
            .any(|s| s["steps"] == json!([e_shop, "$end"]) && s["count"] == 1),
        "{b}"
    );
    let (st, b) = adm(
        post.clone(),
        "/analytics/paths",
        Some(json!({ "event": e_buy, "direction": "before", "depth": 1, "from": from })),
    )
    .await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["levels"][0]["items"][0]["name"], e_shop, "{b}");

    // --- funnel cohort -> managed audience ---------------------------------------------------------------
    let coh = format!("coh_{sfx}");
    let funnel = json!({ "steps": [{ "name": e_start }, { "name": e_buy }], "window_secs": 86400, "from": from });
    let mut req = funnel.clone();
    req["step"] = json!(2);
    req["audience_key"] = json!(coh);
    let (st, b) = adm(post.clone(), "/analytics/funnel/cohort", Some(req.clone())).await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["matched"], 1, "{b}");
    assert_eq!(b["members"], 1, "{b}");
    let (st, b) = adm(get.clone(), &format!("/audiences/{coh}"), None).await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["kind"], "managed", "{b}");
    req["reached"] = json!(false);
    req["audience_key"] = json!(format!("{coh}_drop"));
    let (st, b) = adm(post.clone(), "/analytics/funnel/cohort", Some(req)).await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["matched"], 0, "the only player converted: {b}");
    let mut bad = funnel.clone();
    bad["step"] = json!(1);
    bad["reached"] = json!(false);
    bad["audience_key"] = json!(format!("{coh}_x"));
    let (st, _) = adm(post.clone(), "/analytics/funnel/cohort", Some(bad)).await;
    assert_eq!(st, 400);

    // --- timeline -----------------------------------------------------------------------------------
    let (st, b) = adm(
        get.clone(),
        &format!("/identities/{iid}/timeline?limit=50"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{b}");
    let items = b["items"].as_array().unwrap();
    assert!(
        items.iter().filter(|i| i["kind"] == "event").count() >= 5,
        "{b}"
    );
    assert!(
        items.iter().any(|i| i["kind"] == "session"),
        "a play session was opened by the events: {b}"
    );
    let ts: Vec<&str> = items.iter().map(|i| i["at"].as_str().unwrap()).collect();
    assert!(ts.windows(2).all(|w| w[0] >= w[1]), "newest first");

    // --- cleanup ------------------------------------------------------------------------------------
    let (st, _) = adm(Method::DELETE, &format!("/analytics/reports/{rk}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = adm(Method::DELETE, &format!("/analytics/queries/{qk}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = adm(Method::DELETE, &format!("/analytics/alerts/{ak}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = adm(get.clone(), &format!("/analytics/alerts/{ak}"), None).await;
    assert_eq!(st, 404);
}
