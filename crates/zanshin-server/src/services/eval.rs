//! Per-identity evaluation that stitches audiences, experiments, overrides and flags together.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use uuid::Uuid;
use zanshin_core::{
    experiments::Assignment,
    flags::{Evaluation, Pinned},
    properties::PropertyMap,
    Identity, PropertySet, Result,
};
use zanshin_store::identities::IdentityRepo;

use zanshin_core::{live_events::ActiveEvent, messages::InboxMessage};

use super::{
    audiences::AudienceService, experiments::ExperimentService, flags::FlagService,
    live_events::LiveEventService, messages::MessageService,
};

#[derive(Clone)]
pub struct Evaluator {
    pub flags: FlagService,
    pub audiences: AudienceService,
    pub experiments: ExperimentService,
    pub live_events: LiveEventService,
    pub messages: MessageService,
    identities: IdentityRepo,
    cache: zanshin_store::cache::Cache,
}

/// Everything known about one identity for one evaluation pass.
pub struct EvalContext {
    pub flat: PropertyMap,
    pub audiences: HashSet<String>,
    pub assignments: Vec<Assignment>,
    /// flag key → (experiment key, variant key, value)
    pub flag_pins: HashMap<String, (String, String, Value)>,
    pub overrides: HashMap<String, Value>,
    /// flag key → (live event key, value) from events active for the player
    pub live_pins: HashMap<String, (String, Value)>,
    /// Evaluation clock (QA time travel or real now).
    pub now: chrono::DateTime<chrono::Utc>,
}

impl Evaluator {
    pub fn new(
        flags: FlagService,
        audiences: AudienceService,
        experiments: ExperimentService,
        live_events: LiveEventService,
        messages: MessageService,
        identities: IdentityRepo,
        cache: zanshin_store::cache::Cache,
    ) -> Self {
        Self {
            flags,
            audiences,
            experiments,
            live_events,
            messages,
            identities,
            cache,
        }
    }

    fn qa_key(&self, identity_id: Uuid) -> String {
        self.cache.key(&["qa_time", &identity_id.to_string()])
    }

    /// The clock used for this identity: a QA "time travel" override if one is set, else now.
    pub async fn now_for(&self, identity_id: Uuid) -> chrono::DateTime<chrono::Utc> {
        self.cache
            .get_json::<chrono::DateTime<chrono::Utc>>(&self.qa_key(identity_id))
            .await
            .ok()
            .flatten()
            .unwrap_or_else(chrono::Utc::now)
    }

    pub async fn qa_time(&self, identity_id: Uuid) -> Option<chrono::DateTime<chrono::Utc>> {
        self.cache
            .get_json(&self.qa_key(identity_id))
            .await
            .ok()
            .flatten()
    }

    /// Pin the evaluation clock for an identity (expires after 7 days).
    pub async fn set_qa_time(
        &self,
        identity_id: Uuid,
        at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<()> {
        let key = self.qa_key(identity_id);
        match at {
            Some(t) => {
                self.cache
                    .set_json(&key, &t, std::time::Duration::from_secs(7 * 86400))
                    .await
            }
            None => self.cache.del(&key).await,
        }
    }

    pub async fn live_events(
        &self,
        identity: &Identity,
        names: &[String],
        active_only: bool,
    ) -> Vec<ActiveEvent> {
        let flat = flatten_with_identity(identity, &identity.properties);
        let audiences = self.audiences.memberships(identity, &flat).await;
        let now = self.now_for(identity.id).await;
        let joins = self.live_events.joins_for(identity.id).await;
        self.live_events
            .for_participant(&flat, &audiences, names, active_only, now, &joins)
            .await
    }

    /// Join / leave a live event on behalf of the player.
    pub async fn join_live_event(&self, identity: &Identity, key: &str) -> Result<ActiveEvent> {
        let flat = flatten_with_identity(identity, &identity.properties);
        let audiences = self.audiences.memberships(identity, &flat).await;
        let now = self.now_for(identity.id).await;
        self.live_events
            .join(identity.id, key, &flat, &audiences, now)
            .await
    }

    /// Full override chain for one flag and identity, for the console "explain" view.
    pub async fn explain_flag(&self, identity: &Identity, key: &str) -> Result<Value> {
        let flag = self.flags.get(key).await?;
        let ctx = self.context(identity, &identity.properties, true).await?;
        let member = |k: &str| ctx.audiences.contains(k);
        let rules: Vec<Value> = flag
            .rules
            .iter()
            .map(|r| {
                let rctx = zanshin_core::rules::Context { props: &ctx.flat };
                serde_json::json!({
                    "id": r.id, "name": r.name,
                    "condition_matches": r.condition.matches(&rctx),
                    "audiences_match": r.audiences.iter().all(|a| member(a)),
                    "in_rollout": zanshin_core::hash::in_rollout(&format!("flag:{}", flag.key), &format!("{}:{}", flag.salt, r.id), &identity.external_id, r.percentage),
                    "value": r.value, "variants": r.variants,
                })
            })
            .collect();
        let final_eval = self
            .flags_with(identity, &ctx, std::slice::from_ref(&key.to_string()))
            .await
            .pop();
        Ok(serde_json::json!({
            "flag": flag.key,
            "enabled": flag.enabled,
            "identity_override": ctx.overrides.get(key),
            "experiment": ctx.flag_pins.get(key).map(|(e, v, val)| serde_json::json!({"experiment": e, "variant": v, "value": val})),
            "live_event": ctx.live_pins.get(key).map(|(e, val)| serde_json::json!({"live_event": e, "value": val})),
            "rules": rules,
            "default_value": flag.default_value,
            "audiences": ctx.audiences.iter().collect::<Vec<_>>(),
            "now": ctx.now,
            "result": final_eval,
        }))
    }

    pub async fn inbox(&self, identity: &Identity, limit: i64) -> Result<Vec<InboxMessage>> {
        let flat = flatten_with_identity(identity, &identity.properties);
        let audiences = self.audiences.memberships(identity, &flat).await;
        self.messages
            .inbox(identity, &flat, &audiences, limit)
            .await
    }

    pub async fn context(
        &self,
        identity: &Identity,
        props: &PropertySet,
        persist_assignments: bool,
    ) -> Result<EvalContext> {
        let flat = flatten_with_identity(identity, props);
        let audiences = self.audiences.memberships(identity, &flat).await;
        let mut assignments = Vec::new();
        let mut flag_pins = HashMap::new();
        if persist_assignments {
            for e in self
                .experiments
                .enrolments(identity, &flat, &audiences, &[])
                .await?
            {
                if let Some(fk) = &e.experiment.flag_key {
                    flag_pins.insert(
                        fk.clone(),
                        (
                            e.experiment.key.clone(),
                            e.variant.key.clone(),
                            e.variant.value.clone(),
                        ),
                    );
                }
                assignments.push(e.to_assignment());
            }
        }
        let overrides = if persist_assignments {
            self.flags.overrides_for(identity.id).await
        } else {
            HashMap::new()
        };
        let now = self.now_for(identity.id).await;
        let joins = if persist_assignments {
            self.live_events.joins_for(identity.id).await
        } else {
            HashMap::new()
        };
        let catalog = self.live_events.list().await;
        let live_pins = if catalog.iter().any(|e| !e.flag_overrides.is_empty()) {
            let active = self
                .live_events
                .for_participant(&flat, &audiences, &[], true, now, &joins)
                .await;
            self.live_events.flag_pins(&active, &catalog)
        } else {
            HashMap::new()
        };
        Ok(EvalContext {
            flat,
            audiences,
            assignments,
            flag_pins,
            overrides,
            live_pins,
            now,
        })
    }

    pub async fn flags_with(
        &self,
        identity: &Identity,
        ctx: &EvalContext,
        names: &[String],
    ) -> Vec<Evaluation> {
        let member = |k: &str| ctx.audiences.contains(k);
        let snapshot = self.flags.catalog.snapshot().await;
        let mut keys: Vec<&String> = if names.is_empty() {
            snapshot.keys().collect()
        } else {
            names.iter().filter(|n| snapshot.contains_key(*n)).collect()
        };
        keys.sort();
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            let flag = &snapshot[k];
            let pinned = if let Some(v) = ctx.overrides.get(k) {
                Some(Pinned::Override(v))
            } else if let Some((e, v, val)) = ctx.flag_pins.get(k) {
                Some(Pinned::Experiment {
                    experiment: e,
                    variant: v,
                    value: val,
                })
            } else {
                ctx.live_pins.get(k).map(|(e, val)| Pinned::LiveEvent {
                    live_event: e,
                    value: val,
                })
            };
            out.push(flag.evaluate(&identity.external_id, &ctx.flat, &member, pinned));
        }
        metrics::counter!("zanshin_flag_evaluations_total").increment(out.len() as u64);
        out
    }

    pub async fn flags(&self, identity: &Identity, names: &[String]) -> Result<Vec<Evaluation>> {
        let ctx = self.context(identity, &identity.properties, true).await?;
        Ok(self.flags_with(identity, &ctx, names).await)
    }

    pub async fn flag(&self, identity: &Identity, key: &str) -> Result<Evaluation> {
        self.flags.get(key).await?;
        let mut v = self
            .flags(identity, std::slice::from_ref(&key.to_string()))
            .await?;
        v.pop()
            .ok_or_else(|| zanshin_core::Error::not_found(format!("flag {key:?} not found")))
    }

    pub async fn audience_keys(&self, identity: &Identity) -> Vec<String> {
        let flat = flatten_with_identity(identity, &identity.properties);
        let mut v: Vec<_> = self
            .audiences
            .memberships(identity, &flat)
            .await
            .into_iter()
            .collect();
        v.sort();
        v
    }

    pub async fn assignments(
        &self,
        identity: &Identity,
        names: &[String],
    ) -> Result<Vec<Assignment>> {
        let flat = flatten_with_identity(identity, &identity.properties);
        let audiences = self.audiences.memberships(identity, &flat).await;
        Ok(self
            .experiments
            .enrolments(identity, &flat, &audiences, names)
            .await?
            .iter()
            .map(|e| e.to_assignment())
            .collect())
    }

    pub async fn identity_by_external_id(&self, external_id: &str) -> Result<Identity> {
        self.identities.get_by_external_id(external_id).await
    }

    /// Admin dry run with ad-hoc properties: no assignments are written, overrides are ignored.
    pub async fn dry_run_flag(
        &self,
        subject: &str,
        props: &PropertySet,
        key: &str,
    ) -> Result<Evaluation> {
        self.flags.get(key).await?;
        let identity = synthetic_identity(subject, props);
        let ctx = self.context(&identity, props, false).await?;
        let mut v = self
            .flags_with(&identity, &ctx, std::slice::from_ref(&key.to_string()))
            .await;
        v.pop()
            .ok_or_else(|| zanshin_core::Error::not_found(format!("flag {key:?} not found")))
    }

    pub async fn dry_run_audience(
        &self,
        subject: &str,
        props: &PropertySet,
        key: &str,
    ) -> Result<bool> {
        let a = self.audiences.get(key).await?;
        let identity = synthetic_identity(subject, props);
        Ok(a.contains(subject, &flatten_with_identity(&identity, props)))
    }
}

pub fn synthetic_identity(subject: &str, props: &PropertySet) -> Identity {
    let now = chrono::Utc::now();
    Identity {
        id: uuid::Uuid::nil(),
        external_id: subject.to_string(),
        properties: props.clone(),
        created_at: now,
        updated_at: now,
        last_seen_at: now,
    }
}

/// Flattened properties plus `identity.id` / `identity.external_id` so rules can target them.
pub fn flatten_with_identity(identity: &Identity, props: &PropertySet) -> PropertyMap {
    let mut flat = props.flatten();
    flat.insert("identity.id".into(), Value::String(identity.id.to_string()));
    flat.insert(
        "identity.external_id".into(),
        Value::String(identity.external_id.clone()),
    );
    flat
}
