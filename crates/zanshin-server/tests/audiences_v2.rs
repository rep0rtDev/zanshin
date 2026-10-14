//! End-to-end tests for audiences v2: stored membership, lists, managed audiences, expressions.
//! Run only when ZANSHIN_TEST_DATABASE_URL and ZANSHIN_TEST_REDIS_URL are set.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-audiences";

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
    let status = resp.status().as_u16();
    (status, resp.json::<Value>().await.unwrap_or(Value::Null))
}

async fn key(c: &reqwest::Client, base: &str, kind: &str) -> String {
    let (_, v) = call(
        c,
        ADMIN_KEY,
        reqwest::Method::POST,
        format!("{base}/admin/v1/api-keys"),
        Some(json!({"name": format!("aud-{kind}"), "kind": kind})),
    )
    .await;
    v["secret"].as_str().unwrap().to_string()
}

async fn login(c: &reqwest::Client, base: &str, ck: &str, id: &str, custom: Value) -> Value {
    c.post(format!("{base}/v1/authenticate"))
        .bearer_auth(ck)
        .json(&json!({"id": id, "custom": custom}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn my_audiences(c: &reqwest::Client, base: &str, token: &str) -> Vec<String> {
    let v: Value = c
        .get(format!("{base}/v1/audiences"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = v
        .as_array()
        .cloned()
        .or_else(|| v["audiences"].as_array().cloned())
        .unwrap_or_default();
    arr.iter()
        .filter_map(|a| {
            a.as_str()
                .map(String::from)
                .or_else(|| a["key"].as_str().map(String::from))
        })
        .collect()
}

#[tokio::test]
async fn stored_membership_lists_managed_and_expressions() {
    let Some((base, c)) = start().await else {
        return;
    };
    let sfx = uuid::Uuid::now_v7().simple().to_string();
    let ck = key(&c, &base, "client").await;
    let sk = key(&c, &base, "server").await;
    let adm = |m: reqwest::Method, p: String, b: Option<Value>| {
        call(&c, ADMIN_KEY, m, format!("{base}{p}"), b)
    };

    // Default audiences exist and all_players matches everyone.
    let (_, all) = adm(reqwest::Method::GET, "/admin/v1/audiences".into(), None).await;
    let keys: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["key"].as_str())
        .collect();
    for k in [
        "all_players",
        "new_players",
        "payers",
        "inactive_7d",
        "ios",
        "android",
    ] {
        assert!(keys.contains(&k), "{keys:?}");
    }

    // Create from an expression; expression round-trips.
    let akey = format!("whales_{sfx}");
    let (st, a) = adm(reqwest::Method::POST, "/admin/v1/audiences".into(), Some(json!({
        "key": akey, "name": "Whales", "expression": format!("custom.spend_{sfx} >= 100 and not custom.banned == true")
    }))).await;
    assert_eq!(st, 201, "{a}");
    assert_eq!(a["kind"], "dynamic");
    assert_eq!(a["condition"]["all"][0]["op"], "gte");
    let (_, e) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/audiences/{akey}/expression"),
        None,
    )
    .await;
    assert_eq!(
        e["expression"],
        format!("custom.spend_{sfx} >= 100 and not custom.banned == true")
    );
    let (st, conv) = adm(reqwest::Method::POST, "/admin/v1/audiences/expression".into(), Some(json!({"expression": "default.country in [\"DE\", \"FR\"] and computed.last_seen_at within_last 3d"}))).await;
    assert_eq!(st, 200, "{conv}");
    assert_eq!(conv["condition"]["all"][1]["value"], 259200);
    let (st, _) = adm(
        reqwest::Method::POST,
        "/admin/v1/audiences/expression".into(),
        Some(json!({"expression": "custom.x ~ 1"})),
    )
    .await;
    assert_eq!(st, 400);

    // Live membership is mirrored into the members list with join time.
    let whale = format!("whale_{sfx}");
    let s1 = login(&c, &base, &ck, &whale, json!({format!("spend_{sfx}"): 250})).await;
    let t1 = s1["token"].as_str().unwrap().to_string();
    let mine = my_audiences(&c, &base, &t1).await;
    assert!(
        mine.contains(&akey) && mine.contains(&"all_players".to_string()),
        "{mine:?}"
    );
    let minnow = format!("minnow_{sfx}");
    let s2 = login(&c, &base, &ck, &minnow, json!({format!("spend_{sfx}"): 5})).await;
    let t2 = s2["token"].as_str().unwrap().to_string();
    assert!(!my_audiences(&c, &base, &t2).await.contains(&akey));
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let mine = my_audiences(&c, &base, &t1).await;
    assert!(
        mine.contains(&"new_players".to_string()),
        "first_seen_at folded from $authenticate: {mine:?}"
    );
    let (_, members) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/audiences/{akey}/members"),
        None,
    )
    .await;
    let items = members["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{members}");
    assert_eq!(items[0]["external_id"], whale);
    assert_eq!(items[0]["source"], "rule");
    assert!(items[0]["joined_at"].is_string());

    // Include list adds the minnow; a temporary exclude removes the whale until it expires.
    let (st, _) = adm(
        reqwest::Method::PUT,
        format!("/admin/v1/audiences/{akey}/lists"),
        Some(json!({"external_id": minnow, "mode": "include", "reason": "VIP support"})),
    )
    .await;
    assert_eq!(st, 200);
    assert!(my_audiences(&c, &base, &t2).await.contains(&akey));
    let until = chrono::Utc::now() + chrono::Duration::seconds(2);
    let (st, _) = adm(reqwest::Method::PUT, format!("/admin/v1/audiences/{akey}/lists"), Some(json!({"external_id": whale, "mode": "exclude", "until": until.to_rfc3339(), "reason": "fraud review"}))).await;
    assert_eq!(st, 200);
    assert!(!my_audiences(&c, &base, &t1).await.contains(&akey));
    let (_, lists) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/audiences/{akey}/lists"),
        None,
    )
    .await;
    assert_eq!(lists.as_array().unwrap().len(), 2);
    tokio::time::sleep(std::time::Duration::from_millis(2300)).await;
    assert!(
        my_audiences(&c, &base, &t1).await.contains(&akey),
        "exclusion expired"
    );
    let (st, _) = adm(
        reqwest::Method::DELETE,
        format!("/admin/v1/audiences/{akey}/lists"),
        Some(json!({"external_id": minnow})),
    )
    .await;
    assert_eq!(st, 200);
    assert!(!my_audiences(&c, &base, &t2).await.contains(&akey));

    // Scheduled recompute stores membership and serves it without live evaluation.
    let skey = format!("sched_{sfx}");
    let (st, _) = adm(reqwest::Method::POST, "/admin/v1/audiences".into(), Some(json!({
        "key": skey, "expression": format!("custom.spend_{sfx} >= 100"), "recompute": "scheduled", "schedule_cron": "0 0 * * * *"
    }))).await;
    assert_eq!(st, 201);
    assert!(
        !my_audiences(&c, &base, &t1).await.contains(&skey),
        "nothing stored yet"
    );
    let (st, r) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/audiences/{skey}/recompute"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{r}");
    assert!(r["member_count"].as_i64().unwrap() >= 1);
    assert!(my_audiences(&c, &base, &t1).await.contains(&skey));
    assert!(!my_audiences(&c, &base, &t2).await.contains(&skey));
    let (_, sa) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/audiences/{skey}"),
        None,
    )
    .await;
    assert!(sa["last_computed_at"].is_string() && sa["member_count"].as_i64().unwrap() >= 1);

    // Managed audience: membership pushed by the server (Nakama) API.
    let mkey = format!("guild_{sfx}");
    let (st, m) = adm(
        reqwest::Method::POST,
        "/admin/v1/audiences".into(),
        Some(json!({"key": mkey, "kind": "managed", "source": "nakama:guilds"})),
    )
    .await;
    assert_eq!(st, 201, "{m}");
    let r = c
        .post(format!("{base}/server/v1/audiences/{mkey}/members"))
        .bearer_auth(&sk)
        .json(&json!({"external_ids": [minnow, format!("ghost_{sfx}")]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["added"], 1);
    assert_eq!(v["missing"][0], format!("ghost_{sfx}"));
    assert!(my_audiences(&c, &base, &t2).await.contains(&mkey));
    assert!(!my_audiences(&c, &base, &t1).await.contains(&mkey));
    let r = c
        .delete(format!("{base}/server/v1/audiences/{mkey}/members"))
        .bearer_auth(&sk)
        .json(&json!({"external_ids": [minnow]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(!my_audiences(&c, &base, &t2).await.contains(&mkey));
    let (st, _) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/audiences/{akey}/members"),
        Some(json!({"external_ids": [minnow]})),
    )
    .await;
    assert_eq!(st, 400, "dynamic audiences reject managed membership");

    // Usage + copy.
    let fkey = format!("flag_{sfx}");
    let (st, _) = adm(reqwest::Method::POST, "/admin/v1/flags".into(), Some(json!({
        "key": fkey, "value_type": "bool", "default_value": false,
        "rules": [{"id": "whales", "audiences": [akey], "condition": {"all": []}, "value": true}]
    }))).await;
    assert_eq!(st, 201);
    let (_, u) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/audiences/{akey}/usage"),
        None,
    )
    .await;
    assert!(
        u.as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "flag" && r["key"] == fkey),
        "{u}"
    );
    let (st, cp) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/audiences/{akey}/copy"),
        Some(json!({"key": format!("{akey}_copy")})),
    )
    .await;
    assert_eq!(st, 201, "{cp}");
    assert_eq!(cp["copied_from"], akey);
    assert_eq!(cp["enabled"], false);
    assert_eq!(cp["condition"], a["condition"]);
}
