//! Fan-out of short operational notices to Slack / Discord / webhook channels.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use zanshin_core::{
    notifications::{ChannelKind, Notice, NotificationChannel},
    Error, Result,
};
use zanshin_store::notifications::NotificationRepo;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChannelPatch {
    pub name: Option<String>,
    pub kind: Option<ChannelKind>,
    pub url: Option<String>,
    pub topics: Option<Vec<String>>,
    pub kinds: Option<Vec<String>>,
    pub enabled: Option<bool>,
    pub headers: Option<Value>,
    pub tags: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct NotificationService {
    repo: NotificationRepo,
    http: reqwest::Client,
    cache: Arc<tokio::sync::RwLock<Option<Vec<NotificationChannel>>>>,
}

fn mask(mut c: NotificationChannel) -> NotificationChannel {
    if let Some(idx) = c.url.rfind('/') {
        let tail = &c.url[idx + 1..];
        if tail.len() > 8 && matches!(c.kind, ChannelKind::Slack | ChannelKind::Discord) {
            c.url = format!("{}/…{}", &c.url[..idx], &tail[tail.len() - 4..]);
        }
    }
    c
}

impl NotificationService {
    pub fn new(repo: NotificationRepo) -> Self {
        Self {
            repo,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("http client"),
            cache: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    pub async fn list(&self) -> Result<Vec<NotificationChannel>> {
        Ok(self.repo.list().await?.into_iter().map(mask).collect())
    }

    pub async fn get(&self, key: &str) -> Result<NotificationChannel> {
        Ok(mask(self.repo.get(key).await?))
    }

    async fn invalidate(&self) {
        *self.cache.write().await = None;
    }

    pub async fn create(&self, key: &str, p: ChannelPatch) -> Result<NotificationChannel> {
        let now = chrono::Utc::now();
        let c = NotificationChannel {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            kind: p.kind.ok_or_else(|| Error::invalid("kind is required"))?,
            url: p.url.unwrap_or_default(),
            topics: p.topics.unwrap_or_default(),
            kinds: p.kinds.unwrap_or_default(),
            enabled: p.enabled.unwrap_or(true),
            headers: p.headers.unwrap_or_else(|| json!({})),
            last_status: None,
            last_error: String::new(),
            last_sent_at: None,
            tags: p.tags.unwrap_or_default(),
            version: 1,
            created_at: now,
            updated_at: now,
        };
        c.validate()?;
        let out = self.repo.create(&c).await?;
        self.invalidate().await;
        Ok(mask(out))
    }

    pub async fn update(&self, key: &str, p: ChannelPatch) -> Result<NotificationChannel> {
        let mut c = self.repo.get(key).await?;
        if let Some(v) = p.name {
            c.name = v;
        }
        if let Some(v) = p.kind {
            c.kind = v;
        }
        if let Some(v) = p.url.filter(|u| !u.contains('…')) {
            c.url = v;
        }
        if let Some(v) = p.topics {
            c.topics = v;
        }
        if let Some(v) = p.kinds {
            c.kinds = v;
        }
        if let Some(v) = p.enabled {
            c.enabled = v;
        }
        if let Some(v) = p.headers {
            c.headers = v;
        }
        if let Some(v) = p.tags {
            c.tags = v;
        }
        c.validate()?;
        let out = self.repo.update(&c).await?;
        self.invalidate().await;
        Ok(mask(out))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.repo.delete(key).await?;
        self.invalidate().await;
        Ok(())
    }

    /// Send a test notice to one channel and return the delivery outcome.
    pub async fn test(&self, key: &str) -> Result<Value> {
        let c = self.repo.get(key).await?;
        let n = Notice::new(
            "test",
            "Zanshin test notification",
            format!("Channel `{}` is wired up.", c.key),
        )
        .field("channel", c.key.clone());
        let (status, err) = self.send_one(&c, &n).await;
        self.repo.mark(c.id, status, &err).await.ok();
        Ok(json!({ "ok": err.is_empty(), "status": status, "error": err }))
    }

    async fn channels(&self) -> Vec<NotificationChannel> {
        if let Some(c) = self.cache.read().await.as_ref() {
            return c.clone();
        }
        let list = self.repo.list().await.unwrap_or_default();
        *self.cache.write().await = Some(list.clone());
        list
    }

    /// Fire-and-forget fan-out; never blocks or fails the caller.
    pub fn notify(&self, n: Notice) {
        let me = self.clone();
        tokio::spawn(async move {
            for c in me.channels().await {
                if !c.wants(&n.topic, n.kind.as_deref()) {
                    continue;
                }
                let (status, err) = me.send_one(&c, &n).await;
                metrics::counter!("zanshin_notifications_total", "channel" => c.key.clone(), "ok" => err.is_empty().to_string())
                    .increment(1);
                if let Err(e) = me.repo.mark(c.id, status, &err).await {
                    tracing::warn!(error = %e, "notification status not recorded");
                }
            }
        });
    }

    /// Synchronous delivery to the named channels (or every channel subscribed to the topic when
    /// `keys` is empty); returns `(channel, ok, error)` per channel.
    pub async fn send_to(&self, keys: &[String], n: &Notice) -> Vec<(String, bool, String)> {
        let mut out = Vec::new();
        for c in self.channels().await {
            let wanted = if keys.is_empty() {
                c.wants(&n.topic, n.kind.as_deref())
            } else {
                keys.contains(&c.key)
            };
            if !wanted {
                continue;
            }
            let (status, err) = self.send_one(&c, n).await;
            if let Err(e) = self.repo.mark(c.id, status, &err).await {
                tracing::warn!(error = %e, "notification status not recorded");
            }
            out.push((c.key.clone(), err.is_empty(), err));
        }
        out
    }

    pub fn payload(c: &NotificationChannel, n: &Notice) -> Value {
        let fields = n
            .fields
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(" · ");
        match c.kind {
            ChannelKind::Slack => {
                let mut blocks = vec![
                    json!({"type": "header", "text": {"type": "plain_text", "text": n.title.chars().take(150).collect::<String>()}}),
                    json!({"type": "section", "text": {"type": "mrkdwn", "text": n.text.chars().take(2900).collect::<String>()}}),
                ];
                if !fields.is_empty() {
                    blocks.push(json!({"type": "context", "elements": [{"type": "mrkdwn", "text": fields}]}));
                }
                json!({ "text": format!("{} — {}", n.title, n.text), "blocks": blocks })
            }
            ChannelKind::Discord => {
                let embed = json!({
                    "title": n.title.chars().take(256).collect::<String>(),
                    "description": n.text.chars().take(4000).collect::<String>(),
                    "timestamp": n.at.to_rfc3339(),
                    "fields": n.fields.iter().take(25).map(|(k, v)| json!({"name": k, "value": v.chars().take(1024).collect::<String>(), "inline": true})).collect::<Vec<_>>(),
                    "color": match n.topic.as_str() { "export.failed" | "integration.unhealthy" | "alert" => 0xd9534f, "config.deleted" => 0xf0ad4e, _ => 0x1a5fb4 },
                });
                json!({ "username": "Zanshin", "embeds": [embed] })
            }
            ChannelKind::Webhook => json!({
                "topic": n.topic, "title": n.title, "text": n.text, "kind": n.kind,
                "fields": n.fields.iter().map(|(k, v)| json!({"name": k, "value": v})).collect::<Vec<_>>(),
                "at": n.at.to_rfc3339(),
            }),
        }
    }

    async fn send_one(&self, c: &NotificationChannel, n: &Notice) -> (Option<i32>, String) {
        let mut req = self.http.post(&c.url).json(&Self::payload(c, n));
        if let Some(h) = c.headers.as_object() {
            for (k, v) in h {
                if let Some(v) = v.as_str() {
                    req = req.header(k.as_str(), v);
                }
            }
        }
        match req.send().await {
            Ok(res) => {
                let st = res.status();
                if st.is_success() {
                    (Some(st.as_u16() as i32), String::new())
                } else {
                    let body = res.text().await.unwrap_or_default();
                    (
                        Some(st.as_u16() as i32),
                        format!(
                            "status {st}: {}",
                            body.chars().take(200).collect::<String>()
                        ),
                    )
                }
            }
            Err(e) => (None, e.to_string()),
        }
    }
}
