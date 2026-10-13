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
