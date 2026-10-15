//! Update check: `GET /admin/v1/updates` reports a newer stable release from a GitHub-shaped
//! endpoint, ignores drafts / pre-releases, and stays silent when `updates.check = false`.

use std::{net::SocketAddr, sync::Arc};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-updates";

async fn fake_releases(body: &'static str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });
    format!("http://{addr}/releases")
}

async fn start(url: &str, check: bool) -> Option<(String, reqwest::Client)> {
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
    std::env::set_var("ZANSHIN__UPDATES__URL", url);
    std::env::set_var(
        "ZANSHIN__UPDATES__CHECK",
        if check { "true" } else { "false" },
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

async fn updates(c: &reqwest::Client, base: &str) -> (u16, Value) {
    let resp = c
        .get(format!("{base}/admin/v1/updates"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .unwrap();
    (
        resp.status().as_u16(),
        resp.json::<Value>().await.unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn update_check_reports_newer_stable_release_and_can_be_disabled() {
    // A list in GitHub shape: a draft, a pre-release and two stable releases out of order.
    let url = fake_releases(
        r#"[
          {"tag_name":"v99.9.9","html_url":"https://example.invalid/draft","draft":true},
          {"tag_name":"v98.0.0-rc.1","html_url":"https://example.invalid/rc","prerelease":true},
          {"tag_name":"v97.1.0","html_url":"https://example.invalid/v97.1.0","published_at":"2030-01-02T00:00:00Z"},
          {"tag_name":"v97.4.0","html_url":"https://example.invalid/v97.4.0","published_at":"2030-02-02T00:00:00Z"}
        ]"#,
    )
    .await;
    let Some((base, c)) = start(&url, true).await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL not set");
        return;
    };

    let (st, u) = updates(&c, &base).await;
    assert_eq!(st, 200, "{u}");
    assert_eq!(u["enabled"], true);
    assert_eq!(u["current"], env!("CARGO_PKG_VERSION"));
    assert_eq!(u["latest"], "97.4.0", "{u}");
    assert_eq!(u["update_available"], true, "{u}");
    assert_eq!(u["url"], "https://example.invalid/v97.4.0");
    assert!(u["checked_at"].is_string());
    assert!(u["error"].is_null(), "{u}");

    // Unauthenticated callers get nothing.
    let st = c
        .get(format!("{base}/admin/v1/updates"))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16();
    assert_eq!(st, 401);

    // Opt-out: no request is made and the answer says so.
    let (base2, c2) = start("http://127.0.0.1:9/never", false).await.unwrap();
    let (st, u) = updates(&c2, &base2).await;
    assert_eq!(st, 200, "{u}");
    assert_eq!(u["enabled"], false);
    assert_eq!(u["update_available"], false);
    assert!(u["checked_at"].is_null(), "{u}");
}
