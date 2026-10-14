//! End-to-end tests for core events + analytics v4: currency rates → USD, core-schema lock,
//! taxonomy filters, test flag, computed core properties, funnel count/cohort modes, explore
//! overlay, lifetime engagement.
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-analytics4";

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
async fn analytics_v4_flow() {
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

    // --- currency rates ---------------------------------------------------------------------
    let (st, b) = adm(
        reqwest::Method::PUT,
        "/admin/v1/currency-rates".into(),
        Some(json!({"day": "2020-01-01", "rates": {"EUR": 1.25, "jpy": 0.008}})),
    )
    .await;
    assert_eq!(st, 200, "{b}");
    let (st, b) = adm(
        reqwest::Method::PUT,
        "/admin/v1/currency-rates".into(),
        Some(json!({"rates": {"EURO": 1.0}})),
    )
    .await;
    assert_eq!(st, 400, "{b}");
    let (st, b) = adm(
        reqwest::Method::GET,
        "/admin/v1/currency-rates/convert?amount=10&currency=eur".into(),
        None,
    )
    .await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["amount_usd"], json!(12.5), "{b}");
    let (_, b) = adm(
        reqwest::Method::GET,
        "/admin/v1/currency-rates/convert?amount=3&currency=USD".into(),
        None,
    )
    .await;
    assert_eq!(b["amount_usd"], json!(3.0));
    let (_, b) = adm(
        reqwest::Method::GET,
        "/admin/v1/currency-rates/convert?amount=3&currency=XXX".into(),
        None,
    )
    .await;
    assert!(b["amount_usd"].is_null(), "{b}");
    let (_, b) = adm(
        reqwest::Method::GET,
        "/admin/v1/currency-rates?since=2019-12-31".into(),
        None,
    )
    .await;
    assert!(b["currencies"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "JPY"));
    let (st, b) = adm(
        reqwest::Method::POST,
        "/admin/v1/currency-rates/refresh".into(),
        None,
    )
    .await;
    assert_eq!(st, 400, "no feed configured: {b}");

    // --- core catalog is immutable ----------------------------------------------------------
    let (st, b) = adm(
        reqwest::Method::PUT,
        "/admin/v1/taxonomy/events/purchaseCompleted".into(),
        Some(json!({"value_type": "boolean"})),
    )
    .await;
    assert_eq!(st, 400, "{b}");
    let (st, b) = adm(
        reqwest::Method::PUT,
        "/admin/v1/taxonomy/events/currencyGranted".into(),
        Some(json!({"description": "gold grants", "tags": ["economy"]})),
    )
    .await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["is_core"], true);
    let (_, core) = adm(
        reqwest::Method::GET,
        "/admin/v1/taxonomy/events?core=true".into(),
        None,
    )
    .await;
    let core = core.as_array().unwrap();
    assert!(core.iter().all(|d| d["is_core"] == true));
    assert!(core.iter().any(|d| d["name"] == "adImpression"));
    let (_, syn) = adm(
        reqwest::Method::GET,
        "/admin/v1/taxonomy/events?synthetic=true".into(),
        None,
    )
    .await;
    assert!(syn
        .as_array()
        .unwrap()
        .iter()
        .all(|d| d["name"].as_str().unwrap().starts_with('$')));
    let (_, nosyn) = adm(
        reqwest::Method::GET,
        "/admin/v1/taxonomy/events?synthetic=false&core=false".into(),
        None,
    )
    .await;
    assert!(nosyn.as_array().unwrap().iter().all(|d| !d["name"]
        .as_str()
        .unwrap()
        .starts_with('$')
        && d["is_core"] == false));

    // --- events: USD conversion, test flag, computed core props ------------------------------
    let sess: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .json(&json!({"id": format!("buyer_{sfx}"), "custom": {"cohort": sfx}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tok = sess["token"].as_str().unwrap().to_string();
    let iid = sess["identity"]["id"].as_str().unwrap().to_string();
    let ev_name = format!("step_{sfx}");
    let from_ts = (chrono::Utc::now() - chrono::Duration::days(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (st, b) = call(
        &c,
        &tok,
        reqwest::Method::POST,
        format!("{base}/v1/events"),
        Some(json!({"events": [
            {"name": "purchaseCompleted", "value": "sku1", "metadata": {"amount": 10, "currency": "EUR"}},
            {"name": "purchaseCompleted", "value": "sku_qa", "metadata": {"amount": 500, "currency": "USD", "test": true}},
            {"name": "adImpression", "value": "banner", "metadata": {"amount": 0.02, "currency": "USD"}},
            {"name": "currencyGranted", "value": "quest", "metadata": {"currency": "Gold", "amount": 100}},
            {"name": "currencySpent", "value": "shop", "metadata": {"currency": "Gold", "amount": 40}},
            {"name": ev_name, "value": "1"},
            {"name": ev_name, "value": "2", "metadata": {"test": true}}
        ]})),
    )
    .await;
    assert_eq!(st, 200, "{b}");

    let mut computed = Value::Null;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let (_, idn) = adm(
            reqwest::Method::GET,
            format!("/admin/v1/identities/{iid}"),
            None,
        )
        .await;
        let cp = idn["properties"]["computed"].clone();
        if cp.get("currency.gold.spent").is_some() && cp.get("spend_usd").is_some() {
            computed = cp;
            break;
        }
    }
    let num = |k: &str| computed[k].as_f64().unwrap_or(f64::NAN);
    assert_eq!(num("purchases"), 1.0, "{computed}");
    assert_eq!(num("spend_usd"), 12.5, "test purchase excluded: {computed}");
    assert_eq!(num("ad_impressions"), 1.0);
    assert_eq!(num("ad_revenue_usd"), 0.02);
    assert_eq!(num("currency.gold.granted"), 100.0);
    assert_eq!(num("currency.gold.spent"), 40.0);
    assert!(computed.get("last_purchase_at").is_some());
    assert_eq!(
        num(&format!("events.step_{sfx}.count")),
        1.0,
        "test-flagged event not counted: {computed}"
    );

    // stored event carries amount_usd; analytics hide test events unless include_test.
    let (st, evs) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/identities/{iid}/events?name=purchaseCompleted"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{evs}");
    let items = evs["events"].as_array().cloned().unwrap_or_default();
    assert!(
        items
            .iter()
            .any(|e| e["metadata"]["amount_usd"] == json!(12.5)),
        "{evs}"
    );
    let explore = |include_test: bool| {
        adm(
            reqwest::Method::POST,
            "/admin/v1/analytics/explore".into(),
            Some(
                json!({"event": ev_name, "from": from_ts, "filter": {"include_test": include_test}}),
            ),
        )
    };
    let (st, x) = explore(false).await;
    assert_eq!(st, 200, "{x}");
    assert_eq!(x["total"]["count"], json!(1), "{x}");
    let (_, x) = explore(true).await;
    assert_eq!(x["total"]["count"], json!(2), "{x}");

    // overlay: several events in one call
    let (st, ov) = adm(
        reqwest::Method::POST,
        "/admin/v1/analytics/explore".into(),
        Some(json!({"events": [ev_name, "currencyGranted"], "from": from_ts})),
    )
    .await;
    assert_eq!(st, 200, "{ov}");
    assert_eq!(ov["series"].as_array().unwrap().len(), 2);
    assert_eq!(ov["series"][0]["event"], json!(ev_name));

    // --- funnel count / cohort modes --------------------------------------------------------
    let funnel = |count: &str, cohort: &str| {
        adm(
            reqwest::Method::POST,
            "/admin/v1/analytics/funnel".into(),
            Some(json!({
                "steps": [{"name": "currencyGranted"}, {"name": "currencySpent"}],
                "from": from_ts, "model": "unlimited", "count": count, "cohort": cohort,
                "filter": {"include_test": true}
            })),
        )
    };
    let (st, f) = funnel("unique", "static").await;
    assert_eq!(st, 200, "{f}");
    assert!(f["steps"][1]["count"].as_i64().unwrap() >= 1, "{f}");
    let (st, f) = funnel("total", "shifting").await;
    assert_eq!(st, 200, "{f}");
    assert!(f["steps"][0]["count"].as_i64().unwrap() >= 1, "{f}");
    let (st, f) = adm(
        reqwest::Method::POST,
        "/admin/v1/analytics/funnel".into(),
        Some(json!({"steps": [{"name": "a"}], "count": "weird"})),
    )
    .await;
    assert_eq!(st, 422, "{f}");

    // --- lifetime engagement ----------------------------------------------------------------
    let (st, lt) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/analytics/lifetime?from={from_ts}"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{lt}");
    assert!(lt["players"].as_u64().unwrap() >= 1, "{lt}");
    assert!(lt["averages"]["spend_usd"].as_f64().unwrap() > 0.0, "{lt}");
    assert_eq!(lt["buckets"]["spend_usd"].as_array().unwrap().len(), 5);
    assert!(lt["averages"]["payers_pct"].as_f64().unwrap() > 0.0);
}
