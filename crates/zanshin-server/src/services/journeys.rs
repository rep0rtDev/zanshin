//! Journeys: trigger → per-player run through steps (actions, waits, branches), executed by an
//! in-process worker and persisted so waits survive restarts.

use std::{collections::HashSet, sync::Arc, time::Duration};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;
use zanshin_core::{
    flags::validate_key,
    identity::Identity,
    journeys::{find_cycle, next_cron, next_local_time, Action, Journey, Step, Trigger, PLAYER_TZ},
    properties::PropertyMap,
    rules::{Condition, Context},
    webhooks::WebhookEvent,
    Error, Result,
};
use zanshin_store::{
    identities::IdentityRepo,
    journeys::{JourneyRepo, LogEntry, Run, RunView, Timing},
    live_events::LiveEventRepo,
};

use super::{
    eval::{flatten_with_identity, Evaluator},
    events::EventService,
    experiments::ExperimentService,
    flags::{normalize_tags, FlagService},
    integrations::{IntegrationService, Outbound},
    live_events::LiveEventService,
    messages::{DirectMessage, MessageService},
    webhooks::WebhookBus,
};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct JourneyPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub trigger: Option<Trigger>,
    #[serde(default)]
    pub condition: Option<Condition>,
    #[serde(default)]
    pub audiences: Option<Vec<String>>,
    #[serde(default)]
    pub steps: Option<Vec<Step>>,
    #[serde(default)]
    pub cooldown_secs: Option<i64>,
    #[serde(default, deserialize_with = "double_option")]
    pub max_entries: Option<Option<i32>>,
    #[serde(default, deserialize_with = "double_option")]
    pub starts_at: Option<Option<chrono::DateTime<chrono::Utc>>>,
    #[serde(default, deserialize_with = "double_option")]
    pub ends_at: Option<Option<chrono::DateTime<chrono::Utc>>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

fn double_option<'de, T, D>(d: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Everything a step can touch.
#[derive(Clone)]
pub struct JourneyDeps {
    pub identities: IdentityRepo,
    pub eval: Evaluator,
    pub messages: MessageService,
    pub events: EventService,
    pub flags: FlagService,
    pub experiments: ExperimentService,
    pub integrations: IntegrationService,
    pub live_events: LiveEventRepo,
    pub live_event_svc: LiveEventService,
}

/// Runs listing plus the funnel / timing numbers drawn on the canvas.
pub struct RunsReport {
    pub journey: Journey,
    pub items: Vec<RunView>,
    pub by_status: Vec<(String, i64)>,
    pub by_step: Vec<(i32, i64)>,
    pub passed: Vec<(i32, i64)>,
    pub timing: Timing,
}

#[derive(Clone)]
pub struct JourneyService {
    repo: JourneyRepo,
    deps: JourneyDeps,
    hooks: WebhookBus,
    cache: Arc<RwLock<Vec<Arc<Journey>>>>,
}

const TICK: Duration = Duration::from_secs(5);
const LIVE_EVENT_SWEEP: Duration = Duration::from_secs(60);
const MAX_HOPS: usize = 1000;

impl JourneyService {
    pub fn new(repo: JourneyRepo, deps: JourneyDeps, hooks: WebhookBus) -> Self {
        let svc = Self {
            repo,
            deps,
            hooks: hooks.clone(),
            cache: Arc::new(RwLock::new(Vec::new())),
        };
        let worker = svc.clone();
        tokio::spawn(async move { worker.run(hooks.subscribe()).await });
        svc
    }

    pub async fn reload(&self) -> Result<()> {
        let all = self.repo.list().await?.into_iter().map(Arc::new).collect();
        *self.cache.write().await = all;
        Ok(())
    }

    async fn live(&self) -> Vec<Arc<Journey>> {
        let now = chrono::Utc::now();
        self.cache
            .read()
            .await
            .iter()
            .filter(|j| j.is_live(now))
            .cloned()
            .collect()
    }

    async fn run(self, mut events: broadcast::Receiver<WebhookEvent>) {
        if let Err(e) = self.reload().await {
            tracing::warn!(error = %e, "journey catalog load failed");
        }
        let mut tick = tokio::time::interval(TICK);
        let mut sweep = tokio::time::interval(LIVE_EVENT_SWEEP);
        loop {
            tokio::select! {
                ev = events.recv() => match ev {
                    Ok(ev) => self.on_event(ev).await,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        metrics::counter!("zanshin_journey_events_dropped_total").increment(n);
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                _ = tick.tick() => {
                    let _ = self.reload().await;
                    self.resume_due().await;
                }
                _ = sweep.tick() => self.sweep_live_events().await,
            }
        }
    }

    async fn on_event(&self, ev: WebhookEvent) {
        let Some(identity_id) = ev
            .data
            .get("identity_id")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Uuid>().ok())
        else {
            return;
        };
        let journeys = self.live().await;
        let by_type: Vec<_> = journeys
            .iter()
            .filter(|j| j.trigger.event_type().as_deref() == Some(ev.event_type.as_str()))
            .cloned()
            .collect();
        let by_audience: Vec<_> = journeys
            .iter()
            .filter(|j| matches!(j.trigger, Trigger::AudienceEnter { .. }))
            .cloned()
            .collect();
        if by_type.is_empty() && by_audience.is_empty() {
            return;
        }
        let Ok(identity) = self.deps.identities.get(identity_id).await else {
            return;
        };
        let ctx = json!({ "trigger": ev.event_type, "event": ev.data });
        for j in by_type {
            if let Err(e) = self.try_enter(&j, &identity, ctx.clone(), false).await {
                tracing::debug!(journey = %j.key, error = %e, "entry skipped");
            }
        }
        if !by_audience.is_empty() {
            let flat = flatten_with_identity(&identity, &identity.properties);
            let members = self.deps.eval.audiences.memberships(&identity, &flat).await;
            for j in by_audience {
                let Trigger::AudienceEnter { audience } = &j.trigger else {
                    continue;
                };
                if !members.contains(audience) {
                    continue;
                }
                // Enter once per membership: a previous run (any status) within the cooldown counts.
                let _ = self
                    .try_enter(
                        &j,
                        &identity,
                        json!({ "trigger": "audience.enter", "audience": audience }),
                        false,
                    )
                    .await;
            }
        }
    }

    async fn sweep_live_events(&self) {
        let now = chrono::Utc::now();
        let journeys: Vec<_> = self
            .live()
            .await
            .into_iter()
            .filter(|j| matches!(j.trigger, Trigger::LiveEventStart { .. }))
            .collect();
        for j in journeys {
            let Trigger::LiveEventStart { live_event } = &j.trigger else {
                continue;
            };
            let Ok(ev) = self.deps.live_events.get(live_event).await else {
                continue;
            };
            let Some(occ) = ev.current(now) else { continue };
            let occurrence = occ.start.to_rfc3339();
            let mut after = None;
            loop {
                let Ok(page) = self.deps.identities.list(after, 500).await else {
                    break;
                };
                let Some(last) = page.last().map(|i| i.id) else {
                    break;
                };
                for identity in &page {
                    if self
                        .repo
                        .has_run_with_context(j.id, identity.id, "occurrence", &occurrence)
                        .await
                        .unwrap_or(true)
                    {
                        continue;
                    }
                    let _ = self
                        .try_enter(&j, identity, json!({ "trigger": "live_event.start", "live_event": live_event, "occurrence": occurrence }), false)
                        .await;
                }
                after = Some(last);
            }
        }
    }

    /// Enter a player if the journey is live, targets them and cooldown / cap allow it.
    /// `force` skips the live-window check (admin / server entry still honours targeting and caps).
    pub async fn try_enter(
        &self,
        j: &Journey,
        identity: &Identity,
        context: Value,
        force: bool,
    ) -> Result<Run> {
        let now = chrono::Utc::now();
        if !force && !j.is_live(now) {
            return Err(Error::invalid("journey is not live"));
        }
        let flat = flatten_with_identity(identity, &identity.properties);
        let members = self.deps.eval.audiences.memberships(identity, &flat).await;
        if !j.targets(&flat, &|a| members.contains(a)) {
            return Err(Error::invalid(
                "player does not match the journey targeting",
            ));
        }
        let (entries, last, active) = self.repo.entry_state(j.id, identity.id).await?;
        if active {
            return Err(Error::invalid("player is already in this journey"));
        }
        if j.max_entries.is_some_and(|m| entries >= m as i64) {
            return Err(Error::invalid("max_entries reached for this player"));
        }
        if let Some(last) = last {
            if j.cooldown_secs > 0 && last + chrono::Duration::seconds(j.cooldown_secs) > now {
                return Err(Error::invalid("player is in cooldown for this journey"));
            }
            if j.cooldown_secs == 0 && matches!(j.trigger, Trigger::AudienceEnter { .. }) && !force
            {
                return Err(Error::invalid("audience entry already happened"));
            }
        }
        let run = self.repo.start_run(j.id, identity.id, &context).await?;
        metrics::counter!("zanshin_journey_entries_total", "journey" => j.key.clone()).increment(1);
        let _ = self.hooks.send(WebhookEvent::new(
            "journey.entered",
            json!({ "journey": j.key, "run_id": run.id, "identity_id": identity.id, "external_id": identity.external_id }),
        ));
        self.execute(j, run, identity, flat, members).await
    }

    async fn resume_due(&self) {
        let due = match self.repo.claim_due(chrono::Utc::now(), 200).await {
            Ok(d) => d,
            Err(e) => {
                tracing::debug!(error = %e, "journey wake query failed");
                return;
            }
        };
        if due.is_empty() {
            return;
        }
        let journeys = self.cache.read().await.clone();
        for run in due {
            let Some(j) = journeys.iter().find(|j| j.id == run.journey_id).cloned() else {
                let _ = self
                    .repo
                    .advance(run.id, run.step, "exited", None, "journey removed")
                    .await;
                continue;
            };
            let Ok(identity) = self.deps.identities.get(run.identity_id).await else {
                let _ = self
                    .repo
                    .advance(run.id, run.step, "exited", None, "identity removed")
                    .await;
                continue;
            };
            let flat = flatten_with_identity(&identity, &identity.properties);
            let members = self.deps.eval.audiences.memberships(&identity, &flat).await;
            if let Err(e) = self.execute(&j, run, &identity, flat, members).await {
                tracing::warn!(journey = %j.key, error = %e, "journey step failed");
            }
        }
    }

    /// Run steps from `run.step` until a wait, an exit or the end.
    async fn execute(
        &self,
        j: &Journey,
        mut run: Run,
        identity: &Identity,
        flat: PropertyMap,
        members: HashSet<String>,
    ) -> Result<Run> {
        let mut idx = run.step.max(0) as usize;
        for _ in 0..MAX_HOPS {
            let Some(step) = j.steps.get(idx) else {
                run = self
                    .repo
                    .advance(run.id, idx as i32, "done", None, "")
                    .await?;
                self.finished(j, &run, identity, "done");
                return Ok(run);
            };
            let now = chrono::Utc::now();
            let outcome = self
                .perform(j, &run, step, identity, &flat, &members, now)
                .await;
            match outcome {
                Err(e) => {
                    let _ = self
                        .repo
                        .log(run.id, idx as i32, &step.id, &format!("failed: {e}"))
                        .await;
                    run = self
                        .repo
                        .advance(run.id, idx as i32, "failed", None, &e.to_string())
                        .await?;
                    self.finished(j, &run, identity, "failed");
                    return Ok(run);
                }
                Ok(Flow::Next(detail)) => {
                    let _ = self.repo.log(run.id, idx as i32, &step.id, &detail).await;
                    idx += 1;
                }
                Ok(Flow::Jump(target, detail)) => {
                    let _ = self.repo.log(run.id, idx as i32, &step.id, &detail).await;
                    idx = target;
                }
                Ok(Flow::Sleep(until)) => {
                    let _ = self
                        .repo
                        .log(
                            run.id,
                            idx as i32,
                            &step.id,
                            &format!("waiting until {}", until.to_rfc3339()),
                        )
                        .await;
                    return self
                        .repo
                        .advance(run.id, (idx + 1) as i32, "active", Some(until), "")
                        .await;
                }
                Ok(Flow::Exit) => {
                    let _ = self.repo.log(run.id, idx as i32, &step.id, "exit").await;
                    run = self
                        .repo
                        .advance(run.id, idx as i32, "exited", None, "")
                        .await?;
                    self.finished(j, &run, identity, "exited");
                    return Ok(run);
                }
            }
        }
        run = self
            .repo
            .advance(run.id, idx as i32, "failed", None, "too many hops (loop?)")
            .await?;
        Ok(run)
    }

    fn finished(&self, j: &Journey, run: &Run, identity: &Identity, status: &str) {
        metrics::counter!("zanshin_journey_runs_finished_total", "journey" => j.key.clone(), "status" => status.to_string())
            .increment(1);
        let _ = self.hooks.send(WebhookEvent::new(
            "journey.finished",
            json!({ "journey": j.key, "run_id": run.id, "identity_id": identity.id, "external_id": identity.external_id, "status": status }),
        ));
    }

    #[allow(clippy::too_many_arguments)]
    async fn perform(
        &self,
        j: &Journey,
        run: &Run,
        step: &Step,
        identity: &Identity,
        flat: &PropertyMap,
        members: &HashSet<String>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Flow> {
        let _ = members;
        match &step.action {
            Action::Message {
                title,
                body,
                image_url,
                payload,
                template_key,
                push,
            } => {
                let id = match template_key {
                    Some(t) => {
                        let (_, id) = self
                            .deps
                            .messages
                            .send_template(identity, flat, t, None, *push)
                            .await?;
                        id
                    }
                    None => {
                        self.deps
                            .messages
                            .send_direct(
                                identity,
                                DirectMessage {
                                    inbox: true,
                                    template_key: None,
                                    template_name: None,
                                    channels: Vec::new(),
                                    locale: None,
                                    title: zanshin_core::messages::MessageCampaign::render(
                                        title, flat,
                                    ),
                                    push: *push,
                                    body: zanshin_core::messages::MessageCampaign::render(
                                        body, flat,
                                    ),
                                    image_url: image_url.clone(),
                                    payload: if payload.is_null() {
                                        json!({ "journey": j.key, "step": step.id })
                                    } else {
                                        payload.clone()
                                    },
                                    priority: 0,
                                    ttl_secs: None,
                                },
                            )
                            .await?
                    }
                };
                Ok(Flow::Next(format!("message {id}")))
            }
            Action::Track {
                name,
                value,
                metadata,
            } => {
                let mut meta = metadata.as_object().cloned().unwrap_or_default();
                meta.insert("journey".into(), Value::String(j.key.clone()));
                meta.insert("journey_run".into(), Value::String(run.id.to_string()));
                self.deps
                    .events
                    .system(identity, name, value.clone(), Some(meta), None)
                    .await;
                Ok(Flow::Next(format!("tracked {name}")))
            }
            Action::FlagOverride { flag, value } => {
                self.deps
                    .flags
                    .set_override(flag, identity.id, value.clone())
                    .await?;
                Ok(Flow::Next(format!("override {flag} = {value}")))
            }
            Action::ExperimentAssign {
                experiment,
                variant,
            } => {
                self.deps
                    .experiments
                    .force_assign(experiment, identity.id, variant)
                    .await?;
                Ok(Flow::Next(format!("assigned {experiment}:{variant}")))
            }
            Action::Integration {
                integration,
                title,
                body,
                payload,
            } => {
                let title = zanshin_core::messages::MessageCampaign::render(title, flat);
                let body = zanshin_core::messages::MessageCampaign::render(body, flat);
                let locale = zanshin_core::templates::Template::locale_of(flat).unwrap_or_default();
                let out = Outbound {
                    message_key: &format!("journey:{}:{}", j.key, step.id),
                    identity_id: identity.id,
                    external_id: &identity.external_id,
                    occurrence: None,
                    locale: &locale,
                    title: &title,
                    body: &body,
                    image_url: "",
                    payload,
                    props: flat,
                };
                let o = self.deps.integrations.deliver(integration, &out).await;
                if o.status == "failed" {
                    return Err(Error::unavailable(format!(
                        "integration {integration}: {}",
                        o.detail
                    )));
                }
                Ok(Flow::Next(format!(
                    "{integration}: {} {}",
                    o.status, o.detail
                )))
            }
            Action::Wait { secs } => Ok(Flow::Sleep(now + chrono::Duration::seconds(*secs))),
            Action::WaitUntil {
                hour,
                minute,
                timezone_prop,
            } => {
                let prop = timezone_prop.as_deref().unwrap_or("default.timezone");
                let tz = flat.get(prop).and_then(|v| v.as_str());
                Ok(Flow::Sleep(next_local_time(now, *hour, *minute, tz)))
            }
            Action::Branch {
                condition,
                then,
                otherwise,
            } => {
                let matched = condition.matches(&Context { props: flat });
                let target = if matched {
                    Some(then.as_str())
                } else {
                    otherwise.as_deref()
                };
                match target {
                    Some(t) => {
                        let i = j
                            .step_index(t)
                            .ok_or_else(|| Error::invalid(format!("unknown step {t:?}")))?;
                        Ok(Flow::Jump(
                            i,
                            format!(
                                "branch {} → {t}",
                                if matched { "matched" } else { "not matched" }
                            ),
                        ))
                    }
                    None => Ok(Flow::Next("branch not matched → next".into())),
                }
            }
            Action::Goto { step: target } => {
                let i = j
                    .step_index(target)
                    .ok_or_else(|| Error::invalid(format!("unknown step {target:?}")))?;
                Ok(Flow::Jump(i, format!("goto {target}")))
            }
            Action::TimeWindow {
                at,
                cron,
                timezone,
                timezone_prop,
            } => {
                if let Some(at) = at {
                    return Ok(if *at > now {
                        Flow::Sleep(*at)
                    } else {
                        Flow::Next("time window already open".into())
                    });
                }
                let expr = cron.as_deref().unwrap_or_default();
                let tz: chrono_tz::Tz = match timezone.as_deref() {
                    Some(PLAYER_TZ) => {
                        let prop = timezone_prop.as_deref().unwrap_or("default.timezone");
                        flat.get(prop)
                            .and_then(|v| v.as_str())
                            .and_then(|t| t.parse().ok())
                            .unwrap_or(chrono_tz::UTC)
                    }
                    Some(z) => z.parse().unwrap_or(chrono_tz::UTC),
                    None => chrono_tz::UTC,
                };
                let until = next_cron(now, expr, tz)
                    .ok_or_else(|| Error::invalid(format!("cron {expr:?} has no next slot")))?;
                Ok(Flow::Sleep(until))
            }
            Action::LiveEvent {
                live_event,
                duration_secs,
            } => {
                let view = self
                    .deps
                    .live_event_svc
                    .join_personal(identity.id, live_event, now, *duration_secs)
                    .await?;
                Ok(Flow::Next(format!(
                    "live event {live_event} until {}",
                    view.participation_ends_at
                        .map(|d| d.to_rfc3339())
                        .unwrap_or_default()
                )))
            }
            Action::Revert { step: target } => {
                let t = j
                    .steps
                    .iter()
                    .find(|s| &s.id == target)
                    .ok_or_else(|| Error::invalid(format!("unknown step {target:?}")))?;
                let detail = match &t.action {
                    Action::FlagOverride { flag, .. } => {
                        self.deps.flags.delete_override(flag, identity.id).await?;
                        format!("removed override {flag}")
                    }
                    Action::ExperimentAssign { experiment, .. } => {
                        self.deps
                            .experiments
                            .delete_assignment(experiment, identity.id)
                            .await?;
                        format!("removed assignment {experiment}")
                    }
                    Action::LiveEvent { live_event, .. } => {
                        self.deps
                            .live_event_svc
                            .leave(identity.id, live_event)
                            .await?;
                        format!("left live event {live_event}")
                    }
                    _ => {
                        return Err(Error::invalid(format!(
                            "step {target:?} cannot be reverted"
                        )))
                    }
                };
                Ok(Flow::Next(detail))
            }
            Action::EnterJourney { journey } => {
                let target = self
                    .cache
                    .read()
                    .await
                    .iter()
                    .find(|t| &t.key == journey)
                    .cloned();
                let Some(target) = target else {
                    return Ok(Flow::Next(format!("journey {journey} not found, skipped")));
                };
                let ctx = json!({ "trigger": "journey", "parent": j.key, "parent_run": run.id });
                match Box::pin(self.try_enter(&target, identity, ctx, false)).await {
                    Ok(r) => Ok(Flow::Next(format!("entered {journey} (run {})", r.id))),
                    Err(e) => Ok(Flow::Next(format!("{journey} not entered: {e}"))),
                }
            }
            Action::Exit => Ok(Flow::Exit),
        }
    }

    // ---- admin surface

    pub async fn list(&self) -> Result<Vec<Journey>> {
        self.repo.list().await
    }

    pub async fn get(&self, key: &str) -> Result<Journey> {
        self.repo.get(key).await
    }

    pub async fn create(&self, key: &str, p: JourneyPatch) -> Result<Journey> {
        validate_key(key)?;
        let now = chrono::Utc::now();
        let j = Journey {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            description: p.description.unwrap_or_default(),
            enabled: p.enabled.unwrap_or(true),
            trigger: p.trigger.unwrap_or_default(),
            condition: p.condition.unwrap_or_else(Condition::always),
            audiences: p.audiences.unwrap_or_default(),
            steps: p.steps.unwrap_or_default(),
            cooldown_secs: p.cooldown_secs.unwrap_or(0),
            max_entries: p.max_entries.flatten(),
            starts_at: p.starts_at.flatten(),
            ends_at: p.ends_at.flatten(),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        j.validate()?;
        self.check_cycles(&j).await?;
        let created = self.repo.create(&j).await?;
        self.reload().await?;
        Ok(created)
    }

    pub async fn update(&self, key: &str, p: JourneyPatch) -> Result<Journey> {
        let mut j = self.repo.get(key).await?;
        if let Some(v) = p.name {
            j.name = v;
        }
        if let Some(v) = p.description {
            j.description = v;
        }
        if let Some(v) = p.enabled {
            j.enabled = v;
        }
        if let Some(v) = p.trigger {
            j.trigger = v;
        }
        if let Some(v) = p.condition {
            j.condition = v;
        }
        if let Some(v) = p.audiences {
            j.audiences = v;
        }
        if let Some(v) = p.steps {
            j.steps = v;
        }
        if let Some(v) = p.cooldown_secs {
            j.cooldown_secs = v;
        }
        if let Some(v) = p.max_entries {
            j.max_entries = v;
        }
        if let Some(v) = p.starts_at {
            j.starts_at = v;
        }
        if let Some(v) = p.ends_at {
            j.ends_at = v;
        }
        if let Some(v) = p.tags {
            j.tags = normalize_tags(v)?;
        }
        j.validate()?;
        self.check_cycles(&j).await?;
        let updated = self.repo.update(&j).await?;
        self.reload().await?;
        Ok(updated)
    }

    /// `EnterJourney` steps must not form a cycle across journeys.
    async fn check_cycles(&self, j: &Journey) -> Result<()> {
        let others = self.cache.read().await.clone();
        let children = |k: &str| -> Vec<String> {
            if k == j.key {
                j.child_journeys().map(String::from).collect()
            } else {
                others
                    .iter()
                    .find(|o| o.key == k)
                    .map(|o| o.child_journeys().map(String::from).collect())
                    .unwrap_or_default()
            }
        };
        if let Some(path) = find_cycle(&j.key, &children) {
            return Err(Error::invalid(format!(
                "journeys would form a cycle: {}",
                path.join(" → ")
            )));
        }
        Ok(())
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        self.repo.delete(key).await?;
        self.reload().await
    }

    /// Admin / server entry: honours targeting, cooldown and caps but not the live window.
    pub async fn enter(&self, key: &str, identity: &Identity) -> Result<Run> {
        let j = self.repo.get(key).await?;
        self.try_enter(&j, identity, json!({ "trigger": "manual" }), true)
            .await
    }

    pub async fn runs(
        &self,
        key: &str,
        status: Option<&str>,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<RunsReport> {
        let journey = self.repo.get(key).await?;
        let items = self
            .repo
            .runs(journey.id, status, after, limit.clamp(1, 500))
            .await?;
        let (by_status, by_step) = self.repo.stats(journey.id).await?;
        let passed = self.repo.passed(journey.id).await?;
        let timing = self.repo.timing(journey.id).await?;
        Ok(RunsReport {
            journey,
            items,
            by_status,
            by_step,
            passed,
            timing,
        })
    }

    pub async fn run_detail(&self, key: &str, id: Uuid) -> Result<(Run, Vec<LogEntry>)> {
        let j = self.repo.get(key).await?;
        let run = self.repo.get_run(j.id, id).await?;
        let log = self.repo.run_log(run.id).await?;
        Ok((run, log))
    }

    pub async fn exit_run(&self, key: &str, id: Uuid) -> Result<Run> {
        let j = self.repo.get(key).await?;
        let run = self.repo.get_run(j.id, id).await?;
        if run.status != "active" {
            return Err(Error::invalid("run is not active"));
        }
        let _ = self
            .repo
            .log(run.id, run.step, "admin", "exited by operator")
            .await;
        self.repo
            .advance(run.id, run.step, "exited", None, "")
            .await
    }

    /// A player's journey history: every run with its step log (newest first).
    pub async fn runs_for_identity(
        &self,
        identity_id: Uuid,
    ) -> Result<Vec<(String, Run, Vec<LogEntry>)>> {
        let runs = self.repo.runs_for_identity(identity_id).await?;
        let ids: Vec<Uuid> = runs.iter().map(|(_, r)| r.id).collect();
        let logs = self.repo.logs_for_runs(&ids).await?;
        Ok(runs
            .into_iter()
            .map(|(k, r)| {
                let log = logs
                    .iter()
                    .filter(|(id, _)| *id == r.id)
                    .map(|(_, e)| e.clone())
                    .collect();
                (k, r, log)
            })
            .collect())
    }

    pub async fn purge(&self, keep: chrono::Duration) -> Result<u64> {
        self.repo.purge_runs(chrono::Utc::now() - keep).await
    }
}

enum Flow {
    Next(String),
    Jump(usize, String),
    Sleep(chrono::DateTime<chrono::Utc>),
    Exit,
}
