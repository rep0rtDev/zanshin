//! Analytics ingest: validate → tag with experiment assignments → queue → batch-insert into the
//! event store → fold computed properties per identity.

use std::{collections::HashMap, sync::Arc, time::Duration};

use super::webhooks::WebhookBus;
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;
use zanshin_core::{
    events::{fold_computed, Event, EventInput, MAX_BATCH},
    webhooks::{type_for_event, WebhookEvent},
    Error, Identity, PropertyMap, Result,
};
use zanshin_store::{
    cache::Cache,
    events::{Bucket, EventStore, Granularity, NameCount, StoredEvent, VariantMetric},
    experiments::ExperimentRepo,
    identities::IdentityRepo,
};

const QUEUE_CAPACITY: usize = 50_000;
const FLUSH_EVERY: Duration = Duration::from_millis(500);
const FLUSH_ROWS: usize = 2_000;
/// How long an ingest request waits for queue space before answering 503.
const ENQUEUE_WAIT: Duration = Duration::from_millis(250);
/// Client-supplied event ids are remembered this long so retried batches are not double counted.
const DEDUPE_TTL: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone)]
pub struct EventService {
    tx: mpsc::Sender<Event>,
    store: EventStore,
    identities: IdentityRepo,
    experiments: ExperimentRepo,
    hooks: WebhookBus,
    cache: Option<Cache>,
    inner: Arc<Inner>,
}

struct Inner {
    rx: std::sync::Mutex<Option<mpsc::Receiver<Event>>>,
}

impl EventService {
    pub fn new(
        store: EventStore,
        identities: IdentityRepo,
        experiments: ExperimentRepo,
        hooks: WebhookBus,
    ) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
        Self {
            tx,
            hooks,
            store,
            identities,
            experiments,
            cache: None,
            inner: Arc::new(Inner {
                rx: std::sync::Mutex::new(Some(rx)),
            }),
        }
    }

    /// Enable cross-replica idempotency for client-supplied event ids.
    pub fn with_cache(mut self, cache: Cache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// True when `id` was already accepted (by any replica) within the dedupe window.
    async fn is_duplicate(&self, id: Uuid) -> bool {
        let Some(cache) = &self.cache else {
            return false;
        };
        let key = cache.key(&["ev", "dedupe", &id.to_string()]);
        match cache.set_nx(&key, DEDUPE_TTL).await {
            Ok(created) => !created,
            // Redis trouble must not reject events; the store's primary key is the backstop.
            Err(_) => false,
        }
    }

    pub fn backend(&self) -> &'static str {
        self.store.backend()
    }

    /// Events accepted but not yet flushed to the store.
    pub fn queue_depth(&self) -> usize {
        QUEUE_CAPACITY - self.tx.capacity()
    }

    /// Retention purge (see `retention.events`); returns rows removed in this batch.
    pub async fn purge_before(
        &self,
        before: chrono::DateTime<chrono::Utc>,
        batch: u32,
    ) -> Result<u64> {
        self.store.purge_before(before, batch).await
    }

    /// Start the batching writer. Call once per process.
    pub fn spawn_writer(&self) {
        let Some(mut rx) = self.inner.rx.lock().expect("rx lock").take() else {
            return;
        };
        let store = self.store.clone();
        let identities = self.identities.clone();
        tokio::spawn(async move {
            let mut batch: Vec<Event> = Vec::with_capacity(FLUSH_ROWS);
            loop {
                let timeout = tokio::time::sleep(FLUSH_EVERY);
                tokio::pin!(timeout);
                let mut closed = false;
                loop {
                    tokio::select! {
                        ev = rx.recv() => match ev {
                            Some(e) => { batch.push(e); if batch.len() >= FLUSH_ROWS { break; } }
                            None => { closed = true; break; }
                        },
                        _ = &mut timeout => break,
                    }
                }
                if !batch.is_empty() {
                    flush(&store, &identities, &batch).await;
                    batch.clear();
                }
                if closed {
                    return;
                }
            }
        });
    }

    /// Queue client-reported events. `allow_system` lets trusted callers emit `$`-prefixed names.
    pub async fn track(
        &self,
        identity: &Identity,
        inputs: Vec<EventInput>,
        allow_system: bool,
        session_id: Option<Uuid>,
    ) -> Result<usize> {
        if inputs.is_empty() {
            return Ok(0);
        }
        if inputs.len() > MAX_BATCH {
            return Err(Error::invalid(format!(
                "at most {MAX_BATCH} events per request"
            )));
        }
        for i in &inputs {
            i.validate(allow_system)?;
        }
        let experiments = self
            .experiments
            .assignment_keys_for(identity.id)
            .await
            .unwrap_or_default();
        let now = chrono::Utc::now();
        let mut seen = std::collections::HashSet::new();
        let mut n = 0;
        for input in inputs {
            if let Some(id) = input.id {
                if !seen.insert(id) || self.is_duplicate(id).await {
                    metrics::counter!("zanshin_events_deduplicated_total").increment(1);
                    continue;
                }
            }
            let ev = Event::from_input(
                input,
                identity.id,
                &identity.external_id,
                session_id,
                experiments.clone(),
                now,
            );
            if self.hooks.receiver_count() > 0 {
                let _ = self.hooks.send(WebhookEvent::new(
                    type_for_event(&ev.name),
                    serde_json::json!({
                        "identity_id": ev.identity_id, "external_id": ev.external_id, "name": ev.name,
                        "value": ev.value, "metadata": ev.metadata, "session_id": ev.session_id, "event_time": ev.event_time,
                    }),
                ));
            }
            // Brief backpressure instead of an immediate 503 when the writer is momentarily behind.
            match tokio::time::timeout(ENQUEUE_WAIT, self.tx.send(ev)).await {
                Ok(Ok(())) => n += 1,
                Err(_) => {
                    metrics::counter!("zanshin_events_dropped_total", "reason" => "queue_full")
                        .increment(1);
                    return Err(Error::unavailable("event queue is full, retry later"));
                }
                Ok(Err(_)) => return Err(Error::internal(anyhow::anyhow!("event writer stopped"))),
            }
        }
        metrics::counter!("zanshin_events_ingested_total").increment(n as u64);
        Ok(n)
    }

    /// Emit a server-side event (`$authenticate`, `$exposure`, …). Never fails the caller.
    pub async fn system(
        &self,
        identity: &Identity,
        name: &str,
        value: impl Into<String>,
        metadata: Option<serde_json::Map<String, Value>>,
        session_id: Option<Uuid>,
    ) {
        let input = EventInput {
            name: name.to_string(),
            value: Some(value.into()),
            metadata,
            timestamp: None,
            id: None,
        };
        if let Err(e) = self.track(identity, vec![input], true, session_id).await {
            tracing::debug!(error = %e, name, "system event not recorded");
        }
    }

    pub async fn timeseries(
        &self,
        name: &str,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        g: Granularity,
    ) -> Result<Vec<Bucket>> {
        self.store.timeseries(name, from, to, g).await
    }

    pub async fn top_names(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        limit: i64,
    ) -> Result<Vec<NameCount>> {
        self.store.top_names(from, to, limit).await
    }

    pub async fn experiment_metric(
        &self,
        experiment: &str,
        metric: &str,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<VariantMetric>> {
        self.store
            .experiment_metric(experiment, metric, from, to)
            .await
    }

    pub async fn experiment_metric_field(
        &self,
        experiment: &str,
        metric: &str,
        field: Option<&str>,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<zanshin_store::events::VariantMetric>> {
        self.store
            .experiment_metric_field(experiment, metric, field, from, to)
            .await
    }

    pub async fn experiment_metric_series(
        &self,
        experiment: &str,
        metric: &str,
        field: Option<&str>,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<zanshin_store::events::VariantDayMetric>> {
        self.store
            .experiment_metric_series(experiment, metric, field, from, to)
            .await
    }

    pub async fn delete_before(
        &self,
        before: chrono::DateTime<chrono::Utc>,
        names: &[String],
    ) -> Result<u64> {
        self.store.delete_before(before, names).await
    }

    pub async fn export_batch(
        &self,
        after: Option<(chrono::DateTime<chrono::Utc>, Uuid)>,
        names: &[String],
        limit: i64,
    ) -> Result<Vec<zanshin_store::events::ExportedEvent>> {
        self.store.export_batch(after, names, limit).await
    }

    pub async fn active_per_day(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<(String, i64)>> {
        self.store.active_per_day(from, to).await
    }

    pub async fn active_in_range(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<i64> {
        self.store.active_in_range(from, to).await
    }

    pub async fn revenue_per_day(
        &self,
        names: &[String],
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<(String, f64, i64, i64)>> {
        self.store.revenue_per_day(names, from, to).await
    }

    pub async fn funnel_rows(
        &self,
        names: &[String],
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        limit: i64,
    ) -> Result<Vec<(Uuid, String, chrono::DateTime<chrono::Utc>, Value)>> {
        self.store.funnel_rows(names, from, to, limit).await
    }

    pub async fn active_days(
        &self,
        identities: &[Uuid],
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<(Uuid, String)>> {
        self.store.active_days(identities, from, to).await
    }

    pub async fn recent_for_identity(
        &self,
        identity_id: Uuid,
        limit: i64,
    ) -> Result<Vec<StoredEvent>> {
        self.store.recent_for_identity(identity_id, limit).await
    }

    pub async fn delete_identity(&self, identity_id: Uuid) -> Result<()> {
        self.store.delete_identity(identity_id).await
    }
}

async fn flush(store: &EventStore, identities: &IdentityRepo, batch: &[Event]) {
    let started = std::time::Instant::now();
    // Resolve identities first so events for identities deleted meanwhile are dropped, not stored.
    let mut by_identity: HashMap<Uuid, (Option<Identity>, Vec<Event>)> = HashMap::new();
    for e in batch {
        by_identity
            .entry(e.identity_id)
            .or_insert_with(|| (None, Vec::new()))
            .1
            .push(e.clone());
    }
    let ids: Vec<Uuid> = by_identity.keys().copied().collect();
    match identities.get_many(&ids).await {
        Ok(found) => {
            for identity in found {
                if let Some(slot) = by_identity.get_mut(&identity.id) {
                    slot.0 = Some(identity);
                }
            }
        }
        Err(e) => {
            metrics::counter!("zanshin_events_dropped_total", "reason" => "identity_lookup_failed")
                .increment(batch.len() as u64);
            tracing::warn!(error = %e, rows = batch.len(), "event batch dropped: identity lookup failed");
            return;
        }
    }
    let rows: Vec<Event> = by_identity
        .values()
        .filter(|(i, _)| i.is_some())
        .flat_map(|(_, evs)| evs.iter().cloned())
        .collect();
    let dropped = batch.len() - rows.len();
    if dropped > 0 {
        metrics::counter!("zanshin_events_dropped_total", "reason" => "identity_deleted")
            .increment(dropped as u64);
    }
    if rows.is_empty() {
        return;
    }
    if let Err(e) = store.insert_batch(&rows).await {
        metrics::counter!("zanshin_events_dropped_total", "reason" => "insert_failed")
            .increment(rows.len() as u64);
        tracing::warn!(error = %e, rows = rows.len(), "event batch insert failed");
        return;
    }
    metrics::counter!("zanshin_events_stored_total").increment(rows.len() as u64);
    metrics::histogram!("zanshin_events_flush_seconds").record(started.elapsed().as_secs_f64());

    let patches: Vec<(Uuid, PropertyMap)> = by_identity
        .into_iter()
        .filter_map(|(id, (identity, evs))| {
            identity.map(|i| (id, fold_computed(&i.properties.computed, &evs)))
        })
        .collect();
    if let Err(e) = identities.merge_computed_many(&patches).await {
        tracing::warn!(error = %e, identities = patches.len(), "computed properties not updated");
    }
}
