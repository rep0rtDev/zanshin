//! End-to-end tests for taxonomy, identify/merge and play sessions.
//! Run only when ZANSHIN_TEST_DATABASE_URL and ZANSHIN_TEST_REDIS_URL are set.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-taxonomy";

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
    std::env::set_var("ZANSHIN__SERVER__TRUST_PROXY", "true");
    std::env::set_var("ZANSHIN__PUSH__PROVIDER", "log");
    std::env::set_var("ZANSHIN__TAXONOMY__MODE", "warn");
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

async fn admin(
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
    let status = resp.status().as_u16();
    let v = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, v)
}

async fn client_key(c: &reqwest::Client, base: &str) -> String {
    let (_, v) = admin(
        c,
        reqwest::Method::POST,
        format!("{base}/admin/v1/api-keys"),
        Some(json!({"name": "tax-client", "kind": "client"})),
    )
    .await;
    v["secret"].as_str().unwrap().to_string()
}

async fn server_key(c: &reqwest::Client, base: &str) -> String {
    let (_, v) = admin(
        c,
        reqwest::Method::POST,
        format!("{base}/admin/v1/api-keys"),
        Some(json!({"name": "tax-server", "kind": "server"})),
    )
    .await;
    v["secret"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn labels_event_definitions_schemas_and_read_only_properties() {
    let Some((base, c)) = start().await else {
        return;
    };
    let suffix = uuid::Uuid::now_v7().simple().to_string();

    // Labels: create, list, usage, rename propagates to tagged flags, delete strips tag.
    let label = format!("season-{suffix}");
    let (st, l) = admin(
        &c,
        reqwest::Method::POST,
        format!("{base}/admin/v1/labels"),
        Some(json!({"name": label, "color": "#ff0000"})),
    )
    .await;
    assert_eq!(st, 200, "{l}");
    let label_id = l["id"].as_str().unwrap().to_string();
    let flag_key = format!("tax_flag_{suffix}");
    let (st, _) = admin(
        &c,
        reqwest::Method::POST,
        format!("{base}/admin/v1/flags"),
        Some(json!({
            "key": flag_key, "value_type": "bool", "default_value": true, "tags": [label]
        })),
    )
    .await;
    assert_eq!(st, 201);
    let (_, filtered) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/flags?tags={label}"),
        None,
    )
    .await;
    assert_eq!(filtered.as_array().unwrap().len(), 1);
    let (_, none) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/flags?tags=nope-{suffix}"),
        None,
    )
    .await;
    assert_eq!(none.as_array().unwrap().len(), 0);
    let (_, usage) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/labels/{label_id}/usage"),
        None,
    )
    .await;
    let flags_usage = usage
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["table"] == "flags")
        .unwrap();
    assert_eq!(flags_usage["count"], 1);
    let renamed = format!("season2-{suffix}");
    let (st, _) = admin(
        &c,
        reqwest::Method::PUT,
        format!("{base}/admin/v1/labels/{label_id}"),
        Some(json!({"name": renamed})),
    )
    .await;
    assert_eq!(st, 200);
    let (_, f) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/flags/{flag_key}"),
        None,
    )
    .await;
    assert_eq!(f["tags"], json!([renamed]));
    let (st, _) = admin(
        &c,
        reqwest::Method::DELETE,
        format!("{base}/admin/v1/labels/{label_id}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let (_, f) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/flags/{flag_key}"),
        None,
    )
    .await;
    assert_eq!(f["tags"], json!([]));

    // Event definitions: core events seeded; custom definition with revenue + metadata schema.
    let (_, defs) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/taxonomy/events"),
        None,
    )
    .await;
    assert!(defs
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["name"] == "purchaseCompleted" && d["is_core"] == true));
    let ev_name = format!("bossKill_{suffix}");
    let (st, d) = admin(&c, reqwest::Method::POST, format!("{base}/admin/v1/taxonomy/events"), Some(json!({
        "name": ev_name, "value_type": "number",
        "metadata_schema": {"type": "object", "required": ["boss"], "properties": {"boss": {"type": "string"}}}
    }))).await;
    assert_eq!(st, 200, "{d}");
    let (st, _) = admin(
        &c,
        reqwest::Method::DELETE,
        format!("{base}/admin/v1/taxonomy/events/purchaseCompleted"),
        None,
    )
    .await;
    assert_eq!(st, 404, "core events cannot be deleted");

    // Strict mode: invalid events rejected and visible in the debugger; valid ones accepted.
    let (st, settings) = admin(
        &c,
        reqwest::Method::PUT,
        format!("{base}/admin/v1/taxonomy/settings"),
        Some(json!({"mode": "strict"})),
    )
    .await;
    assert_eq!(st, 200, "{settings}");
    assert_eq!(settings["mode"], "strict");
    let ck = client_key(&c, &base).await;
    let sess: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .header("X-Zanshin-Platform", "ios")
        .header("X-Zanshin-App-Version", "1.2.3")
        .header("Accept-Language", "de-DE,de;q=0.9")
        .header("X-Forwarded-For", "203.0.113.9")
        .header("CF-IPCountry", "de")
        .json(&json!({"id": format!("tax-user-{suffix}")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = sess["token"].as_str().unwrap().to_string();
    let d = &sess["properties"]["default"];
    assert_eq!(d["platform"], "ios");
    assert_eq!(d["app_version"], "1.2.3");
    assert_eq!(d["locale"], "de-DE");
    assert_eq!(d["language"], "de");
    assert_eq!(d["country"], "DE");

    let r = c.post(format!("{base}/v1/events")).bearer_auth(&token)
        .json(&json!({"events": [{"name": ev_name, "value": "not-a-number", "metadata": {"boss": "x"}}]}))
        .send().await.unwrap();
    assert_eq!(r.status(), 400, "numeric value required");
    let r = c
        .post(format!("{base}/v1/events"))
        .bearer_auth(&token)
        .json(&json!({"events": [{"name": ev_name, "value": "3", "metadata": {}}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400, "metadata schema violated");
    let r: Value = c
        .post(format!("{base}/v1/events"))
        .bearer_auth(&token)
        .json(&json!({"events": [
            {"name": ev_name, "value": "3", "metadata": {"boss": "ogre"}},
            {"name": "purchaseCompleted", "value": "gems_100"}
        ]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        r["accepted"], 1,
        "revenue event without amount/currency dropped, valid one kept"
    );
    let (_, rejected) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/taxonomy/rejected"),
        None,
    )
    .await;
    let rejected = rejected.as_array().unwrap();
    assert!(rejected.len() >= 3, "{rejected:?}");
    assert!(rejected
        .iter()
        .any(|r| r["name"] == "purchaseCompleted" && r["rejected"] == true));
    let r = c
        .post(format!("{base}/v1/events"))
        .bearer_auth(&token)
        .json(&json!({"events": [{"name": format!("unknown_{suffix}"), "value": "1"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "unknown events auto-discovered");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (_, defs) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/taxonomy/events"),
        None,
    )
    .await;
    assert!(defs
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["name"] == format!("unknown_{suffix}") && d["discovered"] == true));
    admin(
        &c,
        reqwest::Method::PUT,
        format!("{base}/admin/v1/taxonomy/settings"),
        Some(json!({"mode": "warn"})),
    )
    .await;

    // Read-only custom property: client rejected, server allowed.
    let (st, _) = admin(
        &c,
        reqwest::Method::PUT,
        format!("{base}/admin/v1/taxonomy/properties/vip_{suffix}"),
        Some(json!({"read_only": true})),
    )
    .await;
    assert_eq!(st, 200);
    let r = c
        .put(format!("{base}/v1/properties"))
        .bearer_auth(&token)
        .json(&json!({"custom": {format!("vip_{suffix}"): true}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let r = c
        .put(format!("{base}/v1/properties"))
        .bearer_auth(&token)
        .json(&json!({"custom": {"nickname": "zed"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let (_, props) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/taxonomy/properties"),
        None,
    )
    .await;
    assert!(props["config"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["key"] == format!("vip_{suffix}")));
    assert!(props["custom"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["key"] == "nickname"));

    // Value schemas: validate endpoint, attach to flag, invalid values rejected.
    let (st, v) = admin(&c, reqwest::Method::POST, format!("{base}/admin/v1/schemas/validate"), Some(json!({
        "schema": {"type": "object", "required": ["hp"], "properties": {"hp": {"type": "integer", "minimum": 1}}},
        "samples": [{"hp": 10}, {"hp": 0}, {}]
    }))).await;
    assert_eq!(st, 200, "{v}");
    let results = v["results"].as_array().unwrap();
    assert!(results[0].is_null() && results[1].is_string() && results[2].is_string());
    let (st, sch) = admin(&c, reqwest::Method::POST, format!("{base}/admin/v1/schemas"), Some(json!({
        "name": format!("boss-{suffix}"), "schema": {"type": "object", "required": ["hp"], "properties": {"hp": {"type": "integer"}}}
    }))).await;
    assert_eq!(st, 200, "{sch}");
    let (st, _) = admin(
        &c,
        reqwest::Method::POST,
        format!("{base}/admin/v1/schemas"),
        Some(json!({
            "name": format!("bad-{suffix}"), "schema": {"type": "not-a-type"}
        })),
    )
    .await;
    assert_eq!(st, 400);
}

#[tokio::test]
async fn identify_merges_identities_and_tracks_sessions() {
    let Some((base, c)) = start().await else {
        return;
    };
    let suffix = uuid::Uuid::now_v7().simple().to_string();
    let ck = client_key(&c, &base).await;
    let sk = server_key(&c, &base).await;

    // Anonymous device id plays, earns custom props and events.
    let anon_id = format!("device-{suffix}");
    let sess: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .json(&json!({"id": anon_id, "custom": {"level": 3, "from_anon": true}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let anon_token = sess["token"].as_str().unwrap().to_string();
    let anon_uuid = sess["identity"]["id"].as_str().unwrap().to_string();
    c.post(format!("{base}/v1/events")).bearer_auth(&anon_token)
        .json(&json!({"events": [{"name": "levelCompleted", "value": "1"}, {"name": "levelCompleted", "value": "2"}]}))
        .send().await.unwrap();

    // Identify to a brand-new account id: rename in place, same identity uuid.
    let acct = format!("acct-{suffix}");
    let r = c
        .post(format!("{base}/v1/identify"))
        .bearer_auth(&anon_token)
        .json(&json!({"id": acct}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let renamed: Value = r.json().await.unwrap();
    assert_eq!(renamed["identity"]["id"], anon_uuid);
    assert_eq!(renamed["identity"]["external_id"], acct);
    let token = renamed["token"].as_str().unwrap().to_string();

    // A second device creates another anonymous identity, then identifies into the account → merge.
    let dev2 = format!("device2-{suffix}");
    let sess2: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .json(&json!({"id": dev2, "custom": {"level": 1, "from_dev2": true}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let dev2_token = sess2["token"].as_str().unwrap().to_string();
    let dev2_uuid = sess2["identity"]["id"].as_str().unwrap().to_string();
    c.post(format!("{base}/v1/events"))
        .bearer_auth(&dev2_token)
        .json(&json!({"events": [{"name": "levelCompleted", "value": "9"}]}))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    let merged: Value = c
        .post(format!("{base}/v1/identify"))
        .bearer_auth(&dev2_token)
        .json(&json!({"id": acct}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        merged["identity"]["id"], anon_uuid,
        "merged into the existing account"
    );
    let custom = &merged["properties"]["custom"];
    assert_eq!(custom["level"], 3, "target wins on conflict");
    assert_eq!(custom["from_anon"], true);
    assert_eq!(custom["from_dev2"], true);
    let computed = &merged["properties"]["computed"];
    assert_eq!(computed["events.levelCompleted.count"], 3, "{computed}");

    // Old identity gone, its token no longer valid, merge recorded, events moved.
    let (st, _) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/identities/{dev2_uuid}"),
        None,
    )
    .await;
    assert_eq!(st, 404);
    let r = c
        .get(format!("{base}/v1/identity"))
        .bearer_auth(&dev2_token)
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 200);
    let (_, merges) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/identities/{anon_uuid}/merges"),
        None,
    )
    .await;
    assert_eq!(merges.as_array().unwrap().len(), 1);
    assert_eq!(merges[0]["from_external"], dev2);
    let (_, events) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/identities/{anon_uuid}/events?limit=50"),
        None,
    )
    .await;
    let names: Vec<&str> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(
        names.iter().filter(|n| **n == "levelCompleted").count() >= 3,
        "{names:?}"
    );
    assert!(names.contains(&"$identify"));
    assert!(names.contains(&"$identity_create"));
    assert!(names.contains(&"$session_start"));

    // Server-side identify (Nakama path) renames again.
    let acct2 = format!("acct2-{suffix}");
    let r = c
        .post(format!("{base}/server/v1/identities/{acct}/identify"))
        .bearer_auth(&sk)
        .json(&json!({"id": acct2}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["external_id"], acct2);

    // Sessions: logout closes the play session with a duration and emits $session_end.
    let _ = token;
    let new_token = merged["token"].as_str().unwrap().to_string();
    let r = c
        .post(format!("{base}/v1/authenticate/logout"))
        .bearer_auth(&ck)
        .json(&json!({"token": new_token}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let (_, sessions) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/identities/{anon_uuid}/sessions"),
        None,
    )
    .await;
    let sessions = sessions.as_array().unwrap();
    assert!(sessions.len() >= 3, "{sessions:?}");
    assert!(sessions
        .iter()
        .any(|s| s["ended_at"].is_string() && s["duration_sec"].is_number()));
    let (_, events) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/identities/{anon_uuid}/events?limit=50"),
        None,
    )
    .await;
    assert!(events["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["name"] == "$session_end"));
    let (_, ident) = admin(
        &c,
        reqwest::Method::GET,
        format!("{base}/admin/v1/identities/{anon_uuid}"),
        None,
    )
    .await;
    assert!(
        ident["properties"]["computed"]["play_time_sec"].is_number(),
        "{ident}"
    );
}
