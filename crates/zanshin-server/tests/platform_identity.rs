//! End-to-end tests for SCIM 2.0 provisioning, SAML endpoints and config promotion.
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-plat47";
const SCIM_TOKEN: &str = "scim-secret-token-47";

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
    std::env::set_var("ZANSHIN__ADMIN__SCIM__TOKEN", SCIM_TOKEN);
    std::env::set_var("ZANSHIN__ADMIN__SCIM__DEFAULT_ARCHETYPE", "analyst");
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
    Some((
        format!("http://{addr}"),
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
    ))
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

#[tokio::test]
async fn platform_identity_flow() {
    let Some((base, c)) = start().await else {
        return;
    };
    use reqwest::Method;
    let (get, post, put, patch, delete) = (
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
    );
    let sfx = uuid::Uuid::now_v7().simple().to_string();
    let scim = |m: Method, p: &str, b: Option<Value>| {
        call(&c, SCIM_TOKEN, m, format!("{base}/scim/v2{p}"), b)
    };
    let adm = |m: Method, p: &str, b: Option<Value>| {
        call(&c, ADMIN_KEY, m, format!("{base}/admin/v1{p}"), b)
    };

    // --- SCIM auth ----------------------------------------------------------------------------------
    let (st, _) = call(
        &c,
        "wrong",
        get.clone(),
        format!("{base}/scim/v2/Users"),
        None,
    )
    .await;
    assert_eq!(st, 401);
    let (st, b) = scim(get.clone(), "/ServiceProviderConfig", None).await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["patch"]["supported"], true);
    let (st, b) = scim(get.clone(), "/Schemas", None).await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["totalResults"], 2);

    // --- provision a user ---------------------------------------------------------------------------
    let email = format!("scim_{sfx}@example.com");
    let (st, u) = scim(
        post.clone(),
        "/Users",
        Some(json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": email, "name": { "givenName": "Ada", "familyName": "Lovelace" },
            "emails": [{ "value": email, "primary": true }], "active": true
        })),
    )
    .await;
    assert_eq!(st, 201, "{u}");
    assert_eq!(u["userName"], email);
    assert_eq!(u["name"]["formatted"], "Ada Lovelace");
    assert_eq!(u["active"], true);
    let uid = u["id"].as_str().unwrap().to_string();
    let (st, _) = scim(post.clone(), "/Users", Some(json!({ "userName": email }))).await;
    assert_eq!(st, 409, "duplicate userName");
    // default archetype applied
    let (st, cu) = adm(get.clone(), &format!("/users/{uid}"), None).await;
    assert_eq!(st, 200, "{cu}");
    assert_eq!(cu["archetype"], "analyst", "{cu}");

    // filter + paging
    let (st, l) = scim(
        get.clone(),
        &format!("/Users?filter=userName%20eq%20%22{email}%22"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{l}");
    assert_eq!(l["totalResults"], 1, "{l}");
    assert_eq!(l["Resources"][0]["id"], uid);
    let (st, l) = scim(
        get.clone(),
        "/Users?filter=userName%20eq%20%22nobody@example.com%22",
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(l["totalResults"], 0);
    let (st, _) = scim(get.clone(), "/Users?filter=userName%20co%20%22x%22", None).await;
    assert_eq!(st, 400, "unsupported operator");

    // PATCH active=false → disabled; PATCH displayName
    let (st, u) = scim(
        patch.clone(),
        &format!("/Users/{uid}"),
        Some(json!({ "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                     "Operations": [{ "op": "replace", "path": "active", "value": false }, { "op": "replace", "path": "displayName", "value": "Ada L." }] })),
    )
    .await;
    assert_eq!(st, 200, "{u}");
    assert_eq!(u["active"], false);
    assert_eq!(u["displayName"], "Ada L.");
    let (_, cu) = adm(get.clone(), &format!("/users/{uid}"), None).await;
    assert_eq!(cu["disabled"], true, "{cu}");
    // Azure-style PATCH without path
    let (st, u) = scim(
        patch.clone(),
        &format!("/Users/{uid}"),
        Some(
            json!({ "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                     "Operations": [{ "op": "replace", "value": { "active": true } }] }),
        ),
    )
    .await;
    assert_eq!(st, 200, "{u}");
    assert_eq!(u["active"], true);
    // PUT replace
    let (st, u) = scim(
        put.clone(),
        &format!("/Users/{uid}"),
        Some(json!({ "userName": email, "displayName": "Ada Byron", "active": true })),
    )
    .await;
    assert_eq!(st, 200, "{u}");
    assert_eq!(u["displayName"], "Ada Byron");
    // groups endpoint (no group map configured → empty list, unknown group 404)
    let (st, g) = scim(get.clone(), "/Groups", None).await;
    assert_eq!(st, 200, "{g}");
    assert_eq!(g["totalResults"], 0);
    let (st, _) = scim(get.clone(), "/Groups/admins", None).await;
    assert_eq!(st, 404);
    // deprovision
    let (st, _) = scim(delete.clone(), &format!("/Users/{uid}"), None).await;
    assert_eq!(st, 204);
    let (st, _) = scim(get.clone(), &format!("/Users/{uid}"), None).await;
    assert_eq!(st, 404);

    // --- SAML (not configured here) -----------------------------------------------------------------
    let cfg: Value = c
        .get(format!("{base}/console/auth/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cfg["saml"], false, "{cfg}");
    let r = c
        .get(format!("{base}/console/auth/saml/login"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        503,
        "saml login unavailable when not configured"
    );
    let r = c
        .get(format!("{base}/console/auth/saml/metadata"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 503);
    let r = c
        .post(format!("{base}/console/auth/saml/acs"))
        .form(&[("SAMLResponse", "bm90LXhtbA==")])
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        303,
        "acs always redirects back to the console"
    );
    assert!(r.headers()["location"].to_str().unwrap().contains("error="));
    let (st, sys) = adm(get.clone(), "/system", None).await;
    assert_eq!(st, 200);
    assert_eq!(sys["auth"]["saml"], false);
    assert_eq!(sys["auth"]["scim"], true, "{sys}");

    // --- promote (to ourselves, dry run) --------------------------------------------------------------
    let fk = format!("promo_{sfx}");
    let (st, _) = adm(
        post.clone(),
        "/flags",
        Some(json!({ "key": fk, "value_type": "bool", "default_value": true })),
    )
    .await;
    assert_eq!(st, 201);
    let (st, p) = adm(
        post.clone(),
        "/config/promote",
        Some(json!({ "url": base, "admin_key": ADMIN_KEY, "kinds": ["flags"], "keys": [fk], "dry_run": true })),
    )
    .await;
    assert_eq!(st, 200, "{p}");
    assert_eq!(p["dry_run"], true);
    assert_eq!(p["plan"]["kinds"]["flags"]["unchanged"], 1, "{p}");
    assert_eq!(p["plan"]["summary"]["added"], 0, "{p}");
    let (st, _) = adm(
        post.clone(),
        "/config/promote",
        Some(json!({ "url": "ftp://x", "admin_key": "k" })),
    )
    .await;
    assert_eq!(st, 400);
    let (st, _) = adm(delete.clone(), &format!("/flags/{fk}"), None).await;
    assert!(st == 204 || st == 200);
}
