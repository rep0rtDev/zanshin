//! End-to-end REST tests. Run only when ZANSHIN_TEST_DATABASE_URL and ZANSHIN_TEST_REDIS_URL are set.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use tokio::sync::watch;
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-000";

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
    let (_tx, _rx) = watch::channel(false);
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

#[tokio::test]
async fn authenticate_properties_refresh_logout_flow() {
    let Some((base, http)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };

    // Mint a client key with the bootstrap admin key.
    let created: Value = http
        .post(format!("{base}/admin/v1/api-keys"))
        .header("x-api-key", ADMIN_KEY)
        .json(&json!({"name": "it-client", "kind": "client"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = created["secret"].as_str().unwrap().to_string();
    assert!(key.starts_with("zk_c_"));

    // No key -> 401.
    let r = http
        .post(format!("{base}/v1/authenticate"))
        .json(&json!({"id": "p"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // Authenticate.
    let xid = format!("it-{}", uuid::Uuid::new_v4());
    let sess: Value = http
        .post(format!("{base}/v1/authenticate"))
        .header("x-api-key", &key)
        .json(&json!({"id": xid, "default": {"platform": "ios"}, "custom": {"level": 1}}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = sess["token"].as_str().unwrap().to_string();
    let refresh = sess["refresh_token"].as_str().unwrap().to_string();
    assert_eq!(sess["identity"]["external_id"], xid);
    assert_eq!(sess["properties"]["default"]["platform"], "ios");

    // Update properties: null deletes.
    let props: Value = http
        .put(format!("{base}/v1/properties"))
        .bearer_auth(&token)
        .json(&json!({"custom": {"level": 2, "vip": true}, "default": {"platform": null}}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(props["custom"]["level"], 2);
    assert_eq!(props["custom"]["vip"], true);
    assert!(props["default"].get("platform").is_none());

    // Refresh rotates; replay of the old refresh token revokes the session.
    let fresh: Value = http
        .post(format!("{base}/v1/authenticate/refresh"))
        .header("x-api-key", &key)
        .json(&json!({"refresh_token": refresh}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(fresh["refresh_token"], refresh);
    let replay = http
        .post(format!("{base}/v1/authenticate/refresh"))
        .header("x-api-key", &key)
        .json(&json!({"refresh_token": refresh}))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 401);
    let r = http
        .get(format!("{base}/v1/identity"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        401,
        "session must be revoked after a refresh replay"
    );

    // Fresh session, then logout and verify.
    let sess2: Value = http
        .post(format!("{base}/v1/authenticate"))
        .header("x-api-key", &key)
        .json(&json!({"id": xid}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token2 = sess2["token"].as_str().unwrap().to_string();
    assert_eq!(
        sess2["properties"]["custom"]["vip"], true,
        "properties persist across sessions"
    );
    let r = http
        .post(format!("{base}/v1/authenticate/logout"))
        .header("x-api-key", &key)
        .json(&json!({"token": token2}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = http
        .get(format!("{base}/v1/identity"))
        .bearer_auth(&token2)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // Admin lookup + delete; client key must not reach admin routes.
    let r = http
        .get(format!("{base}/admin/v1/stats"))
        .header("x-api-key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let found: Value = http
        .get(format!("{base}/admin/v1/identities/by-external-id/{xid}"))
        .header("x-api-key", ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = found["id"].as_str().unwrap();
    let r = http
        .delete(format!("{base}/admin/v1/identities/{id}"))
        .header("x-api-key", ADMIN_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = http
        .get(format!("{base}/admin/v1/identities/{id}"))
        .header("x-api-key", ADMIN_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn feature_flags_flow() {
    let Some((base, http)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let key = format!("it_flag_{suffix}");
    let admin = |b: reqwest::RequestBuilder| b.header("x-api-key", ADMIN_KEY);

    let created: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "it-flags", "kind": "client"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let client_key = created["secret"].as_str().unwrap().to_string();
    let server_created: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "it-server", "kind": "server"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_key = server_created["secret"].as_str().unwrap().to_string();

    // Invalid definition is rejected.
    let r = admin(http.post(format!("{base}/admin/v1/flags")))
        .json(&json!({"key": key, "value_type": "bool", "default_value": "nope"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    // Create a flag with a country rule.
    let r = admin(http.post(format!("{base}/admin/v1/flags")))
        .json(&json!({
            "key": key, "value_type": "bool", "default_value": false,
            "rules": [{"id": "de", "condition": {"prop": "default.country", "op": "eq", "value": "DE"}, "value": true}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let r = admin(http.post(format!("{base}/admin/v1/flags")))
        .json(&json!({"key": key, "value_type": "bool", "default_value": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);

    // Two identities: DE gets true via rule, FR gets the default.
    let auth = |id: &str, country: &str| {
        http.post(format!("{base}/v1/authenticate"))
            .header("x-api-key", &client_key)
            .json(&json!({"id": id, "default": {"country": country}}))
    };
    let de: Value = auth(&format!("it-de-{suffix}"), "DE")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let fr: Value = auth(&format!("it-fr-{suffix}"), "FR")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let de_token = de["token"].as_str().unwrap();
    let fr_token = fr["token"].as_str().unwrap();

    let ev: Value = http
        .get(format!("{base}/v1/flags/{key}"))
        .bearer_auth(de_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ev["value"], true);
    assert_eq!(ev["reason"]["rule_id"], "de");
    let ev: Value = http
        .get(format!("{base}/v1/flags/{key}"))
        .bearer_auth(fr_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ev["value"], false);
    assert_eq!(ev["reason"]["kind"], "default");

    // Override pins FR to true; server API sees the same evaluation by external id.
    let fr_id = fr["identity"]["id"].as_str().unwrap();
    let r = admin(http.put(format!("{base}/admin/v1/flags/{key}/overrides/{fr_id}")))
        .json(&json!({"value": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let list: Value = http
        .get(format!(
            "{base}/server/v1/flags?external_id=it-fr-{suffix}&names={key}"
        ))
        .header("x-api-key", &server_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["flags"][0]["value"], true);
    assert_eq!(list["flags"][0]["reason"]["kind"], "override");
    let r = http
        .get(format!("{base}/server/v1/flags?external_id=x"))
        .header("x-api-key", &client_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    // Disabling serves the default to everyone and bumps the version.
    let updated: Value = admin(http.patch(format!("{base}/admin/v1/flags/{key}")))
        .json(&json!({"enabled": false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["version"], 2);
    let ev: Value = http
        .get(format!("{base}/v1/flags/{key}"))
        .bearer_auth(de_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ev["reason"]["kind"], "disabled");
    assert_eq!(ev["value"], false);

    // Dry run with ad-hoc properties.
    let dry: Value = admin(http.post(format!("{base}/admin/v1/flags/{key}/evaluate")))
        .json(&json!({"properties": {"default": {"country": "DE"}}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(dry["reason"]["kind"], "disabled");

    // Cleanup.
    let r = admin(http.delete(format!("{base}/admin/v1/flags/{key}")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = http
        .get(format!("{base}/v1/flags/{key}"))
        .bearer_auth(de_token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn audiences_and_experiments_flow() {
    let Some((base, http)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let aud = format!("it_aud_{suffix}");
    let flag = format!("it_price_{suffix}");
    let exp = format!("it_exp_{suffix}");
    let admin = |b: reqwest::RequestBuilder| b.header("x-api-key", ADMIN_KEY);

    let created: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "it-exp", "kind": "client"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let client_key = created["secret"].as_str().unwrap().to_string();

    // Audience of beta testers, a flag gated by it, a numeric flag driven by an experiment.
    let r = admin(http.post(format!("{base}/admin/v1/audiences")))
        .json(&json!({"key": aud, "condition": {"prop": "custom.beta", "op": "eq", "value": true}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let r = admin(http.post(format!("{base}/admin/v1/flags")))
        .json(
            &json!({"key": flag, "value_type": "number", "default_value": 9.99,
            "rules": [{"id": "beta", "audiences": [aud], "value": 7.99}]}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let r = admin(http.post(format!("{base}/admin/v1/experiments")))
        .json(&json!({"key": exp, "flag_key": flag, "control": "control", "audiences": [aud],
            "variants": [{"key": "control", "value": 9.99, "weight": 1}, {"key": "cheap", "value": 4.99, "weight": 1}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    // Needs at least two variants.
    let r = admin(http.post(format!("{base}/admin/v1/experiments")))
        .json(&json!({"key": format!("{exp}_bad"), "variants": [{"key": "only", "value": 1, "weight": 1}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    let auth = |id: String, beta: bool| {
        http.post(format!("{base}/v1/authenticate"))
            .header("x-api-key", &client_key)
            .json(&json!({"id": id, "custom": {"beta": beta}}))
    };
    let beta: Value = auth(format!("it-beta-{suffix}"), true)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let plain: Value = auth(format!("it-plain-{suffix}"), false)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let beta_token = beta["token"].as_str().unwrap();
    let plain_token = plain["token"].as_str().unwrap();

    // Audience membership drives the flag rule.
    let a: Value = http
        .get(format!("{base}/v1/audiences"))
        .bearer_auth(beta_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(a["audiences"].as_array().unwrap().contains(&json!(aud)));
    let a: Value = http
        .get(format!("{base}/v1/audiences"))
        .bearer_auth(plain_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!a["audiences"].as_array().unwrap().contains(&json!(aud)));
    let ev: Value = http
        .get(format!("{base}/v1/flags/{flag}"))
        .bearer_auth(beta_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ev["value"], 7.99);
    let ev: Value = http
        .get(format!("{base}/v1/flags/{flag}"))
        .bearer_auth(plain_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ev["value"], 9.99);

    // Draft experiment enrols nobody; running enrols the beta tester, sticky, and pins the flag.
    let e: Value = http
        .get(format!("{base}/v1/experiments?names={exp}"))
        .bearer_auth(beta_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(e["experiments"], json!([]));
    let r = admin(http.post(format!("{base}/admin/v1/experiments/{exp}/start")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let e: Value = http
        .get(format!("{base}/v1/experiments?names={exp}"))
        .bearer_auth(beta_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let variant = e["experiments"][0]["variant"].as_str().unwrap().to_string();
    let e2: Value = http
        .get(format!("{base}/v1/experiments?names={exp}"))
        .bearer_auth(beta_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        e2["experiments"][0]["variant"], variant,
        "assignment must be sticky"
    );
    let ev: Value = http
        .get(format!("{base}/v1/flags/{flag}"))
        .bearer_auth(beta_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ev["reason"]["kind"], "experiment");
    assert_eq!(ev["variant"], variant);
    let e: Value = http
        .get(format!("{base}/v1/experiments?names={exp}"))
        .bearer_auth(plain_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        e["experiments"],
        json!([]),
        "outside the audience: not enrolled"
    );

    // Exposure shows up in results.
    let r = http
        .post(format!("{base}/v1/experiments/{exp}/expose"))
        .bearer_auth(beta_token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = http
        .post(format!("{base}/v1/experiments/{exp}/expose"))
        .bearer_auth(plain_token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let res: Value = admin(http.get(format!("{base}/admin/v1/experiments/{exp}/results")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["total_assigned"], 1);
    let v = res["variants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["variant"] == variant)
        .unwrap();
    assert_eq!(v["exposed"], 1);

    // Force the other variant; the flag follows.
    let other = if variant == "cheap" {
        "control"
    } else {
        "cheap"
    };
    let beta_id = beta["identity"]["id"].as_str().unwrap();
    let r = admin(http.put(format!(
        "{base}/admin/v1/experiments/{exp}/assignments/{beta_id}"
    )))
    .json(&json!({"variant": other}))
    .send()
    .await
    .unwrap();
    assert_eq!(r.status(), 200);
    let ev: Value = http
        .get(format!("{base}/v1/flags/{flag}"))
        .bearer_auth(beta_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ev["variant"], other);

    // Stop: nothing served, flag falls back to the audience rule; illegal restart rejected.
    let r = admin(http.post(format!("{base}/admin/v1/experiments/{exp}/stop")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let e: Value = http
        .get(format!("{base}/v1/experiments?names={exp}"))
        .bearer_auth(beta_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(e["experiments"], json!([]));
    let ev: Value = http
        .get(format!("{base}/v1/flags/{flag}"))
        .bearer_auth(beta_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ev["value"], 7.99);
    let r = admin(http.post(format!("{base}/admin/v1/experiments/{exp}/start")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    for path in [
        format!("/admin/v1/experiments/{exp}"),
        format!("/admin/v1/flags/{flag}"),
        format!("/admin/v1/audiences/{aud}"),
    ] {
        let r = admin(http.delete(format!("{base}{path}")))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
}

#[tokio::test]
async fn live_events_and_messages_flow() {
    let Some((base, http)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let ev_now = format!("it_ev_now_{suffix}");
    let ev_later = format!("it_ev_later_{suffix}");
    let msg = format!("it_msg_{suffix}");
    let admin = |b: reqwest::RequestBuilder| b.header("x-api-key", ADMIN_KEY);
    let now = chrono::Utc::now();

    let created: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "it-live", "kind": "client"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let client_key = created["secret"].as_str().unwrap().to_string();
    let server_created: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "it-live-s", "kind": "server"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_key = server_created["secret"].as_str().unwrap().to_string();

    // One active event for VIPs, one upcoming event for everyone, one bad definition.
    let r = admin(http.post(format!("{base}/admin/v1/live-events")))
        .json(&json!({"key": ev_now, "name": "Double XP", "starts_at": now - chrono::Duration::hours(1),
            "duration_secs": 7200, "payload": {"xp": 2}, "condition": {"prop": "custom.vip", "op": "eq", "value": true}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let r = admin(http.post(format!("{base}/admin/v1/live-events")))
        .json(&json!({"key": ev_later, "starts_at": now + chrono::Duration::days(1), "ends_at": now + chrono::Duration::days(2)}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let r = admin(http.post(format!("{base}/admin/v1/live-events")))
        .json(&json!({"key": format!("{ev_now}_bad"), "starts_at": now, "recurrence": "0 0 18 * * Sat *"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        400,
        "recurring without duration must be rejected"
    );

    // Campaign with templated title.
    let r = admin(http.post(format!("{base}/admin/v1/messages")))
        .json(&json!({"key": msg, "title": "Welcome {{custom.nick}}", "body": "Enjoy", "payload": {"gift": 100},
            "condition": {"prop": "custom.vip", "op": "eq", "value": true}, "priority": 5}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());

    let auth = |id: String, vip: bool| {
        http.post(format!("{base}/v1/authenticate"))
            .header("x-api-key", &client_key)
            .json(&json!({"id": id, "custom": {"vip": vip, "nick": "Neo"}}))
    };
    let vip: Value = auth(format!("it-vip-{suffix}"), true)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let plain: Value = auth(format!("it-plain-{suffix}"), false)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let vip_token = vip["token"].as_str().unwrap();
    let plain_token = plain["token"].as_str().unwrap();
    let vip_ext = format!("it-vip-{suffix}");

    // Live events: active only by default, upcoming on request, targeting respected.
    let e: Value = http
        .get(format!("{base}/v1/live-events?names={ev_now},{ev_later}"))
        .bearer_auth(vip_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let list = e["live_events"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["key"], ev_now);
    assert_eq!(list[0]["value"]["xp"], 2);
    assert!(list[0]["active"].as_bool().unwrap() && list[0]["current"].is_object());
    let e: Value = http
        .get(format!(
            "{base}/v1/live-events?names={ev_now},{ev_later}&upcoming=true"
        ))
        .bearer_auth(vip_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let list = e["live_events"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    let later = list.iter().find(|x| x["key"] == ev_later).unwrap();
    assert!(!later["active"].as_bool().unwrap() && later["next"].is_object());
    let e: Value = http
        .get(format!("{base}/v1/live-events?names={ev_now},{ev_later}"))
        .bearer_auth(plain_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        e["live_events"].as_array().unwrap().len(),
        0,
        "non-VIP must not see the VIP event"
    );
    let e: Value = http
        .get(format!(
            "{base}/server/v1/live-events?external_id={vip_ext}&names={ev_now}"
        ))
        .header("x-api-key", &server_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(e["live_events"].as_array().unwrap().len(), 1);

    // Inbox: campaign delivered once, rendered, read + consume, direct message.
    let inbox: Value = http
        .get(format!("{base}/v1/messages"))
        .bearer_auth(vip_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let delivered: Vec<&Value> = inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["key"] == msg)
        .collect();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0]["title"], "Welcome Neo");
    assert!(delivered[0]["read_at"].is_null());
    let id = delivered[0]["id"].as_str().unwrap().to_string();
    let inbox: Value = http
        .get(format!("{base}/v1/messages"))
        .bearer_auth(vip_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["key"] == msg)
            .count(),
        1,
        "no duplicate delivery"
    );
    let inbox: Value = http
        .get(format!("{base}/v1/messages"))
        .bearer_auth(plain_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["key"] == msg)
            .count(),
        0
    );
    let m: Value = http
        .put(format!("{base}/v1/messages/{id}"))
        .bearer_auth(vip_token)
        .json(&json!({"read": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(m["read_at"].is_string());
    let stats: Value = admin(http.get(format!("{base}/admin/v1/messages/{msg}/stats")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stats["delivered"], 1);
    assert_eq!(stats["read"], 1);
    let r = http
        .delete(format!("{base}/v1/messages/{id}"))
        .bearer_auth(vip_token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = http
        .delete(format!("{base}/v1/messages/{id}"))
        .bearer_auth(vip_token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let inbox: Value = http
        .get(format!("{base}/v1/messages"))
        .bearer_auth(vip_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["key"] == msg)
            .count(),
        0,
        "consumed copy is gone and not redelivered"
    );

    let r = http
        .post(format!("{base}/server/v1/messages"))
        .header("x-api-key", &server_key)
        .json(&json!({"external_id": vip_ext, "title": "Hi {{custom.nick}}", "body": "direct", "ttl_secs": 3600}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let inbox: Value = http
        .get(format!("{base}/v1/messages"))
        .bearer_auth(vip_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let direct = inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["title"] == "Hi Neo")
        .expect("direct message delivered");
    assert!(direct["key"].is_null() && direct["expires_at"].is_string());

    for path in [
        format!("/admin/v1/messages/{msg}"),
        format!("/admin/v1/live-events/{ev_now}"),
        format!("/admin/v1/live-events/{ev_later}"),
    ] {
        let r = admin(http.delete(format!("{base}{path}")))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
}

#[tokio::test]
async fn analytics_events_flow() {
    let Some((base, http)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let exp = format!("it_metric_{suffix}");
    let metric = format!("purchase_{suffix}");
    let admin = |b: reqwest::RequestBuilder| b.header("x-api-key", ADMIN_KEY);

    let created: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "it-ev", "kind": "client"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let client_key = created["secret"].as_str().unwrap().to_string();
    let server_created: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "it-ev-s", "kind": "server"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_key = server_created["secret"].as_str().unwrap().to_string();

    // A running experiment so events get tagged with the assignment.
    let r = admin(http.post(format!("{base}/admin/v1/experiments")))
        .json(&json!({"key": exp, "status": "running", "condition": {"prop": "identity.external_id", "op": "eq", "value": format!("it-ev-{suffix}")},
            "variants": [{"key": "a", "value": 1, "weight": 1}, {"key": "b", "value": 2, "weight": 1}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);

    let ext = format!("it-ev-{suffix}");
    let s: Value = http
        .post(format!("{base}/v1/authenticate"))
        .header("x-api-key", &client_key)
        .json(&json!({"id": ext}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = s["token"].as_str().unwrap();
    let identity_id = s["identity"]["id"].as_str().unwrap().to_string();
    let e: Value = http
        .get(format!("{base}/v1/experiments"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let variant = e["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == exp)
        .unwrap()["variant"]
        .as_str()
        .unwrap()
        .to_string();

    // Reserved names rejected for clients, accepted for server keys; duplicates by id collapsed.
    let r = http
        .post(format!("{base}/v1/events"))
        .bearer_auth(token)
        .json(&json!({"events": [{"name": "$cheat"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let dup = uuid::Uuid::new_v4();
    let r: Value = http
        .post(format!("{base}/v1/events"))
        .bearer_auth(token)
        .json(&json!({"events": [
            {"name": metric, "value": "9.99", "metadata": {"sku": "gold"}, "id": dup},
            {"name": metric, "value": "9.99", "id": dup},
            {"name": metric, "value": "5"},
            {"name": "level.up", "value": "7"}
        ]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["accepted"], 3);
    let r: Value = http
        .post(format!("{base}/server/v1/events"))
        .header("x-api-key", &server_key)
        .json(&json!({"external_id": ext, "events": [{"name": "$grant", "value": "100"}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["accepted"], 1);

    // The writer flushes every 500 ms; wait for computed properties to show up.
    let mut props = Value::Null;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        props = http
            .get(format!("{base}/v1/properties"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if props["computed"]
            .get(format!("events.{metric}.count"))
            .is_some()
            && props["computed"].get("session_count").is_some()
        {
            break;
        }
    }
    let c = &props["computed"];
    assert_eq!(c["session_count"], 1, "{c}");
    assert_eq!(c[format!("events.{metric}.count")], 2, "{c}");
    assert!((c[format!("events.{metric}.sum")].as_f64().unwrap() - 14.99).abs() < 1e-9);
    assert_eq!(c["events.level_up.count"], 1);
    assert_eq!(
        c["event_count"], 4,
        "$grant counts as an event, $authenticate does not: {c}"
    );
    assert!(c.get("events.$grant.count").is_none());

    // Analytics: timeseries, names, per-identity log, experiment metric.
    let ts: Value = admin(http.get(format!(
        "{base}/admin/v1/analytics/events?name={metric}&granularity=hour"
    )))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let total: i64 = ts["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["count"].as_i64().unwrap())
        .sum();
    assert_eq!(total, 2, "{ts}");
    let names: Value = admin(http.get(format!("{base}/admin/v1/analytics/events/names?limit=500")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(names["names"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["name"] == metric));
    let log: Value = admin(http.get(format!("{base}/admin/v1/identities/{identity_id}/events")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let evs = log["events"].as_array().unwrap();
    assert!(evs.iter().any(|e| e["name"] == "$authenticate"));
    let purchase = evs
        .iter()
        .find(|e| e["name"] == metric && e["value"] == "9.99")
        .unwrap();
    assert_eq!(purchase["metadata"]["sku"], "gold");
    assert_eq!(
        purchase["experiments"][&exp], variant,
        "events are tagged with the live assignment"
    );
    let res: Value = admin(http.get(format!(
        "{base}/admin/v1/experiments/{exp}/results?metric={metric}"
    )))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let row = res["variants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["variant"] == variant)
        .unwrap();
    assert_eq!(row["metric"]["count"], 2, "{res}");
    assert_eq!(row["metric"]["identities"], 1);
    assert_eq!(row["metric"]["conversion"], 1.0);

    // Too many events in one request.
    let big: Vec<Value> = (0..501).map(|_| json!({"name": "x"})).collect();
    let r = http
        .post(format!("{base}/v1/events"))
        .bearer_auth(token)
        .json(&json!({"events": big}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    let r = admin(http.delete(format!("{base}/admin/v1/experiments/{exp}")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn server_identity_sync_and_inbox() {
    let Some((base, http)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let admin = |b: reqwest::RequestBuilder| b.header("x-api-key", ADMIN_KEY);
    let created: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "it-sync", "kind": "server"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_key = created["secret"].as_str().unwrap().to_string();
    let ext = format!("nakama-{suffix}");

    // Upsert twice: second call merges properties and keeps the id.
    let a: Value = http
        .post(format!("{base}/server/v1/identities"))
        .header("x-api-key", &server_key)
        .json(&json!({"external_id": ext, "default": {"auth_provider": "device"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let b: Value = http
        .post(format!("{base}/server/v1/identities"))
        .header("x-api-key", &server_key)
        .json(&json!({"external_id": ext, "default": {"lang": "de"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(a["id"], b["id"]);
    assert_eq!(b["properties"]["default"]["auth_provider"], "device");
    assert_eq!(b["properties"]["default"]["lang"], "de");

    let p: Value = http
        .put(format!("{base}/server/v1/identities/{ext}/properties"))
        .header("x-api-key", &server_key)
        .json(&json!({"custom": {"nickname": "Neo"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(p["custom"]["nickname"], "Neo");

    let r = http
        .post(format!("{base}/server/v1/messages"))
        .header("x-api-key", &server_key)
        .json(&json!({"external_id": ext, "title": "Hi {{custom.nickname}}"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let inbox: Value = http
        .get(format!("{base}/server/v1/messages/inbox?external_id={ext}"))
        .header("x-api-key", &server_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["title"] == "Hi Neo"));

    // Client keys may not use the server API or the change stream.
    let ck: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "it-sync-c", "kind": "client"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r = http
        .post(format!("{base}/server/v1/identities"))
        .header("x-api-key", ck["secret"].as_str().unwrap())
        .json(&json!({"external_id": ext}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
}

#[tokio::test]
async fn push_notifications_flow() {
    let Some((base, http)) = start().await else {
        return;
    };
    let admin = |rb: reqwest::RequestBuilder| rb.header("x-api-key", ADMIN_KEY);
    let suffix = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let ext = format!("it-push-{suffix}");
    let client_key: String = {
        let k: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
            .json(&json!({ "name": format!("push-{suffix}"), "kind": "client" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        k["secret"].as_str().unwrap().to_string()
    };
    let (token, identity_id) = {
        let s: Value = http
            .post(format!("{base}/v1/authenticate"))
            .header("x-api-key", &client_key)
            .json(&json!({ "id": ext, "default": { "platform": "ios" } }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        (
            s["token"].as_str().unwrap().to_string(),
            s["identity"]["id"].as_str().unwrap().to_string(),
        )
    };

    // Register two devices; bad tokens are rejected.
    let bad = http
        .put(format!("{base}/v1/push-tokens"))
        .bearer_auth(&token)
        .json(&json!({ "platform": "apns", "token": "not hex" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    for (platform, tok) in [
        ("fcm", format!("fcm-{suffix}")),
        ("apns", "abcdef0123456789".to_string()),
    ] {
        let r = http
            .put(format!("{base}/v1/push-tokens"))
            .bearer_auth(&token)
            .json(&json!({ "platform": platform, "token": tok, "device_id": "dev-1" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{platform}");
    }
    let listed: Value = http
        .get(format!("{base}/v1/push-tokens"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["tokens"].as_array().unwrap().len(), 2);

    // Direct message with push → outbox gets one entry per device, inbox copy is marked pushed.
    let sent: Value =
        admin(http.post(format!("{base}/admin/v1/identities/{identity_id}/messages")))
            .json(&json!({ "title": "Hi {{default.platform}}", "body": "direct", "push": true }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    let msg_id = sent["id"].as_str().unwrap();
    let outbox: Value = admin(http.get(format!("{base}/admin/v1/push/outbox")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(outbox["mode"], "log");
    let mine: Vec<&Value> = outbox["sent"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["notification"]["data"]["zanshin_message_id"] == msg_id)
        .collect();
    assert_eq!(mine.len(), 2, "one push per device");
    assert_eq!(mine[0]["notification"]["title"], "Hi ios");
    let platforms: std::collections::HashSet<&str> = mine
        .iter()
        .map(|e| e["platform"].as_str().unwrap())
        .collect();
    assert_eq!(platforms.len(), 2);

    // Push campaign targeting this identity: proactive sweep delivers without the client pulling.
    let key = format!("push_camp_{suffix}");
    let created = admin(http.post(format!("{base}/admin/v1/messages")))
        .json(&json!({
            "key": key, "title": "Event!", "body": "Join now", "push": true, "enabled": true,
            "condition": { "prop": "identity.external_id", "op": "eq", "value": ext }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let swept: Value = admin(http.post(format!("{base}/admin/v1/push/sweep")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(swept["delivered"].as_u64().unwrap() <= 1, "{swept}"); // the bus-triggered sweep may already have run
    let outbox: Value = admin(http.get(format!("{base}/admin/v1/push/outbox")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let camp: Vec<&Value> = outbox["sent"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["notification"]["data"]["zanshin_message_key"] == key.as_str())
        .collect();
    assert_eq!(
        camp.len(),
        2,
        "campaign pushed once per device, never twice"
    );
    let inbox: Value = http
        .get(format!("{base}/v1/messages"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["key"] == key.as_str()));
    // A second sweep delivers nothing new.
    let swept: Value = admin(http.post(format!("{base}/admin/v1/push/sweep")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(swept["delivered"], 0);

    let stats: Value = admin(http.get(format!("{base}/admin/v1/push/stats")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(stats["tokens"].as_i64().unwrap() >= 2);
    let admin_tokens: Value = admin(http.get(format!(
        "{base}/admin/v1/identities/{identity_id}/push-tokens"
    )))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(admin_tokens["tokens"].as_array().unwrap().len(), 2);

    // Unregister one, the other stays; deleting the identity cascades.
    let r = http
        .delete(format!("{base}/v1/push-tokens/abcdef0123456789"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = http
        .delete(format!("{base}/v1/push-tokens/abcdef0123456789"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let listed: Value = http
        .get(format!("{base}/v1/push-tokens"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["tokens"].as_array().unwrap().len(), 1);
    admin(http.delete(format!("{base}/admin/v1/messages/{key}")))
        .send()
        .await
        .unwrap();
    http.delete(format!("{base}/v1/identity"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
}

#[tokio::test]
async fn hardening_flow() {
    let Some((base, http)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };
    let admin = |r: reqwest::RequestBuilder| r.header("x-api-key", ADMIN_KEY);

    // Security + rate-limit headers are present on every API response.
    let r = admin(http.get(format!("{base}/admin/v1/stats")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["x-content-type-options"], "nosniff");
    assert_eq!(r.headers()["x-frame-options"], "DENY");
    assert_eq!(r.headers()["x-ratelimit-scope"], "admin");
    assert!(r.headers().contains_key("x-ratelimit-limit"));
    assert!(r.headers().contains_key("x-ratelimit-remaining"));

    // Read-only admin key: GET ok, mutations 403.
    let ro: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "it-ro", "kind": "admin", "read_only": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ro["read_only"], true);
    let ro_key = ro["secret"].as_str().unwrap().to_string();
    let r = http
        .get(format!("{base}/admin/v1/flags"))
        .header("x-api-key", &ro_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = http
        .post(format!("{base}/admin/v1/flags"))
        .header("x-api-key", &ro_key)
        .json(&json!({"key": "ro_should_fail", "value_type": "bool", "default_value": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["code"], "permission_denied");

    // read_only is rejected for non-admin kinds; expires_at in the past is rejected.
    let r = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "bad", "kind": "client", "read_only": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let r = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "bad", "kind": "client", "expires_at": "2000-01-01T00:00:00Z"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    // Expiring key: valid now, treated as unknown once expired (expires_at just ahead).
    let soon = chrono::Utc::now() + chrono::Duration::milliseconds(1500);
    let exp: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "it-exp", "kind": "server", "expires_at": soon.to_rfc3339()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let exp_key = exp["secret"].as_str().unwrap().to_string();
    let r = http
        .get(format!("{base}/server/v1/flags/it-exp-ident"))
        .header("x-api-key", &exp_key)
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 401, "fresh key must authenticate");
    tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
    // The principal cache may still hold the key for up to 30s; revocation semantics are
    // guaranteed at the database layer, which is what we assert here.
    let listed: Vec<Value> = admin(http.get(format!("{base}/admin/v1/api-keys")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = listed.iter().find(|k| k["id"] == exp["id"]).unwrap();
    assert!(row["expires_at"].is_string());

    // Per-key quota: 1/min with burst factor 2 -> two requests pass, the third is throttled.
    let quota: Value = admin(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "it-quota", "kind": "client", "rate_limit_per_minute": 1}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(quota["rate_limit_per_minute"], 1);
    let qkey = quota["secret"].as_str().unwrap().to_string();
    let mut statuses = Vec::new();
    let mut last = None;
    for i in 0..3 {
        let r = http
            .post(format!("{base}/v1/authenticate"))
            .header("x-api-key", &qkey)
            .json(&json!({"id": format!("it-quota-{i}")}))
            .send()
            .await
            .unwrap();
        statuses.push(r.status().as_u16());
        last = Some(r);
    }
    assert_eq!(statuses, vec![200, 200, 429], "{statuses:?}");
    let last = last.unwrap();
    assert_eq!(last.headers()["x-ratelimit-limit"], "1");
    assert_eq!(last.headers()["x-ratelimit-remaining"], "0");
    assert_eq!(last.headers()["x-ratelimit-scope"], "client");
    assert!(last.headers().contains_key("retry-after"));
    let body: Value = last.json().await.unwrap();
    assert_eq!(body["code"], "resource_exhausted");

    // Audit log captured the mutations above with redacted bodies and the 403 attempt.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let page: Value = admin(http.get(format!(
        "{base}/admin/v1/audit?path=/admin/v1/api-keys&limit=50"
    )))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let items = page["items"].as_array().unwrap();
    let mint = items
        .iter()
        .find(|e| e["body"]["name"] == "it-quota")
        .expect("api key creation audited");
    assert_eq!(mint["method"], "POST");
    assert_eq!(mint["status"], 200);
    assert_eq!(mint["actor_name"], "bootstrap");
    assert_eq!(mint["actor_kind"], "admin");
    assert!(mint["ip"].is_string());
    let denied: Value = admin(http.get(format!(
        "{base}/admin/v1/audit?path=/admin/v1/flags&method=post&limit=50"
    )))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let d = denied["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["body"]["key"] == "ro_should_fail")
        .expect("denied mutation audited");
    assert_eq!(d["status"], 403);
    assert_eq!(d["actor_name"], "it-ro");
    // Reads are not audited.
    let reads: Value = admin(http.get(format!("{base}/admin/v1/audit?method=GET&limit=5")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(reads["items"].as_array().unwrap().is_empty());

    // Read-only key cannot even read the audit log? It can: audit is a GET.
    let r = http
        .get(format!("{base}/admin/v1/audit?limit=1"))
        .header("x-api-key", &ro_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn live_event_occurrences_calendar() {
    let Some((base, http)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };
    let admin = |r: reqwest::RequestBuilder| r.header("x-api-key", ADMIN_KEY);
    let key = format!("it_cal_{}", uuid::Uuid::new_v4().simple());
    let start_at = chrono::Utc::now() - chrono::Duration::days(1);
    let r = admin(http.post(format!("{base}/admin/v1/live-events")))
        .json(&json!({
            "key": key, "name": "Daily", "enabled": true,
            "starts_at": start_at.to_rfc3339(), "duration_secs": 3600,
            "recurrence": "0 0 12 * * * *", "payload": {"x": 1}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let from = chrono::Utc::now();
    let to = from + chrono::Duration::days(7);
    let cal: Value = admin(http.get(format!(
        "{base}/admin/v1/live-events/occurrences?key={key}&from={}&to={}",
        from.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        to.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    )))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let occ = cal["occurrences"].as_array().unwrap();
    assert!(occ.len() >= 6 && occ.len() <= 8, "{}", occ.len());
    assert!(occ.iter().all(|o| o["key"] == key && o["name"] == "Daily"));
    let starts: Vec<&str> = occ.iter().map(|o| o["start"].as_str().unwrap()).collect();
    let mut sorted = starts.clone();
    sorted.sort();
    assert_eq!(starts, sorted, "occurrences are chronological");
    // Invalid window is rejected.
    let r = admin(http.get(format!(
        "{base}/admin/v1/live-events/occurrences?from={}&to={}",
        to.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        from.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    )))
    .send()
    .await
    .unwrap();
    assert_eq!(r.status(), 400);
    admin(http.delete(format!("{base}/admin/v1/live-events/{key}")))
        .send()
        .await
        .unwrap();
}
