//! Multi-project: isolated configuration / players per project, routing by header, query and
//! the `prj` token claim, archive / resume. Run only with ZANSHIN_TEST_DATABASE_URL / _REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, services::projects::Projects};

#[test]
fn projects_isolation_and_routing() {
    // Two full AppStates + routers make a deep future tree in debug builds; give it a big stack.
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(64 << 20)
                .build()
                .unwrap()
                .block_on(run())
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn run() {
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
        "test-bootstrap-admin-key-projects",
    );
    std::env::set_var("ZANSHIN__SERVER__HTTP_ADDR", "127.0.0.1:0");
    std::env::set_var("ZANSHIN__PUSH__PROVIDER", "log");
    let cfg = Config::load(None).expect("config");
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let projects = Projects::start(Arc::new(cfg), rx).await.expect("projects");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let app = projects.clone().router();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let base = format!("http://{addr}");
    let c = reqwest::Client::new();
    let admin = "test-bootstrap-admin-key-projects";
    let sfx = uuid::Uuid::new_v4().simple().to_string();
    let slug = format!("t{}", &sfx[..10]);

    // registry on the default project
    let r = c
        .post(format!("{base}/admin/v1/projects"))
        .bearer_auth(admin)
        .json(&json!({"slug": slug, "name": "Test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let bad = c
        .post(format!("{base}/admin/v1/projects"))
        .bearer_auth(admin)
        .json(&json!({"slug": "Bad-Slug"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    let list: Value = c
        .get(format!("{base}/admin/v1/projects"))
        .bearer_auth(admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["slug"] == slug && p["running"] == true));
    assert_eq!(list["current"], "default");

    // the new project has its own (empty) config and keys; the bootstrap key works there too
    let fkey = format!("f_{sfx}");
    let r = c
        .post(format!("{base}/admin/v1/flags"))
        .bearer_auth(admin)
        .header("x-zanshin-project", &slug)
        .json(&json!({"key": fkey, "value_type": "bool", "default_value": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let in_default = c
        .get(format!("{base}/admin/v1/flags/{fkey}"))
        .bearer_auth(admin)
        .send()
        .await
        .unwrap();
    assert_eq!(in_default.status(), 404, "flag must not leak into default");
    let via_query = c
        .get(format!("{base}/admin/v1/flags/{fkey}?project={slug}"))
        .bearer_auth(admin)
        .send()
        .await
        .unwrap();
    assert_eq!(via_query.status(), 200);

    // a client key created in the project authenticates players of that project only
    let k: Value = c
        .post(format!("{base}/admin/v1/api-keys"))
        .bearer_auth(admin)
        .header("x-zanshin-project", &slug)
        .json(&json!({"name": "client", "kind": "client"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ck = k["secret"].as_str().expect("client key").to_string();
    let s: Value = c
        .post(format!("{base}/v1/authenticate"))
        .header("x-api-key", &ck)
        .header("x-zanshin-project", &slug)
        .json(&json!({"id": format!("p_{sfx}")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tok = s["token"].as_str().expect("token").to_string();
    assert_eq!(
        zanshin_core::token::peek_project(&tok).as_deref(),
        Some(slug.as_str())
    );
    // bare token (no header) is routed by its `prj` claim
    let flags: Value = c
        .get(format!("{base}/v1/flags"))
        .bearer_auth(&tok)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(flags["flags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["key"] == fkey));
    // the same client key is unknown to the default project
    let wrong = c
        .post(format!("{base}/v1/authenticate"))
        .header("x-api-key", &ck)
        .json(&json!({"id": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);
    let unknown = c
        .get(format!("{base}/v1/healthcheck"))
        .header("x-zanshin-project", "nope")
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);

    // archive stops routing, resume brings it back with the data intact
    let r = c
        .put(format!("{base}/admin/v1/projects/{slug}"))
        .bearer_auth(admin)
        .json(&json!({"archived": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let gone = c
        .get(format!("{base}/admin/v1/flags/{fkey}"))
        .bearer_auth(admin)
        .header("x-zanshin-project", &slug)
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 404);
    let r = c
        .put(format!("{base}/admin/v1/projects/{slug}"))
        .bearer_auth(admin)
        .json(&json!({"archived": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let back = c
        .get(format!("{base}/admin/v1/flags/{fkey}"))
        .bearer_auth(admin)
        .header("x-zanshin-project", &slug)
        .send()
        .await
        .unwrap();
    assert_eq!(back.status(), 200);
    let nope = c
        .put(format!("{base}/admin/v1/projects/default"))
        .bearer_auth(admin)
        .json(&json!({"archived": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(nope.status(), 400);

    // cleanup: archive + drop the schema
    let _ = c
        .put(format!("{base}/admin/v1/projects/{slug}"))
        .bearer_auth(admin)
        .json(&json!({"archived": true}))
        .send()
        .await;
    let pg = &projects.default_state().stores.pg;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS \"zanshin_{slug}\" CASCADE"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("DELETE FROM projects WHERE slug = $1")
        .bind(&slug)
        .execute(pg)
        .await
        .unwrap();
}
