//! End-to-end tests for data management: export sinks, config bundles, history/undo, sample data, GDPR tools.
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-data";

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

/// Minimal HTTP receiver that records request bodies and answers 200.
async fn receiver() -> (
    String,
    std::sync::Arc<tokio::sync::Mutex<Vec<(Vec<(String, String)>, String)>>>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/ingest", l.local_addr().unwrap());
    let got = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let g = got.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let g = g.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let (headers, body_start) = loop {
                    let n = s.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let h = String::from_utf8_lossy(&buf[..p]).to_string();
                        break (h, p + 4);
                    }
                };
                let hdrs: Vec<(String, String)> = headers
                    .lines()
                    .skip(1)
                    .filter_map(|l| {
                        l.split_once(": ")
                            .map(|(a, b)| (a.to_lowercase(), b.to_string()))
                    })
                    .collect();
                let len: usize = hdrs
                    .iter()
                    .find(|(k, _)| k == "content-length")
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(0);
                while buf.len() < body_start + len {
                    let n = s.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let body = String::from_utf8_lossy(&buf[body_start..]).to_string();
                g.lock().await.push((hdrs, body));
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    )
                    .await;
            });
        }
    });
    (url, got)
}

#[tokio::test]
async fn data_management_flow() {
    let Some((base, c)) = start().await else {
        return;
    };
    let sfx = uuid::Uuid::now_v7().simple().to_string();
    let adm = |m: reqwest::Method, p: String, b: Option<Value>| {
        call(&c, ADMIN_KEY, m, format!("{base}{p}"), b)
    };
    let (_, k) = adm(
        reqwest::Method::POST,
        "/admin/v1/api-keys".into(),
        Some(json!({"name": "c", "kind": "client"})),
    )
    .await;
    let ck = k["secret"].as_str().unwrap().to_string();
    let ev = format!("exp_{sfx}");

    // ---- export sink to a local HTTP receiver
    let (url, got) = receiver().await;
    let skey = format!("sink_{sfx}");
    let (st, bad) = adm(
        reqwest::Method::POST,
        "/admin/v1/export-sinks".into(),
        Some(json!({"key": skey, "kind": "s3", "config": {"bucket": "b"}})),
    )
    .await;
    assert_eq!(st, 400, "{bad}");
    let (st, s) = adm(reqwest::Method::POST, "/admin/v1/export-sinks".into(), Some(json!({
        "key": skey, "kind": "http", "gzip": false, "events": [ev], "interval_secs": 60, "batch_size": 100,
        "config": {"url": url, "secret": "topsecret", "headers": {"x-tenant": "t1"}}
    }))).await;
    assert_eq!(st, 201, "{s}");
    assert_eq!(s["config"]["secret"], "***");
    let auth: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .json(&json!({"id": format!("p_{sfx}")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tok = auth["token"].as_str().unwrap().to_string();
    let pid = auth["identity"]["id"].as_str().unwrap().to_string();
    let (st, _) = call(
        &c,
        &tok,
        reqwest::Method::POST,
        format!("{base}/v1/events"),
        Some(json!({"events": [{"name": ev, "value": "1"}, {"name": format!("other_{sfx}")}]})),
    )
    .await;
    assert!(st == 200 || st == 202);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let (st, run) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/export-sinks/{skey}/run"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{run}");
    assert_eq!(run["status"], "ok", "{run}");
    assert_eq!(run["rows"], 1);
    let (st, t) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/export-sinks/{skey}/test"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{t}");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let reqs = got.lock().await.clone();
    assert_eq!(reqs.len(), 2, "{reqs:?}");
    let (hdrs, body) = &reqs[0];
    assert!(hdrs.iter().any(|(k, v)| k == "x-tenant" && v == "t1"));
    assert!(hdrs.iter().any(|(k, _)| k == "x-zanshin-signature"));
    let line: Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
    assert_eq!(line["name"], ev);
    assert_eq!(line["value"], "1");
    // Second run exports nothing new; a re-tracked event shows up after the cursor.
    let (_, run2) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/export-sinks/{skey}/run"),
        None,
    )
    .await;
    assert_eq!(run2["status"], "empty");
    let (_, runs) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/export-sinks/{skey}/runs"),
        None,
    )
    .await;
    assert_eq!(runs["items"].as_array().unwrap().len(), 2);
    let (st, _) = adm(
        reqwest::Method::PUT,
        format!("/admin/v1/export-sinks/{skey}"),
        Some(json!({"config": {"url": url, "secret": "***", "headers": {}}})),
    )
    .await;
    assert_eq!(st, 200);
    let (_, s2) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/export-sinks/{skey}"),
        None,
    )
    .await;
    assert_eq!(s2["config"]["secret"], "***"); // still set (masked), not overwritten with the mask

    // ---- config bundle: export → diff → import (merge + replace + dry run)
    let fkey = format!("cfg_{sfx}");
    let fkey2 = format!("cfg2_{sfx}");
    for k in [&fkey, &fkey2] {
        let (st, _) = adm(reqwest::Method::POST, "/admin/v1/flags".into(), Some(json!({"key": k, "value_type": "bool", "default_value": false, "description": "v1"}))).await;
        assert_eq!(st, 201);
    }
    let (st, bundle) = adm(
        reqwest::Method::GET,
        "/admin/v1/config/export?kinds=flags,export_sinks".into(),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(bundle["format"], "zanshin-config/1");
    let flags = bundle["entities"]["flags"].as_array().unwrap();
    assert!(
        flags.iter().any(|f| f["key"] == fkey) && flags.iter().all(|f| f.get("version").is_none())
    );
    let mut edited = bundle.clone();
    let arr = edited["entities"]["flags"].as_array_mut().unwrap();
    arr.retain(|f| f["key"] != fkey2);
    for f in arr.iter_mut() {
        if f["key"] == fkey {
            f["description"] = json!("from-bundle");
        }
    }
    arr.push(json!({"key": format!("cfg3_{sfx}"), "value_type": "string", "default_value": "x"}));
    let (st, d) = adm(
        reqwest::Method::POST,
        "/admin/v1/config/diff".into(),
        Some(json!({"bundle": edited, "mode": "replace", "kinds": ["flags"]})),
    )
    .await;
    assert_eq!(st, 200, "{d}");
    let fl = &d["kinds"]["flags"];
    assert!(fl["added"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["key"] == format!("cfg3_{sfx}")));
    assert!(
        fl["changed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["key"] == fkey && a["fields"] == json!(["description"])),
        "{fl}"
    );
    assert!(fl["removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["key"] == fkey2));
    // A hand-written file that only spells out some fields is not drift: absent keys keep the
    // server's values, and runtime state (`stats`, timestamps) is never compared.
    let partial = json!({"format": "zanshin-config/1", "entities": {"flags": [
        {"key": fkey, "value_type": "bool", "default_value": false, "description": "v1"}
    ]}});
    let (st, d2) = adm(
        reqwest::Method::POST,
        "/admin/v1/config/diff".into(),
        Some(json!({"bundle": partial, "mode": "merge", "kinds": ["flags"]})),
    )
    .await;
    assert_eq!(st, 200, "{d2}");
    let fl2 = &d2["kinds"]["flags"];
    assert_eq!(fl2["unchanged"], 1, "{fl2}");
    assert!(fl2["changed"].as_array().unwrap().is_empty(), "{fl2}");
    let (_, dry) = adm(
        reqwest::Method::POST,
        "/admin/v1/config/import".into(),
        Some(json!({"bundle": edited, "mode": "merge", "kinds": ["flags"], "dry_run": true})),
    )
    .await;
    assert_eq!(dry["dry_run"], true);
    let (_, f) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/flags/{fkey}"),
        None,
    )
    .await;
    assert_eq!(f["description"], "v1");
    let (st, res) = adm(
        reqwest::Method::POST,
        "/admin/v1/config/import".into(),
        Some(json!({"bundle": edited, "mode": "merge", "kinds": ["flags"]})),
    )
    .await;
    assert_eq!(st, 200, "{res}");
    assert_eq!(res["errors"].as_array().unwrap().len(), 0, "{res}");
    let (_, f) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/flags/{fkey}"),
        None,
    )
    .await;
    assert_eq!(f["description"], "from-bundle");
    let (st, _) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/flags/cfg3_{sfx}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let (st, _) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/flags/{fkey2}"),
        None,
    )
    .await;
    assert_eq!(st, 200, "merge keeps entities missing from the bundle");
    // Replace only within the flags we own: bundle with just our three → other flags would be removed, so keep merge for the shared DB.

    // ---- history + undo
    let (st, _) = adm(
        reqwest::Method::PUT,
        format!("/admin/v1/flags/{fkey}"),
        Some(json!({"description": "v3"})),
    )
    .await;
    assert_eq!(st, 200);
    let mut hist = Value::Null;
    for _ in 0..50 {
        let (_, h) = adm(
            reqwest::Method::GET,
            format!("/admin/v1/config/history?kind=flags&key={fkey}"),
            None,
        )
        .await;
        if h["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["body"]["description"] == "v3")
        {
            hist = h;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let items = hist["items"].as_array().expect("history recorded");
    assert!(
        items
            .iter()
            .any(|s| s["body"]["description"] == "from-bundle"),
        "{hist}"
    );
    let (_, audit) = adm(
        reqwest::Method::GET,
        "/admin/v1/audit?limit=100".into(),
        None,
    )
    .await;
    let entry = audit["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["method"] == "PUT" && e["path"] == format!("/admin/v1/flags/{fkey}"))
        .expect("audit entry");
    let (st, u) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/audit/{}/undo", entry["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(st, 200, "{u}");
    assert_eq!(u["action"], "restored");
    let (_, f) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/flags/{fkey}"),
        None,
    )
    .await;
    assert_eq!(f["description"], "from-bundle");
    // Revert to the very first snapshot (v1 as created via the API) and undo a create.
    let first = items
        .iter()
        .rfind(|s| s["body"]["description"] == "v1")
        .map(|s| s["id"].as_str().unwrap().to_string());
    if let Some(id) = first {
        let (st, r) = adm(
            reqwest::Method::POST,
            format!("/admin/v1/config/history/{id}/revert"),
            None,
        )
        .await;
        assert_eq!(st, 200, "{r}");
        let (_, f) = adm(
            reqwest::Method::GET,
            format!("/admin/v1/flags/{fkey}"),
            None,
        )
        .await;
        assert_eq!(f["description"], "v1");
    }
    let create = audit["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            e["method"] == "POST" && e["path"] == "/admin/v1/flags" && e["body"]["key"] == fkey2
        })
        .expect("create audit entry");
    let (st, u) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/audit/{}/undo", create["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(st, 200, "{u}");
    assert_eq!(u["action"], "deleted");
    let (st, _) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/flags/{fkey2}"),
        None,
    )
    .await;
    assert_eq!(st, 404);

    // ---- sample data, takeout, GDPR delete, purge
    let (st, smp) = adm(
        reqwest::Method::POST,
        "/admin/v1/data/sample".into(),
        Some(json!({"players": 15, "days": 3, "seed": 42})),
    )
    .await;
    assert_eq!(st, 200, "{smp}");
    assert!(smp["events"].as_i64().unwrap() > 15);
    let (st, tk) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/data/identities/{pid}/takeout"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{tk}");
    assert!(tk["events"].as_array().unwrap().len() >= 2, "{tk}");
    let (st, _) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/data/identities/{pid}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
    let (st, _) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/identities/{pid}"),
        None,
    )
    .await;
    assert_eq!(st, 404);
    let (st, bad) = adm(
        reqwest::Method::POST,
        "/admin/v1/data/events/purge".into(),
        Some(json!({"before": chrono::Utc::now()})),
    )
    .await;
    assert_eq!(st, 400, "{bad}");
    let (st, p) = adm(
        reqwest::Method::POST,
        "/admin/v1/data/events/purge".into(),
        Some(json!({"before": chrono::Utc::now() - chrono::Duration::days(1), "names": [ev]})),
    )
    .await;
    assert_eq!(st, 200, "{p}");

    // cleanup
    for k in [fkey.clone(), format!("cfg3_{sfx}")] {
        adm(
            reqwest::Method::DELETE,
            format!("/admin/v1/flags/{k}"),
            None,
        )
        .await;
    }
    let (st, _) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/export-sinks/{skey}"),
        None,
    )
    .await;
    assert_eq!(st, 200);
}
