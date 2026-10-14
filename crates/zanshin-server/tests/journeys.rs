//! End-to-end tests for journeys: triggers, steps, waits, branches, caps, manual entry, runs.
//! Run only with ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use zanshin_server::{config::Config, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-journeys";

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

async fn wait_runs(
    c: &reqwest::Client,
    base: &str,
    key: &str,
    pred: impl Fn(&Value) -> bool,
) -> Value {
    for _ in 0..150 {
        let (_, r) = call(
            c,
            ADMIN_KEY,
            reqwest::Method::GET,
            format!("{base}/admin/v1/journeys/{key}/runs"),
            None,
        )
        .await;
        if pred(&r) {
            return r;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (_, r) = call(
        c,
        ADMIN_KEY,
        reqwest::Method::GET,
        format!("{base}/admin/v1/journeys/{key}/runs"),
        None,
    )
    .await;
    panic!("runs never reached expected state: {r}");
}

fn status_count(r: &Value, status: &str) -> i64 {
    r["stats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["status"] == status)
        .and_then(|s| s["count"].as_i64())
        .unwrap_or(0)
}

#[tokio::test]
async fn journeys_flow() {
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

    let fkey = format!("vip_{sfx}");
    let (st, _) = adm(
        reqwest::Method::POST,
        "/admin/v1/flags".into(),
        Some(json!({"key": fkey, "value_type": "bool", "default_value": false})),
    )
    .await;
    assert_eq!(st, 201);
    let ekey = format!("jexp_{sfx}");
    let (st, _) = adm(reqwest::Method::POST, "/admin/v1/experiments".into(), Some(json!({"key": ekey, "status": "running", "variants": [{"key": "a", "value": 1, "weight": 1}, {"key": "b", "value": 2, "weight": 1}]}))).await;
    assert_eq!(st, 201);

    // Validation: unknown branch target.
    let jkey = format!("first_buy_{sfx}");
    let (st, bad) = adm(reqwest::Method::POST, "/admin/v1/journeys".into(), Some(json!({"key": jkey, "steps": [{"id": "b", "kind": "branch", "condition": {"all": []}, "then": "nope"}]}))).await;
    assert_eq!(st, 400, "{bad}");

    let steps = json!([
        {"id": "thanks", "kind": "message", "title": "Thanks {{custom.name}}", "body": "first purchase!"},
        {"id": "pause", "kind": "wait", "secs": 1},
        {"id": "vip", "kind": "branch", "condition": {"all": []}, "then": "flag", "else": "track"},
        {"id": "flag", "kind": "flag_override", "flag": fkey, "value": true},
        {"id": "assign", "kind": "experiment_assign", "experiment": ekey, "variant": "b"},
        {"id": "track", "kind": "track", "name": "journey_done", "value": "1"},
        {"id": "end", "kind": "exit"}
    ]);
    let (st, j) = adm(reqwest::Method::POST, "/admin/v1/journeys".into(), Some(json!({
        "key": jkey, "name": "First purchase", "trigger": {"kind": "event", "name": "purchase"}, "steps": steps, "cooldown_secs": 3600, "max_entries": 1, "condition": {"prop": "custom.cohort", "op": "eq", "value": sfx}
    }))).await;
    assert_eq!(st, 201, "{j}");
    assert_eq!(j["steps"].as_array().unwrap().len(), 7);

    let ext = format!("ana_{sfx}");
    let s: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .json(&json!({"id": ext, "custom": {"name": "Ana", "cohort": sfx}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tok = s["token"].as_str().unwrap().to_string();
    let (st, _) = call(
        &c,
        &tok,
        reqwest::Method::POST,
        format!("{base}/v1/events"),
        Some(json!({"events": [{"name": "purchase", "value": "4.99"}]})),
    )
    .await;
    assert!(st == 200 || st == 202);

    // Message immediately, then a 1s wait resumed by the worker, then the rest.
    let r = wait_runs(&c, &base, &jkey, |r| {
        status_count(r, "active") + status_count(r, "exited") >= 1
            && r["items"].as_array().is_some_and(|a| !a.is_empty())
    })
    .await;
    assert_eq!(r["items"].as_array().unwrap().len(), 1, "{r}");
    let r = wait_runs(&c, &base, &jkey, |r| status_count(r, "exited") == 1).await;
    let run = &r["items"][0];
    assert_eq!(run["external_id"], ext);
    let (_, inbox) = call(
        &c,
        &tok,
        reqwest::Method::GET,
        format!("{base}/v1/messages"),
        None,
    )
    .await;
    assert!(
        inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["title"] == "Thanks Ana"),
        "{inbox}"
    );
    let (_, f) = call(
        &c,
        &tok,
        reqwest::Method::GET,
        format!("{base}/v1/flags?names={fkey}"),
        None,
    )
    .await;
    assert_eq!(f["flags"][0]["value"], true, "override applied: {f}");
    let (_, e) = call(
        &c,
        &tok,
        reqwest::Method::GET,
        format!("{base}/v1/experiments?names={ekey}"),
        None,
    )
    .await;
    assert_eq!(e["experiments"][0]["variant"], "b", "{e}");
    let run_id = run["id"].as_str().unwrap();
    let (st, d) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/journeys/{jkey}/runs/{run_id}"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{d}");
    let log = d["log"].as_array().unwrap();
    assert!(log.len() >= 7, "{d}");
    assert!(log
        .iter()
        .any(|l| l["action"] == "vip" && l["detail"].as_str().unwrap().contains("flag")));
    assert!(r["steps"].as_array().unwrap().len() == 7);

    // Cap + cooldown: a second purchase does not re-enter; manual entry is refused too.
    let (st, _) = call(
        &c,
        &tok,
        reqwest::Method::POST,
        format!("{base}/v1/events"),
        Some(json!({"events": [{"name": "purchase", "value": "1"}]})),
    )
    .await;
    assert!(st == 200 || st == 202);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let (_, r) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/journeys/{jkey}/runs"),
        None,
    )
    .await;
    assert_eq!(r["items"].as_array().unwrap().len(), 1, "{r}");
    let (st, m) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/journeys/{jkey}/enter"),
        Some(json!({"external_id": ext})),
    )
    .await;
    assert_eq!(st, 400, "{m}");
    let (st, ij) = adm(
        reqwest::Method::GET,
        format!(
            "/admin/v1/identities/{}/journeys",
            run["identity_id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(ij["items"][0]["journey"], jkey);

    // identity_created trigger.
    let wkey = format!("welcome_{sfx}");
    let (st, _) = adm(reqwest::Method::POST, "/admin/v1/journeys".into(), Some(json!({"key": wkey, "trigger": {"kind": "identity_created"}, "condition": {"prop": "custom.cohort", "op": "eq", "value": sfx}, "steps": [{"id": "t", "kind": "track", "name": "welcomed"}]}))).await;
    assert_eq!(st, 201);
    let _: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .json(&json!({"id": format!("new_{sfx}"), "custom": {"cohort": sfx}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r = wait_runs(&c, &base, &wkey, |r| status_count(r, "done") == 1).await;
    assert_eq!(r["items"][0]["external_id"], format!("new_{sfx}"));

    // Long wait + operator exit; disabled journeys can still be entered manually, but not by trigger.
    let lkey = format!("slow_{sfx}");
    let (st, _) = adm(reqwest::Method::POST, "/admin/v1/journeys".into(), Some(json!({"key": lkey, "enabled": false, "trigger": {"kind": "event", "name": "purchase"}, "steps": [{"id": "w", "kind": "wait_until", "hour": 9, "timezone_prop": "custom.tz"}, {"id": "t", "kind": "track", "name": "morning"}]}))).await;
    assert_eq!(st, 201);
    let (st, run) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/journeys/{lkey}/enter"),
        Some(json!({"external_id": ext})),
    )
    .await;
    assert_eq!(st, 200, "{run}");
    assert_eq!(run["status"], "active");
    assert_eq!(run["step"], 1);
    assert!(run["wake_at"].is_string());
    let rid = run["id"].as_str().unwrap();
    let (st, x) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/journeys/{lkey}/runs/{rid}/exit"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{x}");
    assert_eq!(x["status"], "exited");
    for key in [&lkey, &jkey, &wkey] {
        let (st, _) = adm(
            reqwest::Method::DELETE,
            format!("/admin/v1/journeys/{key}"),
            None,
        )
        .await;
        assert_eq!(st, 200);
    }
}

/// Journeys v2: lifecycle status, time windows, live events on the player's timeline, revert,
/// nested journeys with cycle protection, timing / per-step stats and identity history.
#[tokio::test]
async fn journeys_v2_flow() {
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

    let fkey = format!("v2flag_{sfx}");
    let (st, _) = adm(
        reqwest::Method::POST,
        "/admin/v1/flags".into(),
        Some(json!({"key": fkey, "value_type": "bool", "default_value": false})),
    )
    .await;
    assert_eq!(st, 201);
    // A live event scheduled far in the future: only a personal window can make it active.
    let lkey = format!("v2event_{sfx}");
    let (st, le) = adm(reqwest::Method::POST, "/admin/v1/live-events".into(), Some(json!({
        "key": lkey, "starts_at": "2099-01-01T00:00:00Z", "duration_secs": 3600, "payload": {"bonus": 2}
    }))).await;
    assert_eq!(st, 201, "{le}");

    // Child journey (manual trigger) entered from the parent.
    let child = format!("v2child_{sfx}");
    let (st, _) = adm(reqwest::Method::POST, "/admin/v1/journeys".into(), Some(json!({
        "key": child, "trigger": {"kind": "manual"}, "steps": [{"id": "t", "kind": "track", "name": "child_done", "value": "1"}]
    }))).await;
    assert_eq!(st, 201);

    // Validation: cron / timezone / revert target.
    let jkey = format!("v2_{sfx}");
    let (st, bad) = adm(
        reqwest::Method::POST,
        "/admin/v1/journeys".into(),
        Some(json!({"key": jkey, "steps": [{"id": "w", "kind": "time_window", "cron": "nope"}]})),
    )
    .await;
    assert_eq!(st, 400, "{bad}");
    let (st, bad) = adm(reqwest::Method::POST, "/admin/v1/journeys".into(), Some(json!({"key": jkey, "steps": [{"id": "w", "kind": "time_window", "cron": "0 0 9 * * Sat", "timezone": "Mars/Olympus"}]}))).await;
    assert_eq!(st, 400, "{bad}");
    let (st, bad) = adm(reqwest::Method::POST, "/admin/v1/journeys".into(), Some(json!({"key": jkey, "steps": [{"id": "m", "kind": "message", "title": "x"}, {"id": "r", "kind": "revert", "step": "m"}]}))).await;
    assert_eq!(st, 400, "{bad}");

    let steps = json!([
        {"id": "window", "kind": "time_window", "at": "2020-01-01T00:00:00Z"},
        {"id": "event", "kind": "live_event", "live_event": lkey, "duration_secs": 600},
        {"id": "flag", "kind": "flag_override", "flag": fkey, "value": true},
        {"id": "undo", "kind": "revert", "step": "flag"},
        {"id": "sub", "kind": "enter_journey", "journey": child}
    ]);
    let (st, j) = adm(reqwest::Method::POST, "/admin/v1/journeys".into(), Some(json!({
        "key": jkey, "trigger": {"kind": "manual"}, "steps": steps, "starts_at": "2020-01-01T00:00:00Z"
    }))).await;
    assert_eq!(st, 201, "{j}");
    assert_eq!(j["status"], "running");

    // Cycle protection: child → parent would close the loop.
    let (st, cyc) = adm(
        reqwest::Method::PUT,
        format!("/admin/v1/journeys/{child}"),
        Some(json!({
            "steps": [{"id": "back", "kind": "enter_journey", "journey": jkey}]
        })),
    )
    .await;
    assert_eq!(st, 400, "{cyc}");
    assert!(cyc.to_string().contains("cycle"), "{cyc}");

    // Scheduled status for a future window.
    let sched = format!("v2sched_{sfx}");
    let (st, sj) = adm(reqwest::Method::POST, "/admin/v1/journeys".into(), Some(json!({
        "key": sched, "steps": [{"id": "w", "kind": "time_window", "cron": "0 0 9 * * Sat", "timezone": "player"}], "starts_at": "2099-01-01T00:00:00Z"
    }))).await;
    assert_eq!(st, 201, "{sj}");
    assert_eq!(sj["status"], "scheduled");

    let ext = format!("bo_{sfx}");
    let s: Value = c
        .post(format!("{base}/v1/authenticate"))
        .bearer_auth(&ck)
        .json(&json!({"id": ext, "default": {"timezone": "Europe/Berlin"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tok = s["token"].as_str().unwrap().to_string();
    let identity_id = s["identity"]["id"].as_str().unwrap().to_string();

    let (st, run) = adm(
        reqwest::Method::POST,
        format!("/admin/v1/journeys/{jkey}/enter"),
        Some(json!({"external_id": ext})),
    )
    .await;
    assert_eq!(st, 200, "{run}");
    let r = wait_runs(&c, &base, &jkey, |r| status_count(r, "done") == 1).await;
    assert_eq!(r["status"], "running");
    assert_eq!(r["timing"]["completed"], 1, "{r}");
    assert!(r["timing"]["avg_secs"].as_f64().is_some());
    for s in r["steps"].as_array().unwrap() {
        assert!(s["passed"].as_i64().unwrap() >= 1, "{s}");
    }
    // The child journey ran too.
    let cr = wait_runs(&c, &base, &child, |r| status_count(r, "done") == 1).await;
    assert_eq!(cr["items"][0]["external_id"], ext);

    // Live event is active for this player despite its 2099 schedule; override was reverted.
    let (st, evs) = call(
        &c,
        &tok,
        reqwest::Method::GET,
        format!("{base}/v1/live-events?names={lkey}"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{evs}");
    let ev = evs["live_events"]
        .as_array()
        .or_else(|| evs["items"].as_array())
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(ev["active"], true, "{evs}");
    assert!(ev["participation_ends_at"].is_string(), "{ev}");
    let (_, fl) = call(
        &c,
        &tok,
        reqwest::Method::GET,
        format!("{base}/v1/flags/{fkey}"),
        None,
    )
    .await;
    assert_eq!(fl["value"], false, "{fl}");

    // Identity history lists both runs with their step logs.
    let (st, hist) = adm(
        reqwest::Method::GET,
        format!("/admin/v1/identities/{identity_id}/journeys"),
        None,
    )
    .await;
    assert_eq!(st, 200, "{hist}");
    let items = hist["items"].as_array().unwrap();
    assert!(items.len() >= 2, "{hist}");
    let parent = items.iter().find(|i| i["journey"] == jkey).unwrap();
    let log = parent["log"].as_array().unwrap();
    assert!(log.len() >= 5, "{parent}");
    assert!(
        log.iter()
            .any(|l| l["detail"].as_str().unwrap().contains("removed override")),
        "{parent}"
    );
    let sub = items.iter().find(|i| i["journey"] == child).unwrap();
    assert_eq!(sub["run"]["context"]["parent"], jkey);
}
