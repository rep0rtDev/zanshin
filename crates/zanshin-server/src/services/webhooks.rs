//! Outbound webhooks: subscriptions matched against system events, delivered signed, batched and
//! retried with exponential backoff. The worker runs inside the server process (one per replica);
//! deliveries are persisted so retries survive restarts.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex, RwLock};
use uuid::Uuid;
use zanshin_core::{
    flags::validate_key,
    webhooks::{default_batch_size, default_events, default_max_attempts, Webhook, WebhookEvent},
    Error, Result,
};
use zanshin_store::webhooks::{Delivery, WebhookRepo};

use super::{bus::ChangeBus, flags::normalize_tags, integrations::sign};

/// In-process fan-out of things that happened; every service with something to say sends here.
pub type WebhookBus = broadcast::Sender<WebhookEvent>;

pub fn new_bus() -> WebhookBus {
    broadcast::channel(8192).0
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WebhookPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub events: Option<Vec<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub batch_size: Option<i32>,
    #[serde(default)]
    pub batch_window_secs: Option<i32>,
    #[serde(default)]
    pub headers: Option<Value>,
    #[serde(default)]
    pub max_attempts: Option<i32>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

struct Buffer {
    events: Vec<WebhookEvent>,
    first_at: Instant,
}

#[derive(Clone)]
pub struct WebhookService {
    repo: WebhookRepo,
    http: reqwest::Client,
    bus: WebhookBus,
    cache: Arc<RwLock<Vec<Webhook>>>,
    pending: Arc<Mutex<HashMap<Uuid, Buffer>>>,
}

const RETRY_TICK: Duration = Duration::from_secs(5);
const CACHE_REFRESH: Duration = Duration::from_secs(30);

impl WebhookService {
    /// Builds the service and spawns its worker (event matching, batching, retries).
    pub fn new(repo: WebhookRepo, bus: WebhookBus, changes: ChangeBus) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client");
        let svc = Self {
            repo,
            http,
            bus: bus.clone(),
            cache: Arc::new(RwLock::new(Vec::new())),
            pending: Arc::new(Mutex::new(HashMap::new())),
        };
        let worker = svc.clone();
        tokio::spawn(async move { worker.run(bus.subscribe(), changes.subscribe()).await });
        svc
    }

    pub fn bus(&self) -> &WebhookBus {
        &self.bus
    }

    /// Emit an event to every matching subscription.
    pub fn emit(&self, event_type: impl Into<String>, data: Value) {
        let _ = self.bus.send(WebhookEvent::new(event_type, data));
    }

    pub async fn reload(&self) -> Result<()> {
        let all = self.repo.list().await?;
        *self.cache.write().await = all;
        Ok(())
    }

    async fn run(
        self,
        mut events: broadcast::Receiver<WebhookEvent>,
        mut changes: broadcast::Receiver<super::bus::Change>,
    ) {
        if let Err(e) = self.reload().await {
            tracing::warn!(error = %e, "webhook catalog load failed");
        }
        let mut tick = tokio::time::interval(RETRY_TICK);
        let mut refresh = tokio::time::interval(CACHE_REFRESH);
        loop {
            tokio::select! {
                ev = events.recv() => match ev {
                    Ok(ev) => self.enqueue(ev).await,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        metrics::counter!("zanshin_webhook_events_dropped_total").increment(n);
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                c = changes.recv() => match c {
                    Ok(c) if c.identity_id.is_none() => {
                        let kind = format!("{:?}", c.kind).to_lowercase();
                        let kind = match kind.as_str() { "liveevent" => "live_event".to_string(), k => k.to_string() };
                        let action = if c.deleted { "deleted" } else { "updated" };
                        self.enqueue(WebhookEvent::new(
                            format!("{kind}.{action}"),
                            json!({ "key": c.key, "version": c.version }),
                        ))
                        .await;
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                _ = tick.tick() => {
                    self.flush_expired().await;
                    self.retry_due().await;
                }
                _ = refresh.tick() => {
                    let _ = self.reload().await;
                }
            }
        }
    }

    async fn enqueue(&self, ev: WebhookEvent) {
        let hooks = self.cache.read().await;
        let targets: Vec<Webhook> = hooks
            .iter()
            .filter(|w| w.enabled && w.matches(&ev.event_type))
            .cloned()
            .collect();
        drop(hooks);
        for w in targets {
            let flush_now = {
                let mut pending = self.pending.lock().await;
                let buf = pending.entry(w.id).or_insert_with(|| Buffer {
                    events: Vec::new(),
                    first_at: Instant::now(),
                });
                buf.events.push(ev.clone());
                buf.events.len() >= w.batch_size as usize || w.batch_window_secs == 0
            };
            if flush_now {
                self.flush(&w).await;
            }
        }
    }

    async fn flush(&self, w: &Webhook) {
        let events = {
            let mut pending = self.pending.lock().await;
            match pending.remove(&w.id) {
                Some(b) if !b.events.is_empty() => b.events,
                _ => return,
            }
        };
        if let Err(e) = self.dispatch(w, events).await {
            tracing::warn!(webhook = %w.key, error = %e, "webhook dispatch failed");
        }
    }

    async fn flush_expired(&self) {
        let hooks = self.cache.read().await.clone();
        let expired: Vec<Uuid> = {
            let pending = self.pending.lock().await;
            pending
                .iter()
                .filter(|(id, b)| {
                    hooks.iter().find(|w| w.id == **id).is_none_or(|w| {
                        b.first_at.elapsed() >= Duration::from_secs(w.batch_window_secs as u64)
                    })
                })
                .map(|(id, _)| *id)
                .collect()
        };
        for id in expired {
            match hooks.iter().find(|w| w.id == id) {
                Some(w) => self.flush(w).await,
                None => {
                    self.pending.lock().await.remove(&id);
                }
            }
        }
    }

    /// Persist a delivery for `events` and try it once right away.
    async fn dispatch(&self, w: &Webhook, events: Vec<WebhookEvent>) -> Result<Delivery> {
        let types: Vec<String> = events.iter().map(|e| e.event_type.clone()).collect();
        let payload = json!({ "webhook": w.key, "events": events });
        let d = self.repo.insert_delivery(w.id, &types, &payload).await?;
        self.attempt(w, d).await
    }

    async fn attempt(&self, w: &Webhook, d: Delivery) -> Result<Delivery> {
        let mut body = d.payload.clone();
        body["delivery_id"] = json!(d.id);
        body["attempt"] = json!(d.attempts + 1);
        body["sent_at"] = json!(chrono::Utc::now());
        let bytes = serde_json::to_vec(&body).unwrap_or_default();
        let ts = chrono::Utc::now().timestamp().to_string();
        let event_header = if d.event_types.len() == 1 {
            d.event_types[0].clone()
        } else {
            "batch".to_string()
        };
        let mut req = self
            .http
            .post(&w.url)
            .header("content-type", "application/json")
            .header("user-agent", "zanshin-webhooks/1")
            .header("x-zanshin-event", event_header)
            .header("x-zanshin-delivery", d.id.to_string())
            .header("x-zanshin-webhook", &w.key)
            .header("x-zanshin-timestamp", &ts);
        if !w.secret.is_empty() {
            req = req.header("x-zanshin-signature", sign(&w.secret, &ts, &bytes));
        }
        if let Some(h) = w.headers.as_object() {
            for (k, v) in h {
                if let Some(v) = v.as_str() {
                    req = req.header(k.as_str(), v);
                }
            }
        }
        let attempts = d.attempts + 1;
        let (ok, status, err) = match req.body(bytes).send().await {
            Ok(res) => {
                let st = res.status();
                let text = res.text().await.unwrap_or_default();
                let snippet: String = text.chars().take(300).collect();
                (
                    st.is_success(),
                    Some(st.as_u16() as i32),
                    if st.is_success() {
                        String::new()
                    } else {
                        format!("HTTP {}: {snippet}", st.as_u16())
                    },
                )
            }
            Err(e) => (false, None, format!("request failed: {e}")),
        };
        metrics::counter!("zanshin_webhook_deliveries_total", "webhook" => w.key.clone(), "outcome" => if ok { "sent" } else { "failed" })
            .increment(1);
        if ok {
            return self
                .repo
                .mark_delivery(d.id, "sent", attempts, None, status, "")
                .await;
        }
        if attempts >= w.max_attempts {
            return self
                .repo
                .mark_delivery(d.id, "dead", attempts, None, status, &err)
                .await;
        }
        let backoff = Duration::from_secs((5u64 << (attempts.min(10) as u32)).min(3600));
        let next = chrono::Utc::now() + chrono::Duration::from_std(backoff).unwrap_or_default();
        self.repo
            .mark_delivery(d.id, "failed", attempts, Some(next), status, &err)
            .await
    }

    async fn retry_due(&self) {
        let due = match self.repo.due(chrono::Utc::now(), 100).await {
            Ok(d) => d,
            Err(e) => {
                tracing::debug!(error = %e, "webhook retry query failed");
                return;
            }
        };
        if due.is_empty() {
            return;
        }
        let hooks = self.cache.read().await.clone();
        for d in due {
            let Some(w) = hooks.iter().find(|w| w.id == d.webhook_id) else {
                let _ = self
                    .repo
                    .mark_delivery(
                        d.id,
                        "dead",
                        d.attempts,
                        None,
                        d.last_status,
                        "webhook removed",
                    )
                    .await;
                continue;
            };
            if !w.enabled {
                continue;
            }
            let _ = self.attempt(w, d).await;
        }
    }

    // ---- admin surface

    pub async fn list(&self) -> Result<Vec<Webhook>> {
        Ok(self
            .repo
            .list()
            .await?
            .iter()
            .map(Webhook::masked)
            .collect())
    }

    pub async fn get(&self, key: &str) -> Result<Webhook> {
        Ok(self.repo.get(key).await?.masked())
    }

    pub async fn create(&self, key: &str, p: WebhookPatch) -> Result<Webhook> {
        validate_key(key)?;
        let now = chrono::Utc::now();
        let w = Webhook {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            url: p.url.ok_or_else(|| Error::invalid("url is required"))?,
            secret: p.secret.unwrap_or_default(),
            events: p.events.unwrap_or_else(default_events),
            enabled: p.enabled.unwrap_or(true),
            batch_size: p.batch_size.unwrap_or_else(default_batch_size),
            batch_window_secs: p.batch_window_secs.unwrap_or(0),
            headers: p.headers.unwrap_or_else(|| json!({})),
            max_attempts: p.max_attempts.unwrap_or_else(default_max_attempts),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        w.validate()?;
        let created = self.repo.create(&w).await?;
        self.reload().await?;
        Ok(created.masked())
    }

    pub async fn update(&self, key: &str, p: WebhookPatch) -> Result<Webhook> {
        let mut w = self.repo.get(key).await?;
        if let Some(v) = p.name {
            w.name = v;
        }
        if let Some(v) = p.url {
            w.url = v;
        }
        if let Some(v) = p.secret {
            if v != "••••" {
                w.secret = v;
            }
        }
        if let Some(v) = p.events {
            w.events = v;
        }
        if let Some(v) = p.enabled {
            w.enabled = v;
        }
        if let Some(v) = p.batch_size {
            w.batch_size = v;
        }
        if let Some(v) = p.batch_window_secs {
            w.batch_window_secs = v;
        }
        if let Some(v) = p.headers {
            w.headers = v;
        }
        if let Some(v) = p.max_attempts {
            w.max_attempts = v;
        }
        if let Some(v) = p.tags {
            w.tags = normalize_tags(v)?;
        }
        w.validate()?;
        let updated = self.repo.update(&w).await?;
        self.reload().await?;
        Ok(updated.masked())
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.repo.delete(key).await?;
        self.reload().await
    }

    /// Send a `webhook.test` event synchronously and return the delivery outcome.
    pub async fn test(&self, key: &str) -> Result<Delivery> {
        let w = self.repo.get(key).await?;
        let ev = WebhookEvent::new(
            "webhook.test",
            json!({ "webhook": w.key, "message": "Zanshin webhook test" }),
        );
        self.dispatch(&w, vec![ev]).await
    }

    pub async fn deliveries(
        &self,
        key: &str,
        status: Option<&str>,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<Delivery>, Vec<(String, i64)>)> {
        let w = self.repo.get(key).await?;
        let items = self
            .repo
            .deliveries(w.id, status, after, limit.clamp(1, 500))
            .await?;
        let stats = self.repo.delivery_stats(w.id).await?;
        Ok((items, stats))
    }

    pub async fn retry(&self, key: &str, id: Uuid) -> Result<Delivery> {
        let w = self.repo.get(key).await?;
        let d = self.repo.get_delivery(w.id, id).await?;
        if d.status == "sent" {
            return Err(Error::invalid("delivery already sent"));
        }
        // A manual retry gets a fresh attempt budget from here.
        let d = self
            .repo
            .mark_delivery(d.id, "pending", 0, None, d.last_status, &d.last_error)
            .await?;
        self.attempt(&w, d).await
    }

    pub async fn purge(&self, keep: chrono::Duration) -> Result<u64> {
        self.repo.purge_deliveries(chrono::Utc::now() - keep).await
    }
}
