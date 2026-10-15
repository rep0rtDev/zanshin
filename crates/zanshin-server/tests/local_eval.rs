//! Rules bundle for SDK local evaluation (`GET /server/v1/rules`).
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-local48";

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
    Some((format!("http://{addr}"), reqwest::Client::new()))
}

#[tokio::test]
async fn rules_bundle_for_local_evaluation() {
    let Some((base, c)) = start().await else {
        return;
    };
    let sfx = uuid::Uuid::now_v7().simple().to_string();
    let adm = |r: reqwest::RequestBuilder| r.bearer_auth(ADMIN_KEY);
    let created: Value = adm(c.post(format!("{base}/admin/v1/api-keys")))
        .json(&json!({"name": format!("le-{sfx}"), "kind": "server"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_key = created["secret"].as_str().unwrap().to_string();

    let dyn_aud = format!("le_dyn_{sfx}");
    let man_aud = format!("le_man_{sfx}");
    let r = adm(c.post(format!("{base}/admin/v1/audiences")))
        .json(&json!({"key": dyn_aud, "name": "dyn", "condition": {"prop": "custom.level", "op": "gte", "value": 10}, "percentage": 50}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let r = adm(c.post(format!("{base}/admin/v1/audiences")))
        .json(&json!({"key": man_aud, "name": "man", "kind": "managed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let flag = format!("le_flag_{sfx}");
    let r = adm(c.post(format!("{base}/admin/v1/flags")))
        .json(&json!({"key": flag, "value_type": "bool", "default_value": false,
            "rules": [{"id": "r1", "condition": {"all": []}, "audiences": [dyn_aud], "percentage": 100,
                       "variants": [{"key": "a", "value": true, "weight": 1}, {"key": "b", "value": false, "weight": 3}]}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());

    // client keys / no key are rejected
    let r = c
        .get(format!("{base}/server/v1/rules"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = c
        .get(format!("{base}/server/v1/rules"))
        .header("x-api-key", &server_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let b: Value = r.json().await.unwrap();
    assert!(b["generated_at"].is_string());
    let f = b["flags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["key"] == flag)
        .expect("flag in bundle");
    assert_eq!(f["rules"][0]["variants"][1]["weight"], 3.0);
    assert!(f["salt"].is_string(), "{f}");
    assert_eq!(f["enabled"], true);
    let a = b["audiences"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["key"] == dyn_aud)
        .expect("dynamic audience in bundle");
    assert_eq!(a["percentage"], 50.0);
    assert_eq!(a["condition"]["prop"], "custom.level");
    assert!(a["salt"].is_string() && a["enabled"].is_boolean());
    assert!(
        !b["audiences"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["key"] == man_aud),
        "managed audiences carry no rules"
    );
    assert!(b["managed_audiences"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| *k == json!(man_aud)));

    // the bundle must reproduce the server's verdict for a real identity
    let ext = format!("le_user_{sfx}");
    let r = c
        .post(format!("{base}/server/v1/identities"))
        .header("x-api-key", &server_key)
        .json(&json!({"external_id": ext, "custom": {"level": 42}}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.text().await.unwrap());
    let server: Value = c
        .get(format!(
            "{base}/server/v1/flags?external_id={ext}&names={flag}"
        ))
        .header("x-api-key", &server_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sf = &server["flags"][0];
    // replicate the SDK algorithm in the test with the core crate
    let in_aud = zanshin_core::hash::in_rollout(
        &format!("audience:{dyn_aud}"),
        a["salt"].as_str().unwrap(),
        &ext,
        50.0,
    );
    let expected = if in_aud {
        let i = zanshin_core::hash::weighted_pick(
            &format!("flag:{flag}"),
            &format!("{}:r1:variant", f["salt"].as_str().unwrap()),
            &ext,
            &[1.0, 3.0],
        )
        .unwrap();
        (
            json!(i == 0),
            Some(if i == 0 { "a" } else { "b" }),
            "variant",
        )
    } else {
        (json!(false), None, "default")
    };
    assert_eq!(sf["value"], expected.0, "{sf}");
    assert_eq!(sf["variant"].as_str(), expected.1, "{sf}");
    assert_eq!(sf["reason"]["kind"], expected.2, "{sf}");
}
