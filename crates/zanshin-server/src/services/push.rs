//! Push notifications: token registry, providers (FCM HTTP v1, APNs), delivery.

use std::{collections::VecDeque, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;
use zanshin_core::{
    push::{validate_token, Notification, Platform, PushToken},
    Error, Result,
};
use zanshin_store::push::PushTokenRepo;

use crate::config::PushConfig;

#[derive(Debug)]
pub enum SendError {
    /// Token is dead; drop it.
    InvalidToken,
    /// Try again later.
    Transient(String),
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn platform(&self) -> Platform;
    async fn send(&self, token: &str, n: &Notification) -> std::result::Result<(), SendError>;
}

/// A sent notification as recorded by the `log` provider (dev/test).
#[derive(Debug, Clone, Serialize)]
pub struct OutboxEntry {
    pub at: chrono::DateTime<chrono::Utc>,
    pub platform: Platform,
    pub token: String,
    pub notification: Notification,
}

struct LogProvider {
    platform: Platform,
    outbox: Arc<Mutex<VecDeque<OutboxEntry>>>,
}

#[async_trait]
impl Provider for LogProvider {
    fn platform(&self) -> Platform {
        self.platform
    }
    async fn send(&self, token: &str, n: &Notification) -> std::result::Result<(), SendError> {
        let mut o = self.outbox.lock().await;
        if o.len() >= 1000 {
            o.pop_front();
        }
        o.push_back(OutboxEntry {
            at: chrono::Utc::now(),
            platform: self.platform,
            token: token.to_string(),
            notification: n.clone(),
        });
        Ok(())
    }
}

// ---- FCM HTTP v1 ------------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct ServiceAccount {
    client_email: String,
    private_key: String,
}

struct FcmProvider {
    http: reqwest::Client,
    project_id: String,
    account: ServiceAccount,
    access: RwLock<Option<(String, chrono::DateTime<chrono::Utc>)>>,
}

#[derive(Serialize)]
struct GoogleClaims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    iat: i64,
    exp: i64,
}

impl FcmProvider {
    async fn access_token(&self) -> std::result::Result<String, SendError> {
        if let Some((t, exp)) = self.access.read().await.clone() {
            if exp > chrono::Utc::now() + chrono::Duration::minutes(1) {
                return Ok(t);
            }
        }
        let now = chrono::Utc::now().timestamp();
        let claims = GoogleClaims {
            iss: &self.account.client_email,
            scope: "https://www.googleapis.com/auth/firebase.messaging",
            aud: "https://oauth2.googleapis.com/token",
            iat: now,
            exp: now + 3600,
        };
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(self.account.private_key.as_bytes())
            .map_err(|e| SendError::Transient(format!("fcm key: {e}")))?;
        let assertion = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
            &claims,
            &key,
        )
        .map_err(|e| SendError::Transient(format!("fcm jwt: {e}")))?;
        #[derive(serde::Deserialize)]
        struct Tok {
            access_token: String,
            expires_in: i64,
        }
        let res = self
            .http
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ])
            .send()
            .await
            .map_err(|e| SendError::Transient(format!("fcm oauth: {e}")))?;
        if !res.status().is_success() {
            return Err(SendError::Transient(format!(
                "fcm oauth status {}",
                res.status()
            )));
        }
        let tok: Tok = res
            .json()
            .await
            .map_err(|e| SendError::Transient(format!("fcm oauth body: {e}")))?;
        let exp = chrono::Utc::now() + chrono::Duration::seconds(tok.expires_in);
        *self.access.write().await = Some((tok.access_token.clone(), exp));
        Ok(tok.access_token)
    }
}

#[async_trait]
impl Provider for FcmProvider {
    fn platform(&self) -> Platform {
        Platform::Fcm
    }
    async fn send(&self, token: &str, n: &Notification) -> std::result::Result<(), SendError> {
        let access = self.access_token().await?;
        let data: serde_json::Map<String, serde_json::Value> = n
            .data
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    serde_json::Value::String(match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    }),
                )
            })
            .collect();
        let mut notification = serde_json::json!({ "title": n.title, "body": n.body });
        if !n.image_url.is_empty() {
            notification["image"] = serde_json::Value::String(n.image_url.clone());
        }
        let mut body = serde_json::json!({ "message": { "token": token, "notification": notification, "data": data } });
        apply_fcm_options(&mut body, n);
        let res = self
            .http
            .post(format!(
                "https://fcm.googleapis.com/v1/projects/{}/messages:send",
                self.project_id
            ))
            .bearer_auth(access)
            .json(&body)
            .send()
            .await
            .map_err(|e| SendError::Transient(e.to_string()))?;
        match res.status().as_u16() {
            200..=299 => Ok(()),
            404 | 400 => {
                let text = res.text().await.unwrap_or_default();
                if text.contains("UNREGISTERED")
                    || text.contains("INVALID_ARGUMENT") && text.contains("token")
                {
                    Err(SendError::InvalidToken)
                } else {
                    Err(SendError::Transient(text))
                }
            }
            s => Err(SendError::Transient(format!("fcm status {s}"))),
        }
    }
}

// ---- APNs -------------------------------------------------------------------------------------

struct ApnsProvider {
    http: reqwest::Client,
    host: &'static str,
    bundle_id: String,
    team_id: String,
    key_id: String,
    key: jsonwebtoken::EncodingKey,
    jwt: RwLock<Option<(String, chrono::DateTime<chrono::Utc>)>>,
}

impl ApnsProvider {
    async fn provider_token(&self) -> std::result::Result<String, SendError> {
        if let Some((t, at)) = self.jwt.read().await.clone() {
            if at > chrono::Utc::now() - chrono::Duration::minutes(40) {
                return Ok(t);
            }
        }
        #[derive(Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            iat: i64,
        }
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        let now = chrono::Utc::now();
        let t = jsonwebtoken::encode(
            &header,
            &Claims {
                iss: &self.team_id,
                iat: now.timestamp(),
            },
            &self.key,
        )
        .map_err(|e| SendError::Transient(format!("apns jwt: {e}")))?;
        *self.jwt.write().await = Some((t.clone(), now));
        Ok(t)
    }
}

#[async_trait]
impl Provider for ApnsProvider {
    fn platform(&self) -> Platform {
        Platform::Apns
    }
    async fn send(&self, token: &str, n: &Notification) -> std::result::Result<(), SendError> {
        let jwt = self.provider_token().await?;
        let mut body = serde_json::json!({ "aps": { "alert": { "title": n.title, "body": n.body }, "sound": "default", "mutable-content": 1 } });
        apply_apns_options(&mut body, n);
        for (k, v) in &n.data {
            body[k] = v.clone();
        }
        if !n.image_url.is_empty() {
            body["image_url"] = serde_json::Value::String(n.image_url.clone());
        }
        let res = self
            .http
            .post(format!("https://{}/3/device/{token}", self.host))
            .header("authorization", format!("bearer {jwt}"))
            .header("apns-topic", &self.bundle_id)
            .header("apns-push-type", "alert")
            .header("apns-priority", "10")
            .json(&body)
            .send()
            .await
            .map_err(|e| SendError::Transient(e.to_string()))?;
        match res.status().as_u16() {
            200 => Ok(()),
            400 | 410 => {
                let text = res.text().await.unwrap_or_default();
                if text.contains("BadDeviceToken")
                    || text.contains("Unregistered")
                    || text.contains("DeviceTokenNotForTopic")
                {
                    Err(SendError::InvalidToken)
                } else {
                    Err(SendError::Transient(text))
                }
            }
            s => Err(SendError::Transient(format!("apns status {s}"))),
        }
    }
}

// ---- service ----------------------------------------------------------------------------------

#[derive(Clone)]
pub struct PushService {
    repo: PushTokenRepo,
    providers: Arc<Vec<Arc<dyn Provider>>>,
    outbox: Arc<Mutex<VecDeque<OutboxEntry>>>,
    mode: &'static str,
}

impl PushService {
    pub fn new(repo: PushTokenRepo, cfg: &PushConfig) -> anyhow::Result<Self> {
        let outbox = Arc::new(Mutex::new(VecDeque::new()));
        let mut providers: Vec<Arc<dyn Provider>> = Vec::new();
        let mode = match cfg.provider.as_str() {
            "" | "none" => "none",
            "log" => {
                providers.push(Arc::new(LogProvider {
                    platform: Platform::Fcm,
                    outbox: outbox.clone(),
                }));
                providers.push(Arc::new(LogProvider {
                    platform: Platform::Apns,
                    outbox: outbox.clone(),
                }));
                "log"
            }
            "live" => {
                let http = reqwest::Client::builder()
                    .timeout(Duration::from_secs(10))
                    .build()?;
                if !cfg.fcm.service_account_file.is_empty() {
                    let raw = std::fs::read_to_string(&cfg.fcm.service_account_file)?;
                    let account: ServiceAccount = serde_json::from_str(&raw)?;
                    anyhow::ensure!(
                        !cfg.fcm.project_id.is_empty(),
                        "push.fcm.project_id is required"
                    );
                    providers.push(Arc::new(FcmProvider {
                        http: http.clone(),
                        project_id: cfg.fcm.project_id.clone(),
                        account,
                        access: RwLock::new(None),
                    }));
                }
                if !cfg.apns.key_file.is_empty() {
                    let pem = std::fs::read(&cfg.apns.key_file)?;
                    let key = jsonwebtoken::EncodingKey::from_ec_pem(&pem)?;
                    anyhow::ensure!(
                        !cfg.apns.team_id.is_empty()
                            && !cfg.apns.key_id.is_empty()
                            && !cfg.apns.bundle_id.is_empty(),
                        "push.apns.team_id, key_id and bundle_id are required"
                    );
                    providers.push(Arc::new(ApnsProvider {
                        http,
                        host: if cfg.apns.sandbox {
                            "api.sandbox.push.apple.com"
                        } else {
                            "api.push.apple.com"
                        },
                        bundle_id: cfg.apns.bundle_id.clone(),
                        team_id: cfg.apns.team_id.clone(),
                        key_id: cfg.apns.key_id.clone(),
                        key,
                        jwt: RwLock::new(None),
                    }));
                }
                anyhow::ensure!(
                    !providers.is_empty(),
                    "push.provider = live needs fcm and/or apns configured"
                );
                "live"
            }
            other => anyhow::bail!("unknown push.provider {other:?}"),
        };
        Ok(Self {
            repo,
            providers: Arc::new(providers),
            outbox,
            mode,
        })
    }

    pub fn mode(&self) -> &'static str {
        self.mode
    }

    pub fn enabled(&self) -> bool {
        !self.providers.is_empty()
    }

    pub async fn register(
        &self,
        identity_id: Uuid,
        platform: Platform,
        token: &str,
        device_id: &str,
    ) -> Result<PushToken> {
        validate_token(platform, token)?;
        if device_id.len() > 128 {
            return Err(Error::invalid("device_id too long"));
        }
        let t = self
            .repo
            .upsert(identity_id, platform, token.trim(), device_id)
            .await?;
        metrics::counter!("zanshin_push_tokens_registered_total", "platform" => platform.as_str())
            .increment(1);
        Ok(t)
    }

    pub async fn unregister(&self, identity_id: Uuid, token: &str) -> Result<()> {
        self.repo.delete(identity_id, token).await
    }

    pub async fn tokens(&self, identity_id: Uuid) -> Result<Vec<PushToken>> {
        self.repo.for_identity(identity_id).await
    }

    pub async fn identities_with_tokens(
        &self,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<Uuid>> {
        self.repo.identities_with_tokens(after, limit).await
    }

    pub async fn stats(&self) -> Result<(i64, i64)> {
        self.repo.count().await
    }

    pub async fn outbox(&self, limit: usize) -> Vec<OutboxEntry> {
        self.outbox
            .lock()
            .await
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Send to every device of the identity. Returns how many sends succeeded.
    pub async fn notify(&self, identity_id: Uuid, n: &Notification) -> usize {
        if self.providers.is_empty() {
            return 0;
        }
        let tokens = match self.repo.for_identity(identity_id).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "push tokens lookup failed");
                return 0;
            }
        };
        let mut sent = 0;
        for t in tokens {
            let Some(p) = self.providers.iter().find(|p| p.platform() == t.platform) else {
                continue;
            };
            match p.send(&t.token, n).await {
                Ok(()) => {
                    sent += 1;
                    metrics::counter!("zanshin_push_sent_total", "platform" => t.platform.as_str())
                        .increment(1);
                    let _ = self.repo.record_success(t.platform, &t.token).await;
                }
                Err(SendError::InvalidToken) => {
                    metrics::counter!("zanshin_push_failed_total", "platform" => t.platform.as_str(), "reason" => "invalid_token").increment(1);
                    let _ = self.repo.remove_invalid(t.platform, &t.token).await;
                }
                Err(SendError::Transient(msg)) => {
                    metrics::counter!("zanshin_push_failed_total", "platform" => t.platform.as_str(), "reason" => "transient").increment(1);
                    tracing::warn!(platform = t.platform.as_str(), error = %msg, "push send failed");
                    let _ = self.repo.record_failure(t.platform, &t.token).await;
                }
            }
        }
        sent
    }
}

/// Rich push extras → FCM v1 message fields.
fn apply_fcm_options(body: &mut serde_json::Value, n: &Notification) {
    let Some(o) = &n.options else { return };
    let msg = &mut body["message"];
    let mut android = serde_json::Map::new();
    if !o.sound.is_empty() {
        android.insert("sound".into(), serde_json::Value::String(o.sound.clone()));
    }
    if !o.channel_id.is_empty() {
        android.insert(
            "channel_id".into(),
            serde_json::Value::String(o.channel_id.clone()),
        );
    }
    if !o.deep_link.is_empty() {
        android.insert(
            "click_action".into(),
            serde_json::Value::String(o.deep_link.clone()),
        );
    }
    if !android.is_empty() {
        msg["android"]["notification"] = serde_json::Value::Object(android);
    }
    if !o.collapse_key.is_empty() {
        msg["android"]["collapse_key"] = serde_json::Value::String(o.collapse_key.clone());
    }
    if !o.actions.is_empty() {
        msg["data"]["zanshin_actions"] =
            serde_json::Value::String(serde_json::to_string(&o.actions).unwrap_or_default());
    }
    if !o.deep_link.is_empty() {
        msg["data"]["zanshin_deep_link"] = serde_json::Value::String(o.deep_link.clone());
    }
    if let Some(b) = o.badge {
        msg["apns"]["payload"]["aps"]["badge"] = serde_json::Value::from(b);
    }
}

/// Rich push extras → APNs payload (category = first action id, mutable-content for actions/images).
fn apply_apns_options(body: &mut serde_json::Value, n: &Notification) {
    let Some(o) = &n.options else { return };
    if !o.sound.is_empty() {
        body["aps"]["sound"] = serde_json::Value::String(o.sound.clone());
    }
    if let Some(b) = o.badge {
        body["aps"]["badge"] = serde_json::Value::from(b);
    }
    if let Some(first) = o.actions.first() {
        body["aps"]["category"] = serde_json::Value::String(first.id.clone());
        body["aps"]["mutable-content"] = serde_json::Value::from(1);
        body["zanshin_actions"] = serde_json::to_value(&o.actions).unwrap_or_default();
    }
    if !o.deep_link.is_empty() {
        body["zanshin_deep_link"] = serde_json::Value::String(o.deep_link.clone());
    }
}
