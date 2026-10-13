//! Analytics ingest: validate → tag with experiment assignments → queue → batch-insert into the
//! event store → fold computed properties per identity.

use std::{collections::HashMap, sync::Arc, time::Duration};

use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;
use zanshin_core::{
    events::{fold_computed, Event, EventInput, MAX_BATCH},
    Error, Identity, Result,
};
use zanshin_store::{
    events::{Bucket, EventStore, Granularity, NameCount, StoredEvent, VariantMetric},
    experiments::ExperimentRepo,
    identities::IdentityRepo,
};

const QUEUE_CAPACITY: usize = 50_000;
const FLUSH_EVERY: Duration = Duration::from_millis(500);
const FLUSH_ROWS: usize = 2_000;

#[derive(Clone)]
pub struct EventService {
    tx: mpsc::Sender<Event>,
    store: EventStore,
    identities: IdentityRepo,
    experiments: ExperimentRepo,
    inner: Arc<Inner>,
}

struct Inner {
    rx: std::sync::Mutex<Option<mpsc::Receiver<Event>>>,
}

impl EventService {
    pub fn new(store: EventStore, identities: IdentityRepo, experiments: ExperimentRepo) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
        Self {
            tx,
            store,
            identities,
            experiments,
            inner: Arc::new(Inner {
                rx: std::sync::Mutex::new(Some(rx)),
            }),
        }
    }

    pub fn backend(&self) -> &'static str {
        self.store.backend()
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
                if !seen.insert(id) {
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
            match self.tx.try_send(ev) {
                Ok(()) => n += 1,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    metrics::counter!("zanshin_events_dropped_total", "reason" => "queue_full")
                        .increment(1);
                    return Err(Error::unavailable("event queue is full, retry later"));
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    return Err(Error::internal(anyhow::anyhow!("event writer stopped")))
                }
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
    for (id, slot) in by_identity.iter_mut() {
        slot.0 = identities.get(*id).await.ok();
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

    for (id, (identity, evs)) in by_identity {
        let Some(identity) = identity else { continue };
        let patch = fold_computed(&identity.properties.computed, &evs);
        if let Err(e) = identities.merge_computed(id, &patch).await {
            tracing::warn!(error = %e, identity = %id, "computed properties not updated");
        }
    }
}
