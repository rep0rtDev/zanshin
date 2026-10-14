//! Configuration as data: export bundles, diff, import (merge / replace), history snapshots and undo.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use zanshin_core::{
    export::{diff_fields, ConfigSnapshot, VOLATILE},
    Error, Result,
};

use super::bus::ChangeKind;
use crate::state::AppState;

pub const BUNDLE_FORMAT: &str = "zanshin-config/1";

/// Entity kinds in a bundle, in dependency order (templates before messages, metrics before experiments…).
pub const KINDS: &[&str] = &[
    "event_definitions",
    "metrics",
    "templates",
    "integrations",
    "flags",
    "audiences",
    "experiments",
    "live_events",
    "messages",
    "webhooks",
    "journeys",
    "dashboards",
    "export_sinks",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    #[default]
    Merge,
    Replace,
}

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    pub bundle: Value,
    #[serde(default)]
    pub mode: ImportMode,
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
    /// Only these entity keys (any kind); `None` = everything in the selected kinds.
    #[serde(default)]
    pub keys: Option<Vec<String>>,
    #[serde(default)]
    pub dry_run: bool,
}

fn key_of(v: &Value) -> Option<String> {
    v.get("key")
        .or_else(|| v.get("name"))
        .and_then(|k| k.as_str())
        .map(|s| s.to_string())
}

fn strip_volatile(mut v: Value) -> Value {
    if let Some(m) = v.as_object_mut() {
        for k in VOLATILE {
            m.remove(*k);
        }
    }
    v
}

fn to_json<T: serde::Serialize>(items: Vec<T>) -> Vec<Value> {
    items
        .into_iter()
        .filter_map(|i| serde_json::to_value(i).ok())
        .collect()
}

/// Current entities of one kind as JSON objects keyed by `key` (or `name` for event definitions).
pub async fn list_kind(state: &AppState, kind: &str) -> Result<Vec<Value>> {
    Ok(match kind {
        "event_definitions" => to_json(
            state
                .taxonomy
                .repo
                .list_event_definitions()
                .await?
                .into_iter()
                .filter(|d| !d.is_core)
                .collect(),
        ),
        "metrics" => to_json(state.metrics.list().await?),
        "templates" => to_json(state.templates.list().await?),
        "integrations" => to_json(state.integrations.list().await?),
        "flags" => to_json(state.flags.list().await),
        "audiences" => to_json(state.audiences.list().await),
        "experiments" => to_json(state.experiments.list().await),
        "live_events" => to_json(state.live_events.list().await),
        "messages" => to_json(state.messages.list().await),
        "webhooks" => to_json(state.webhooks.list().await?),
        "journeys" => to_json(state.journeys.list().await?),
        "dashboards" => to_json(state.analytics.list_dashboards().await?),
        "export_sinks" => to_json(state.export.list().await?),
        other => return Err(Error::invalid(format!("unknown config kind {other}"))),
    })
}

pub async fn get_entity(state: &AppState, kind: &str, key: &str) -> Result<Option<Value>> {
    Ok(list_kind(state, kind)
        .await?
        .into_iter()
        .find(|v| key_of(v).as_deref() == Some(key)))
}

/// Create or update an entity from its JSON representation (unknown / volatile fields are ignored).
pub async fn apply(state: &AppState, kind: &str, key: &str, body: &Value) -> Result<Value> {
    let exists = get_entity(state, kind, key).await?.is_some();
    macro_rules! upsert {
        ($svc:expr, $patch:ty) => {{
            let p: $patch = serde_json::from_value(body.clone())
                .map_err(|e| Error::invalid(format!("{kind} {key}: {e}")))?;
            if exists {
                serde_json::to_value($svc.update(key, p).await?)
            } else {
                serde_json::to_value($svc.create(key, p).await?)
            }
        }};
    }
    let out = match kind {
        "event_definitions" => {
            let p: zanshin_store::taxonomy::EventDefinitionInput =
                serde_json::from_value(body.clone())
                    .map_err(|e| Error::invalid(format!("{kind} {key}: {e}")))?;
            let d = state.taxonomy.repo.upsert_event_definition(key, p).await?;
            state.taxonomy.invalidate();
            serde_json::to_value(d)
        }
        "metrics" => upsert!(state.metrics, super::metrics::MetricPatch),
        "templates" => upsert!(state.templates, super::templates::TemplatePatch),
        "integrations" => upsert!(state.integrations, super::integrations::IntegrationPatch),
        "flags" => {
            let p: super::flags::FlagPatch = serde_json::from_value(body.clone())
                .map_err(|e| Error::invalid(format!("{kind} {key}: {e}")))?;
            if exists {
                serde_json::to_value(state.flags.update(key, p.into()).await?)
            } else {
                serde_json::to_value(state.flags.create(key, p.into()).await?)
            }
        }
        "audiences" => upsert!(state.audiences, super::audiences::AudiencePatch),
        "experiments" => upsert!(state.experiments, super::experiments::ExperimentPatch),
        "live_events" => upsert!(state.live_events, super::live_events::LiveEventPatch),
        "messages" => upsert!(state.messages, super::messages::MessagePatch),
        "webhooks" => upsert!(state.webhooks, super::webhooks::WebhookPatch),
        "journeys" => upsert!(state.journeys, super::journeys::JourneyPatch),
        "dashboards" => {
            let p: super::analytics::DashboardPatch = serde_json::from_value(body.clone())
                .map_err(|e| Error::invalid(format!("{kind} {key}: {e}")))?;
            if exists {
                serde_json::to_value(state.analytics.update_dashboard(key, p).await?)
            } else {
                serde_json::to_value(state.analytics.create_dashboard(key, p).await?)
            }
        }
        "export_sinks" => upsert!(state.export, super::export::SinkPatch),
        other => return Err(Error::invalid(format!("unknown config kind {other}"))),
    }
    .map_err(|e| Error::internal(anyhow::anyhow!(e)))?;
    record(state, kind, key, Some(&out), false, "import").await;
    Ok(out)
}

pub async fn remove(state: &AppState, kind: &str, key: &str) -> Result<()> {
    match kind {
        "event_definitions" => {
            state.taxonomy.repo.delete_event_definition(key).await?;
            state.taxonomy.invalidate();
        }
        "metrics" => state.metrics.delete(key).await?,
        "templates" => state.templates.delete(key).await?,
        "integrations" => state.integrations.delete(key).await?,
        "flags" => state.flags.delete(key).await?,
        "audiences" => state.audiences.delete(key).await?,
        "experiments" => state.experiments.delete(key).await?,
        "live_events" => state.live_events.delete(key).await?,
        "messages" => state.messages.delete(key).await?,
        "webhooks" => state.webhooks.delete(key).await?,
        "journeys" => state.journeys.delete(key).await?,
        "dashboards" => state.analytics.delete_dashboard(key).await?,
        "export_sinks" => state.export.delete(key).await?,
        other => return Err(Error::invalid(format!("unknown config kind {other}"))),
    }
    record(state, kind, key, None, true, "import").await;
    Ok(())
}

async fn record(
    state: &AppState,
    kind: &str,
    key: &str,
    body: Option<&Value>,
    deleted: bool,
    actor: &str,
) {
    let version = body
        .and_then(|b| b.get("version"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if let Err(e) = state
        .config_history
        .insert(kind, key, version, body, deleted, actor)
        .await
    {
        tracing::warn!(error = %e, kind, key, "config history write failed");
    }
}

fn key_selected(keys: Option<&[String]>, v: &Value) -> bool {
    match keys {
        None => true,
        Some(ks) => key_of(v).is_some_and(|k| ks.contains(&k)),
    }
}

/// Export the configuration, optionally narrowed to some entity `kinds` and/or specific `keys`.
pub async fn export_bundle(
    state: &AppState,
    kinds: Option<&[String]>,
    keys: Option<&[String]>,
) -> Result<Value> {
    let mut entities = serde_json::Map::new();
    for kind in KINDS {
        if kinds.is_some_and(|k| !k.iter().any(|x| x == kind)) {
            continue;
        }
        let items: Vec<Value> = list_kind(state, kind)
            .await?
            .into_iter()
            .filter(|v| key_selected(keys, v))
            .map(strip_volatile)
            .collect();
        entities.insert(kind.to_string(), Value::Array(items));
    }
    Ok(json!({ "format": BUNDLE_FORMAT, "exported_at": Utc::now(), "entities": entities }))
}

fn bundle_entities(bundle: &Value) -> Result<&serde_json::Map<String, Value>> {
    if bundle.get("format").and_then(|f| f.as_str()) != Some(BUNDLE_FORMAT) {
        return Err(Error::invalid(format!(
            "bundle format must be {BUNDLE_FORMAT}"
        )));
    }
    bundle
        .get("entities")
        .and_then(|e| e.as_object())
        .ok_or_else(|| Error::invalid("bundle.entities must be an object"))
}

/// Compare a bundle with the current configuration.
pub async fn diff(
    state: &AppState,
    bundle: &Value,
    mode: ImportMode,
    kinds: Option<&[String]>,
    keys: Option<&[String]>,
) -> Result<Value> {
    let entities = bundle_entities(bundle)?;
    let mut out = serde_json::Map::new();
    let mut totals = (0usize, 0usize, 0usize, 0usize);
    for kind in KINDS {
        if kinds.is_some_and(|k| !k.iter().any(|x| x == kind)) {
            continue;
        }
        let Some(incoming) = entities.get(*kind).and_then(|v| v.as_array()) else {
            continue;
        };
        let incoming: Vec<Value> = incoming
            .iter()
            .filter(|v| key_selected(keys, v))
            .cloned()
            .collect();
        let incoming = &incoming;
        let local: BTreeMap<String, Value> = list_kind(state, kind)
            .await?
            .into_iter()
            .filter_map(|v| key_of(&v).map(|k| (k, v)))
            .collect();
        let mut added = vec![];
        let mut changed = vec![];
        let mut unchanged = 0usize;
        let mut seen = std::collections::HashSet::new();
        for item in incoming {
            let Some(key) = key_of(item) else { continue };
            seen.insert(key.clone());
            match local.get(&key) {
                None => added.push(json!({ "key": key })),
                Some(cur) => {
                    let fields = diff_fields(cur, item);
                    if fields.is_empty() {
                        unchanged += 1;
                    } else {
                        changed.push(json!({ "key": key, "fields": fields }));
                    }
                }
            }
        }
        let removed: Vec<Value> = if mode == ImportMode::Replace {
            local
                .keys()
                .filter(|k| !seen.contains(*k))
                .map(|k| json!({ "key": k }))
                .collect()
        } else {
            vec![]
        };
        totals.0 += added.len();
        totals.1 += changed.len();
        totals.2 += removed.len();
        totals.3 += unchanged;
        out.insert(kind.to_string(), json!({ "added": added, "changed": changed, "removed": removed, "unchanged": unchanged }));
    }
    Ok(
        json!({ "kinds": out, "summary": { "added": totals.0, "changed": totals.1, "removed": totals.2, "unchanged": totals.3 } }),
    )
}

/// Apply a bundle. Returns the diff that was (or would be, with `dry_run`) applied plus per-entity errors.
pub async fn import(state: &AppState, req: &ImportRequest) -> Result<Value> {
    let kinds = req.kinds.as_deref();
    let plan = diff(state, &req.bundle, req.mode, kinds, req.keys.as_deref()).await?;
    if req.dry_run {
        return Ok(json!({ "dry_run": true, "plan": plan }));
    }
    let entities = bundle_entities(&req.bundle)?;
    let mut applied = 0usize;
    let mut errors = vec![];
    for kind in KINDS {
        let Some(k) = plan["kinds"].get(*kind) else {
            continue;
        };
        let items: BTreeMap<String, &Value> = entities[*kind]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| key_of(v).map(|k| (k, v)))
            .collect();
        for e in k["added"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(k["changed"].as_array().into_iter().flatten())
        {
            let key = e["key"].as_str().unwrap_or_default();
            if let Some(body) = items.get(key) {
                match apply(state, kind, key, body).await {
                    Ok(_) => applied += 1,
                    Err(err) => {
                        errors.push(json!({ "kind": kind, "key": key, "error": err.to_string() }))
                    }
                }
            }
        }
        for e in k["removed"].as_array().into_iter().flatten() {
            let key = e["key"].as_str().unwrap_or_default();
            match remove(state, kind, key).await {
                Ok(_) => applied += 1,
                Err(err) => {
                    errors.push(json!({ "kind": kind, "key": key, "error": err.to_string() }))
                }
            }
        }
    }
    Ok(json!({ "dry_run": false, "plan": plan, "applied": applied, "errors": errors }))
}

/// Fetch a bundle from another Zanshin server and import it.
pub async fn import_from(
    state: &AppState,
    url: &str,
    admin_key: &str,
    mode: ImportMode,
    kinds: Option<Vec<String>>,
    dry_run: bool,
) -> Result<Value> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(Error::invalid("url must be http(s)"));
    }
    let endpoint = format!("{}/admin/v1/config/export", url.trim_end_matches('/'));
    let res = reqwest::Client::new()
        .get(&endpoint)
        .bearer_auth(admin_key)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| Error::invalid(format!("remote: {e}")))?;
    if !res.status().is_success() {
        return Err(Error::invalid(format!("remote status {}", res.status())));
    }
    let bundle: Value = res
        .json()
        .await
        .map_err(|e| Error::invalid(format!("remote: {e}")))?;
    import(
        state,
        &ImportRequest {
            bundle,
            mode,
            kinds,
            keys: None,
            dry_run,
        },
    )
    .await
}

// ---- history / undo

pub async fn history(
    state: &AppState,
    kind: Option<&str>,
    key: Option<&str>,
    limit: i64,
) -> Result<Vec<ConfigSnapshot>> {
    state
        .config_history
        .list(kind, key, limit.clamp(1, 500))
        .await
}

/// Restore an entity to how it looked in a snapshot (or delete it if the snapshot records a deletion).
pub async fn revert(state: &AppState, snapshot_id: uuid::Uuid) -> Result<Value> {
    let snap = state.config_history.get(snapshot_id).await?;
    restore(state, &snap).await
}

async fn restore(state: &AppState, snap: &ConfigSnapshot) -> Result<Value> {
    match (&snap.body, snap.deleted) {
        (Some(body), false) => {
            let v = apply(state, &snap.kind, &snap.key, body).await?;
            Ok(json!({ "action": "restored", "kind": snap.kind, "key": snap.key, "entity": v }))
        }
        _ => {
            match remove(state, &snap.kind, &snap.key).await {
                Ok(()) | Err(Error::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
            Ok(json!({ "action": "deleted", "kind": snap.kind, "key": snap.key }))
        }
    }
}

/// Map an admin API path to a (kind, key) pair, e.g. `/admin/v1/flags/new_ui` → (`flags`, `new_ui`).
pub fn entity_from_path(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix("/admin/v1/")?;
    let mut parts = rest.split('/');
    let seg = parts.next()?;
    let key = parts.next()?;
    let kind = match seg {
        "flags" | "audiences" | "experiments" | "live-events" | "messages" | "webhooks"
        | "journeys" | "dashboards" | "templates" | "integrations" | "metrics" | "export-sinks" => {
            seg.replace('-', "_")
        }
        "taxonomy" if key == "events" => {
            let name = parts.next()?;
            return Some(("event_definitions".into(), name.to_string()));
        }
        _ => return None,
    };
    Some((kind, key.to_string()))
}

/// Undo the change recorded by an audit entry: restore the entity to its last snapshot before that entry.
pub async fn undo_audit(state: &AppState, audit_id: uuid::Uuid) -> Result<Value> {
    let entry = state.audit.get(audit_id).await?;
    if !(200..300).contains(&entry.status)
        || !matches!(entry.method.as_str(), "PUT" | "PATCH" | "DELETE" | "POST")
    {
        return Err(Error::invalid("only successful mutations can be undone"));
    }
    let (kind, key) = match entity_from_path(&entry.path) {
        Some(x) => x,
        None if entry.method == "POST" => {
            let key = entry
                .body
                .as_ref()
                .and_then(|b| b.get("key").or(b.get("name")))
                .and_then(|k| k.as_str())
                .map(|s| s.to_string());
            let seg = entry
                .path
                .strip_prefix("/admin/v1/")
                .unwrap_or_default()
                .split('/')
                .next()
                .unwrap_or_default()
                .replace('-', "_");
            match key {
                Some(k) if KINDS.contains(&seg.as_str()) => (seg, k),
                _ => {
                    return Err(Error::invalid(
                        "this audit entry does not reference a configuration entity",
                    ))
                }
            }
        }
        None => {
            return Err(Error::invalid(
                "this audit entry does not reference a configuration entity",
            ))
        }
    };
    let before: DateTime<Utc> = entry.at;
    // The audit row is written after the response, so the snapshot of the change itself may precede `entry.at`:
    // skip snapshots that already look like the current state.
    let current = get_entity(state, &kind, &key).await?.map(strip_volatile);
    let candidates = state
        .config_history
        .list(Some(&kind), Some(&key), 200)
        .await?;
    let prior =
        candidates
            .into_iter()
            .filter(|s| s.at <= before)
            .find(|s| match (&s.body, &current) {
                (Some(b), Some(c)) => &strip_volatile(b.clone()) != c,
                (None, None) => false,
                _ => true,
            });
    match prior {
        Some(snap) => restore(state, &snap).await,
        None => {
            // Nothing recorded before the change: the entry created the entity.
            if entry.method == "POST" || entry.method == "PUT" {
                match remove(state, &kind, &key).await {
                    Ok(()) | Err(Error::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
                Ok(json!({ "action": "deleted", "kind": kind, "key": key }))
            } else {
                Err(Error::invalid("no earlier snapshot of this entity exists"))
            }
        }
    }
}

/// Snapshot configuration entities as they change (fed by the change bus).
pub fn spawn_history(state: AppState) {
    let mut rx = state.bus.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(c) if c.identity_id.is_none() => {
                    let kind = match c.kind {
                        ChangeKind::Flag => "flags",
                        ChangeKind::Audience => "audiences",
                        ChangeKind::Experiment => "experiments",
                        ChangeKind::LiveEvent => "live_events",
                        ChangeKind::Message => "messages",
                    };
                    if c.deleted {
                        record(&state, kind, &c.key, None, true, "").await;
                    } else {
                        // Let the originating replica finish its own write before reading back.
                        match get_entity(&state, kind, &c.key).await {
                            Ok(Some(body)) => {
                                record(&state, kind, &c.key, Some(&body), false, "").await
                            }
                            Ok(None) => {}
                            Err(e) => tracing::debug!(error = %e, "history snapshot skipped"),
                        }
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}
