//! Secrets at rest: integration / sink / webhook / channel secrets are sealed in Postgres under
//! `security.data_keys`, API responses stay masked, rotation rewraps, legacy plaintext still opens.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use sqlx::Row;
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-secrets";
const K1: &str = "k1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const K2: &str = "k2:qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqo=";

async fn start() -> Option<(String, reqwest::Client, sqlx::PgPool)> {
    let base_db = std::env::var("ZANSHIN_TEST_DATABASE_URL").ok()?;
    let redis = std::env::var("ZANSHIN_TEST_REDIS_URL").ok()?;
    // Rewrap touches every row in the schema, so this test gets a scratch database of its own
    // instead of leaving k2-sealed rows behind for tests that run without a keyring.
    let (prefix, name) = base_db.rsplit_once('/').expect("database name in url");
    let name = name.split('?').next().unwrap();
    let scratch = format!("{name}_secrets");
    let db = format!("{prefix}/{scratch}");
    {
        let admin = sqlx::PgPool::connect(&base_db).await.unwrap();
        sqlx::query(&format!("DROP DATABASE IF EXISTS {scratch} WITH (FORCE)"))
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query(&format!("CREATE DATABASE {scratch}"))
            .execute(&admin)
            .await
            .unwrap();
    }
    std::env::set_var("ZANSHIN__DATABASE__URL", &db);
    std::env::set_var("ZANSHIN__REDIS__URL", redis);
    std::env::set_var(
        "ZANSHIN__AUTH__JWT_SECRET",
        "test-secret-test-secret-test-secret-0123",
    );
    std::env::set_var("ZANSHIN__ADMIN__BOOTSTRAP_KEY", ADMIN_KEY);
    std::env::set_var("ZANSHIN__SERVER__HTTP_ADDR", "127.0.0.1:0");
    std::env::set_var("ZANSHIN__PUSH__PROVIDER", "log");
    std::env::set_var("ZANSHIN__SECURITY__DATA_KEYS", K1);
    let cfg = Config::load(None).expect("config");
    zanshin_store::Stores::migrate_only(&cfg.database)
        .await
        .expect("migrate scratch db");
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
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    Some((format!("http://{addr}"), reqwest::Client::new(), pool))
}

async fn adm(
    c: &reqwest::Client,
    base: &str,
    m: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut r = c.request(m, format!("{base}{path}")).bearer_auth(ADMIN_KEY);
    if let Some(b) = body {
        r = r.json(&b);
    }
    let resp = r.send().await.unwrap();
    (
        resp.status().as_u16(),
        resp.json::<Value>().await.unwrap_or(Value::Null),
    )
}

async fn raw(pool: &sqlx::PgPool, sql: &str, key: &str) -> String {
    sqlx::query(sql)
        .bind(key)
        .fetch_one(pool)
        .await
        .unwrap()
        .get::<String, _>(0)
}

#[tokio::test]
async fn secrets_are_sealed_at_rest_masked_in_api_and_rewrapped_on_rotation() {
    let Some((base, c, pool)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let suffix = &suffix[..8];
    let ikey = format!("sec_int_{suffix}");
    let skey = format!("sec_sink_{suffix}");
    let wkey = format!("sec_wh_{suffix}");
    let ckey = format!("sec_ch_{suffix}");

    let (st, i) = adm(&c, &base, reqwest::Method::POST, "/admin/v1/integrations", Some(json!({
        "key": ikey, "kind": "webhook", "config": {"url": "http://127.0.0.1:9/hook", "secret": "int-s3cret"}
    }))).await;
    assert_eq!(st, 201, "{i}");
    assert_eq!(i["config"]["secret"], "••••");
    let (st, s) = adm(&c, &base, reqwest::Method::POST, "/admin/v1/export-sinks", Some(json!({
        "key": skey, "kind": "http", "gzip": false, "events": ["sec_ev"], "interval_secs": 60, "batch_size": 100,
        "config": {"url": "http://127.0.0.1:9/sink", "secret": "sink-s3cret", "headers": {"x-tenant": "t1"}}
    }))).await;
    assert_eq!(st, 201, "{s}");
    assert_eq!(s["config"]["secret"], "***");
    let (st, w) = adm(
        &c,
        &base,
        reqwest::Method::POST,
        "/admin/v1/webhooks",
        Some(json!({
            "key": wkey, "url": "http://127.0.0.1:9/wh", "secret": "wh-s3cret", "events": ["flag.*"]
        })),
    )
    .await;
    assert_eq!(st, 201, "{w}");
    assert_eq!(w["secret"], "••••");
    let (st, ch) = adm(
        &c,
        &base,
        reqwest::Method::POST,
        "/admin/v1/notification-channels",
        Some(json!({
            "key": ckey, "kind": "webhook", "url": "http://127.0.0.1:9/ch/very-secret-token-value",
            "headers": {"authorization": "Bearer ch-s3cret"}
        })),
    )
    .await;
    assert_eq!(st, 201, "{ch}");

    // At rest: sealed under k1, non-secret fields untouched.
    let int_secret = raw(
        &pool,
        "SELECT config->>'secret' FROM integrations WHERE key = $1",
        &ikey,
    )
    .await;
    assert!(int_secret.starts_with("enc:v1:k1:"), "{int_secret}");
    let int_url = raw(
        &pool,
        "SELECT config->>'url' FROM integrations WHERE key = $1",
        &ikey,
    )
    .await;
    assert_eq!(int_url, "http://127.0.0.1:9/hook");
    let sink_secret = raw(
        &pool,
        "SELECT config->>'secret' FROM export_sinks WHERE key = $1",
        &skey,
    )
    .await;
    assert!(sink_secret.starts_with("enc:v1:k1:"), "{sink_secret}");
    let sink_hdr = raw(
        &pool,
        "SELECT config->'headers'->>'x-tenant' FROM export_sinks WHERE key = $1",
        &skey,
    )
    .await;
    assert_eq!(sink_hdr, "t1");
    let wh_secret = raw(&pool, "SELECT secret FROM webhooks WHERE key = $1", &wkey).await;
    assert!(wh_secret.starts_with("enc:v1:k1:"), "{wh_secret}");
    let ch_url = raw(
        &pool,
        "SELECT url FROM notification_channels WHERE key = $1",
        &ckey,
    )
    .await;
    assert!(ch_url.starts_with("enc:v1:k1:"), "{ch_url}");
    let ch_auth = raw(
        &pool,
        "SELECT headers->>'authorization' FROM notification_channels WHERE key = $1",
        &ckey,
    )
    .await;
    assert!(ch_auth.starts_with("enc:v1:k1:"), "{ch_auth}");

    // Reads decrypt transparently; the API keeps masking.
    let (st, i2) = adm(
        &c,
        &base,
        reqwest::Method::GET,
        &format!("/admin/v1/integrations/{ikey}"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{i2}");
    assert_eq!(i2["config"]["secret"], "••••");
    assert_eq!(i2["config"]["url"], "http://127.0.0.1:9/hook");
    // Patching with the mask keeps the stored secret (and re-seals it).
    let (st, i3) = adm(
        &c,
        &base,
        reqwest::Method::PUT,
        &format!("/admin/v1/integrations/{ikey}"),
        Some(json!({
            "config": {"url": "http://127.0.0.1:9/hook2", "secret": "••••"}
        })),
    )
    .await;
    assert_eq!(st, 200, "{i3}");
    assert_eq!(i3["config"]["url"], "http://127.0.0.1:9/hook2");
    let again = raw(
        &pool,
        "SELECT config->>'secret' FROM integrations WHERE key = $1",
        &ikey,
    )
    .await;
    assert!(again.starts_with("enc:v1:k1:"), "{again}");
    assert_eq!(
        zanshin_store::crypto::open_str(&again).unwrap(),
        "int-s3cret"
    );

    // Legacy plaintext row (written before the key existed) still opens.
    sqlx::query("UPDATE webhooks SET secret = 'legacy-plain' WHERE key = $1")
        .bind(&wkey)
        .execute(&pool)
        .await
        .unwrap();
    let (st, w2) = adm(
        &c,
        &base,
        reqwest::Method::GET,
        &format!("/admin/v1/webhooks/{wkey}"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{w2}");
    assert_eq!(w2["secret"], "••••");

    // Rotation: k2 becomes active, k1 still opens; rewrap rewrites everything under k2.
    zanshin_store::crypto::install(Some(&format!("{K2},{K1}"))).unwrap();
    let report = zanshin_store::rewrap::rewrap_pool(&pool).await.unwrap();
    assert_eq!(report.active_key.as_deref(), Some("k2"));
    let public = report
        .schemas
        .iter()
        .find(|s| s.schema == "public")
        .expect("public schema");
    assert!(
        public.integrations >= 1
            && public.export_sinks >= 1
            && public.webhooks >= 1
            && public.notification_channels >= 1,
        "{report}"
    );
    for (sql, key) in [
        (
            "SELECT config->>'secret' FROM integrations WHERE key = $1",
            &ikey,
        ),
        (
            "SELECT config->>'secret' FROM export_sinks WHERE key = $1",
            &skey,
        ),
        ("SELECT secret FROM webhooks WHERE key = $1", &wkey),
        (
            "SELECT url FROM notification_channels WHERE key = $1",
            &ckey,
        ),
        (
            "SELECT headers->>'authorization' FROM notification_channels WHERE key = $1",
            &ckey,
        ),
    ] {
        let v = raw(&pool, sql, key).await;
        assert!(v.starts_with("enc:v1:k2:"), "{sql}: {v}");
    }
    assert_eq!(
        zanshin_store::crypto::open_str(
            &raw(&pool, "SELECT secret FROM webhooks WHERE key = $1", &wkey).await
        )
        .unwrap(),
        "legacy-plain"
    );
    // Idempotent.
    let report2 = zanshin_store::rewrap::rewrap_pool(&pool).await.unwrap();
    let public2 = report2
        .schemas
        .iter()
        .find(|s| s.schema == "public")
        .unwrap();
    assert_eq!(
        public2.integrations
            + public2.export_sinks
            + public2.webhooks
            + public2.notification_channels,
        0,
        "{report2}"
    );
    let (st, i4) = adm(
        &c,
        &base,
        reqwest::Method::GET,
        &format!("/admin/v1/integrations/{ikey}"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{i4}");

    // Dropping k2 while rows are sealed with it is a loud error, not silent garbage.
    zanshin_store::crypto::install(Some(K1)).unwrap();
    let (st, e) = adm(
        &c,
        &base,
        reqwest::Method::GET,
        &format!("/admin/v1/integrations/{ikey}"),
        None,
    )
    .await;
    assert_eq!(st, 500, "{e}");
    zanshin_store::crypto::install(Some(&format!("{K2},{K1}"))).unwrap();

    for p in [
        format!("/admin/v1/integrations/{ikey}"),
        format!("/admin/v1/export-sinks/{skey}"),
        format!("/admin/v1/webhooks/{wkey}"),
        format!("/admin/v1/notification-channels/{ckey}"),
    ] {
        adm(&c, &base, reqwest::Method::DELETE, &p, None).await;
    }
}
