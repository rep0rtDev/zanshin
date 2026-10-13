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
        .get(format!("{base}/v1/experiments"))
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
        .get(format!("{base}/v1/experiments"))
        .bearer_auth(beta_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let variant = e["experiments"][0]["variant"].as_str().unwrap().to_string();
    let e2: Value = http
        .get(format!("{base}/v1/experiments"))
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
        .get(format!("{base}/v1/experiments"))
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
        .get(format!("{base}/v1/experiments"))
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
