//! Outbound delivery integrations: webhook (HMAC-signed), OneSignal, Facebook app-to-user
//! notifications, and the server's own FCM / APNs push providers.

use std::time::Duration;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use uuid::Uuid;
use zanshin_core::{
    flags::validate_key,
    integrations::{Integration, IntegrationKind},
    properties::PropertyMap,
    Error, Result,
};
use zanshin_store::integrations::IntegrationRepo;

use super::push::PushService;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct IntegrationPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<IntegrationKind>,
    #[serde(default)]
    pub config: Option<Value>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// One rendered message bound for one player.
pub struct Outbound<'a> {
    pub message_key: &'a str,
    pub identity_id: Uuid,
    pub external_id: &'a str,
    pub occurrence: Option<chrono::DateTime<chrono::Utc>>,
    pub locale: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub image_url: &'a str,
    pub payload: &'a Value,
    pub props: &'a PropertyMap,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub channel: String,
    pub status: &'static str,
    pub detail: String,
}

impl Outcome {
    fn sent(channel: &str, detail: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            status: "sent",
            detail: detail.into(),
        }
    }
    fn failed(channel: &str, detail: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            status: "failed",
            detail: detail.into(),
        }
    }
    fn skipped(channel: &str, detail: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            status: "skipped",
            detail: detail.into(),
        }
    }
}

#[derive(Clone)]
pub struct IntegrationService {
    repo: IntegrationRepo,
    http: reqwest::Client,
    push: PushService,
}

impl IntegrationService {
    pub fn new(repo: IntegrationRepo, push: PushService) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        Self { repo, http, push }
    }

    pub async fn list(&self) -> Result<Vec<Integration>> {
        Ok(self
            .repo
            .list()
            .await?
            .iter()
            .map(Integration::masked)
            .collect())
    }

    pub async fn get(&self, key: &str) -> Result<Integration> {
        Ok(self.repo.get(key).await?.masked())
    }

    pub async fn exists(&self, key: &str) -> Result<()> {
        self.repo.get(key).await.map(|_| ())
    }

    pub async fn create(&self, key: &str, p: IntegrationPatch) -> Result<Integration> {
        validate_key(key)?;
        let now = chrono::Utc::now();
        let i = Integration {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            kind: p.kind.ok_or_else(|| Error::invalid("kind is required"))?,
            config: p.config.unwrap_or_else(|| json!({})),
            enabled: p.enabled.unwrap_or(true),
            health: Default::default(),
            version: 1,
            created_at: now,
            updated_at: now,
        };
        i.validate()?;
        Ok(self.repo.create(&i).await?.masked())
    }

    pub async fn update(&self, key: &str, p: IntegrationPatch) -> Result<Integration> {
        let mut i = self.repo.get(key).await?;
        if let Some(v) = p.name {
            i.name = v;
        }
        if let Some(v) = p.kind {
            i.kind = v;
        }
        if let Some(v) = p.config {
            i.config = Integration::merge_config(&i.config, v);
        }
        if let Some(v) = p.enabled {
            i.enabled = v;
        }
        i.validate()?;
        Ok(self.repo.update(&i).await?.masked())
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.repo.delete(key).await
    }

    /// Deliver through the named integration. Never fails hard: every problem becomes an outcome.
    pub async fn deliver(&self, key: &str, out: &Outbound<'_>) -> Outcome {
        let o = self.deliver_inner(key, out).await;
        if o.status != "skipped" {
            if let Err(e) = self.repo.record_outcome(key, o.status, &o.detail).await {
                tracing::warn!(integration = key, error = %e, "integration health not recorded");
            }
        }
        o
    }

    async fn deliver_inner(&self, key: &str, out: &Outbound<'_>) -> Outcome {
        let i = match self.repo.get(key).await {
            Ok(i) => i,
            Err(e) => return Outcome::failed(key, format!("integration unavailable: {e}")),
        };
        if !i.enabled {
            return Outcome::skipped(key, "integration disabled");
        }
        match i.kind {
            IntegrationKind::Webhook => self.webhook(&i, out).await,
            IntegrationKind::Onesignal => self.onesignal(&i, out).await,
            IntegrationKind::Facebook => self.facebook(&i, out).await,
            IntegrationKind::Fcm | IntegrationKind::Apns => {
                if !self.push.enabled() {
                    return Outcome::skipped(key, "no push provider configured on the server");
                }
                let mut data = serde_json::Map::new();
                data.insert(
                    "zanshin_message_key".into(),
                    Value::String(out.message_key.into()),
                );
                if let Some(o) = out.payload.as_object() {
                    for (k, v) in o {
                        data.insert(k.clone(), v.clone());
                    }
                }
                let n = zanshin_core::push::Notification {
                    title: out.title.to_string(),
                    body: out.body.to_string(),
                    image_url: out.image_url.to_string(),
                    data,
                };
                match self.push.notify(out.identity_id, &n).await {
                    0 => Outcome::skipped(key, "no registered devices"),
                    n => Outcome::sent(key, format!("{n} device(s)")),
                }
            }
        }
    }

    fn envelope(out: &Outbound<'_>) -> Value {
        json!({
            "event": "message",
            "message_key": out.message_key,
            "identity_id": out.identity_id,
            "external_id": out.external_id,
            "occurrence": out.occurrence,
            "locale": out.locale,
            "title": out.title,
            "body": out.body,
            "image_url": out.image_url,
            "payload": out.payload,
            "sent_at": chrono::Utc::now(),
        })
    }

    async fn post_json(&self, key: &str, req: reqwest::RequestBuilder, body: Vec<u8>) -> Outcome {
        match req
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
        {
            Ok(res) => {
                let status = res.status();
                let text = res.text().await.unwrap_or_default();
                let snippet: String = text.chars().take(300).collect();
                if status.is_success() {
                    Outcome::sent(key, format!("HTTP {}", status.as_u16()))
                } else {
                    Outcome::failed(key, format!("HTTP {}: {snippet}", status.as_u16()))
                }
            }
            Err(e) => Outcome::failed(key, format!("request failed: {e}")),
        }
    }

    async fn webhook(&self, i: &Integration, out: &Outbound<'_>) -> Outcome {
        let url = i.config["url"].as_str().unwrap_or_default();
        let body = serde_json::to_vec(&Self::envelope(out)).unwrap_or_default();
        let ts = chrono::Utc::now().timestamp().to_string();
        let mut req = self
            .http
            .post(url)
            .header("x-zanshin-event", "message")
            .header("x-zanshin-timestamp", &ts)
            .header("x-zanshin-integration", &i.key);
        if let Some(secret) = i.config["secret"].as_str().filter(|s| !s.is_empty()) {
            req = req.header("x-zanshin-signature", sign(secret, &ts, &body));
        }
        if let Some(h) = i.config["headers"].as_object() {
            for (k, v) in h {
                if let Some(v) = v.as_str() {
                    req = req.header(k.as_str(), v);
                }
            }
        }
        self.post_json(&i.key, req, body).await
    }

    async fn onesignal(&self, i: &Integration, out: &Outbound<'_>) -> Outcome {
        let url = i.config["url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .unwrap_or("https://onesignal.com/api/v1/notifications");
        let api_key = i.config["api_key"].as_str().unwrap_or_default();
        let mut headings = serde_json::Map::new();
        headings.insert("en".into(), Value::String(out.title.into()));
        headings.insert(out.locale.into(), Value::String(out.title.into()));
        let mut contents = serde_json::Map::new();
        contents.insert("en".into(), Value::String(out.body.into()));
        contents.insert(out.locale.into(), Value::String(out.body.into()));
        let mut body = json!({
            "app_id": i.config["app_id"],
            "include_external_user_ids": [out.external_id],
            "channel_for_external_user_ids": "push",
            "headings": headings,
            "contents": contents,
            "data": { "zanshin_message_key": out.message_key, "payload": out.payload },
        });
        if !out.image_url.is_empty() {
            body["big_picture"] = Value::String(out.image_url.into());
        }
        let req = self
            .http
            .post(url)
            .header("authorization", format!("Basic {api_key}"));
        self.post_json(&i.key, req, serde_json::to_vec(&body).unwrap_or_default())
            .await
    }

    async fn facebook(&self, i: &Integration, out: &Outbound<'_>) -> Outcome {
        let fb_id = ["custom.facebook_id", "default.facebook_id", "custom.fb_id"]
            .iter()
            .find_map(|k| {
                out.props
                    .get(*k)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            });
        let Some(fb_id) = fb_id else {
            return Outcome::skipped(&i.key, "player has no facebook_id property");
        };
        let url = match i.config["url"].as_str().filter(|u| !u.is_empty()) {
            Some(u) => u.replace("{user_id}", fb_id),
            None => format!("https://graph.facebook.com/v19.0/{fb_id}/notifications"),
        };
        let template: String = if out.body.is_empty() {
            out.title
        } else {
            out.body
        }
        .chars()
        .take(180)
        .collect();
        let mut body = json!({
            "access_token": i.config["access_token"],
            "template": template,
        });
        if let Some(href) = out.payload.get("href").and_then(|v| v.as_str()) {
            body["href"] = Value::String(href.into());
        }
        let req = self.http.post(&url);
        self.post_json(&i.key, req, serde_json::to_vec(&body).unwrap_or_default())
            .await
    }
}

/// `sha256=<hex>` over `"{timestamp}.{body}"`, the same scheme receivers verify with.
pub fn sign(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    let out = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(7 + out.len() * 2);
    hex.push_str("sha256=");
    for b in out {
        use std::fmt::Write;
        let _ = write!(hex, "{b:02x}");
    }
    hex
}
