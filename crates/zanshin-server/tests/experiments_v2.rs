//! End-to-end tests for the metrics catalog and experiments v2 (caps, deadline, lock, copy, interrupt,
//! participants, series). Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-expv2";

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
async fn metrics_and_experiments_v2() {
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
    let auth = |id: String| {
        let c = c.clone();
        let base = base.clone();
        let ck = ck.clone();
        async move {
            let s: Value = c
                .post(format!("{base}/v1/authenticate"))
                .bearer_auth(&ck)
                .json(&json!({"id": id, "custom": {"clan": "red"}}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            s["token"].as_str().unwrap().to_string()
        }
    };

    // Metrics catalog.
    let mkey = format!("purchase_conv_{sfx}");
    let (st, m) = adm(reqwest::Method::POST, "/admin/v1/metrics".into(), Some(json!({"key": mkey, "name": "Purchase conversion", "kind": "goal", "event_name": "purchase", "aggregation": "conversion"}))).await;
    assert_eq!(st, 201, "{m}");
    let rkey = format!("revenue_{sfx}");
    let (st, _) = adm(reqwest::Method::POST, "/admin/v1/metrics".into(), Some(json!({"key": rkey, "kind": "monitor", "event_name": "purchase", "aggregation": "sum", "field": "usd"}))).await;
    assert_eq!(st, 201);
    let (st, bad) = adm(reqwest::Method::POST, "/admin/v1/metrics".into(), Some(json!({"key": format!("bad_{sfx}"), "event_name": "x", "aggregation": "count", "field": "usd"}))).await;
    assert_eq!(st, 400, "field only for sum/avg: {bad}");
    let (_, l) = adm(reqwest::Method::GET, "/admin/v1/metrics".into(), None).await;
    assert!(l["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["key"] == mkey));

    // Experiment with caps, split key, metrics, linked flag.
    let fkey = format!("price_{sfx}");
    let (st, _) = adm(
        reqwest::Method::POST,
        "/admin/v1/flags".into(),
        Some(json!({"key": fkey, "value_type": "number", "default_value": 1})),
    )
    .await;
    assert_eq!(st, 201);
    let ekey = format!("pricing_{sfx}");
    let (st, e) = adm(reqwest::Method::POST, "/admin/v1/experiments".into(), Some(json!({
        "key": ekey, "name": "Pricing", "status": "running", "flag_key": fkey, "control": "a",
        "variants": [{"key": "a", "value": 1, "weight": 1}, {"key": "b", "value": 2, "weight": 1}],
        "metrics": [mkey, rkey], "max_participants": 2, "split_key": "property:custom.clan"
    }))).await;
    assert_eq!(st, 201, "{e}");
    assert_eq!(e["split_key"], "property:custom.clan");
    let (st, bad) = adm(reqwest::Method::POST, "/admin/v1/experiments".into(), Some(json!({"key": format!("x_{sfx}"), "variants": [{"key": "a", "value": 1, "weight": 1}, {"key": "b", "value": 2, "weight": 1}], "split_key": "nope"}))).await;
    assert_eq!(st, 400, "{bad}");

    // Three players share clan "red" → same bucket (split key); cap of 2 stops the third.
    let mut toks = Vec::new();
    for i in 0..3 {
        let t = auth(format!("p{i}_{sfx}")).await;
        let (_, ex) = call(
            &c,
            &t,
            reqwest::Method::GET,
            format!("{base}/v1/experiments?names={ekey}"),
            None,
        )
        .await;
        toks.push((
            t,
            ex["experiments"]
                .as_array()
                .unwrap()
                .first()
                .map(|e| e["variant"].as_str().unwrap().to_string()),
        ));
    }
    assert!(toks[0].1.is_some() && toks[1].1.is_some(), "{toks:?}");
    assert_eq!(toks[0].1, toks[1].1, "same split key → same variant");
    assert!(toks[2].1.is_none(), "cap reached: {:?}", toks[2].1);
    let (_, p) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/experiments/{ekey}/participants"),
        None,
    )
    .await;
    assert_eq!(p["total"], 2);
    assert_eq!(p["items"].as_array().unwrap().len(), 2);
    let csv = c
        .get(format!(
            "{base}/admin/v1/experiments/{ekey}/participants.csv"
        ))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .unwrap();
    assert!(csv.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/csv"));
    let body = csv.text().await.unwrap();
    assert_eq!(body.lines().count(), 3, "{body}");
    assert!(body.contains(&format!("p0_{sfx}")));

    // Metric results: player 0 purchases 5 usd.
    let variant = toks[0].1.clone().unwrap();
    let (st, _) = call(
        &c,
        &toks[0].0,
        reqwest::Method::POST,
        format!("{base}/v1/events"),
        Some(json!({"events": [{"name": "purchase", "value": "5", "metadata": {"usd": 5}}]})),
    )
    .await;
    assert!(st == 200 || st == 202, "{st}");
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let (st, r) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/experiments/{ekey}/results"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{r}");
    let conv = r["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["metric"] == mkey)
        .expect("conversion metric");
    let row = conv["variants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["variant"] == variant)
        .unwrap();
    assert_eq!(row["value"], 0.5, "1 of 2 assigned converted: {conv}");
    let rev = r["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["metric"] == rkey)
        .unwrap();
    let row = rev["variants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["variant"] == variant)
        .unwrap();
    assert_eq!(row["value"], 5.0, "{rev}");
    let (st, s) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/experiments/{ekey}/series?metric={mkey}"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{s}");
    assert_eq!(
        s["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["count"].as_i64().unwrap())
            .sum::<i64>(),
        2
    );
    assert_eq!(s["metric"]["points"][0]["identities"], 1);

    // Lock blocks edits but not status; unlock restores.
    let (st, _) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/experiments/{ekey}/lock"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let (st, _) = adm(
        reqwest::Method::PUT,
        format!("/admin/v1/experiments/{ekey}"),
        Some(json!({"name": "renamed"})),
    )
    .await;
    assert_eq!(st, 400, "locked");
    let (st, _) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/experiments/{ekey}/pause"),
        None,
    )
    .await;
    assert_eq!(st, 200, "status change allowed while locked");
    let (st, _) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/experiments/{ekey}/unlock"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let (st, _) = adm(
        reqwest::Method::PUT,
        format!("/admin/v1/experiments/{ekey}"),
        Some(json!({"name": "renamed"})),
    )
    .await;
    assert_eq!(st, 200);

    // Copy as next phase; enrolment deadline in the past blocks new assignments.
    let (st, cp) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/experiments/{ekey}/copy"),
        Some(json!({"key": format!("{ekey}_p2"), "as_phase": true})),
    )
    .await;
    assert_eq!(st, 201, "{cp}");
    assert_eq!(cp["status"], "draft");
    assert_eq!(cp["phase_of"], ekey);
    assert_eq!(cp["copied_from"], ekey);
    assert_eq!(cp["metrics"].as_array().unwrap().len(), 2);
    let past = (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
    let (st, _) = adm(
        reqwest::Method::PUT,
        format!("/admin/v1/experiments/{ekey}_p2"),
        Some(json!({"status": "running", "enrol_until": past, "max_participants": null})),
    )
    .await;
    assert_eq!(st, 200);
    let t = auth(format!("late_{sfx}")).await;
    let (_, ex) = call(
        &c,
        &t,
        reqwest::Method::GET,
        format!("{base}/v1/experiments?names={ekey}_p2"),
        None,
    )
    .await;
    assert!(
        ex["experiments"].as_array().unwrap().is_empty(),
        "deadline passed: {ex}"
    );

    // Interrupt with winner → flag default updated.
    let (st, out) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/experiments/{ekey}/interrupt"),
        Some(json!({"reason": "b wins", "winner": "b", "apply_to_flag": true})),
    )
    .await;
    assert_eq!(st, 200, "{out}");
    assert_eq!(out["experiment"]["status"], "stopped");
    assert_eq!(out["experiment"]["winner"], "b");
    assert_eq!(out["flag"]["default_value"], 2);
    let (_, m) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/metrics/{mkey}"),
        None,
    )
    .await;
    assert!(m["used_by"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k == &json!(ekey)));
    let (st, _) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/metrics/{rkey}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
}
