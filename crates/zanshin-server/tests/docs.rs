//! `/docs`, `/openapi.json`, `/openapi.yaml` are served without auth and describe the client API.
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::Value;
use zanshin_server::{config::Config, http, state::AppState};

#[tokio::test]
async fn openapi_and_swagger_ui() {
    let (Ok(db), Ok(redis)) = (
        std::env::var("ZANSHIN_TEST_DATABASE_URL"),
        std::env::var("ZANSHIN_TEST_REDIS_URL"),
    ) else {
        return;
    };
    std::env::set_var("ZANSHIN__DATABASE__URL", db);
    std::env::set_var("ZANSHIN__REDIS__URL", redis);
    std::env::set_var(
        "ZANSHIN__AUTH__JWT_SECRET",
        "test-secret-test-secret-test-secret-0123",
    );
    std::env::set_var(
        "ZANSHIN__ADMIN__BOOTSTRAP_KEY",
        "test-bootstrap-admin-key-docs",
    );
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
    let c = reqwest::Client::new();
    let res = c
        .get(format!("http://{addr}/openapi.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let spec: Value = res.json().await.unwrap();
    assert_eq!(spec["openapi"], "3.0.3");
    assert!(spec["paths"]["/v1/flags"]["get"].is_object());
    assert!(spec["paths"]["/server/v1/flags/{key}/explain"]["get"].is_object());
    assert!(spec["paths"]["/admin/v1/notification-channels"]["post"].is_object());
    assert!(spec["components"]["schemas"]["Flag"].is_object());
    let yaml = c
        .get(format!("http://{addr}/openapi.yaml"))
        .send()
        .await
        .unwrap();
    assert_eq!(yaml.status(), 200);
    assert!(yaml.text().await.unwrap().starts_with("# Generated"));
    let ui = c.get(format!("http://{addr}/docs")).send().await.unwrap();
    assert_eq!(ui.status(), 200);
    let html = ui.text().await.unwrap();
    assert!(html.contains("swagger-ui") && html.contains("/openapi.json"));
    // assets are vendored: nothing is loaded from a CDN
    assert!(!html.contains("unpkg") && !html.contains("cdn."));
    for (path, needle) in [
        ("/docs/swagger-ui-bundle.js", "SwaggerUIBundle"),
        ("/docs/swagger-ui.css", ".swagger-ui"),
    ] {
        let r = c.get(format!("http://{addr}{path}")).send().await.unwrap();
        assert_eq!(r.status(), 200, "{path}");
        assert!(r.text().await.unwrap().contains(needle), "{path}");
    }
}
