//! In-app messages: campaign catalog, inbox materialisation, direct sends.

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use zanshin_core::{
    flags::validate_key,
    messages::{InboxMessage, MessageCampaign},
    properties::PropertyMap,
    rules::Condition,
    Error, Identity, Result,
};
use zanshin_store::messages::{Delivery, MessageRepo};

use super::{
    bus::{ChangeBus, ChangeKind},
    catalog::{Catalog, CatalogItem, CatalogSource},
    flags::normalize_tags,
};

impl CatalogItem for MessageCampaign {
    fn key(&self) -> &str {
        &self.key
    }
    fn version(&self) -> i64 {
        self.version
    }
}

#[async_trait]
impl CatalogSource<MessageCampaign> for MessageRepo {
    async fn list(&self) -> Result<Vec<MessageCampaign>> {
        MessageRepo::list(self).await
    }
    async fn get(&self, key: &str) -> Result<MessageCampaign> {
        MessageRepo::get(self, key).await
    }
}

fn double_option<'de, T, D>(d: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MessagePatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub payload: Option<Value>,
    #[serde(default)]
    pub condition: Option<Condition>,
    #[serde(default)]
    pub audiences: Option<Vec<String>>,
    #[serde(default)]
    pub percentage: Option<f64>,
    #[serde(default, deserialize_with = "double_option")]
    pub starts_at: Option<Option<chrono::DateTime<chrono::Utc>>>,
    #[serde(default, deserialize_with = "double_option")]
    pub ends_at: Option<Option<chrono::DateTime<chrono::Utc>>>,
    #[serde(default, deserialize_with = "double_option")]
    pub ttl_secs: Option<Option<i64>>,
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default)]
    pub push: Option<bool>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// A direct (server-sent) message.
#[derive(Debug, Clone, Deserialize)]
pub struct DirectMessage {
    pub title: String,
    /// Also send a push notification.
    #[serde(default)]
    pub push: bool,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub image_url: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub ttl_secs: Option<i64>,
}

#[derive(Clone)]
pub struct MessageService {
    repo: MessageRepo,
    bus: ChangeBus,
    push: super::push::PushService,
    pub catalog: Arc<Catalog<MessageCampaign>>,
}

impl MessageService {
    pub fn new(repo: MessageRepo, bus: ChangeBus, push: super::push::PushService) -> Self {
        let catalog = Catalog::new(ChangeKind::Message, "message", Arc::new(repo.clone()));
        Self {
            repo,
            bus,
            push,
            catalog,
        }
    }

    fn notification(
        title: &str,
        body: &str,
        image_url: &str,
        inbox_id: Uuid,
        key: Option<&str>,
    ) -> zanshin_core::push::Notification {
        let mut data = serde_json::Map::new();
        data.insert(
            "zanshin_message_id".into(),
            Value::String(inbox_id.to_string()),
        );
        if let Some(k) = key {
            data.insert("zanshin_message_key".into(), Value::String(k.to_string()));
        }
        zanshin_core::push::Notification {
            title: title.to_string(),
            body: body.to_string(),
            image_url: image_url.to_string(),
            data,
        }
    }

    async fn push_copy(
        &self,
        identity_id: Uuid,
        inbox_id: Uuid,
        n: &zanshin_core::push::Notification,
    ) {
        if self.push.notify(identity_id, n).await > 0 {
            let _ = self.repo.mark_pushed(inbox_id).await;
        }
    }

    /// Deliver one campaign to one identity if it targets it and was not delivered yet.
    /// Returns the new inbox id.
    async fn deliver_campaign(
        &self,
        m: &MessageCampaign,
        identity: &Identity,
        flat: &PropertyMap,
        audiences: &HashSet<String>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<Uuid>> {
        let member = |k: &str| audiences.contains(k);
        if !m.targets(&identity.external_id, flat, &member) {
            return Ok(None);
        }
        let title = MessageCampaign::render(&m.title, flat);
        let body = MessageCampaign::render(&m.body, flat);
        let expires_at = m.ttl_secs.map(|t| now + chrono::Duration::seconds(t));
        let inserted = self
            .repo
            .deliver(
                identity.id,
                Delivery {
                    message_id: Some(m.id),
                    key: Some(&m.key),
                    title: &title,
                    body: &body,
                    image_url: &m.image_url,
                    payload: &m.payload,
                    priority: m.priority,
                    expires_at,
                },
            )
            .await?;
        if let Some(id) = inserted {
            metrics::counter!("zanshin_messages_delivered_total", "message" => m.key.clone())
                .increment(1);
            if m.push {
                self.push_copy(
                    identity.id,
                    id,
                    &Self::notification(&title, &body, &m.image_url, id, Some(&m.key)),
                )
                .await;
            }
        }
        Ok(inserted)
    }

    /// Proactively deliver live push campaigns to every identity with a registered device, so the
    /// notification goes out without waiting for the client to open its inbox. Returns deliveries.
    pub async fn sweep_push_campaigns(
        &self,
        eval: &super::eval::Evaluator,
        identities: &zanshin_store::identities::IdentityRepo,
    ) -> Result<usize> {
        let now = chrono::Utc::now();
        let campaigns: Vec<_> = self
            .catalog
            .list()
            .await
            .into_iter()
            .filter(|m| m.push && m.is_live(now))
            .collect();
        if campaigns.is_empty() || !self.push.enabled() {
            return Ok(0);
        }
        let mut delivered = 0;
        let mut after = None;
        loop {
            let ids = self.push.identities_with_tokens(after, 500).await?;
            let Some(last) = ids.last().copied() else {
                break;
            };
            for id in ids {
                let Ok(identity) = identities.get(id).await else {
                    continue;
                };
                let flat = super::eval::flatten_with_identity(&identity, &identity.properties);
                let audiences = eval
                    .audiences
                    .memberships(&identity.external_id, &flat)
                    .await;
                let done: HashSet<Uuid> = self
                    .repo
                    .delivered_campaigns(identity.id)
                    .await?
                    .into_iter()
                    .collect();
                for m in campaigns.iter().filter(|m| !done.contains(&m.id)) {
                    if self
                        .deliver_campaign(m, &identity, &flat, &audiences, now)
                        .await?
                        .is_some()
                    {
                        delivered += 1;
                    }
                }
            }
            after = Some(last);
        }
        if delivered > 0 {
            tracing::info!(delivered, "push campaigns swept");
        }
        Ok(delivered)
    }

    pub async fn list(&self) -> Vec<Arc<MessageCampaign>> {
        self.catalog.list().await
    }

    pub async fn get(&self, key: &str) -> Result<Arc<MessageCampaign>> {
        self.catalog.get(key).await
    }

    /// Deliver every live campaign that targets the identity and has not been delivered yet, then
    /// return the inbox.
    pub async fn inbox(
        &self,
        identity: &Identity,
        flat: &PropertyMap,
        audiences: &HashSet<String>,
        limit: i64,
    ) -> Result<Vec<InboxMessage>> {
        let now = chrono::Utc::now();
        let live: Vec<_> = self
            .catalog
            .list()
            .await
            .into_iter()
            .filter(|m| m.is_live(now))
            .collect();
        if !live.is_empty() {
            let delivered: HashSet<Uuid> = self
                .repo
                .delivered_campaigns(identity.id)
                .await?
                .into_iter()
                .collect();
            for m in live.iter().filter(|m| !delivered.contains(&m.id)) {
                self.deliver_campaign(m, identity, flat, audiences, now)
                    .await?;
            }
        }
        self.repo.inbox(identity.id, limit).await
    }

    pub async fn mark_read(&self, identity: &Identity, id: Uuid) -> Result<InboxMessage> {
        self.repo.mark_read(identity.id, id).await
    }

    pub async fn consume(&self, identity: &Identity, id: Uuid) -> Result<()> {
        self.repo.consume(identity.id, id).await?;
        metrics::counter!("zanshin_messages_consumed_total").increment(1);
        Ok(())
    }

    /// Server / admin: drop a one-off message into one identity's inbox.
    pub async fn send_direct(&self, identity: &Identity, msg: DirectMessage) -> Result<Uuid> {
        if msg.title.trim().is_empty() || msg.title.len() > 512 {
            return Err(Error::invalid("title must be 1..512 chars"));
        }
        if msg.ttl_secs.is_some_and(|t| t <= 0) {
            return Err(Error::invalid("ttl_secs must be positive"));
        }
        let flat = identity.properties.flatten();
        let title = MessageCampaign::render(&msg.title, &flat);
        let body = MessageCampaign::render(&msg.body, &flat);
        let expires_at = msg
            .ttl_secs
            .map(|t| chrono::Utc::now() + chrono::Duration::seconds(t));
        let id = self
            .repo
            .deliver(
                identity.id,
                Delivery {
                    message_id: None,
                    key: None,
                    title: &title,
                    body: &body,
                    image_url: &msg.image_url,
                    payload: &msg.payload,
                    priority: msg.priority,
                    expires_at,
                },
            )
            .await?
            .ok_or_else(|| Error::internal(anyhow::anyhow!("direct message not inserted")))?;
        if msg.push {
            self.push_copy(
                identity.id,
                id,
                &Self::notification(&title, &body, &msg.image_url, id, None),
            )
            .await;
        }
        let mut c = self.bus.change(ChangeKind::Message, "direct", 0);
        c.identity_id = Some(identity.id);
        self.bus.publish(c).await;
        metrics::counter!("zanshin_messages_delivered_total", "message" => "direct").increment(1);
        Ok(id)
    }

    pub async fn stats(&self, key: &str) -> Result<(Arc<MessageCampaign>, (i64, i64, i64))> {
        let m = self.get(key).await?;
        let s = self.repo.campaign_stats(m.id).await?;
        Ok((m, s))
    }

    pub async fn create(&self, key: &str, p: MessagePatch) -> Result<MessageCampaign> {
        validate_key(key)?;
        let now = chrono::Utc::now();
        let m = MessageCampaign {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            enabled: p.enabled.unwrap_or(true),
            title: p.title.ok_or_else(|| Error::invalid("title is required"))?,
            body: p.body.unwrap_or_default(),
            image_url: p.image_url.unwrap_or_default(),
            payload: p.payload.unwrap_or(Value::Null),
            condition: p.condition.unwrap_or_else(Condition::always),
            audiences: p.audiences.unwrap_or_default(),
            percentage: p.percentage.unwrap_or(100.0),
            starts_at: p.starts_at.flatten(),
            ends_at: p.ends_at.flatten(),
            ttl_secs: p.ttl_secs.flatten(),
            priority: p.priority.unwrap_or(0),
            push: p.push.unwrap_or(false),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        m.validate()?;
        let created = self.repo.create(&m).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::Message, &created.key, created.version),
            )
            .await;
        Ok(created)
    }

    pub async fn update(&self, key: &str, p: MessagePatch) -> Result<MessageCampaign> {
        let mut m = self.repo.get(key).await?;
        if let Some(v) = p.name {
            m.name = v;
        }
        if let Some(v) = p.enabled {
            m.enabled = v;
        }
        if let Some(v) = p.title {
            m.title = v;
        }
        if let Some(v) = p.body {
            m.body = v;
        }
        if let Some(v) = p.image_url {
            m.image_url = v;
        }
        if let Some(v) = p.payload {
            m.payload = v;
        }
        if let Some(v) = p.condition {
            m.condition = v;
        }
        if let Some(v) = p.audiences {
            m.audiences = v;
        }
        if let Some(v) = p.percentage {
            m.percentage = v;
        }
        if let Some(v) = p.starts_at {
            m.starts_at = v;
        }
        if let Some(v) = p.ends_at {
            m.ends_at = v;
        }
        if let Some(v) = p.ttl_secs {
            m.ttl_secs = v;
        }
        if let Some(v) = p.priority {
            m.priority = v;
        }
        if let Some(v) = p.push {
            m.push = v;
        }
        if let Some(v) = p.tags {
            m.tags = normalize_tags(v)?;
        }
        m.validate()?;
        let updated = self.repo.update(&m).await?;
        self.bus
            .publish(
                self.bus
                    .change(ChangeKind::Message, &updated.key, updated.version),
            )
            .await;
        Ok(updated)
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.repo.delete(key).await?;
        let mut c = self.bus.change(ChangeKind::Message, key, 0);
        c.deleted = true;
        self.bus.publish(c).await;
        Ok(())
    }

    pub async fn purge_expired(&self) -> Result<u64> {
        self.repo.purge_expired().await
    }
}
