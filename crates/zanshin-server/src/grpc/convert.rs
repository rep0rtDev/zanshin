//! Conversions between domain types and protobuf messages.

use std::collections::HashMap;

use prost_types::{value::Kind, ListValue, Struct, Timestamp, Value};
use zanshin_core::{
    events::EventInput,
    experiments::Assignment,
    flags::{Evaluation, Reason},
    live_events::{ActiveEvent, Occurrence as CoreOccurrence},
    messages::InboxMessage,
};
use zanshin_core::{properties::PropertyMap, Identity as CoreIdentity, PropertySet};
use zanshin_proto::api::v1::{
    Event as ProtoEvent, Experiment, Flag, Identity, LiveEvent, Message, Occurrence, Properties,
    Session,
};

use crate::services::identity::SessionTokens;

pub fn json_to_value(v: serde_json::Value) -> Value {
    let kind = match v {
        serde_json::Value::Null => Kind::NullValue(0),
        serde_json::Value::Bool(b) => Kind::BoolValue(b),
        serde_json::Value::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => Kind::StringValue(s),
        serde_json::Value::Array(a) => Kind::ListValue(ListValue {
            values: a.into_iter().map(json_to_value).collect(),
        }),
        serde_json::Value::Object(m) => Kind::StructValue(Struct {
            fields: m.into_iter().map(|(k, v)| (k, json_to_value(v))).collect(),
        }),
    };
    Value { kind: Some(kind) }
}

pub fn value_to_json(v: Value) -> serde_json::Value {
    match v.kind {
        None | Some(Kind::NullValue(_)) => serde_json::Value::Null,
        Some(Kind::BoolValue(b)) => serde_json::Value::Bool(b),
        Some(Kind::NumberValue(n)) => serde_json::Number::from_f64(n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Some(Kind::StringValue(s)) => serde_json::Value::String(s),
        Some(Kind::ListValue(l)) => {
            serde_json::Value::Array(l.values.into_iter().map(value_to_json).collect())
        }
        Some(Kind::StructValue(s)) => serde_json::Value::Object(
            s.fields
                .into_iter()
                .map(|(k, v)| (k, value_to_json(v)))
                .collect(),
        ),
    }
}

pub fn value_map_to_json(m: HashMap<String, Value>) -> PropertyMap {
    m.into_iter().map(|(k, v)| (k, value_to_json(v))).collect()
}

pub fn json_map_to_values(m: &PropertyMap) -> HashMap<String, Value> {
    m.iter()
        .map(|(k, v)| (k.clone(), json_to_value(v.clone())))
        .collect()
}

pub fn timestamp(t: chrono::DateTime<chrono::Utc>) -> Timestamp {
    Timestamp {
        seconds: t.timestamp(),
        nanos: t.timestamp_subsec_nanos() as i32,
    }
}

pub fn properties_to_proto(p: &PropertySet) -> Properties {
    Properties {
        default: json_map_to_values(&p.default),
        computed: json_map_to_values(&p.computed),
        custom: json_map_to_values(&p.custom),
    }
}

pub fn identity_to_proto(i: &CoreIdentity) -> Identity {
    Identity {
        id: i.id.to_string(),
        external_id: i.external_id.clone(),
        created_at: Some(timestamp(i.created_at)),
        updated_at: Some(timestamp(i.updated_at)),
        last_seen_at: Some(timestamp(i.last_seen_at)),
    }
}

pub fn session_to_proto(s: SessionTokens) -> Session {
    Session {
        token: s.token,
        refresh_token: s.refresh_token,
        identity: Some(identity_to_proto(&s.identity)),
        properties: Some(properties_to_proto(&s.identity.properties)),
    }
}

pub fn evaluation_to_proto(e: Evaluation) -> Flag {
    let (reason, rule_id, experiment) = match &e.reason {
        Reason::Disabled => ("disabled", String::new(), String::new()),
        Reason::Default => ("default", String::new(), String::new()),
        Reason::Override => ("override", String::new(), String::new()),
        Reason::Rule { rule_id } => ("rule", rule_id.clone(), String::new()),
        Reason::Variant { rule_id, .. } => ("variant", rule_id.clone(), String::new()),
        Reason::Experiment { experiment, .. } => ("experiment", String::new(), experiment.clone()),
    };
    Flag {
        name: e.key,
        value: Some(json_to_value(e.value)),
        value_type: e.value_type.as_str().to_string(),
        variant: e.variant.unwrap_or_default(),
        reason: reason.to_string(),
        rule_id,
        version: e.version,
        experiment,
    }
}

pub fn assignment_to_proto(a: Assignment) -> Experiment {
    Experiment {
        name: a.key,
        display_name: a.name,
        variant: a.variant,
        value: Some(json_to_value(a.value)),
        version: a.version,
    }
}

fn occurrence_to_proto(o: CoreOccurrence) -> Occurrence {
    Occurrence {
        start_unix: o.start.timestamp(),
        end_unix: o.end.timestamp(),
    }
}

pub fn live_event_to_proto(e: ActiveEvent) -> LiveEvent {
    LiveEvent {
        name: e.key,
        display_name: e.name,
        description: e.description,
        value: Some(json_to_value(e.value)),
        active: e.active,
        current: e.current.map(occurrence_to_proto),
        next: e.next.map(occurrence_to_proto),
        version: e.version,
    }
}

pub fn inbox_to_proto(m: InboxMessage) -> Message {
    Message {
        id: m.id.to_string(),
        key: m.key.unwrap_or_default(),
        title: m.title,
        body: m.body,
        image_url: m.image_url,
        payload: Some(json_to_value(m.payload)),
        priority: m.priority,
        delivered_unix: m.delivered_at.timestamp(),
        read_unix: m.read_at.map(|t| t.timestamp()).unwrap_or(0),
        expires_unix: m.expires_at.map(|t| t.timestamp()).unwrap_or(0),
    }
}

pub fn event_from_proto(e: ProtoEvent) -> EventInput {
    let metadata = e.metadata.map(|st| {
        st.fields
            .into_iter()
            .map(|(k, v)| (k, value_to_json(v)))
            .collect()
    });
    EventInput {
        name: e.name,
        value: (!e.value.is_empty()).then_some(e.value),
        metadata,
        timestamp: (e.timestamp_ms > 0)
            .then(|| chrono::DateTime::from_timestamp_millis(e.timestamp_ms))
            .flatten(),
        id: uuid::Uuid::parse_str(&e.id).ok(),
    }
}
