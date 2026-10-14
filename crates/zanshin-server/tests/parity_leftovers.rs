//! End-to-end tests for the Satori parity leftovers: resolved flags per identity, validator
//! presets, console settings, redacted configuration, default data, gated delete-all, new webhook
//! event types. Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-parity";

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
async fn parity_leftovers_flow() {
    let Some((base, c, state)) = start().await else {
        return;
    };
    use reqwest::Method;
    let (get, post, put, delete) = (Method::GET, Method::POST, Method::PUT, Method::DELETE);
    let suffix = uuid::Uuid::now_v7().simple().to_string();
    let admin = |p: &str| format!("{base}/admin/v1{p}");

    // --- resolved flags for an identity, with reasons --------------------------------------
    let fkey = format!("parity_flag_{suffix}");
    let (st, _) = call(&c, post.clone(),
        admin("/flags"),
        Some(json!({
            "key": fkey, "value_type": "number", "default_value": 1, "enabled": true,
            "rules": [{ "id": "vip", "name": "vip", "condition": { "prop": "custom.vip", "op": "eq", "value": true },
                        "audiences": [], "percentage": 100, "value": 5, "variants": [] }]
        })),
    )
    .await;
    assert_eq!(st, 201);
    let mut custom = serde_json::Map::new();
    custom.insert("vip".into(), json!(true));
    let identity = state
        .identity
        .upsert(&format!("parity-{suffix}"), serde_json::Map::new(), custom)
        .await
        .unwrap();
    let (st, body) = call(
        &c,
        get.clone(),
        admin(&format!("/identities/{}/flags", identity.id)),
        None,
    )
    .await;
    assert_eq!(st, 200, "{body}");
    let mine = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["key"] == fkey)
        .cloned()
        .expect("flag resolved");
    assert_eq!(mine["value"], 5);
    assert_eq!(mine["reason"]["kind"], "rule");
    assert_eq!(mine["reason"]["rule_id"], "vip");

    // --- rule-level webhook events are known types ----------------------------------------
    let (st, types) = call(&c, get.clone(), admin("/webhooks/event-types"), None).await;
    assert_eq!(st, 200);
    let types = types.to_string();
    for t in [
        "flag.rule.created",
        "flag.rule.updated",
        "flag.rule.deleted",
        "journey.step_reached",
        "message.direct_sent",
    ] {
        assert!(types.contains(t), "missing {t} in {types}");
    }

    // --- validator presets: idempotent --------------------------------------------------
    let (st, first) = call(&c, post.clone(), admin("/schemas/presets"), None).await;
    assert_eq!(st, 200, "{first}");
    let (st, second) = call(&c, post.clone(), admin("/schemas/presets"), None).await;
    assert_eq!(st, 200);
    assert_eq!(second["created"].as_array().unwrap().len(), 0, "{second}");
    let (_, schemas) = call(&c, get.clone(), admin("/schemas"), None).await;
    let names: Vec<&str> = schemas
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    for n in [
        "ANY",
        "Number",
        "String",
        "Object",
        "Event-Revenue-Metadata",
    ] {
        assert!(names.contains(&n), "missing preset {n}: {names:?}");
    }
    // the revenue preset rejects a payload without currency
    let revenue = schemas
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Event-Revenue-Metadata")
        .unwrap();
    let (st, v) = call(&c, post.clone(),
        admin("/schemas/validate"),
        Some(json!({ "schema": revenue["schema"], "samples": [{ "amount": 4.99 }, { "amount": 4.99, "currency": "USD" }] })),
    )
    .await;
    assert_eq!(st, 200, "{v}");
    assert!(v["results"][0].is_string(), "{v}");
    assert!(v["results"][1].is_null(), "{v}");

    // --- console settings ------------------------------------------------------------------
    let (st, s) = call(&c, get.clone(), admin("/console/settings"), None).await;
    assert_eq!(st, 200, "{s}");
    let (st, s) = call(
        &c,
        put.clone(),
        admin("/console/settings"),
        Some(json!({ "timezone": "Europe/Berlin", "default_range_days": 14 })),
    )
    .await;
    assert_eq!(st, 200, "{s}");
    assert_eq!(s["timezone"], "Europe/Berlin");
    let (st, _) = call(
        &c,
        put.clone(),
        admin("/console/settings"),
        Some(json!({ "timezone": "not a zone!" })),
    )
    .await;
    assert_eq!(st, 400);
    let (_, s) = call(&c, get.clone(), admin("/console/settings"), None).await;
    assert_eq!(s["timezone"], "Europe/Berlin");
    assert_eq!(s["default_range_days"], 14);
    let (st, _) = call(
        &c,
        put.clone(),
        admin("/console/settings"),
        Some(json!({ "timezone": "" })),
    )
    .await;
    assert_eq!(st, 200);

    // --- effective configuration with secrets masked ---------------------------------------
    let (st, cfg) = call(&c, get.clone(), admin("/system/config"), None).await;
    assert_eq!(st, 200, "{cfg}");
    assert_eq!(cfg["config"]["auth"]["jwt_secret"], "***");
    assert_eq!(cfg["config"]["admin"]["bootstrap_key"], "***");
    let db_url = cfg["config"]["database"]["url"].as_str().unwrap();
    assert!(
        !db_url.contains("zanshin:zanshin@") || db_url.contains("***@"),
        "{db_url}"
    );
    assert_eq!(cfg["config"]["push"]["provider"], "log");
    assert!(!cfg.to_string().contains(ADMIN_KEY));

    // --- default data: created once, then skipped --------------------------------------------
    let (st, d) = call(&c, post.clone(), admin("/data/defaults"), None).await;
    assert_eq!(st, 200, "{d}");
    let (st, d2) = call(&c, post.clone(), admin("/data/defaults"), None).await;
    assert_eq!(st, 200);
    assert_eq!(d2["audiences"].as_array().unwrap().len(), 0, "{d2}");
    assert_eq!(d2["metrics"].as_array().unwrap().len(), 0, "{d2}");
    let (st, a) = call(&c, get.clone(), admin("/audiences/spenders"), None).await;
    assert_eq!(st, 200, "{a}");
    assert_eq!(a["name"], "Spenders");
    let (st, m) = call(&c, get.clone(), admin("/metrics/revenue"), None).await;
    assert_eq!(st, 200, "{m}");
    assert_eq!(m["metric"]["aggregation"], "sum", "{m}");
    // the default-audience expressions round-trip through the parser
    let (st, e) = call(
        &c,
        get.clone(),
        admin("/audiences/new_players/expression"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert!(
        e["expression"].as_str().unwrap().contains("within_last"),
        "{e}"
    );

    // --- delete-all identities is gated (flag off by default) --------------------------------
    let (st, _) = call(
        &c,
        delete.clone(),
        admin("/audiences/spenders/members/all"),
        None,
    )
    .await;
    assert_eq!(st, 403);

    // --- rule diff emits flag.rule.* webhooks ----------------------------------------------
    let mut rx = state.webhooks.bus().subscribe();
    let (st, _) = call(&c, put.clone(),
        admin(&format!("/flags/{fkey}")),
        Some(json!({
            "rules": [{ "id": "vip", "name": "vip renamed", "condition": { "prop": "custom.vip", "op": "eq", "value": true },
                        "audiences": [], "percentage": 100, "value": 7, "variants": [] },
                      { "id": "beta", "name": "beta", "condition": { "prop": "custom.beta", "op": "eq", "value": true },
                        "audiences": [], "percentage": 100, "value": 9, "variants": [] }]
        })),
    )
    .await;
    assert_eq!(st, 200);
    let mut seen = Vec::new();
    while let Ok(Ok(ev)) = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await
    {
        if ev.event_type.starts_with("flag.rule.") && ev.data["flag"] == fkey {
            seen.push((ev.event_type.clone(), ev.data["rule"]["id"].to_string()));
        }
        if seen.len() == 2 {
            break;
        }
    }
    assert!(
        seen.contains(&("flag.rule.updated".to_string(), "\"vip\"".to_string())),
        "{seen:?}"
    );
    assert!(
        seen.contains(&("flag.rule.created".to_string(), "\"beta\"".to_string())),
        "{seen:?}"
    );
}
