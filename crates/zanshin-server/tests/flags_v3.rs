//! End-to-end tests for flags v3: prerequisites, kill switch, schedules, stats, change requests.
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-flagsv3";

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

#[tokio::test]
async fn flags_v3_flow() {
    let Some((base, c, state)) = start().await else {
        return;
    };
    use reqwest::Method;
    let (get, post, put, delete) = (Method::GET, Method::POST, Method::PUT, Method::DELETE);
    let sfx = uuid::Uuid::now_v7().simple().to_string();
    let admin = |p: &str| format!("{base}/admin/v1{p}");

    // --- prerequisites -------------------------------------------------------------------------
    let (parent, child) = (format!("f3_parent_{sfx}"), format!("f3_child_{sfx}"));
    let (st, _) = call(
        &c,
        post.clone(),
        admin("/flags"),
        Some(json!({ "key": parent, "value_type": "bool", "default_value": true })),
    )
    .await;
    assert_eq!(st, 201);
    let (st, bad) = call(
        &c,
        post.clone(),
        admin("/flags"),
        Some(
            json!({ "key": child, "value_type": "string", "default_value": "off",
        "rules": [{ "id": "r1", "condition": { "all": [] }, "percentage": 100, "value": "on" }],
        "prerequisites": [{ "flag": "f3_missing_flag" }] }),
        ),
    )
    .await;
    assert_eq!(st, 400, "{bad}");
    let (st, ch) = call(
        &c,
        post.clone(),
        admin("/flags"),
        Some(
            json!({ "key": child, "value_type": "string", "default_value": "off",
        "rules": [{ "id": "r1", "condition": { "all": [] }, "percentage": 100, "value": "on" }],
        "prerequisites": [{ "flag": parent, "value": true }] }),
        ),
    )
    .await;
    assert_eq!(st, 201, "{ch}");
    assert_eq!(ch["prerequisites"][0]["flag"], parent);
    // cycle: parent depending on child is rejected
    let (st, cyc) = call(
        &c,
        put.clone(),
        admin(&format!("/flags/{parent}")),
        Some(json!({ "prerequisites": [{ "flag": child }] })),
    )
    .await;
    assert_eq!(st, 400, "{cyc}");

    let identity = state
        .identity
        .upsert(&format!("f3-{sfx}"), Default::default(), Default::default())
        .await
        .unwrap();
    let evals = state
        .eval
        .flags(&identity, std::slice::from_ref(&child))
        .await
        .unwrap();
    assert_eq!(
        evals[0].value,
        json!("on"),
        "prerequisite met → rule applies"
    );
    // flip the parent → child falls back to default with reason prerequisite
    let (st, _) = call(
        &c,
        put.clone(),
        admin(&format!("/flags/{parent}")),
        Some(json!({ "default_value": false })),
    )
    .await;
    assert_eq!(st, 200);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let evals = state
        .eval
        .flags(&identity, std::slice::from_ref(&child))
        .await
        .unwrap();
    assert_eq!(evals[0].value, json!("off"), "{:?}", evals[0]);
    assert!(
        matches!(
            evals[0].reason,
            zanshin_core::flags::Reason::Prerequisite { .. }
        ),
        "{:?}",
        evals[0].reason
    );
    // the client-facing JSON carries the reason
    let v = serde_json::to_value(&evals[0]).unwrap();
    assert_eq!(v["reason"]["kind"], "prerequisite", "{v}");
    assert_eq!(v["reason"]["flag"], parent, "{v}");

    // --- kill switch ----------------------------------------------------------------------------
    let (st, k) = call(
        &c,
        post.clone(),
        admin(&format!("/flags/{parent}/kill")),
        None,
    )
    .await;
    assert_eq!(st, 200, "{k}");
    assert_eq!(k["flag"]["enabled"], false);
    let (st, k) = call(
        &c,
        post.clone(),
        admin(&format!("/flags/{parent}/restore")),
        None,
    )
    .await;
    assert_eq!(st, 200, "{k}");
    assert_eq!(k["flag"]["enabled"], true);

    // --- schedules ------------------------------------------------------------------------------
    let (st, bad) = call(
        &c,
        post.clone(),
        admin(&format!("/flags/{parent}/schedules")),
        Some(json!({ "at": "2000-01-01T00:00:00Z", "patch": { "enabled": false } })),
    )
    .await;
    assert_eq!(st, 400, "{bad}");
    let (st, bad) = call(
        &c,
        post.clone(),
        admin(&format!("/flags/{parent}/schedules")),
        Some(json!({ "at": "2999-01-01T00:00:00Z", "patch": { "rules": "nope" } })),
    )
    .await;
    assert_eq!(st, 400, "{bad}");
    let (st, s1) = call(&c, post.clone(), admin(&format!("/flags/{parent}/schedules")), Some(json!({ "at": "2999-01-01T00:00:00Z", "patch": { "enabled": false }, "note": "far future" }))).await;
    assert_eq!(st, 201, "{s1}");
    let (st, s2) = call(&c, post.clone(), admin(&format!("/flags/{parent}/schedules")), Some(json!({ "at": chrono::Utc::now(), "patch": { "default_value": true, "tags": ["scheduled"] } }))).await;
    assert_eq!(st, 201, "{s2}");
    // the due one is applied by the housekeeping sweep; the far-future one stays pending
    state
        .flags_v3
        .due_schedules(chrono::Utc::now())
        .await
        .unwrap();
    zanshin_server::housekeeping::sweep_once(&state).await;
    let (st, list) = call(
        &c,
        get.clone(),
        admin(&format!("/flags/{parent}/schedules")),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let items = list["items"].as_array().unwrap();
    let status_of = |id: &Value| {
        items
            .iter()
            .find(|i| i["id"] == *id)
            .map(|i| i["status"].clone())
            .unwrap_or_default()
    };
    assert_eq!(status_of(&s2["id"]), "applied", "{list}");
    assert_eq!(status_of(&s1["id"]), "pending", "{list}");
    let (_, f) = call(&c, get.clone(), admin(&format!("/flags/{parent}")), None).await;
    assert_eq!(f["default_value"], true, "{f}");
    assert_eq!(f["tags"][0], "scheduled");
    let (st, _) = call(
        &c,
        delete.clone(),
        admin(&format!(
            "/flags/{parent}/schedules/{}",
            s1["id"].as_str().unwrap()
        )),
        None,
    )
    .await;
    assert_eq!(st, 200);

    // --- evaluation stats + stale ---------------------------------------------------------------
    let (st, stats) = call(
        &c,
        get.clone(),
        admin(&format!("/flags/{child}/stats?days=7")),
        None,
    )
    .await;
    assert_eq!(st, 200, "{stats}");
    assert!(stats["total"].as_i64().unwrap() >= 2, "{stats}");
    let (st, stale) = call(&c, get.clone(), admin("/flags/stale?days=1"), None).await;
    assert_eq!(st, 200, "{stale}");
    assert!(
        !stale["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["key"] == child),
        "evaluated flag is not stale"
    );

    // --- change requests ------------------------------------------------------------------------
    let (st, cr) = call(&c, post.clone(), admin("/change-requests"), Some(json!({ "kind": "flags", "key": parent, "body": { "description": "via change request" }, "title": "Describe parent" }))).await;
    assert_eq!(st, 201, "{cr}");
    let id = cr["id"].as_str().unwrap().to_string();
    assert_eq!(cr["status"], "open");
    // four eyes: the author (same bootstrap principal) cannot approve
    let (st, denied) = call(
        &c,
        post.clone(),
        admin(&format!("/change-requests/{id}/approve")),
        Some(json!({ "comment": "lgtm" })),
    )
    .await;
    assert_eq!(st, 403, "{denied}");
    // apply before approval is refused
    let (st, _) = call(
        &c,
        post.clone(),
        admin(&format!("/change-requests/{id}/apply")),
        None,
    )
    .await;
    assert_eq!(st, 409);
    // simulate a second reviewer directly in storage, then apply
    let approved = state
        .flags_v3
        .update_change_request(
            uuid::Uuid::parse_str(&id).unwrap(),
            "approved",
            &json!([{ "by": "reviewer", "at": chrono::Utc::now() }]),
            "",
            false,
        )
        .await
        .unwrap();
    assert_eq!(approved.status, "approved");
    let (st, applied) = call(
        &c,
        post.clone(),
        admin(&format!("/change-requests/{id}/apply")),
        None,
    )
    .await;
    assert_eq!(st, 200, "{applied}");
    assert_eq!(applied["status"], "applied");
    let (_, f) = call(&c, get.clone(), admin(&format!("/flags/{parent}")), None).await;
    assert_eq!(f["description"], "via change request");
    let (st, detail) = call(
        &c,
        get.clone(),
        admin(&format!("/change-requests/{id}")),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(
        detail["current"]["description"], "via change request",
        "{detail}"
    );
    let (st, bad) = call(
        &c,
        post.clone(),
        admin("/change-requests"),
        Some(json!({ "kind": "nope", "key": parent, "body": {} })),
    )
    .await;
    assert_eq!(st, 400, "{bad}");
    let (st, list) = call(
        &c,
        get.clone(),
        admin("/change-requests?status=applied"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert!(list["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["id"] == id));
}
