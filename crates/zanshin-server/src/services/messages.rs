//! In-app messages: campaign catalog, inbox materialisation, direct sends.

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use zanshin_core::templates::Template;
use zanshin_core::{
    flags::validate_key,
    messages::{default_channels, Schedule},
    messages::{InboxMessage, MessageCampaign},
    properties::PropertyMap,
    rules::Condition,
    Error, Identity, Result,
};
use zanshin_store::{
    live_events::LiveEventRepo,
    messages::{Delivery, MessageRepo},
    templates::TemplateRepo,
};

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
    #[serde(default, deserialize_with = "double_option")]
    pub template_key: Option<Option<String>>,
    #[serde(default)]
    pub schedule: Option<Schedule>,
    #[serde(default)]
    pub channels: Option<Vec<String>>,
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
    templates: TemplateRepo,
    integrations: super::integrations::IntegrationService,
    live_events: LiveEventRepo,
    pub catalog: Arc<Catalog<MessageCampaign>>,
}

impl MessageService {
    pub fn new(
        repo: MessageRepo,
        bus: ChangeBus,
        push: super::push::PushService,
        templates: TemplateRepo,
        integrations: super::integrations::IntegrationService,
        live_events: LiveEventRepo,
    ) -> Self {
        let catalog = Catalog::new(ChangeKind::Message, "message", Arc::new(repo.clone()));
        Self {
            repo,
            bus,
            push,
            templates,
            integrations,
            live_events,
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

    /// Delivery occurrence of a campaign at `now`: `Some(None)` for one-shot campaigns, the fire
    /// time for cron / live-event schedules, `None` when nothing is due.
    async fn occurrence_of(
        &self,
        m: &MessageCampaign,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<Option<chrono::DateTime<chrono::Utc>>> {
        match m.schedule.live_event_key() {
            None => m.occurrence(now),
            Some(key) => {
                let ev = self.live_events.get(key).await.ok()?;
                let occ = ev.current(now).or_else(|| {
                    // The end-anchored window of the previous occurrence may still be open.
                    ev.previous(now)
                })?;
                m.schedule
                    .event_occurrence(occ.start, occ.end, now)
                    .map(Some)
            }
        }
    }

    /// Campaigns from `live` that still have an undelivered occurrence for this identity.
    async fn pending<'a>(
        &self,
        live: &'a [Arc<MessageCampaign>],
        identity_id: Uuid,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<
        Vec<(
            &'a Arc<MessageCampaign>,
            Option<chrono::DateTime<chrono::Utc>>,
        )>,
    > {
        let done: HashSet<(Uuid, Option<chrono::DateTime<chrono::Utc>>)> = self
            .repo
            .delivered_occurrences(identity_id)
            .await?
            .into_iter()
            .collect();
        let mut out = Vec::new();
        for m in live {
            if let Some(occ) = self.occurrence_of(m, now).await {
                if !done.contains(&(m.id, occ)) {
                    out.push((m, occ));
                }
            }
        }
        Ok(out)
    }

    /// Deliver one campaign occurrence to one identity if it targets it and was not delivered yet.
    /// Returns the new inbox id.
    async fn deliver_campaign(
        &self,
        m: &MessageCampaign,
        identity: &Identity,
        flat: &PropertyMap,
        audiences: &HashSet<String>,
        now: chrono::DateTime<chrono::Utc>,
        occurrence: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Option<Uuid>> {
        let member = |k: &str| audiences.contains(k);
        if !m.targets(&identity.external_id, flat, &member) {
            return Ok(None);
        }
        let locale_pref = Template::locale_of(flat);
        let (title, body, image_url, payload, locale) = match &m.template_key {
            Some(tk) => match self.templates.get(tk).await {
                Ok(t) => {
                    let r = t.render(locale_pref.as_deref(), flat);
                    let payload = if m.payload.is_null() {
                        t.payload.clone()
                    } else {
                        m.payload.clone()
                    };
                    let image = if m.image_url.is_empty() {
                        t.image_url.clone()
                    } else {
                        m.image_url.clone()
                    };
                    (r.title, r.body, image, payload, r.locale)
                }
                Err(e) => {
                    tracing::warn!(message = %m.key, template = %tk, error = %e, "template missing; using inline copy");
                    (
                        MessageCampaign::render(&m.title, flat),
                        MessageCampaign::render(&m.body, flat),
                        m.image_url.clone(),
                        m.payload.clone(),
                        locale_pref.clone().unwrap_or_default(),
                    )
                }
            },
            None => (
                MessageCampaign::render(&m.title, flat),
                MessageCampaign::render(&m.body, flat),
                m.image_url.clone(),
                m.payload.clone(),
                locale_pref.clone().unwrap_or_default(),
            ),
        };
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
                    image_url: &image_url,
                    payload: &payload,
                    priority: m.priority,
                    expires_at,
                    occurrence,
                },
            )
            .await?;
        if let Some(id) = inserted {
            metrics::counter!("zanshin_messages_delivered_total", "message" => m.key.clone())
                .increment(1);
            let _ = self
                .repo
                .record_delivery(m.id, identity.id, occurrence, "inbox", "sent", "")
                .await;
            if m.push {
                self.push_copy(
                    identity.id,
                    id,
                    &Self::notification(&title, &body, &image_url, id, Some(&m.key)),
                )
                .await;
            }
            let out = super::integrations::Outbound {
                message_key: &m.key,
                identity_id: identity.id,
                external_id: &identity.external_id,
                occurrence,
                locale: &locale,
                title: &title,
                body: &body,
                image_url: &image_url,
                payload: &payload,
                props: flat,
            };
            for channel in m.channels.iter().filter(|c| c.as_str() != "inbox") {
                let o = self.integrations.deliver(channel, &out).await;
                metrics::counter!("zanshin_messages_channel_total", "channel" => channel.clone(), "status" => o.status)
                    .increment(1);
                let _ = self
                    .repo
                    .record_delivery(m.id, identity.id, occurrence, channel, o.status, &o.detail)
                    .await;
            }
        }
        Ok(inserted)
    }

    /// Deliver due cron / live-event campaigns to every identity (housekeeping, once a minute).
    pub async fn sweep_scheduled(
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
            .filter(|m| m.schedule != Schedule::Once && m.is_live(now))
            .collect();
        if campaigns.is_empty() {
            return Ok(0);
        }
        let mut due = Vec::new();
        for m in campaigns {
            if self.occurrence_of(&m, now).await.is_some() {
                due.push(m);
            }
        }
        if due.is_empty() {
            return Ok(0);
        }
        let mut delivered = 0;
        let mut after = None;
        loop {
            let page = identities.list(after, 500).await?;
            let Some(last) = page.last().map(|i| i.id) else {
                break;
            };
            for identity in &page {
                let flat = super::eval::flatten_with_identity(identity, &identity.properties);
                let audiences = eval.audiences.memberships(identity, &flat).await;
                for (m, occ) in self.pending(&due, identity.id, now).await? {
                    if self
                        .deliver_campaign(m, identity, &flat, &audiences, now, occ)
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
            tracing::info!(delivered, "scheduled campaigns swept");
        }
        Ok(delivered)
    }

    async fn check_refs(&self, m: &MessageCampaign) -> Result<()> {
        if let Some(t) = &m.template_key {
            self.templates
                .get(t)
                .await
                .map_err(|_| Error::invalid(format!("template {t:?} does not exist")))?;
        }
        for c in m.channels.iter().filter(|c| c.as_str() != "inbox") {
            self.integrations
                .exists(c)
                .await
                .map_err(|_| Error::invalid(format!("integration {c:?} does not exist")))?;
        }
        if let Some(k) = m.schedule.live_event_key() {
            self.live_events
                .get(k)
                .await
                .map_err(|_| Error::invalid(format!("live event {k:?} does not exist")))?;
        }
        Ok(())
    }

    pub async fn delete_inbox(&self, identity: &Identity, id: Uuid) -> Result<()> {
        if !self.repo.delete_inbox(identity.id, id).await? {
            return Err(Error::not_found("inbox message not found"));
        }
        Ok(())
    }

    pub async fn deliveries(
        &self,
        key: &str,
        status: Option<&str>,
        channel: Option<&str>,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<(
        Vec<zanshin_store::messages::DeliveryRecord>,
        Vec<(String, String, i64)>,
    )> {
        let m = self.get(key).await?;
        let items = self
            .repo
            .deliveries(m.id, status, channel, after, limit.clamp(1, 500))
            .await?;
        let stats = self.repo.delivery_stats(m.id).await?;
        Ok((items, stats))
    }

    /// Render a template for a player and drop it straight into their inbox (template test send).
    pub async fn send_template(
        &self,
        identity: &Identity,
        flat: &PropertyMap,
        template_key: &str,
        locale: Option<&str>,
        push: bool,
    ) -> Result<(zanshin_core::templates::Rendered, Uuid)> {
        let t = self.templates.get(template_key).await?;
        let locale = locale
            .map(str::to_string)
            .or_else(|| Template::locale_of(flat));
        let r = t.render(locale.as_deref(), flat);
        let id = self
            .repo
            .deliver(
                identity.id,
                Delivery {
                    message_id: None,
                    key: Some(&format!("template:{template_key}")),
                    title: &r.title,
                    body: &r.body,
                    image_url: &t.image_url,
                    payload: &t.payload,
                    priority: 0,
                    expires_at: None,
                    occurrence: None,
                },
            )
            .await?
            .ok_or_else(|| Error::internal(anyhow::anyhow!("inbox insert returned nothing")))?;
        if push {
            self.push_copy(
                identity.id,
                id,
                &Self::notification(&r.title, &r.body, &t.image_url, id, None),
            )
            .await;
        }
        Ok((r, id))
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
                let audiences = eval.audiences.memberships(&identity, &flat).await;
                for (m, occ) in self.pending(&campaigns, identity.id, now).await? {
                    if self
                        .deliver_campaign(m, &identity, &flat, &audiences, now, occ)
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
            for (m, occ) in self.pending(&live, identity.id, now).await? {
                self.deliver_campaign(m, identity, flat, audiences, now, occ)
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
                    occurrence: None,
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
            template_key: p.template_key.flatten(),
            schedule: p.schedule.unwrap_or_default(),
            channels: p.channels.unwrap_or_else(default_channels),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        m.validate()?;
        self.check_refs(&m).await?;
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
        if let Some(v) = p.template_key {
            m.template_key = v;
        }
        if let Some(v) = p.schedule {
            m.schedule = v;
        }
        if let Some(v) = p.channels {
            m.channels = v;
        }
        if let Some(v) = p.tags {
            m.tags = normalize_tags(v)?;
        }
        m.validate()?;
        self.check_refs(&m).await?;
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
