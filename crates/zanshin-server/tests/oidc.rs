//! Console single sign-on against a mock OpenID Connect provider, plus role enforcement.
//! Runs only when `ZANSHIN_TEST_DATABASE_URL` / `ZANSHIN_TEST_REDIS_URL` are set.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{extract::State, routing::get, Form, Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "zk_a_oidc_bootstrap_key_for_tests";
const CLIENT_ID: &str = "zanshin-console";
const PEM: &str = include_str!("fixtures/oidc_rs256.pem");
const JWKS: &str = include_str!("fixtures/oidc_jwks.json");

#[derive(Default)]
struct Mock {
    issuer: String,
    nonce: Option<String>,
    challenge: Option<String>,
    groups: Vec<String>,
    email: String,
}

type Shared = Arc<Mutex<Mock>>;

async fn discovery(State(m): State<Shared>) -> Json<Value> {
    let iss = m.lock().unwrap().issuer.clone();
    Json(json!({
        "issuer": iss,
        "authorization_endpoint": format!("{iss}/authorize"),
        "token_endpoint": format!("{iss}/token"),
        "jwks_uri": format!("{iss}/jwks"),
    }))
}

async fn jwks() -> Json<Value> {
    Json(serde_json::from_str(JWKS).unwrap())
}

async fn token(
    State(m): State<Shared>,
    Form(f): Form<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let m = m.lock().unwrap();
    if f.get("code").map(String::as_str) != Some("code-1") {
        return Err((axum::http::StatusCode::BAD_REQUEST, "bad code".into()));
    }
    let verifier = f.get("code_verifier").cloned().unwrap_or_default();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    if Some(&challenge) != m.challenge.as_ref() {
        return Err((axum::http::StatusCode::BAD_REQUEST, "pkce mismatch".into()));
    }
    let now = chrono::Utc::now().timestamp();
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test-key-1".into());
    let id_token = jsonwebtoken::encode(
        &header,
        &json!({
            "iss": m.issuer, "aud": CLIENT_ID, "sub": "user-1", "email": m.email,
            "name": "Kai Tester", "nonce": m.nonce, "groups": m.groups,
            "iat": now, "exp": now + 300,
        }),
        &jsonwebtoken::EncodingKey::from_rsa_pem(PEM.as_bytes()).unwrap(),
    )
    .unwrap();
    Ok(Json(
        json!({"id_token": id_token, "access_token": "x", "token_type": "Bearer"}),
    ))
}

async fn start_mock() -> (String, Shared) {
    let shared: Shared = Arc::new(Mutex::new(Mock::default()));
    let app = Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/jwks", get(jwks))
        .route("/token", axum::routing::post(token))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    shared.lock().unwrap().issuer = issuer.clone();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (issuer, shared)
}

async fn start(issuer: &str) -> Option<(String, reqwest::Client)> {
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
    let dir = std::env::temp_dir().join(format!("zanshin-oidc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("zanshin.yml");
    std::fs::write(
        &path,
        format!(
            "admin:\n  oidc:\n    issuer: {issuer}\n    client_id: {CLIENT_ID}\n    client_secret: shh\n    redirect_url: http://console.test/console/auth/callback\n    role_map:\n      liveops-admins: admin\n      liveops: editor\n      analysts: viewer\n    allowed_domains: [example.com]\n"
        ),
    )
    .unwrap();
    let cfg = Config::load(Some(&path)).expect("config");
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
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    Some((format!("http://{addr}"), http))
}

fn cookie_of(r: &reqwest::Response, name: &str) -> Option<String> {
    r.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| {
            let (k, v) = c.split_once(';')?.0.split_once('=')?;
            (k == name && !v.is_empty()).then(|| v.to_string())
        })
}

fn query(url: &str, key: &str) -> String {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| panic!("missing {key} in {url}"))
}

/// Full login dance for a user with the given groups/email; returns the console session cookie.
async fn login(
    base: &str,
    http: &reqwest::Client,
    mock: &Shared,
    groups: &[&str],
    email: &str,
) -> reqwest::Response {
    let r = http
        .get(format!("{base}/console/auth/login"))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_redirection(),
        "login redirect: {}",
        r.status()
    );
    let loc = r.headers()["location"].to_str().unwrap().to_string();
    assert!(
        loc.starts_with(&mock.lock().unwrap().issuer),
        "redirects to provider: {loc}"
    );
    assert_eq!(query(&loc, "code_challenge_method"), "S256");
    assert_eq!(query(&loc, "client_id"), CLIENT_ID);
    let state_cookie = cookie_of(&r, "zanshin_oidc_state").expect("state cookie");
    {
        let mut m = mock.lock().unwrap();
        m.nonce = Some(query(&loc, "nonce"));
        m.challenge = Some(query(&loc, "code_challenge"));
        m.groups = groups.iter().map(|s| s.to_string()).collect();
        m.email = email.into();
    }
    http.get(format!(
        "{base}/console/auth/callback?code=code-1&state={}",
        query(&loc, "state")
    ))
    .header("cookie", format!("zanshin_oidc_state={state_cookie}"))
    .send()
    .await
    .unwrap()
}

#[tokio::test]
async fn oidc_login_and_roles() {
    let (issuer, mock) = start_mock().await;
    let Some((base, http)) = start(&issuer).await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };

    // Console advertises SSO.
    let cfg: Value = http
        .get(format!("{base}/console/auth/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cfg["oidc"], true);

    // Editor: full login dance, session cookie, can mutate catalog but not keys.
    let r = login(
        &base,
        &http,
        &mock,
        &["liveops", "random"],
        "kai@example.com",
    )
    .await;
    assert!(r.status().is_redirection());
    assert_eq!(r.headers()["location"], "/console/");
    let session = cookie_of(&r, "zanshin_console").expect("session cookie");
    let with =
        |r: reqwest::RequestBuilder| r.header("cookie", format!("zanshin_console={session}"));

    let me: Value = with(http.get(format!("{base}/admin/v1/me")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["auth"], "oidc");
    assert_eq!(me["role"], "editor");
    assert_eq!(me["name"], "kai@example.com");

    let key = format!("oidc_e2e_{}", uuid::Uuid::now_v7().simple());
    let r = with(http.post(format!("{base}/admin/v1/flags")))
        .json(&json!({"key": key, "value_type": "bool", "default_value": false}))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "editor creates flag: {}",
        r.status()
    );
    let r = with(http.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": "nope", "kind": "client"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "editor cannot manage keys");
    let r = with(http.get(format!("{base}/admin/v1/api-keys")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "editor may list keys");

    // Audit attributes the mutation to the OIDC user.
    let audit: Value = http
        .get(format!(
            "{base}/admin/v1/audit?path=/admin/v1/flags&method=post&limit=50"
        ))
        .header("x-api-key", ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let text = audit.to_string();
    assert!(
        text.contains("kai@example.com") && text.contains("\"oidc\""),
        "audit: {text}"
    );

    // Viewer: GET ok, mutations 403.
    let r = login(&base, &http, &mock, &["analysts"], "ana@example.com").await;
    let viewer = cookie_of(&r, "zanshin_console").expect("viewer session");
    let r = http
        .get(format!("{base}/admin/v1/flags"))
        .header("cookie", format!("zanshin_console={viewer}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = http
        .post(format!("{base}/admin/v1/flags"))
        .header("cookie", format!("zanshin_console={viewer}"))
        .json(&json!({"key": format!("{key}_v"), "value_type": "bool", "default_value": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    // No matching group and no default role -> rejected; wrong domain -> rejected.
    let r = login(&base, &http, &mock, &["guests"], "x@example.com").await;
    assert!(r.headers()["location"].to_str().unwrap().contains("error="));
    assert!(cookie_of(&r, "zanshin_console").is_none());
    let r = login(&base, &http, &mock, &["liveops"], "x@evil.com").await;
    assert!(r.headers()["location"].to_str().unwrap().contains("error="));

    // Replayed / mismatched state is rejected.
    let r = http
        .get(format!(
            "{base}/console/auth/callback?code=code-1&state=bogus"
        ))
        .send()
        .await
        .unwrap();
    assert!(r.headers()["location"].to_str().unwrap().contains("error="));

    // Logout clears the cookie; a forged cookie is ignored.
    let r = http
        .post(format!("{base}/console/auth/logout"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let r = http
        .get(format!("{base}/admin/v1/me"))
        .header("cookie", "zanshin_console=forged.token.value")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // API-key roles: an editor key cannot manage keys, a viewer key cannot write.
    let ed: Value = http
        .post(format!("{base}/admin/v1/api-keys"))
        .header("x-api-key", ADMIN_KEY)
        .json(&json!({"name": "it-editor", "kind": "admin", "role": "editor"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ed["role"], "editor");
    let r = http
        .post(format!("{base}/admin/v1/api-keys"))
        .header("x-api-key", ed["secret"].as_str().unwrap())
        .json(&json!({"name": "nope", "kind": "client"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = http
        .post(format!("{base}/admin/v1/api-keys"))
        .header("x-api-key", ADMIN_KEY)
        .json(&json!({"name": "bad", "kind": "client", "role": "editor"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400, "role only applies to admin keys");
}
