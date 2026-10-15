//! End-to-end tests for experiments v4: holdouts, layers, sequential + CUPED results payload.
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-expv4";

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

fn exp_body(key: &str, pct: f64, extra: Value) -> Value {
    let mut v = json!({
        "key": key, "name": key, "percentage": pct, "control": "a",
        "variants": [{ "key": "a", "name": "A", "value": 1, "weight": 50 }, { "key": "b", "name": "B", "value": 2, "weight": 50 }],
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

#[tokio::test]
async fn experiments_v4_flow() {
    let Some((base, c, state)) = start().await else {
        return;
    };
    use reqwest::Method;
    let (get, post, put, delete) = (Method::GET, Method::POST, Method::PUT, Method::DELETE);
    let sfx = uuid::Uuid::now_v7().simple().to_string();
    let admin = |p: &str| format!("{base}/admin/v1{p}");

    // --- holdouts CRUD -------------------------------------------------------------------------
    let hkey = format!("hold_{sfx}");
    let (st, h) = call(
        &c,
        post.clone(),
        admin("/holdouts"),
        Some(json!({ "key": hkey, "name": "Global holdout", "percentage": 50 })),
    )
    .await;
    assert_eq!(st, 201, "{h}");
    assert_eq!(h["percentage"], 50.0);
    let (st, _) = call(
        &c,
        put.clone(),
        admin(&format!("/holdouts/{hkey}")),
        Some(json!({ "percentage": 100 })),
    )
    .await;
    assert_eq!(st, 200);
    let (st, bad) = call(
        &c,
        put.clone(),
        admin(&format!("/holdouts/{hkey}")),
        Some(json!({ "percentage": 150 })),
    )
    .await;
    assert_eq!(st, 400, "{bad}");

    // --- an experiment behind a 100 % holdout enrols nobody; without it, everybody --------------
    let ekey = format!("exp_hold_{sfx}");
    let (st, e) = call(
        &c,
        post.clone(),
        admin("/experiments"),
        Some(exp_body(&ekey, 100.0, json!({ "holdout": hkey }))),
    )
    .await;
    assert_eq!(st, 201, "{e}");
    let (st, _) = call(
        &c,
        post.clone(),
        admin(&format!("/experiments/{ekey}/start")),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let mut enrolled = 0;
    for i in 0..5 {
        let identity = state
            .identity
            .upsert(
                &format!("v4-{sfx}-{i}"),
                Default::default(),
                Default::default(),
            )
            .await
            .unwrap();
        let flat = zanshin_core::properties::PropertySet::default().flatten();
        let enr = state
            .experiments
            .enrolments(
                &identity,
                &flat,
                &Default::default(),
                std::slice::from_ref(&ekey),
            )
            .await
            .unwrap();
        enrolled += enr.len();
    }
    assert_eq!(enrolled, 0, "holdout must keep everyone out");
    // referenced holdout cannot be deleted
    let (st, _) = call(
        &c,
        delete.clone(),
        admin(&format!("/holdouts/{hkey}")),
        None,
    )
    .await;
    assert_eq!(st, 409);
    // disable the holdout → players enrol
    let (st, _) = call(
        &c,
        put.clone(),
        admin(&format!("/holdouts/{hkey}")),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(st, 200);
    let identity = state
        .identity
        .upsert(
            &format!("v4-{sfx}-free"),
            Default::default(),
            Default::default(),
        )
        .await
        .unwrap();
    let flat = zanshin_core::properties::PropertySet::default().flatten();
    let enr = state
        .experiments
        .enrolments(
            &identity,
            &flat,
            &Default::default(),
            std::slice::from_ref(&ekey),
        )
        .await
        .unwrap();
    assert_eq!(enr.len(), 1);

    // --- layers: over-allocation is rejected; two 50 % experiments never share a player ----------
    let layer = format!("layer_{sfx}");
    let (k1, k2) = (format!("exp_l1_{sfx}"), format!("exp_l2_{sfx}"));
    let (st, _) = call(
        &c,
        post.clone(),
        admin("/experiments"),
        Some(exp_body(&k1, 50.0, json!({ "layer": layer }))),
    )
    .await;
    assert_eq!(st, 201);
    let (st, over) = call(
        &c,
        post.clone(),
        admin("/experiments"),
        Some(exp_body(&k2, 60.0, json!({ "layer": layer }))),
    )
    .await;
    assert_eq!(st, 400, "{over}");
    let (st, _) = call(
        &c,
        post.clone(),
        admin("/experiments"),
        Some(exp_body(&k2, 50.0, json!({ "layer": layer }))),
    )
    .await;
    assert_eq!(st, 201);
    for k in [&k1, &k2] {
        let (st, _) = call(
            &c,
            post.clone(),
            admin(&format!("/experiments/{k}/start")),
            None,
        )
        .await;
        assert_eq!(st, 200);
    }
    let (mut in1, mut in2, mut both) = (0, 0, 0);
    for i in 0..60 {
        let identity = state
            .identity
            .upsert(
                &format!("v4-layer-{sfx}-{i}"),
                Default::default(),
                Default::default(),
            )
            .await
            .unwrap();
        let enr = state
            .experiments
            .enrolments(
                &identity,
                &flat,
                &Default::default(),
                &[k1.clone(), k2.clone()],
            )
            .await
            .unwrap();
        let a = enr.iter().any(|e| e.experiment.key == k1);
        let b = enr.iter().any(|e| e.experiment.key == k2);
        in1 += a as i32;
        in2 += b as i32;
        both += (a && b) as i32;
    }
    assert_eq!(both, 0, "layer must be mutually exclusive");
    assert!(
        in1 > 10 && in2 > 10,
        "both experiments should enrol ({in1}/{in2})"
    );
    assert_eq!(
        in1 + in2,
        60,
        "every player lands in exactly one of two 50 % experiments"
    );

    // --- results payload: sequential on conversion metrics, cuped on count metrics ---------------
    let (st, _) = call(&c, post.clone(), admin("/metrics"), Some(json!({ "key": format!("m_conv_{sfx}"), "name": "conv", "kind": "goal", "event_name": "buy_v4", "aggregation": "conversion" }))).await;
    assert_eq!(st, 201);
    let (st, _) = call(&c, post.clone(), admin("/metrics"), Some(json!({ "key": format!("m_cnt_{sfx}"), "name": "cnt", "kind": "goal", "event_name": "buy_v4", "aggregation": "count" }))).await;
    assert_eq!(st, 201);
    let (st, _) = call(&c, put.clone(), admin(&format!("/experiments/{ekey}")), Some(json!({ "metrics": [format!("m_conv_{sfx}"), format!("m_cnt_{sfx}")], "guardrails": [{ "metric": format!("m_conv_{sfx}"), "max_drop": 0.2, "min_identities": 1 }], "bandit": { "metric": format!("m_conv_{sfx}") } }))).await;
    assert_eq!(st, 200);
    let (st, r) = call(
        &c,
        get.clone(),
        admin(&format!("/experiments/{ekey}/results?cuped=true")),
        None,
    )
    .await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["holdout"], hkey);
    assert_eq!(r["guardrails"][0]["max_drop"], 0.2);
    assert_eq!(r["bandit"]["min_weight"], 5.0);
    assert!(r["cuped"].is_array(), "{r}");
    let conv = r["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["metric"] == format!("m_conv_{sfx}"))
        .unwrap();
    assert!(
        conv["variants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.get("stats").is_some()),
        "{conv}"
    );

    // invalid bandit / guardrail configs are rejected
    let (st, bad) = call(
        &c,
        put.clone(),
        admin(&format!("/experiments/{ekey}")),
        Some(json!({ "guardrails": [{ "metric": "x", "max_drop": 2.0 }] })),
    )
    .await;
    assert_eq!(st, 400, "{bad}");
    let (st, bad) = call(
        &c,
        put.clone(),
        admin(&format!("/experiments/{ekey}")),
        Some(json!({ "bandit": { "metric": "x", "every_secs": 5 } })),
    )
    .await;
    assert_eq!(st, 400, "{bad}");
}
