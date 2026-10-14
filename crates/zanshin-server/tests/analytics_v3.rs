//! Analytics v3: dimension filters, funnel models, cumulative retention, explorer, player map,
//! playtime, LTV / RoAS with ad-spend import, errors, status board and LiveOps calendar.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-000";

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

async fn json(r: reqwest::Response) -> Value {
    let status = r.status();
    let text = r.text().await.unwrap();
    assert!(status.is_success(), "{status}: {text}");
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn filters_models_explorer_map_playtime_ltv_status_calendar() {
    let Some((base, http)) = start().await else {
        eprintln!("skipping: ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL not set");
        return;
    };
    let admin = |m: reqwest::Method, p: &str| {
        http.request(m, format!("{base}{p}"))
            .header("x-api-key", ADMIN_KEY)
    };
    let suffix = uuid::Uuid::now_v7().simple().to_string();
    let platform = format!("v3-{suffix}");

    // Client key; three players on a unique platform, two in ZQ and one in ZR.
    let created = json(
        admin(reqwest::Method::POST, "/admin/v1/api-keys")
            .json(&json!({"name": format!("v3-{suffix}"), "kind": "client"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let client_key = created["secret"].as_str().unwrap().to_string();
    let mut tokens = Vec::new();
    for (i, country) in ["ZQ", "ZQ", "ZR"].iter().enumerate() {
        let s = json(http.post(format!("{base}/v1/authenticate")).header("x-api-key", &client_key)
            .json(&json!({"id": format!("p{i}-{suffix}"), "default": {"country": country, "platform": platform, "app_version": "3.1.0"}}))
            .send().await.unwrap()).await;
        tokens.push(s["token"].as_str().unwrap().to_string());
    }
    let ev = |name: &str, meta: Value| json!({"name": name, "value": "", "metadata": meta});
    for (i, t) in tokens.iter().enumerate() {
        let mut events = vec![
            ev("level_up", json!({"level": 1})),
            ev("level_up", json!({"level": 2})),
        ];
        if i < 2 {
            events.push(ev("purchase", json!({"amount": 4.5, "sku": "gems"})));
        }
        let r = http
            .post(format!("{base}/v1/events"))
            .bearer_auth(t)
            .json(&json!({"events": events}))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "{}", r.text().await.unwrap());
    }
    // The writer flushes asynchronously; poll until the filtered DAU shows all three players.
    let mut overview = Value::Null;
    for _ in 0..50 {
        overview = json(
            admin(
                reqwest::Method::GET,
                &format!("/admin/v1/analytics/overview?platform={platform}"),
            )
            .send()
            .await
            .unwrap(),
        )
        .await;
        if overview["totals"]["dau"].as_i64() == Some(3) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert_eq!(overview["totals"]["dau"], 3, "{overview}");
    assert_eq!(overview["totals"]["purchases"], 2);
    assert!((overview["totals"]["revenue"].as_f64().unwrap() - 9.0).abs() < 1e-6);
    assert_eq!(overview["filter"]["platform"], platform);
    let zq = json(
        admin(
            reqwest::Method::GET,
            &format!("/admin/v1/analytics/overview?platform={platform}&country=ZQ"),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(zq["totals"]["dau"], 2, "country filter narrows");
    let r = admin(
        reqwest::Method::GET,
        "/admin/v1/analytics/overview?variant=a",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(r.status(), 400, "variant without experiment is rejected");

    // Funnel models: step model with a tiny window still works (events sent in one batch); session model matches.
    for model in ["unlimited", "window", "step", "session"] {
        let f = json(admin(reqwest::Method::POST, "/admin/v1/analytics/funnel")
            .json(&json!({"steps": [{"name": "level_up"}, {"name": "purchase"}], "model": model, "window_secs": 3600, "filter": {"platform": platform}}))
            .send().await.unwrap()).await;
        assert_eq!(f["model"], model);
        assert_eq!(f["entered"], 3, "{model}: {f}");
        assert_eq!(f["steps"][1]["count"], 2, "{model}: {f}");
    }

    // Retention with filter + cumulative flag reports the installs of the cohort.
    let ret = json(
        admin(
            reqwest::Method::GET,
            &format!("/admin/v1/analytics/retention?platform={platform}&cumulative=true&days=1,7"),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(ret["cumulative"], true);
    assert_eq!(ret["installs"], 3, "{ret}");

    // Explorer grouped by country.
    let ex = json(admin(reqwest::Method::POST, "/admin/v1/analytics/explore")
        .json(&json!({"event": "level_up", "group_by": "country", "filter": {"platform": platform}}))
        .send().await.unwrap()).await;
    assert_eq!(ex["total"]["count"], 6, "{ex}");
    let groups = ex["groups"].as_array().unwrap();
    assert_eq!(groups[0]["group"], "ZQ");
    assert_eq!(groups[0]["count"], 4);
    let ex = json(admin(reqwest::Method::POST, "/admin/v1/analytics/explore")
        .json(&json!({"event": "purchase", "group_by": "meta:sku", "sum_field": "amount", "granularity": "hour", "filter": {"platform": platform}}))
        .send().await.unwrap()).await;
    assert_eq!(ex["groups"][0]["group"], "gems");
    assert!((ex["total"]["field_sum"].as_f64().unwrap() - 9.0).abs() < 1e-6);
    let r = admin(reqwest::Method::POST, "/admin/v1/analytics/explore")
        .json(&json!({"event": "x", "group_by": "bogus"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);

    // Player map.
    let map = json(
        admin(
            reqwest::Method::GET,
            &format!("/admin/v1/analytics/map?platform={platform}"),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(map["total_players"], 3);
    assert_eq!(map["countries"][0]["country"], "ZQ");
    assert_eq!(map["countries"][0]["players"], 2);

    // Playtime: three open play sessions were started by authenticate.
    let pt = json(
        admin(
            reqwest::Method::GET,
            &format!("/admin/v1/analytics/playtime?platform={platform}"),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(pt["totals"]["sessions"], 3, "{pt}");
    assert_eq!(pt["histogram"].as_array().unwrap().len(), 6);

    // Ad spend import (CSV + JSON) and LTV / RoAS for today's cohort.
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let imported = json(admin(reqwest::Method::POST, "/admin/v1/analytics/ad-spend")
        .json(&json!({"csv": format!("day,platform,country,campaign,spend,installs\n{today},{platform},,ua-{suffix},30,10"), "rows": [{"day": today, "platform": platform, "country": "ZR", "campaign": format!("ua2-{suffix}"), "spend": 6.0, "installs": 2}]}))
        .send().await.unwrap()).await;
    assert_eq!(imported["upserted"], 2);
    let spend = json(
        admin(
            reqwest::Method::GET,
            &format!("/admin/v1/analytics/ad-spend?from={today}T00:00:00Z"),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert!(spend["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["campaign"] == format!("ua-{suffix}")));
    let ltv = json(
        admin(
            reqwest::Method::GET,
            &format!("/admin/v1/analytics/ltv?platform={platform}&days=0,7"),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(ltv["totals"]["installs"], 3, "{ltv}");
    assert!((ltv["totals"]["spend"].as_f64().unwrap() - 36.0).abs() < 1e-6);
    assert!((ltv["totals"]["cpi"].as_f64().unwrap() - 12.0).abs() < 1e-6);
    let d0 = &ltv["totals"]["at"]["d0"];
    assert!(
        (d0["revenue"].as_f64().unwrap() - 9.0).abs() < 1e-6,
        "{ltv}"
    );
    assert!((d0["ltv"].as_f64().unwrap() - 3.0).abs() < 1e-6);
    assert!((d0["roas"].as_f64().unwrap() - 0.25).abs() < 1e-6);
    assert!(ltv["totals"]["at"]["d7"].is_object() || ltv["totals"]["at"]["d7"].is_null());
    let r = admin(reqwest::Method::POST, "/admin/v1/analytics/ad-spend")
        .json(&json!({"csv": "day,platform\nnot-a-date,x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let deleted = json(
        admin(
            reqwest::Method::DELETE,
            &format!("/admin/v1/analytics/ad-spend?from={today}T00:00:00Z"),
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert!(deleted["deleted"].as_i64().unwrap() >= 2);

    // Errors, status, calendar.
    let errs = json(
        admin(reqwest::Method::GET, "/admin/v1/analytics/errors")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(errs["rejected_events"]["count"].is_number());
    assert_eq!(errs["failures"].as_array().unwrap().len(), 3);
    let status = json(
        admin(reqwest::Method::GET, "/admin/v1/analytics/status")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(status["online_now"].as_i64().unwrap() >= 3, "{status}");
    assert!(status["dau_today"].as_i64().unwrap() >= 3);
    assert!(status["events_last_hour"].as_i64().unwrap() >= 8);
    assert!(status["running"]["experiments"].is_array());
    let cal = json(
        admin(reqwest::Method::GET, "/admin/v1/analytics/calendar")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(cal["items"].is_array());
    assert!(
        cal["metrics"].as_array().unwrap().len() >= 7,
        "{}",
        cal["metrics"]
    );

    let _ = admin(
        reqwest::Method::DELETE,
        &format!("/admin/v1/api-keys/{}", created["id"].as_str().unwrap()),
    )
    .send()
    .await;
}
