//! End-to-end tests for live events v3 (variants, monitor metrics, run report) and experiments v3
//! (phases, Bayesian stats, SRM, CSV results).
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-liveops3";

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
async fn liveops_v3_flow() {
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

    let mkey = format!("buy_conv_{sfx}");
    let (st, m) = adm(
        reqwest::Method::POST,
        "/admin/v1/metrics".into(),
        Some(
            json!({"key": mkey, "kind": "goal", "event_name": "buy", "aggregation": "conversion"}),
        ),
    )
    .await;
    assert_eq!(st, 201, "{m}");
    let fkey = format!("v3flag_{sfx}");
    let (st, _) = adm(
        reqwest::Method::POST,
        "/admin/v1/flags".into(),
        Some(json!({"key": fkey, "value_type": "string", "default_value": "base"})),
    )
    .await;
    assert_eq!(st, 201);

    // Live event with two variants, flag overrides per variant and a monitor metric.
    let lkey = format!("sale_{sfx}");
    let (st, bad) = adm(reqwest::Method::POST, "/admin/v1/live-events".into(), Some(json!({"key": lkey, "starts_at": "2020-01-01T00:00:00Z", "ends_at": "2099-01-01T00:00:00Z", "variants": [{"key": "a"}, {"key": "a"}]}))).await;
    assert_eq!(st, 400, "{bad}");
    let (st, le) = adm(reqwest::Method::POST, "/admin/v1/live-events".into(), Some(json!({
        "key": lkey, "starts_at": "2020-01-01T00:00:00Z", "ends_at": "2099-01-01T00:00:00Z", "payload": {"discount": 10},
        "condition": {"prop": "custom.cohort", "op": "eq", "value": sfx},
        "flag_overrides": {fkey.clone(): "sale"},
        "variants": [
            {"key": "small", "weight": 1, "payload": {"discount": 20}},
            {"key": "big", "weight": 1, "flag_overrides": {fkey.clone(): "big_sale"}}
        ],
        "metrics": [mkey]
    }))).await;
    assert_eq!(st, 201, "{le}");
    assert_eq!(le["variants"].as_array().unwrap().len(), 2);

    // 12 players: each gets a deterministic variant; value / flag follow it; events get tagged.
    let mut seen = std::collections::HashSet::new();
    for i in 0..12 {
        let s: Value = c
            .post(format!("{base}/v1/authenticate"))
            .bearer_auth(&ck)
            .json(&json!({"id": format!("p{i}_{sfx}"), "custom": {"cohort": sfx}}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let tok = s["token"].as_str().unwrap().to_string();
        let (st, evs) = call(
            &c,
            &tok,
            reqwest::Method::GET,
            format!("{base}/v1/live-events?names={lkey}"),
            None,
        )
        .await;
        assert_eq!(st, 200, "{evs}");
        let ev = &evs["live_events"][0];
        assert_eq!(ev["active"], true, "{evs}");
        let variant = ev["variant"].as_str().unwrap().to_string();
        seen.insert(variant.clone());
        let (_, again) = call(
            &c,
            &tok,
            reqwest::Method::GET,
            format!("{base}/v1/live-events?names={lkey}"),
            None,
        )
        .await;
        assert_eq!(again["live_events"][0]["variant"], variant, "deterministic");
        let (_, fl) = call(
            &c,
            &tok,
            reqwest::Method::GET,
            format!("{base}/v1/flags/{fkey}"),
            None,
        )
        .await;
        if variant == "small" {
            assert_eq!(ev["value"]["discount"], 20, "{ev}");
            assert_eq!(fl["value"], "sale", "{fl}");
        } else {
            assert_eq!(ev["value"]["discount"], 10, "{ev}");
            assert_eq!(fl["value"], "big_sale", "{fl}");
        }
        let (st, _) = call(
            &c,
            &tok,
            reqwest::Method::POST,
            format!("{base}/v1/events"),
            Some(json!({"events": [{"name": "buy", "value": "1"}]})),
        )
        .await;
        assert!(st == 200 || st == 202);
    }
    assert_eq!(seen.len(), 2, "both variants assigned: {seen:?}");

    // Monitor metrics per variant + run report (writer is async: poll).
    let mut mon = Value::Null;
    for _ in 0..100 {
        let (_, r) = adm(
            reqwest::Method::GET,
            format!("/admin/v1/live-events/{lkey}/metrics"),
            None,
        )
        .await;
        let total: i64 = r["metrics"][0]["variants"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| v["identities"].as_i64().unwrap_or(0))
                    .sum()
            })
            .unwrap_or(0);
        if total >= 12 {
            mon = r;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(!mon.is_null(), "metrics never caught up");
    assert_eq!(mon["variants"], json!(["small", "big"]));
    assert!(
        mon["series"]["points"].as_array().unwrap().len() >= 2,
        "{mon}"
    );
    let (st, runs) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/live-events/{lkey}/runs"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{runs}");
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1);
    assert_eq!(runs["runs"][0]["live"], true);
    assert_eq!(runs["runs"][0]["metrics"][0]["metric"], mkey);

    // Experiments v3: phases family, SRM + Bayesian stats, CSV.
    let ekey = format!("price_{sfx}");
    let (st, _) = adm(reqwest::Method::POST, "/admin/v1/experiments".into(), Some(json!({"key": ekey, "status": "running", "control": "a", "metrics": [mkey], "variants": [{"key": "a", "value": 1, "weight": 1}, {"key": "b", "value": 2, "weight": 1}]}))).await;
    assert_eq!(st, 201);
    let (st, ph) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/experiments/{ekey}/copy"),
        Some(json!({"key": format!("{ekey}_p2"), "as_phase": true})),
    )
    .await;
    assert_eq!(st, 201, "{ph}");
    let (st, phases) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/experiments/{ekey}_p2/phases"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{phases}");
    let items = phases["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{phases}");
    assert_eq!(items[0]["key"], ekey);
    assert_eq!(items[1]["phase"], 2);

    for i in 0..60 {
        let s: Value = c
            .post(format!("{base}/v1/authenticate"))
            .bearer_auth(&ck)
            .json(&json!({"id": format!("e{i}_{sfx}")}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let tok = s["token"].as_str().unwrap().to_string();
        let (_, ex) = call(
            &c,
            &tok,
            reqwest::Method::GET,
            format!("{base}/v1/experiments?names={ekey}"),
            None,
        )
        .await;
        let v = ex["experiments"][0]["variant"].as_str().unwrap_or("");
        if v == "b" || i % 3 == 0 {
            let _ = call(
                &c,
                &tok,
                reqwest::Method::POST,
                format!("{base}/v1/events"),
                Some(json!({"events": [{"name": "buy", "value": "1"}]})),
            )
            .await;
        }
    }
    let mut res = Value::Null;
    for _ in 0..100 {
        let (_, r) = adm(
            reqwest::Method::GET,
            format!("/admin/v1/experiments/{ekey}/results"),
            None,
        )
        .await;
        let b = r["metrics"][0]["variants"]
            .as_array()
            .and_then(|a| a.iter().find(|v| v["variant"] == "b").cloned())
            .unwrap_or(Value::Null);
        if b["stats"]["prob_beats_control"]
            .as_f64()
            .is_some_and(|p| p > 0.9)
        {
            res = r;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(!res.is_null(), "stats never converged");
    assert!(res["srm"]["p_value"].as_f64().is_some(), "{res}");
    assert_eq!(res["srm"]["suspicious"], false, "{res}");
    let a = res["metrics"][0]["variants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["variant"] == "a")
        .cloned()
        .unwrap();
    assert!(
        a["stats"]["prob_beats_control"].is_null(),
        "control has no comparison: {a}"
    );
    assert!(a["stats"]["ci_low"].as_f64().unwrap() <= a["stats"]["rate"].as_f64().unwrap());

    let resp = c
        .get(format!("{base}/admin/v1/experiments/{ekey}/results.csv"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert!(resp.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/csv"));
    let csv = resp.text().await.unwrap();
    assert!(csv.starts_with("experiment,metric,aggregation"), "{csv}");
    assert!(
        csv.lines()
            .any(|l| l.contains(&format!("{ekey},{mkey},conversion,goal,b,false"))),
        "{csv}"
    );
}
