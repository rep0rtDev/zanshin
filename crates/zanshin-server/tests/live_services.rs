//! Hardening suite against real services. Every section is skipped unless its environment
//! variable is set (CI job `live` sets them all):
//!   ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL   (always required)
//!   ZANSHIN_TEST_KEYCLOAK      e.g. http://localhost:8080   (OIDC + SAML end to end)
//!   ZANSHIN_TEST_CLICKHOUSE_URL e.g. http://localhost:8123  (SQL explorer, paths, alerts)
//!   ZANSHIN_TEST_SMTP           host:port   + ZANSHIN_TEST_MAILPIT e.g. http://localhost:8025
//!   ZANSHIN_TEST_S3_ENDPOINT    e.g. http://localhost:9000 (+ ZANSHIN_TEST_S3_BUCKET/KEY/SECRET)
//!   ZANSHIN_TEST_NAKAMA         e.g. http://localhost:7350 (+ ZANSHIN_TEST_NAKAMA_KEY)
//! The server listens on a fixed address (127.0.0.1:7560) because SSO redirect URLs are
//! registered at the IdP.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "live-bootstrap-admin-key-49";
const ADDR: &str = "127.0.0.1:7560";

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

async fn start(extra_yaml: &str) -> Option<(String, reqwest::Client)> {
    let db = env("ZANSHIN_TEST_DATABASE_URL")?;
    let redis = env("ZANSHIN_TEST_REDIS_URL")?;
    let mut yaml = format!(
        "database:\n  url: {db}\nredis:\n  url: {redis}\nauth:\n  jwt_secret: live-secret-live-secret-live-secret-0123\nadmin:\n  bootstrap_key: {ADMIN_KEY}\nserver:\n  http_addr: {ADDR}\npush:\n  provider: log\n"
    );
    yaml.push_str(extra_yaml);
    let path = std::env::temp_dir().join(format!("zanshin-live-{}.yml", uuid::Uuid::now_v7()));
    std::fs::write(&path, yaml).unwrap();
    let cfg = Config::load(Some(&path)).expect("config");
    let state = AppState::build(Arc::new(cfg)).await.expect("state");
    let listener = tokio::net::TcpListener::bind(ADDR)
        .await
        .expect("bind 7560");
    tokio::spawn(async move {
        axum::serve(
            listener,
            http::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Some((
        format!("http://{ADDR}"),
        reqwest::Client::builder()
            .cookie_store(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap(),
    ))
}

async fn adm(
    c: &reqwest::Client,
    m: &reqwest::Method,
    url: String,
    body: Option<Value>,
) -> (u16, Value) {
    let mut r = c.request(m.clone(), url).bearer_auth(ADMIN_KEY);
    if let Some(b) = body {
        r = r.json(&b);
    }
    let resp = r.send().await.unwrap();
    (
        resp.status().as_u16(),
        resp.json::<Value>().await.unwrap_or(Value::Null),
    )
}

fn attr(html: &str, needle: &str) -> Option<String> {
    let i = html.find(needle)? + needle.len();
    let rest = &html[i..];
    let end = rest.find('"')?;
    Some(rest[..end].replace("&amp;", "&"))
}

/// Submits Keycloak's login form; returns the response to the credentials POST (not followed).
async fn keycloak_login(
    c: &reqwest::Client,
    login_page: reqwest::Response,
    user: &str,
    pass: &str,
) -> reqwest::Response {
    let html = login_page.text().await.unwrap();
    let action = attr(&html, "action=\"").expect("keycloak login form");
    c.post(action)
        .form(&[("username", user), ("password", pass), ("credentialId", "")])
        .send()
        .await
        .unwrap()
}

/// Follows redirects inside the IdP; stops at (and returns) the first response from our server.
async fn follow(c: &reqwest::Client, mut r: reqwest::Response, max: usize) -> reqwest::Response {
    for _ in 0..max {
        if !r.status().is_redirection() {
            break;
        }
        let loc = r.headers()["location"].to_str().unwrap().to_string();
        let next = r.url().join(&loc).expect("location url");
        let ours = next.host_str() == Some("127.0.0.1") && next.port() == Some(7560);
        r = c.get(next).send().await.unwrap();
        if ours {
            break;
        }
    }
    r
}

#[tokio::test]
async fn live_services() {
    let kc = env("ZANSHIN_TEST_KEYCLOAK");
    let ch = env("ZANSHIN_TEST_CLICKHOUSE_URL");
    let mut extra = String::new();
    if let Some(kc) = &kc {
        let issuer = format!("{kc}/realms/zanshin");
        // SAML signing certificate from the realm descriptor
        let desc = reqwest::get(format!("{issuer}/protocol/saml/descriptor"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        let cert = {
            let s = desc
                .find("<ds:X509Certificate>")
                .or_else(|| desc.find("<dsig:X509Certificate>"))
                .expect("idp cert");
            let s = s + desc[s..].find('>').unwrap() + 1;
            let e = desc[s..].find('<').unwrap();
            desc[s..s + e].trim().to_string()
        };
        extra.push_str(&format!(
            "  oidc:\n    issuer: {issuer}\n    client_id: zanshin-console\n    client_secret: zanshin-console-secret\n    redirect_url: http://{ADDR}/console/auth/callback\n    role_claim: groups\n    role_map:\n      zanshin-admins: admin\n      zanshin-viewers: viewer\n  saml:\n    idp_sso_url: {issuer}/protocol/saml\n    idp_entity_id: {issuer}\n    idp_certs: ['{cert}']\n    acs_url: http://{ADDR}/console/auth/saml/acs\n    role_attribute: groups\n    role_map:\n      zanshin-admins: admin\n      zanshin-viewers: viewer\n"
        ));
        extra = format!("admin:\n  bootstrap_key: {ADMIN_KEY}\n{extra}");
    }
    if let Some(ch) = &ch {
        extra.push_str(&format!("clickhouse:\n  enabled: true\n  url: {ch}\n  user: zanshin\n  password: zanshin\n  database: zanshin\n"));
    }
    let Some((base, c)) = start(&extra).await else {
        return;
    };
    let sfx = uuid::Uuid::now_v7().simple().to_string();
    let (get, post) = (reqwest::Method::GET, reqwest::Method::POST);

    // ---- Keycloak: OIDC + SAML ------------------------------------------------------------------
    if kc.is_some() {
        for (who, pass, want_role, saml) in [
            ("alice", "alice-pass-123", Some("admin"), false),
            ("bob", "bob-pass-123", Some("viewer"), false),
            ("carol", "carol-pass-123", None, false),
            ("alice", "alice-pass-123", Some("admin"), true),
            ("bob", "bob-pass-123", Some("viewer"), true),
            ("carol", "carol-pass-123", None, true),
        ] {
            let fresh = reqwest::Client::builder()
                .cookie_store(true)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            let start = fresh
                .get(format!(
                    "{base}/console/auth/{}",
                    if saml { "saml/login" } else { "login" }
                ))
                .send()
                .await
                .unwrap();
            assert_eq!(
                start.status().as_u16(),
                303,
                "{who} {saml}: login must redirect to the idp"
            );
            let page = follow(&fresh, start, 5).await;
            assert_eq!(page.status().as_u16(), 200, "{who}: keycloak login page");
            let after = keycloak_login(&fresh, page, who, pass).await;
            let done = if saml {
                // Keycloak answers with an auto-submit form carrying SAMLResponse
                let after = follow(&fresh, after, 3).await;
                let html = after.text().await.unwrap();
                let resp = attr(&html, "name=\"SAMLResponse\" value=\"")
                    .or_else(|| attr(&html, "name=\"SAMLResponse\" VALUE=\""))
                    .expect("SAMLResponse form");
                let relay = attr(&html, "name=\"RelayState\" value=\"").unwrap_or_default();
                if let Some(dump) = env("ZANSHIN_TEST_DUMP_SAML") {
                    std::fs::write(&dump, &resp).unwrap();
                }
                fresh
                    .post(format!("{base}/console/auth/saml/acs"))
                    .form(&[
                        ("SAMLResponse", resp.as_str()),
                        ("RelayState", relay.as_str()),
                    ])
                    .send()
                    .await
                    .unwrap()
            } else {
                follow(&fresh, after, 4).await
            };
            let done_status = done.status().as_u16();
            let loc = done
                .headers()
                .get("location")
                .map(|l| l.to_str().unwrap().to_string())
                .unwrap_or_default();
            if done_status != 303 {
                panic!(
                    "{who} {saml}: our callback must redirect to the console, got {done_status}: {}",
                    done.text().await.unwrap_or_default()
                );
            }
            let (st, me) = {
                let r = fresh
                    .get(format!("{base}/admin/v1/me"))
                    .send()
                    .await
                    .unwrap();
                (
                    r.status().as_u16(),
                    r.json::<Value>().await.unwrap_or(Value::Null),
                )
            };
            match want_role {
                Some(role) => {
                    assert!(!loc.contains("error="), "{who} {saml}: {loc}");
                    assert_eq!(st, 200, "{who} {saml}: {me}");
                    assert_eq!(me["role"], role, "{who} {saml}: {me}");
                    assert_eq!(me["name"], format!("{who}@example.com"), "{me}");
                    assert_eq!(me["auth"], "oidc", "sso sessions report as oidc: {me}");
                }
                None => {
                    assert!(
                        loc.contains("error="),
                        "{who} {saml} without a mapped group must be rejected: {loc}"
                    );
                    assert_eq!(st, 401, "{who} {saml}: {me}");
                }
            }
        }
        let sys = adm(&c, &get, format!("{base}/admin/v1/system"), None)
            .await
            .1;
        assert_eq!(sys["auth"]["oidc"], true);
        assert_eq!(sys["auth"]["saml"], true);
    }

    // ---- ClickHouse -----------------------------------------------------------------------------
    if ch.is_some() {
        let server: Value = adm(
            &c,
            &post,
            format!("{base}/admin/v1/api-keys"),
            Some(json!({"name": format!("live-s-{sfx}"), "kind": "server"})),
        )
        .await
        .1;
        let skey = server["secret"].as_str().unwrap().to_string();
        let ext = format!("live_ch_{sfx}");
        let ev = format!("liveEvt_{sfx}");
        for i in 0..5 {
            let r = c
                .post(format!("{base}/server/v1/identities"))
                .header("x-api-key", &skey)
                .json(&json!({"external_id": ext}))
                .send()
                .await
                .unwrap();
            assert!(r.status().is_success(), "{}", r.text().await.unwrap());
            let r = c.post(format!("{base}/server/v1/events")).header("x-api-key", &skey)
                .json(&json!({"external_id": ext, "events": [{"name": ev, "value": i.to_string(), "metadata": {"i": i}}, {"name": format!("{ev}_next"), "value": "x"}]}))
                .send().await.unwrap();
            assert!(
                r.status().is_success(),
                "ingest: {}",
                r.text().await.unwrap()
            );
        }
        let mut n = 0;
        for _ in 0..40 {
            let (st, b) = adm(
                &c,
                &post,
                format!("{base}/admin/v1/sql"),
                Some(
                    json!({"sql": format!("SELECT count() AS n FROM events WHERE name = '{ev}'")}),
                ),
            )
            .await;
            assert_eq!(st, 200, "{b}");
            n = b["rows"][0][0]
                .as_u64()
                .or_else(|| b["rows"][0][0].as_str().and_then(|s| s.parse().ok()))
                .unwrap_or(0);
            if n >= 5 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert_eq!(n, 5, "sql explorer sees the ingested events on ClickHouse");
        let (st, b) = adm(
            &c,
            &post,
            format!("{base}/admin/v1/sql"),
            Some(json!({"sql": "INSERT INTO events SELECT * FROM events"})),
        )
        .await;
        assert_eq!(st, 400, "dml must be rejected: {b}");
        let (st, p) = adm(
            &c,
            &post,
            format!("{base}/admin/v1/analytics/paths"),
            Some(json!({"event": ev, "direction": "after", "depth": 2, "from": (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339()})),
        )
        .await;
        assert_eq!(st, 200, "{p}");
        assert!(
            p.to_string().contains(&format!("{ev}_next")),
            "paths on ClickHouse: {p}"
        );
        let (st, a) = adm(&c, &post, format!("{base}/admin/v1/analytics/alerts"), Some(json!({"key": format!("al_{sfx}"), "name": "live", "event": ev, "aggregation": "count", "window_secs": 3600, "condition": {"kind": "above", "threshold": 1}, "cooldown_secs": 3600}))).await;
        assert_eq!(st, 201, "{a}");
        let (st, chk) = adm(
            &c,
            &post,
            format!("{base}/admin/v1/analytics/alerts/al_{sfx}/check"),
            None,
        )
        .await;
        assert_eq!(st, 200, "{chk}");
        assert_eq!(chk["fired"], true, "alert evaluated on ClickHouse: {chk}");
    }

    // ---- SMTP via Mailpit -----------------------------------------------------------------------
    if let (Some(smtp), Some(mailpit)) = (env("ZANSHIN_TEST_SMTP"), env("ZANSHIN_TEST_MAILPIT")) {
        let (host, port) = smtp.split_once(':').unwrap();
        let server: Value = adm(
            &c,
            &post,
            format!("{base}/admin/v1/api-keys"),
            Some(json!({"name": format!("live-m-{sfx}"), "kind": "server"})),
        )
        .await
        .1;
        let skey = server["secret"].as_str().unwrap().to_string();
        let ext = format!("live_mail_{sfx}");
        let r = c
            .post(format!("{base}/server/v1/identities"))
            .header("x-api-key", &skey)
            .json(&json!({"external_id": ext, "default": {"email": format!("{ext}@example.com")}}))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success());
        let ikey = format!("smtp_{sfx}");
        let (st, i) = adm(&c, &post, format!("{base}/admin/v1/integrations"), Some(json!({"key": ikey, "name": "mailpit", "kind": "smtp",
            "config": {"host": host, "port": port.parse::<u16>().unwrap(), "tls": "none", "from": "zanshin@example.com"}}))).await;
        assert_eq!(st, 201, "{i}");
        let (st, o) = adm(
            &c,
            &post,
            format!("{base}/admin/v1/integrations/{ikey}/test"),
            Some(json!({"external_id": ext})),
        )
        .await;
        assert_eq!(st, 200, "{o}");
        assert_eq!(o["status"], "sent", "smtp delivery to mailpit: {o}");
        let mut found = false;
        for _ in 0..20 {
            let msgs: Value = reqwest::get(format!("{mailpit}/api/v1/messages?limit=50"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            found = msgs["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["To"].to_string().contains(&ext));
            if found {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(found, "mailpit received the message");
    }

    // ---- S3 (RustFS) export -----------------------------------------------------------------------
    if let Some(endpoint) = env("ZANSHIN_TEST_S3_ENDPOINT") {
        let bucket = env("ZANSHIN_TEST_S3_BUCKET").unwrap_or_else(|| "zanshin-live".into());
        let ak = env("ZANSHIN_TEST_S3_KEY").unwrap_or_else(|| "rustfsadmin".into());
        let sk = env("ZANSHIN_TEST_S3_SECRET").unwrap_or_else(|| "rustfsadmin".into());
        let server: Value = adm(
            &c,
            &post,
            format!("{base}/admin/v1/api-keys"),
            Some(json!({"name": format!("live-s3-{sfx}"), "kind": "server"})),
        )
        .await
        .1;
        let skey = server["secret"].as_str().unwrap().to_string();
        let r = c
            .post(format!("{base}/server/v1/identities"))
            .header("x-api-key", &skey)
            .json(&json!({"external_id": format!("live_s3_{sfx}")}))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "{}", r.text().await.unwrap());
        let r = c.post(format!("{base}/server/v1/events")).header("x-api-key", &skey)
            .json(&json!({"external_id": format!("live_s3_{sfx}"), "events": [{"name": format!("s3Evt_{sfx}"), "value": "1"}]})).send().await.unwrap();
        assert!(r.status().is_success());
        for format in ["jsonl", "parquet"] {
            let key = format!("s3_{format}_{sfx}");
            let (st, s) = adm(&c, &post, format!("{base}/admin/v1/export-sinks"), Some(json!({"key": key, "kind": "s3", "format": format,
                "config": {"endpoint": endpoint, "bucket": bucket, "region": "us-east-1", "access_key": ak, "secret_key": sk, "prefix": format!("live/{format}"), "partition": "event"}}))).await;
            assert_eq!(st, 201, "{s}");
            let (st, t) = adm(
                &c,
                &post,
                format!("{base}/admin/v1/export-sinks/{key}/test"),
                None,
            )
            .await;
            assert_eq!(st, 200, "{t}");
            assert_eq!(t["ok"], true, "s3 test write to RustFS ({format}): {t}");
            let (st, r) = adm(
                &c,
                &post,
                format!("{base}/admin/v1/export-sinks/{key}/run"),
                None,
            )
            .await;
            assert_eq!(st, 200, "{r}");
            assert_eq!(r["status"], "ok", "s3 export run ({format}): {r}");
            assert!(r["rows"].as_u64().unwrap_or(0) >= 1, "{r}");
            assert!(
                r["object"]
                    .as_str()
                    .is_some_and(|o| o.contains(&format!("live/{format}/"))),
                "{r}"
            );
        }
    }

    // ---- Nakama with the Zanshin module ---------------------------------------------------------
    if let Some(nk) = env("ZANSHIN_TEST_NAKAMA") {
        let nkey = env("ZANSHIN_TEST_NAKAMA_KEY").unwrap_or_else(|| "defaultkey".into());
        let device = format!("live-device-{sfx}");
        let auth: Value = c
            .post(format!(
                "{nk}/v2/account/authenticate/device?create=true&username=live_{sfx}"
            ))
            .basic_auth(&nkey, Some(""))
            .json(&json!({"id": device}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let token = auth["token"]
            .as_str()
            .expect("nakama session token")
            .to_string();
        let acct: Value = c
            .get(format!("{nk}/v2/account"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let uid = acct["user"]["id"].as_str().unwrap().to_string();
        // the after-authenticate hook syncs the identity in the background — poll briefly
        let mut last = String::new();
        let mut ok = false;
        for _ in 0..40 {
            let r = c
                .post(format!("{nk}/v2/rpc/zanshin.flags"))
                .bearer_auth(&token)
                .json(&"{}")
                .send()
                .await
                .unwrap();
            let st = r.status().as_u16();
            last = r.text().await.unwrap_or_default();
            if st == 200 {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(ok, "rpc zanshin.flags through the module: {last}");
        let r = c
            .post(format!("{nk}/v2/rpc/zanshin.track"))
            .bearer_auth(&token)
            .json(&json!({"events": [{"name": format!("nkEvt_{sfx}"), "value": "1"}]}).to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status().as_u16(),
            200,
            "rpc zanshin.track: {}",
            r.text().await.unwrap()
        );
        // the after-authenticate hook synced the identity (background) — poll
        let server: Value = adm(
            &c,
            &post,
            format!("{base}/admin/v1/api-keys"),
            Some(json!({"name": format!("live-nk-{sfx}"), "kind": "server"})),
        )
        .await
        .1;
        let skey = server["secret"].as_str().unwrap().to_string();
        let mut ok = false;
        for _ in 0..40 {
            let r = c
                .get(format!("{base}/server/v1/flags?external_id={uid}"))
                .header("x-api-key", &skey)
                .send()
                .await
                .unwrap();
            if r.status().is_success() {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(ok, "nakama user {uid} became a Zanshin identity");
    }
}
