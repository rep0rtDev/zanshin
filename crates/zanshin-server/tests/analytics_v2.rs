//! End-to-end tests for analytics v2: overview, retention, funnels, annotations, dashboards.
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-analytics";

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
async fn analytics_v2_flow() {
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
    let pay = format!("pay_{sfx}");
    let lvl = format!("lvl_{sfx}");

    let mut toks = vec![];
    for i in 0..3 {
        let s: Value = c
            .post(format!("{base}/v1/authenticate"))
            .bearer_auth(&ck)
            .json(&json!({"id": format!("p{i}_{sfx}")}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        toks.push(s["token"].as_str().unwrap().to_string());
    }
    let track = |tok: String, events: Value| {
        let c = c.clone();
        let base = base.clone();
        async move {
            let (st, _) = call(
                &c,
                &tok,
                reqwest::Method::POST,
                format!("{base}/v1/events"),
                Some(json!({"events": events})),
            )
            .await;
            assert!(st == 200 || st == 202);
        }
    };
    track(toks[0].clone(), json!([{"name": pay, "metadata": {"amount": 9.99, "currency": "USD"}}, {"name": "purchase", "metadata": {"amount": 9.99, "currency": "USD"}}])).await;
    track(toks[0].clone(), json!([{"name": lvl, "value": "2"}])).await;
    track(
        toks[1].clone(),
        json!([{"name": pay, "metadata": {"amount": 5, "currency": "USD"}}]),
    )
    .await;
    track(toks[2].clone(), json!([{"name": lvl, "value": "1"}])).await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;

    // Overview: today's players, revenue and new users include ours (the DB is shared, so >=).
    let (st, o) = adm(
        reqwest::Method::GET,
        "/admin/v1/analytics/overview".into(),
        None,
    )
    .await;
    assert_eq!(st, 200, "{o}");
    let t = &o["totals"];
    assert!(t["dau"].as_i64().unwrap() >= 3, "{t}");
    assert!(t["mau"].as_i64().unwrap() >= 3);
    assert!(t["new_users"].as_i64().unwrap() >= 3);
    assert!(t["revenue"].as_f64().unwrap() >= 9.99, "{t}");
    assert!(t["arpdau"].as_f64().unwrap() > 0.0);
    assert!(o["revenue_events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n == "purchase"));
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let day = o["series"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["day"] == today)
        .expect("today in series");
    assert!(
        day["dau"].as_i64().unwrap() >= 3 && day["new_users"].as_i64().unwrap() >= 3,
        "{day}"
    );

    // Retention: today's cohort exists, D1 is still in the future.
    let (st, r) = adm(
        reqwest::Method::GET,
        "/admin/v1/analytics/retention?days=1,7".into(),
        None,
    )
    .await;
    assert_eq!(st, 200, "{r}");
    let cohort = r["cohorts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["cohort"] == today)
        .expect("today cohort");
    assert!(cohort["size"].as_i64().unwrap() >= 3);
    assert!(cohort["retained"]["d1"].is_null());

    // Funnel with unique names: 2 paid, 1 levelled after paying; p3 levelled without paying.
    let (st, f) = adm(
        reqwest::Method::POST,
        "/admin/v1/analytics/funnel".into(),
        Some(json!({"steps": [{"name": pay}, {"name": lvl}], "window_secs": 3600})),
    )
    .await;
    assert_eq!(st, 200, "{f}");
    assert_eq!(f["entered"], 2);
    assert_eq!(f["steps"][1]["count"], 1);
    assert!((f["steps"][1]["conversion"].as_f64().unwrap() - 0.5).abs() < 1e-9);
    // Step filter on metadata.
    let (_, f) = adm(reqwest::Method::POST, "/admin/v1/analytics/funnel".into(), Some(json!({"steps": [{"name": pay, "field": "amount", "op": "gte", "value": 6}, {"name": lvl}]}))).await;
    assert_eq!(f["entered"], 1, "{f}");
    assert_eq!(f["steps"][1]["count"], 1);
    let (st, _) = adm(
        reqwest::Method::POST,
        "/admin/v1/analytics/funnel".into(),
        Some(json!({"steps": []})),
    )
    .await;
    assert_eq!(st, 400);

    // Annotations: automatic from a flag change, manual notes.
    let fkey = format!("ann_{sfx}");
    let (st, _) = adm(
        reqwest::Method::POST,
        "/admin/v1/flags".into(),
        Some(json!({"key": fkey, "value_type": "bool", "default_value": false})),
    )
    .await;
    assert_eq!(st, 201);
    let mut found = false;
    for _ in 0..50 {
        let (_, a) = adm(
            reqwest::Method::GET,
            "/admin/v1/analytics/annotations".into(),
            None,
        )
        .await;
        if a["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["kind"] == "flag" && x["key"] == fkey)
        {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(found, "flag change annotation");
    let (st, n) = adm(
        reqwest::Method::POST,
        "/admin/v1/analytics/annotations".into(),
        Some(json!({"title": "Soft launch", "body": "DE/FR"})),
    )
    .await;
    assert_eq!(st, 201, "{n}");
    assert_eq!(n["kind"], "note");
    let (st, _) = adm(
        reqwest::Method::DELETE,
        format!(
            "/admin/v1/analytics/annotations/{}",
            n["id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(st, 200);

    // Dashboards: validation, CRUD, evaluation of every widget kind.
    let dkey = format!("ops_{sfx}");
    let (st, bad) = adm(
        reqwest::Method::POST,
        "/admin/v1/dashboards".into(),
        Some(json!({"key": dkey, "widgets": [{"id": "a", "kind": "timeseries", "config": {}}]})),
    )
    .await;
    assert_eq!(st, 400, "{bad}");
    let widgets = json!([
        {"id": "dau", "kind": "number", "title": "DAU", "config": {"metric": "dau"}, "width": 1},
        {"id": "lvl", "kind": "timeseries", "title": "Level ups", "config": {"event": lvl}},
        {"id": "fun", "kind": "funnel", "config": {"steps": [{"name": pay}, {"name": lvl}]}},
        {"id": "ret", "kind": "retention", "config": {"days": [1, 7]}},
        {"id": "top", "kind": "top_events", "config": {"limit": 5}},
        {"id": "cnt", "kind": "number", "config": {"metric": "event_count", "event": pay}}
    ]);
    let (st, d) = adm(
        reqwest::Method::POST,
        "/admin/v1/dashboards".into(),
        Some(json!({"key": dkey, "name": "Ops", "widgets": widgets})),
    )
    .await;
    assert_eq!(st, 201, "{d}");
    let (st, data) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/dashboards/{dkey}/data"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{data}");
    let ws = data["widgets"].as_array().unwrap();
    assert_eq!(ws.len(), 6);
    assert!(ws.iter().all(|w| w.get("error").is_none()), "{data}");
    assert!(ws[0]["data"]["value"].as_i64().unwrap() >= 3);
    assert_eq!(ws[2]["data"]["entered"], 2);
    assert_eq!(ws[5]["data"]["value"], 2);
    let (st, d) = adm(
        reqwest::Method::PUT,
        format!("/admin/v1/dashboards/{dkey}"),
        Some(json!({"name": "Ops v2"})),
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(d["version"], 2);
    let (st, l) = adm(reqwest::Method::GET, "/admin/v1/dashboards".into(), None).await;
    assert_eq!(st, 200);
    assert!(l["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["key"] == dkey));
    let (st, _) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/dashboards/{dkey}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let (st, _) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/flags/{fkey}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
}
