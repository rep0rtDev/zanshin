//! End-to-end tests for LiveOps polish: flag diff/sizes, client overrides, experiment precedence,
//! direct messages (skip-inbox, template by name, channels), integration health, 409 on referenced delete, event search, granular export. Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-polish";

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

async fn eventually<F, Fut>(mut f: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..60 {
        if f().await {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    false
}

#[tokio::test]
async fn liveops_polish_flow() {
    let Some((base, c, state)) = start().await else {
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
        .json(&json!({"id": xid, "custom": {"name": "Ana", "vip": true}}))
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
    let cli = |m: reqwest::Method, p: String, b: Option<Value>| {
        call(&c, &tok, m, format!("{base}{p}"), b)
    };

    // --- flag diff + payload sizes ---------------------------------------------------------
    let fkey = format!("f_{sfx}");
    let (st, _) = adm(reqwest::Method::POST, "/admin/v1/flags".into(), Some(json!({
        "key": fkey, "value_type": "json", "default_value": {"price": 1, "skin": "red", "bonus": {"gold": 5}},
        "rules": [{"id": "vip", "condition": {"prop": "custom.vip", "op": "eq", "value": true},
                   "value": {"price": 2, "skin": "red", "bonus": {"gold": 7, "gems": 1}}}]
    }))).await;
    assert_eq!(st, 201, "create flag");
    let (st, sizes) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/flags/{fkey}/sizes"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let sizes = sizes.as_array().unwrap();
    assert!(sizes
        .iter()
        .any(|s| s["selector"] == "default" && s["bytes"].as_u64().unwrap() > 10));
    assert!(sizes.iter().any(|s| s["selector"] == "rule:vip"));
    let (st, d) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/flags/{fkey}/diff?a=default&b=rule:vip"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{d}");
    let paths: Vec<&str> = d["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["bonus/gems", "bonus/gold", "price"], "{d}");
    assert!(d["a"]["bytes"].as_u64().unwrap() < d["b"]["bytes"].as_u64().unwrap());
    assert!(
        d["changes"][0].get("from").is_none(),
        "absent side is omitted: {d}"
    );
    let (st, _) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/flags/{fkey}/diff?a=default&b=rule:nope"),
        None,
    )
    .await;
    assert_eq!(st, 404);

    // --- two running experiments on one flag: highest priority wins, both enrol ------------
    let e1 = format!("e1_{sfx}");
    let e2 = format!("e2_{sfx}");
    for (key, prio, val) in [(&e1, 0, 10), (&e2, 5, 20)] {
        let (st, b) = adm(
            reqwest::Method::POST,
            "/admin/v1/experiments".into(),
            Some(json!({
                "key": key, "name": key, "status": "running", "flag_key": fkey, "priority": prio,
                "variants": [{"key": "a", "value": {"price": val}, "weight": 1},
                         {"key": "b", "value": {"price": val}, "weight": 1}]
            })),
        )
        .await;
        assert_eq!(st, 201, "{b}");
    }
    let (st, flags) = cli(
        reqwest::Method::GET,
        format!("/v1/flags?names={fkey}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(flags["flags"][0]["value"]["price"], 20, "{flags}");
    let (st, ov) = cli(reqwest::Method::GET, "/v1/flags/overrides".into(), None).await;
    assert_eq!(st, 200, "{ov}");
    let ovs = ov["overrides"].as_array().unwrap();
    let hit = ovs
        .iter()
        .find(|o| o["flag_key"] == fkey.as_str())
        .unwrap_or_else(|| panic!("no override for {fkey}: {ov}"));
    assert_eq!(hit["source"], "experiment");
    assert_eq!(hit["source_key"], e2.as_str());
    assert!(hit["variant"] == "a" || hit["variant"] == "b", "{hit}");

    // --- 409 when deleting a flag that experiments still pin -------------------------------
    let (st, body) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/flags/{fkey}"),
        None,
    )
    .await;
    assert_eq!(st, 409, "{body}");
    assert!(
        body.to_string().contains(&e1) && body.to_string().contains(&e2),
        "{body}"
    );
    for e in [&e1, &e2] {
        let (st, _) = adm(
            reqwest::Method::DELETE,
            format!("/admin/v1/experiments/{e}"),
            None,
        )
        .await;
        assert!(st == 200 || st == 204, "delete experiment {st}");
    }
    let (st, _) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/flags/{fkey}"),
        None,
    )
    .await;
    assert!(st == 200 || st == 204, "delete flag after refs gone: {st}");

    // --- direct message: template by name, skip inbox, channel delivery + health -----------
    let tkey = format!("tpl_{sfx}");
    let tname = format!("Direct Hello {sfx}");
    let (st, _) = adm(
        reqwest::Method::POST,
        "/admin/v1/templates".into(),
        Some(json!({
            "key": tkey, "name": tname, "format": "plaintext", "default_locale": "en",
            "translations": {"en": {"title": format!("Hi {{{{custom.name}}}} {sfx}"), "body": "welcome"}}
        })),
    )
    .await;
    assert_eq!(st, 201);
    let ikey = format!("ih_{sfx}");
    let (st, _) = adm(
        reqwest::Method::POST,
        "/admin/v1/integrations".into(),
        Some(json!({"key": ikey, "kind": "webhook", "config": {"url": "http://127.0.0.1:9/hook", "secret": "s"}})),
    )
    .await;
    assert_eq!(st, 201);
    let (st, body) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/identities/{iid}/messages"),
        Some(json!({"template_name": tname, "inbox": false})),
    )
    .await;
    assert_eq!(
        st, 400,
        "skip-inbox without any channel must be rejected: {body}"
    );
    let (st, body) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/identities/{iid}/messages"),
        Some(json!({"template_name": tname, "inbox": false, "channels": [ikey]})),
    )
    .await;
    assert_eq!(st, 201, "{body}");
    let (_, inbox) = cli(reqwest::Method::GET, "/v1/messages".into(), None).await;
    assert!(
        !inbox.to_string().contains(&format!("Hi Ana {sfx}")),
        "skip-inbox message must not land in the inbox"
    );
    let (st, integ) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/integrations/{ikey}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(integ["health"]["last_status"], "failed", "{integ}");
    assert_eq!(integ["health"]["failed_count"], 1, "{integ}");
    assert!(integ["health"]["last_at"].is_string());
    assert!(integ["health"]["last_error"].is_string());
    let (st, body) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/templates/{tkey}"),
        None,
    )
    .await;
    assert!(
        st == 200 || st == 204,
        "unused template deletes fine: {st} {body}"
    );
    let (st, body) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/identities/{iid}/messages"),
        Some(json!({"title": "Plain {{custom.name}}", "body": "no template"})),
    )
    .await;
    assert_eq!(st, 201, "{body}");
    let (_, inbox) = cli(reqwest::Method::GET, "/v1/messages".into(), None).await;
    assert!(
        inbox.to_string().contains("Plain Ana"),
        "inline direct message missing from inbox"
    );

    // --- event size limits -----------------------------------------------------------------
    let big = "x".repeat(2000);
    let (st, body) = cli(
        reqwest::Method::POST,
        "/v1/events".into(),
        Some(json!({"events": [{"name": "too_big", "value": big}]})),
    )
    .await;
    assert_eq!(st, 400, "{body}");
    let (st, body) = cli(
        reqwest::Method::POST,
        "/v1/events".into(),
        Some(json!({"events": [{"name": "meta_big", "metadata": {"blob": "y".repeat(9000)}}]})),
    )
    .await;
    assert_eq!(st, 400, "{body}");

    // --- identity event search -------------------------------------------------------------
    let ev = format!("polish_{sfx}");
    let (st, _) = cli(
        reqwest::Method::POST,
        "/v1/events".into(),
        Some(json!({"events": [
            {"name": ev, "value": "needle-in-value"},
            {"name": format!("other_{sfx}"), "metadata": {"k": "haystack"}}
        ]})),
    )
    .await;
    assert_eq!(st, 200);
    let by_name = format!("/admin/v1/identities/{iid}/events?name={ev}");
    assert!(
        eventually(|| async {
            let (_, r) = adm(reqwest::Method::GET, by_name.clone(), None).await;
            r["events"]
                .as_array()
                .is_some_and(|a| a.len() == 1 && a[0]["name"] == ev.as_str())
        })
        .await,
        "search by name: {:?}",
        adm(reqwest::Method::GET, by_name.clone(), None).await.1["events"]
            .as_array()
            .map(|a| a.iter().map(|e| e["name"].clone()).collect::<Vec<_>>())
    );
    let (_, r) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/identities/{iid}/events?q=NEEDLE"),
        None,
    )
    .await;
    let r = r["events"].as_array().unwrap();
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["name"], ev.as_str());
    let (_, r) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/identities/{iid}/events?q=haystack"),
        None,
    )
    .await;
    assert_eq!(r["events"].as_array().unwrap().len(), 1, "{r}");
    let (_, r) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/identities/{iid}/events?to=2000-01-01T00:00:00Z"),
        None,
    )
    .await;
    assert_eq!(r["events"].as_array().unwrap().len(), 0, "{r}");

    // --- granular export by kind + key ----------------------------------------------------
    let f2 = format!("g_{sfx}");
    let f3 = format!("h_{sfx}");
    for k in [&f2, &f3] {
        let (st, _) = adm(
            reqwest::Method::POST,
            "/admin/v1/flags".into(),
            Some(json!({"key": k, "value_type": "number", "default_value": 1})),
        )
        .await;
        assert_eq!(st, 201);
    }
    let (st, b) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/config/export?kinds=flags&keys={f2}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let ents = b["entities"].as_object().unwrap();
    assert_eq!(ents.keys().collect::<Vec<_>>(), vec!["flags"], "{b}");
    let flags = ents["flags"].as_array().unwrap();
    assert_eq!(flags.len(), 1, "{b}");
    assert_eq!(flags[0]["key"], f2.as_str());
    let (st, plan) = adm(
        reqwest::Method::POST,
        "/admin/v1/config/import".into(),
        Some(json!({"bundle": b, "mode": "merge", "dry_run": true, "keys": [f3]})),
    )
    .await;
    assert_eq!(st, 200, "{plan}");
    assert_eq!(plan["plan"]["summary"]["unchanged"], 0, "{plan}");
    assert_eq!(plan["plan"]["summary"]["changed"], 0, "{plan}");
    let (st, plan) = adm(
        reqwest::Method::POST,
        "/admin/v1/config/import".into(),
        Some(json!({"bundle": b, "mode": "merge", "dry_run": true, "keys": [f2]})),
    )
    .await;
    assert_eq!(st, 200, "{plan}");
    assert_eq!(plan["plan"]["summary"]["unchanged"], 1, "{plan}");

    // --- rejected events reach sinks as $event_error --------------------------------------
    let (_, settings) = adm(
        reqwest::Method::GET,
        "/admin/v1/taxonomy/settings".into(),
        None,
    )
    .await;
    let prev_mode = settings["mode"].clone();
    let bad = format!("bad_{sfx}");
    let (st, _) = adm(
        reqwest::Method::POST,
        "/admin/v1/taxonomy/events".into(),
        Some(json!({"name": bad, "value_type": "number"})),
    )
    .await;
    assert!(st == 200 || st == 201, "{st}");
    let (st, _) = adm(
        reqwest::Method::PUT,
        "/admin/v1/taxonomy/settings".into(),
        Some(json!({"mode": "strict"})),
    )
    .await;
    assert_eq!(st, 200);
    let (st, body) = cli(
        reqwest::Method::POST,
        "/v1/events".into(),
        Some(json!({"events": [{"name": bad, "value": "not-a-number"}]})),
    )
    .await;
    assert_eq!(st, 400, "{body}");
    let errs = format!("/admin/v1/identities/{iid}/events?name=%24event_error");
    assert!(
        eventually(|| async {
            let (_, r) = adm(reqwest::Method::GET, errs.clone(), None).await;
            r["events"].as_array().is_some_and(|a| {
                a.iter().any(|e| {
                    e["metadata"]["event"] == bad.as_str() && e["metadata"]["reason"] != Value::Null
                })
            })
        })
        .await,
        "$event_error for {bad} not stored"
    );
    let (_, r) = adm(reqwest::Method::GET, errs.clone(), None).await;
    let n = r["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["metadata"]["event"] == bad.as_str())
        .count();
    assert_eq!(n, 1, "exactly one $event_error per rejected event: {r}");
    let (st, _) = adm(
        reqwest::Method::PUT,
        "/admin/v1/taxonomy/settings".into(),
        Some(json!({"mode": prev_mode})),
    )
    .await;
    assert_eq!(st, 200);

    // --- $identity_delete survives the GDPR wipe ------------------------------------------
    let (st, r) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/data/identities/{iid}"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{r}");
    let (st, _) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/identities/{iid}"),
        None,
    )
    .await;
    assert_eq!(st, 404);
    let iid_uuid: uuid::Uuid = iid.parse().unwrap();
    let search = zanshin_store::events::EventSearch {
        name: Some("$identity_delete".into()),
        ..Default::default()
    };
    assert!(
        eventually(|| async {
            state
                .events
                .search_for_identity(iid_uuid, &search, 10)
                .await
                .is_ok_and(|evs| evs.len() == 1)
        })
        .await,
        "$identity_delete tombstone missing"
    );
    let gone = state
        .events
        .search_for_identity(iid_uuid, &Default::default(), 100)
        .await
        .unwrap();
    assert!(
        gone.iter().all(|e| e.name == "$identity_delete"),
        "player events must be wiped, only the tombstone stays: {:?}",
        gone.iter().map(|e| e.name.clone()).collect::<Vec<_>>()
    );
}
