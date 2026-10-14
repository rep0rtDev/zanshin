//! Cross-entity reference checks: deleting something another entity still points at is a 409.

use zanshin_core::journeys::{Action, Trigger};
use zanshin_core::{Error, Result};

use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Flag,
    Audience,
    Template,
    Integration,
    Metric,
    Experiment,
    LiveEvent,
}

impl RefKind {
    fn label(self) -> &'static str {
        match self {
            RefKind::Flag => "flag",
            RefKind::Audience => "audience",
            RefKind::Template => "template",
            RefKind::Integration => "integration",
            RefKind::Metric => "metric",
            RefKind::Experiment => "experiment",
            RefKind::LiveEvent => "live event",
        }
    }
}

/// Everything that still references `key`, as `kind:key` strings.
pub async fn referenced_by(state: &AppState, kind: RefKind, key: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    match kind {
        RefKind::Flag => {
            for e in state.experiments.list().await {
                if e.flag_key.as_deref() == Some(key) {
                    out.push(format!("experiment:{}", e.key));
                }
            }
            for e in state.live_events.list().await {
                if e.flag_overrides.contains_key(key) {
                    out.push(format!("live_event:{}", e.key));
                }
            }
        }
        RefKind::Audience => {
            for u in state.audiences.usage(key).await? {
                out.push(format!("{}:{}", u.kind, u.key));
            }
        }
        RefKind::Template => {
            for m in state.messages.list().await {
                if m.template_key.as_deref() == Some(key) {
                    out.push(format!("message:{}", m.key));
                }
            }
        }
        RefKind::Integration => {
            for m in state.messages.list().await {
                if m.channels.iter().any(|c| c == key) {
                    out.push(format!("message:{}", m.key));
                }
            }
        }
        RefKind::Metric => {
            for e in state.experiments.list().await {
                if e.metrics.iter().any(|m| m == key) {
                    out.push(format!("experiment:{}", e.key));
                }
            }
        }
        RefKind::Experiment | RefKind::LiveEvent => {}
    }
    if kind == RefKind::LiveEvent {
        for m in state.messages.list().await {
            if let zanshin_core::messages::Schedule::LiveEvent { live_event, .. } = &m.schedule {
                if live_event == key {
                    out.push(format!("message:{}", m.key));
                }
            }
        }
    }
    for j in state.journeys.list().await? {
        let trigger_hit = match (&j.trigger, kind) {
            (Trigger::AudienceEnter { audience, .. }, RefKind::Audience) => audience == key,
            (Trigger::LiveEventStart { live_event, .. }, RefKind::LiveEvent) => live_event == key,
            _ => false,
        };
        let step_hit = j.steps.iter().any(|s| match (&s.action, kind) {
            (Action::Message { template_key, .. }, RefKind::Template) => {
                template_key.as_deref() == Some(key)
            }
            (Action::FlagOverride { flag, .. }, RefKind::Flag) => flag == key,
            (Action::ExperimentAssign { experiment, .. }, RefKind::Experiment) => experiment == key,
            (Action::Integration { integration, .. }, RefKind::Integration) => integration == key,
            _ => false,
        });
        if trigger_hit
            || step_hit
            || (kind == RefKind::Audience && j.audiences.iter().any(|a| a == key))
        {
            out.push(format!("journey:{}", j.key));
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Fail with 409 (and the list of referrers) when `key` is still in use.
pub async fn ensure_unreferenced(state: &AppState, kind: RefKind, key: &str) -> Result<()> {
    let refs = referenced_by(state, kind, key).await?;
    if refs.is_empty() {
        return Ok(());
    }
    Err(Error::conflict(format!(
        "{} {key:?} is still referenced by {}",
        kind.label(),
        refs.join(", ")
    )))
}
