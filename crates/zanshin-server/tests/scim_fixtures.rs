//! Replays the request shapes real identity providers send (Okta and Microsoft Entra ID SCIM
//! connectors) against `/scim/v2`. Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-scim49";
const SCIM_TOKEN: &str = "scim-fixture-token-49";

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
    std::env::set_var(
        "ZANSHIN__ADMIN__SCIM__GROUP_ARCHETYPES__zanshin-admins",
        "admin",
    );
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

async fn scim(
    c: &reqwest::Client,
    base: &str,
    m: &reqwest::Method,
    p: &str,
    b: Option<Value>,
) -> (u16, Value) {
    let mut r = c
        .request(m.clone(), format!("{base}/scim/v2{p}"))
        .bearer_auth(SCIM_TOKEN)
        .header("accept", "application/scim+json");
    if let Some(b) = b {
        r = r
            .header("content-type", "application/scim+json")
            .body(b.to_string());
    }
    let resp = r.send().await.unwrap();
    (
        resp.status().as_u16(),
        resp.json::<Value>().await.unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn okta_and_entra_request_shapes() {
    let Some((base, c)) = start().await else {
        return;
    };
    let (get, post, put, patch, delete) = (
        reqwest::Method::GET,
        reqwest::Method::POST,
        reqwest::Method::PUT,
        reqwest::Method::PATCH,
        reqwest::Method::DELETE,
    );
    let sfx = uuid::Uuid::now_v7().simple().to_string();

    // ---- Okta: test connection, lookup by userName, create, PATCH active / replace via PUT -----------
    let (st, b) = scim(&c, &base, &get, "/Users?startIndex=1&count=100", None).await;
    assert_eq!(st, 200, "{b}");
    assert_eq!(
        b["schemas"][0],
        "urn:ietf:params:scim:api:messages:2.0:ListResponse"
    );
    let okta_user = format!("okta.{sfx}@example.com");
    let (st, b) = scim(
        &c,
        &base,
        &get,
        &format!("/Users?filter=userName%20eq%20%22{okta_user}%22&startIndex=1&count=100"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(b["totalResults"], 0);
    let (st, u) = scim(&c, &base, &post, "/Users", Some(json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "userName": okta_user, "name": {"givenName": "Okta", "familyName": "Person"},
        "emails": [{"primary": true, "value": okta_user, "type": "work"}],
        "displayName": "Okta Person", "locale": "en-US", "externalId": "00u1abcd", "groups": [], "password": "ignored-by-zanshin", "active": true
    }))).await;
    assert_eq!(st, 201, "{u}");
    let id = u["id"].as_str().unwrap().to_string();
    assert_eq!(u["active"], true);
    // Okta deactivation
    let (st, u) = scim(
        &c,
        &base,
        &patch,
        &format!("/Users/{id}"),
        Some(json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [{"op": "replace", "value": {"active": false}}]
        })),
    )
    .await;
    assert_eq!(st, 200, "{u}");
    assert_eq!(u["active"], false);
    // Okta full PUT (profile push)
    let (st, u) = scim(&c, &base, &put, &format!("/Users/{id}"), Some(json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"], "id": id,
        "userName": okta_user, "name": {"givenName": "Okta", "familyName": "Renamed"},
        "emails": [{"primary": true, "value": okta_user, "type": "work"}], "displayName": "Okta Renamed", "active": true
    }))).await;
    assert_eq!(st, 200, "{u}");
    assert_eq!(u["displayName"], "Okta Renamed");
    assert_eq!(u["active"], true);

    // ---- Entra ID: enterprise extension, PATCH with paths, group membership -----------------------------
    let entra_user = format!("entra.{sfx}@example.com");
    let (st, u) = scim(&c, &base, &post, "/Users", Some(json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User", "urn:ietf:params:scim:schemas:extension:enterprise:2.0:User"],
        "externalId": "7f2a-entra", "userName": entra_user, "active": true,
        "displayName": "Entra Person", "emails": [{"primary": true, "type": "work", "value": entra_user}],
        "meta": {"resourceType": "User"}, "name": {"formatted": "Entra Person", "familyName": "Person", "givenName": "Entra"},
        "urn:ietf:params:scim:schemas:extension:enterprise:2.0:User": {"department": "LiveOps", "manager": {"value": "x"}}
    }))).await;
    assert_eq!(st, 201, "{u}");
    let eid = u["id"].as_str().unwrap().to_string();
    let (st, u) = scim(&c, &base, &patch, &format!("/Users/{eid}"), Some(json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
        "Operations": [
            {"op": "Replace", "path": "displayName", "value": "Entra Renamed"},
            {"op": "Replace", "path": "name.givenName", "value": "E."},
            {"op": "Replace", "path": "active", "value": "False"},
            {"op": "Add", "path": "urn:ietf:params:scim:schemas:extension:enterprise:2.0:User:department", "value": "Ops"}
        ]
    }))).await;
    assert_eq!(st, 200, "{u}");
    assert_eq!(u["active"], false, "{u}");
    assert!(
        u["displayName"].as_str().unwrap().starts_with("E."),
        "last name-op wins: {u}"
    );
    // Entra group provisioning → archetype
    let (st, g) = scim(
        &c,
        &base,
        &get,
        "/Groups?filter=displayName%20eq%20%22zanshin-admins%22",
        None,
    )
    .await;
    assert_eq!(st, 200, "{g}");
    assert_eq!(g["totalResults"], 1, "{g}");
    let (st, g) = scim(
        &c,
        &base,
        &patch,
        "/Groups/zanshin-admins",
        Some(json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [{"op": "Add", "path": "members", "value": [{"value": eid}]}]
        })),
    )
    .await;
    assert_eq!(st, 200, "{g}");
    assert!(
        g["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["value"] == eid),
        "{g}"
    );
    let (st, cu) = c
        .get(format!("{base}/admin/v1/users/{eid}"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .map(|v| (200, v))
        .unwrap();
    assert_eq!(st, 200);
    assert_eq!(cu["archetype"], "admin", "{cu}");
    let (st, g) = scim(
        &c,
        &base,
        &patch,
        "/Groups/zanshin-admins",
        Some(json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [{"op": "Remove", "path": format!("members[value eq \"{eid}\"]")}]
        })),
    )
    .await;
    assert_eq!(st, 200, "{g}");
    assert!(
        !g["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["value"] == eid),
        "removed from group (other admins may exist in the shared db): {g}"
    );
    // Entra uses excludedAttributes on lookups
    let (st, l) = scim(
        &c,
        &base,
        &get,
        &format!("/Users?filter=userName%20eq%20%22{entra_user}%22&excludedAttributes=members"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{l}");
    assert_eq!(l["Resources"][0]["id"], eid);
    for id in [id, eid] {
        let (st, _) = scim(&c, &base, &delete, &format!("/Users/{id}"), None).await;
        assert_eq!(st, 204);
    }
}
