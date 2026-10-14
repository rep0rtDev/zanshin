//! Console users: password + TOTP login, server-side sessions, permission enforcement with
//! `permission required: <resource>:<action>` hints, archetypes, API-key permissions and rotation.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, services::console_users::totp_code, state::AppState};

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
    tokio::spawn(async move {
        axum::serve(
            listener,
            http::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    Some((format!("http://{addr}"), client))
}

#[tokio::test]
async fn users_permissions_mfa_and_key_rotation() {
    let Some((base, http)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };
    let admin = |m: reqwest::Method, p: &str| {
        http.request(m, format!("{base}{p}"))
            .header("x-api-key", ADMIN_KEY)
    };
    let suffix = uuid::Uuid::now_v7().simple().to_string();
    let email = format!("designer-{suffix}@example.com");
    let password = "correct horse battery staple 42";

    // Permission catalog exposes archetypes.
    let catalog: Value = admin(reqwest::Method::GET, "/admin/v1/permissions")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(catalog["archetypes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["name"] == "game_designer"));

    // Weak passwords are rejected; a game designer is created with an initial password.
    let r = admin(reqwest::Method::POST, "/admin/v1/users")
        .json(&json!({"email": email, "name": "Dee Signer", "password": "short", "archetype": "game_designer"}))
        .send().await.unwrap();
    assert_eq!(r.status(), 400);
    let r = admin(reqwest::Method::POST, "/admin/v1/users")
        .json(&json!({"email": email, "name": "Dee Signer", "password": password, "archetype": "game_designer"}))
        .send().await.unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let user: Value = r.json().await.unwrap();
    let user_id = user["id"].as_str().unwrap().to_string();
    assert_eq!(user["archetype"], "game_designer");
    assert!(
        user.get("password_hash").is_none(),
        "hash never leaves the server"
    );

    // Password login sets a cookie; /me reflects the archetype's permissions.
    let r = http
        .post(format!("{base}/console/auth/password"))
        .json(&json!({"email": email, "password": "wrong password here"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = http
        .post(format!("{base}/console/auth/password"))
        .json(&json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let me: Value = http
        .get(format!("{base}/admin/v1/me"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["auth"], "user");
    assert_eq!(me["user_id"], user_id);
    assert!(me["permissions"]["flags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a == "write"));
    assert_eq!(me["must_change_password"], true);

    // Designers edit flags but may not manage API keys or users: 403 carries the needed grant.
    let r = http
        .post(format!("{base}/admin/v1/flags"))
        .json(&json!({"key": format!("cu-{suffix}"), "value_type": "bool", "default_value": false}))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "designer can write flags: {}",
        r.text().await.unwrap()
    );
    let r = http
        .post(format!("{base}/admin/v1/api-keys"))
        .json(&json!({"name": "nope", "kind": "client"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let body: Value = r.json().await.unwrap();
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("permission required: api_keys:write"),
        "{body}"
    );
    let r = http
        .get(format!("{base}/admin/v1/api-keys"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "read access to keys is part of the preset");
    let r = http
        .post(format!("{base}/admin/v1/users"))
        .json(&json!({"email": "x@y.z", "name": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    // Password change invalidates the session; logging in again works with the new password.
    let r = http
        .post(format!("{base}/admin/v1/me/password"))
        .json(&json!({"current_password": password, "new_password": "another long passphrase 99"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204, "{}", r.text().await.unwrap());
    let r = http
        .get(format!("{base}/admin/v1/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "old session revoked after password change");
    let password = "another long passphrase 99";
    let r = http
        .post(format!("{base}/console/auth/password"))
        .json(&json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let me: Value = http
        .get(format!("{base}/admin/v1/me"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["must_change_password"], false);

    // MFA: setup -> enable with a valid code -> recovery codes; login then needs a code.
    let setup: Value = http
        .post(format!("{base}/admin/v1/me/mfa/setup"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = setup["secret"].as_str().unwrap().to_string();
    assert!(setup["otpauth_url"]
        .as_str()
        .unwrap()
        .starts_with("otpauth://totp/"));
    let r = http
        .post(format!("{base}/admin/v1/me/mfa/enable"))
        .json(&json!({"code": "000000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let now = chrono::Utc::now().timestamp();
    let code = totp_code(&secret, now).unwrap();
    let enabled: Value = http
        .post(format!("{base}/admin/v1/me/mfa/enable"))
        .json(&json!({"code": code}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let recovery = enabled["recovery_codes"].as_array().unwrap();
    assert_eq!(recovery.len(), 10);
    let me: Value = http
        .get(format!("{base}/admin/v1/me"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["mfa_enabled"], true);

    let fresh = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let r = fresh
        .post(format!("{base}/console/auth/password"))
        .json(&json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["code"], "mfa_required");
    let code = totp_code(&secret, chrono::Utc::now().timestamp()).unwrap();
    let r = fresh
        .post(format!("{base}/console/auth/password"))
        .json(&json!({"email": email, "password": password, "code": code}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    // A recovery code works once.
    let rc = recovery[0].as_str().unwrap();
    let again = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let r = again
        .post(format!("{base}/console/auth/password"))
        .json(&json!({"email": email, "password": password, "code": rc}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = reqwest::Client::new()
        .post(format!("{base}/console/auth/password"))
        .json(&json!({"email": email, "password": password, "code": rc}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "recovery code is single-use");

    // Sessions are listable and revocable; logout clears the cookie session.
    let sessions: Value = fresh
        .get(format!("{base}/admin/v1/me/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(sessions.as_array().unwrap().len() >= 2);
    let r = fresh
        .post(format!("{base}/console/auth/logout"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let r = fresh
        .get(format!("{base}/admin/v1/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // Admin: disable user -> login refused; reset link -> new password works; delete.
    let r = admin(
        reqwest::Method::PATCH,
        &format!("/admin/v1/users/{user_id}"),
    )
    .json(&json!({"disabled": true}))
    .send()
    .await
    .unwrap();
    assert_eq!(r.status(), 200);
    let r = reqwest::Client::new().post(format!("{base}/console/auth/password"))
        .json(&json!({"email": email, "password": password, "code": totp_code(&secret, chrono::Utc::now().timestamp()).unwrap()}))
        .send().await.unwrap();
    assert_eq!(r.status(), 401);
    admin(
        reqwest::Method::PATCH,
        &format!("/admin/v1/users/{user_id}"),
    )
    .json(&json!({"disabled": false, "archetype": "viewer"}))
    .send()
    .await
    .unwrap();
    admin(
        reqwest::Method::DELETE,
        &format!("/admin/v1/users/{user_id}/mfa"),
    )
    .send()
    .await
    .unwrap();
    let link: Value = admin(
        reqwest::Method::POST,
        &format!("/admin/v1/users/{user_id}/reset-link"),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let token = link["reset_token"].as_str().unwrap();
    let r = reqwest::Client::new()
        .post(format!("{base}/console/auth/reset"))
        .json(&json!({"token": token, "password": "reset passphrase number 7"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let viewer = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let r = viewer
        .post(format!("{base}/console/auth/password"))
        .json(&json!({"email": email, "password": "reset passphrase number 7"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "mfa was disabled by the admin, password reset applied"
    );
    let r = viewer
        .post(format!("{base}/admin/v1/flags"))
        .json(&json!({"key": "nope", "value_type": "bool", "default_value": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "viewer preset is read-only");
    let r = admin(
        reqwest::Method::DELETE,
        &format!("/admin/v1/users/{user_id}"),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(r.status(), 204);

    // API keys: explicit permissions and rotation with a grace period.
    let r = admin(reqwest::Method::POST, "/admin/v1/api-keys")
        .json(&json!({"name": format!("cu-key-{suffix}"), "kind": "admin", "permissions": {"flags": ["read", "write"], "*": ["read"]}}))
        .send().await.unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let created: Value = r.json().await.unwrap();
    let key_id = created["id"].as_str().unwrap().to_string();
    let secret1 = created["secret"].as_str().unwrap().to_string();
    let r = http
        .get(format!("{base}/admin/v1/audiences"))
        .header("x-api-key", &secret1)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = http
        .post(format!("{base}/admin/v1/audiences"))
        .header("x-api-key", &secret1)
        .json(&json!({"key": "nope", "name": "n"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let rotated: Value = admin(
        reqwest::Method::POST,
        &format!("/admin/v1/api-keys/{key_id}/rotate"),
    )
    .json(&json!({"grace_hours": 1}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let secret2 = rotated["secret"].as_str().unwrap();
    assert_eq!(rotated["rotated_from"], key_id);
    assert_eq!(
        rotated["permissions"]["flags"][1], "write",
        "permissions carried over"
    );
    for s in [secret1.as_str(), secret2] {
        let r = http
            .get(format!("{base}/admin/v1/flags"))
            .header("x-api-key", s)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "both keys work during the grace period");
    }
    let keys: Value = admin(reqwest::Method::GET, "/admin/v1/api-keys")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let old = keys
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["id"] == key_id)
        .unwrap();
    assert!(old["expires_at"].as_str().is_some(), "old key now expires");
    let r = admin(
        reqwest::Method::POST,
        &format!("/admin/v1/api-keys/{key_id}/rotate"),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(r.status(), 409, "a key rotates once");
}
