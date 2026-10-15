#![allow(clippy::result_large_err)]

//! gRPC surface (tonic) mirroring the REST API.

mod convert;
mod metrics;

use std::{net::SocketAddr, pin::Pin};

use futures::Stream;
use tokio::sync::watch;
use tonic::{metadata::MetadataMap, Request, Response, Status};
use zanshin_core::Error;
use zanshin_proto::api::v1::{
    flag_event,
    zanshin_server::{Zanshin, ZanshinServer},
    AudienceList, AuthenticateLogoutRequest, AuthenticateRefreshRequest, AuthenticateRequest,
    EventBatch, ExperimentList, ExposeExperimentRequest, Flag, FlagEvent, FlagList,
    FlagOverrideList, GetExperimentsRequest, GetFlagRequest, GetFlagsRequest, GetLiveEventsRequest,
    GetMessagesRequest, IdentifyRequest, Identity, IdentityWithProperties, LeaveLiveEventResponse,
    LiveEvent, LiveEventKeyRequest, LiveEventList, Message, MessageIdRequest, MessageList,
    Properties, PushToken, PushTokenList, RegisterPushTokenRequest, SendEventsResponse,
    ServerAudienceMembersRequest, ServerAudienceMembersResponse, ServerEventBatch,
    ServerGetExperimentsRequest, ServerGetFlagRequest, ServerGetFlagsRequest,
    ServerGetLiveEventsRequest, ServerGetMessagesRequest, ServerIdentifyRequest,
    ServerIdentityRequest, ServerLiveEventRequest, ServerMessageRequest, ServerSendMessageRequest,
    ServerUpdatePropertiesRequest, ServerUpsertIdentityRequest, Session,
    UnregisterPushTokenRequest, UpdateMessageRequest, UpdatePropertiesRequest,
};

use crate::{
    services::{apikeys::Principal, identity::SessionContext, limits::Scope},
    state::AppState,
};

pub use convert::*;

pub struct ZanshinGrpc {
    state: AppState,
}

impl ZanshinGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    fn api_key(meta: &MetadataMap) -> Option<String> {
        if let Some(v) = meta.get("x-api-key").and_then(|v| v.to_str().ok()) {
            return Some(v.trim().to_string());
        }
        let auth = meta.get("authorization").and_then(|v| v.to_str().ok())?;
        if let Some(b64) = auth.strip_prefix("Basic ") {
            use base64::{engine::general_purpose::STANDARD, Engine};
            let decoded = STANDARD.decode(b64.trim()).ok()?;
            let s = String::from_utf8(decoded).ok()?;
            return s
                .split(':')
                .next()
                .map(|u| u.trim().to_string())
                .filter(|u| !u.is_empty());
        }
        if let Some(tok) = auth.strip_prefix("Bearer ") {
            if zanshin_core::apikey::looks_like_key(tok.trim()) {
                return Some(tok.trim().to_string());
            }
        }
        None
    }

    /// Same token buckets as HTTP (`limits.*`), keyed by key id / identity.
    async fn admit(&self, scope: Scope, id: &str, per_minute: Option<u32>) -> Result<(), Status> {
        let d = self.state.limits.check(scope, id, per_minute).await;
        if d.allowed {
            return Ok(());
        }
        let retry = d.retry_after.ceil().max(1.0) as u64;
        let mut st = Status::resource_exhausted(format!(
            "rate limit exceeded for {} scope, retry in {retry}s",
            scope.as_str()
        ));
        if let Ok(v) = retry.to_string().parse() {
            st.metadata_mut().insert("retry-after", v);
        }
        Err(st)
    }

    async fn admit_principal(&self, p: &Principal) -> Result<(), Status> {
        let scope = match p.kind {
            zanshin_core::apikey::ApiKeyKind::Client => Scope::Client,
            zanshin_core::apikey::ApiKeyKind::Server => Scope::Server,
            zanshin_core::apikey::ApiKeyKind::Admin => Scope::Admin,
        };
        let id = p
            .key_id
            .map(|u| u.to_string())
            .unwrap_or_else(|| "bootstrap".into());
        self.admit(scope, &id, p.rate_limit_per_minute).await
    }

    async fn require_client_key(&self, meta: &MetadataMap) -> Result<(), Status> {
        match Self::api_key(meta) {
            Some(k) => {
                let p = self
                    .state
                    .api_keys
                    .authenticate(&k)
                    .await
                    .map_err(to_status)?;
                self.admit_principal(&p).await
            }
            None if self.state.cfg.auth.allow_anonymous_authenticate => Ok(()),
            None => Err(Status::unauthenticated("api key required")),
        }
    }

    async fn require_server_key(&self, meta: &MetadataMap) -> Result<(), Status> {
        let k = Self::api_key(meta)
            .ok_or_else(|| Status::unauthenticated("server api key required"))?;
        let p = self
            .state
            .api_keys
            .authenticate(&k)
            .await
            .map_err(to_status)?;
        if !p.is_server() {
            return Err(Status::permission_denied("server api key required"));
        }
        self.admit_principal(&p).await
    }

    async fn require_session(&self, meta: &MetadataMap) -> Result<SessionContext, Status> {
        let auth = meta
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("session token required"))?;
        let token = auth
            .strip_prefix("Bearer ")
            .ok_or_else(|| Status::unauthenticated("session token required"))?;
        let ctx = self
            .state
            .identity
            .verify_access(token.trim())
            .await
            .map_err(to_status)?;
        self.admit(Scope::Session, &ctx.identity_id().to_string(), None)
            .await?;
        Ok(ctx)
    }
}

pub fn to_status(e: Error) -> Status {
    match e {
        Error::InvalidArgument(m) => Status::invalid_argument(m),
        Error::Unauthenticated(m) => Status::unauthenticated(m),
        Error::PermissionDenied(m) => Status::permission_denied(m),
        Error::NotFound(m) => Status::not_found(m),
        Error::Conflict(m) => Status::already_exists(m),
        Error::Unavailable(m) => Status::unavailable(m),
        Error::ResourceExhausted(m) => Status::resource_exhausted(m),
        Error::Internal(err) => {
            tracing::error!(error = ?err, "internal error");
            Status::internal("internal error")
        }
    }
}

#[tonic::async_trait]
impl Zanshin for ZanshinGrpc {
    async fn authenticate(
        &self,
        request: Request<AuthenticateRequest>,
    ) -> Result<Response<Session>, Status> {
        self.require_client_key(request.metadata()).await?;
        let req = request.into_inner();
        let s = self
            .state
            .identity
            .authenticate(
                &req.id,
                value_map_to_json(req.default),
                value_map_to_json(req.custom),
            )
            .await
            .map_err(to_status)?;
        self.state
            .events
            .system(
                &s.identity,
                zanshin_core::events::EV_AUTHENTICATE,
                "",
                None,
                None,
            )
            .await;
        crate::http::v1::begin_play_session(&self.state, &s).await;
        Ok(Response::new(session_to_proto(s)))
    }

    async fn authenticate_refresh(
        &self,
        request: Request<AuthenticateRefreshRequest>,
    ) -> Result<Response<Session>, Status> {
        self.require_client_key(request.metadata()).await?;
        let s = self
            .state
            .identity
            .refresh(&request.into_inner().refresh_token)
            .await
            .map_err(to_status)?;
        Ok(Response::new(session_to_proto(s)))
    }

    async fn authenticate_logout(
        &self,
        request: Request<AuthenticateLogoutRequest>,
    ) -> Result<Response<()>, Status> {
        self.require_client_key(request.metadata()).await?;
        let req = request.into_inner();
        let token = if req.token.is_empty() {
            None
        } else {
            Some(req.token.as_str())
        };
        let refresh = if req.refresh_token.is_empty() {
            None
        } else {
            Some(req.refresh_token.as_str())
        };
        let sid = self
            .state
            .identity
            .logout(token, refresh)
            .await
            .map_err(to_status)?;
        crate::http::v1::end_play_session(&self.state, sid).await;
        Ok(Response::new(()))
    }

    async fn get_identity(&self, request: Request<()>) -> Result<Response<Identity>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let i = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        Ok(Response::new(identity_to_proto(&i)))
    }

    async fn delete_identity(&self, request: Request<()>) -> Result<Response<()>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        self.state
            .identity
            .delete(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let _ = self.state.events.delete_identity(ctx.identity_id()).await;
        Ok(Response::new(()))
    }

    async fn get_properties(&self, request: Request<()>) -> Result<Response<Properties>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let i = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        Ok(Response::new(properties_to_proto(&i.properties)))
    }

    async fn update_properties(
        &self,
        request: Request<UpdatePropertiesRequest>,
    ) -> Result<Response<Properties>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let req = request.into_inner();
        let i = self
            .state
            .identity
            .update_properties(
                ctx.identity_id(),
                value_map_to_json(req.default),
                value_map_to_json(req.custom),
            )
            .await
            .map_err(to_status)?;
        Ok(Response::new(properties_to_proto(&i.properties)))
    }

    async fn get_flags(
        &self,
        request: Request<GetFlagsRequest>,
    ) -> Result<Response<FlagList>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let evals = self
            .state
            .eval
            .flags(&identity, &request.into_inner().names)
            .await
            .map_err(to_status)?;
        Ok(Response::new(FlagList {
            flags: evals.into_iter().map(evaluation_to_proto).collect(),
        }))
    }

    async fn get_flag(&self, request: Request<GetFlagRequest>) -> Result<Response<Flag>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let ev = self
            .state
            .eval
            .flag(&identity, &request.into_inner().name)
            .await
            .map_err(to_status)?;
        Ok(Response::new(evaluation_to_proto(ev)))
    }

    type StreamFlagsStream =
        Pin<Box<dyn Stream<Item = Result<FlagEvent, Status>> + Send + 'static>>;

    async fn stream_flags(
        &self,
        request: Request<GetFlagsRequest>,
    ) -> Result<Response<Self::StreamFlagsStream>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let names = request.into_inner().names;
        let state = self.state.clone();
        let mut rx = state.bus.subscribe();
        let expires_at = ctx.claims.expires_at();
        let identity_id = ctx.identity_id();
        let stream = async_stream(
            move |tx: tokio::sync::mpsc::Sender<Result<FlagEvent, Status>>| async move {
                loop {
                    if chrono::Utc::now() >= expires_at {
                        let _ = tx
                            .send(Err(Status::unauthenticated("session expired")))
                            .await;
                        return;
                    }
                    let change = match rx.recv().await {
                        Ok(c) => c,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => return,
                    };
                    if change.identity_id.is_some_and(|id| id != identity_id) {
                        continue;
                    }
                    let Ok(identity) = state.identity.get(identity_id).await else {
                        return;
                    };
                    let events: Vec<FlagEvent> = match change.kind {
                        crate::services::bus::ChangeKind::Flag => {
                            if !names.is_empty() && !names.contains(&change.key) {
                                continue;
                            }
                            if change.deleted {
                                vec![FlagEvent {
                                    event: Some(flag_event::Event::Deleted(change.key)),
                                }]
                            } else {
                                match state.eval.flag(&identity, &change.key).await {
                                    Ok(ev) => vec![FlagEvent {
                                        event: Some(flag_event::Event::Flag(evaluation_to_proto(
                                            ev,
                                        ))),
                                    }],
                                    Err(_) => continue,
                                }
                            }
                        }
                        // Audience / experiment changes can move any flag: re-send the requested set.
                        _ => match state.eval.flags(&identity, &names).await {
                            Ok(evs) => evs
                                .into_iter()
                                .map(|ev| FlagEvent {
                                    event: Some(flag_event::Event::Flag(evaluation_to_proto(ev))),
                                })
                                .collect(),
                            Err(_) => continue,
                        },
                    };
                    for ev in events {
                        if tx.send(Ok(ev)).await.is_err() {
                            return;
                        }
                    }
                }
            },
        );
        Ok(Response::new(Box::pin(stream)))
    }

    async fn get_audiences(&self, request: Request<()>) -> Result<Response<AudienceList>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        Ok(Response::new(AudienceList {
            audiences: self.state.eval.audience_keys(&identity).await,
        }))
    }

    async fn get_experiments(
        &self,
        request: Request<GetExperimentsRequest>,
    ) -> Result<Response<ExperimentList>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let a = self
            .state
            .eval
            .assignments(&identity, &request.into_inner().names)
            .await
            .map_err(to_status)?;
        Ok(Response::new(ExperimentList {
            experiments: a.into_iter().map(assignment_to_proto).collect(),
        }))
    }

    async fn expose_experiment(
        &self,
        request: Request<ExposeExperimentRequest>,
    ) -> Result<Response<()>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        self.state
            .experiments
            .expose(&identity, &request.into_inner().name)
            .await
            .map_err(to_status)?;
        Ok(Response::new(()))
    }

    async fn server_get_flags(
        &self,
        request: Request<ServerGetFlagsRequest>,
    ) -> Result<Response<FlagList>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let evals = self
            .state
            .eval
            .flags(&identity, &req.names)
            .await
            .map_err(to_status)?;
        Ok(Response::new(FlagList {
            flags: evals.into_iter().map(evaluation_to_proto).collect(),
        }))
    }

    async fn server_get_audiences(
        &self,
        request: Request<ServerIdentityRequest>,
    ) -> Result<Response<AudienceList>, Status> {
        self.require_server_key(request.metadata()).await?;
        let identity = self
            .state
            .eval
            .identity_by_external_id(&request.into_inner().external_id)
            .await
            .map_err(to_status)?;
        Ok(Response::new(AudienceList {
            audiences: self.state.eval.audience_keys(&identity).await,
        }))
    }

    async fn server_get_experiments(
        &self,
        request: Request<ServerGetExperimentsRequest>,
    ) -> Result<Response<ExperimentList>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let a = self
            .state
            .eval
            .assignments(&identity, &req.names)
            .await
            .map_err(to_status)?;
        Ok(Response::new(ExperimentList {
            experiments: a.into_iter().map(assignment_to_proto).collect(),
        }))
    }

    async fn get_live_events(
        &self,
        request: Request<GetLiveEventsRequest>,
    ) -> Result<Response<LiveEventList>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let req = request.into_inner();
        let events = self
            .state
            .eval
            .live_events(&identity, &req.names, !req.include_upcoming)
            .await;
        Ok(Response::new(LiveEventList {
            live_events: events.into_iter().map(live_event_to_proto).collect(),
        }))
    }

    async fn get_messages(
        &self,
        request: Request<GetMessagesRequest>,
    ) -> Result<Response<MessageList>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let limit = match request.into_inner().limit {
            0 => 50,
            n => n as i64,
        };
        let inbox = self
            .state
            .eval
            .inbox(&identity, limit)
            .await
            .map_err(to_status)?;
        Ok(Response::new(MessageList {
            messages: inbox.into_iter().map(inbox_to_proto).collect(),
        }))
    }

    async fn update_message(
        &self,
        request: Request<UpdateMessageRequest>,
    ) -> Result<Response<Message>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let req = request.into_inner();
        let id = uuid::Uuid::parse_str(&req.id)
            .map_err(|_| Status::invalid_argument("id must be a uuid"))?;
        let m = self
            .state
            .messages
            .mark_read(&identity, id)
            .await
            .map_err(to_status)?;
        Ok(Response::new(inbox_to_proto(m)))
    }

    async fn delete_message(
        &self,
        request: Request<MessageIdRequest>,
    ) -> Result<Response<()>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let id = uuid::Uuid::parse_str(&request.into_inner().id)
            .map_err(|_| Status::invalid_argument("id must be a uuid"))?;
        self.state
            .messages
            .consume(&identity, id)
            .await
            .map_err(to_status)?;
        Ok(Response::new(()))
    }

    async fn server_get_live_events(
        &self,
        request: Request<ServerGetLiveEventsRequest>,
    ) -> Result<Response<LiveEventList>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let events = self
            .state
            .eval
            .live_events(&identity, &req.names, !req.include_upcoming)
            .await;
        Ok(Response::new(LiveEventList {
            live_events: events.into_iter().map(live_event_to_proto).collect(),
        }))
    }

    async fn server_send_message(
        &self,
        request: Request<ServerSendMessageRequest>,
    ) -> Result<Response<MessageIdRequest>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let msg = crate::services::messages::DirectMessage {
            push_options: None,
            inbox: true,
            template_key: None,
            template_name: None,
            channels: Vec::new(),
            locale: None,
            push: false,
            title: req.title,
            body: req.body,
            image_url: req.image_url,
            payload: req
                .payload
                .map(convert::value_to_json)
                .unwrap_or(serde_json::Value::Null),
            priority: req.priority,
            ttl_secs: (req.ttl_secs > 0).then_some(req.ttl_secs),
        };
        let id = self
            .state
            .messages
            .send_direct(&identity, msg)
            .await
            .map_err(to_status)?;
        Ok(Response::new(MessageIdRequest { id: id.to_string() }))
    }

    async fn send_events(
        &self,
        request: Request<EventBatch>,
    ) -> Result<Response<SendEventsResponse>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let inputs: Vec<_> = request
            .into_inner()
            .events
            .into_iter()
            .map(convert::event_from_proto)
            .collect();
        let checked = self
            .state
            .taxonomy
            .check_events(identity.id, &identity.external_id, inputs)
            .await
            .map_err(to_status)?;
        let n = self
            .state
            .events
            .track(&identity, checked.events, false, Some(ctx.claims.sid))
            .await
            .map_err(to_status)?;
        if !checked.errors.is_empty() {
            self.state
                .events
                .track(&identity, checked.errors, true, Some(ctx.claims.sid))
                .await
                .ok();
        }
        Ok(Response::new(SendEventsResponse { accepted: n as i32 }))
    }

    async fn server_send_events(
        &self,
        request: Request<ServerEventBatch>,
    ) -> Result<Response<SendEventsResponse>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let inputs: Vec<_> = req
            .events
            .into_iter()
            .map(convert::event_from_proto)
            .collect();
        let n = self
            .state
            .events
            .track(&identity, inputs, true, None)
            .await
            .map_err(to_status)?;
        Ok(Response::new(SendEventsResponse { accepted: n as i32 }))
    }

    async fn identify(
        &self,
        request: Request<IdentifyRequest>,
    ) -> Result<Response<Session>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let from = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let (s, merged) = self
            .state
            .identity
            .identify(from.id, &request.into_inner().id)
            .await
            .map_err(to_status)?;
        self.state
            .taxonomy
            .repo
            .close_session(ctx.claims.sid, chrono::Utc::now())
            .await
            .ok();
        if let Some(src) = &merged {
            self.state
                .taxonomy
                .repo
                .record_merge(&zanshin_store::taxonomy::IdentityMerge {
                    id: uuid::Uuid::now_v7(),
                    from_identity: src.id,
                    from_external: src.external_id.clone(),
                    into_identity: s.identity.id,
                    into_external: s.identity.external_id.clone(),
                    merged_at: chrono::Utc::now(),
                })
                .await
                .ok();
        }
        let mut meta = serde_json::Map::new();
        meta.insert("from".into(), serde_json::Value::String(from.external_id));
        meta.insert(
            "to".into(),
            serde_json::Value::String(s.identity.external_id.clone()),
        );
        meta.insert("merged".into(), serde_json::Value::Bool(merged.is_some()));
        self.state
            .events
            .system(
                &s.identity,
                zanshin_core::events::EV_IDENTIFY,
                "",
                Some(meta),
                Some(s.session_id),
            )
            .await;
        crate::http::v1::begin_play_session(&self.state, &s).await;
        Ok(Response::new(session_to_proto(s)))
    }

    async fn get_flag_overrides(
        &self,
        request: Request<()>,
    ) -> Result<Response<FlagOverrideList>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let overrides = self
            .state
            .eval
            .overrides(&identity)
            .await
            .map_err(to_status)?;
        Ok(Response::new(FlagOverrideList {
            overrides: overrides.into_iter().map(override_to_proto).collect(),
        }))
    }

    async fn join_live_event(
        &self,
        request: Request<LiveEventKeyRequest>,
    ) -> Result<Response<LiveEvent>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let key = request.into_inner().key;
        let view = self
            .state
            .eval
            .join_live_event(&identity, &key)
            .await
            .map_err(to_status)?;
        self.state
            .events
            .system(
                &identity,
                zanshin_core::events::EV_LIVE_EVENT_JOIN,
                &key,
                None,
                None,
            )
            .await;
        Ok(Response::new(live_event_to_proto(view)))
    }

    async fn leave_live_event(
        &self,
        request: Request<LiveEventKeyRequest>,
    ) -> Result<Response<LeaveLiveEventResponse>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let left = self
            .state
            .live_events
            .leave(ctx.identity_id(), &request.into_inner().key)
            .await
            .map_err(to_status)?;
        Ok(Response::new(LeaveLiveEventResponse { left }))
    }

    async fn remove_message(
        &self,
        request: Request<MessageIdRequest>,
    ) -> Result<Response<()>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let identity = self
            .state
            .identity
            .get(ctx.identity_id())
            .await
            .map_err(to_status)?;
        let id = uuid::Uuid::parse_str(&request.into_inner().id)
            .map_err(|_| Status::invalid_argument("id must be a uuid"))?;
        self.state
            .messages
            .delete_inbox(&identity, id)
            .await
            .map_err(to_status)?;
        Ok(Response::new(()))
    }

    async fn register_push_token(
        &self,
        request: Request<RegisterPushTokenRequest>,
    ) -> Result<Response<PushToken>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let req = request.into_inner();
        let platform = zanshin_core::push::Platform::parse(&req.platform).map_err(to_status)?;
        let t = self
            .state
            .push
            .register(ctx.identity_id(), platform, &req.token, &req.device_id)
            .await
            .map_err(to_status)?;
        Ok(Response::new(push_token_to_proto(t)))
    }

    async fn get_push_tokens(
        &self,
        request: Request<()>,
    ) -> Result<Response<PushTokenList>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        let tokens = self
            .state
            .push
            .tokens(ctx.identity_id())
            .await
            .map_err(to_status)?;
        Ok(Response::new(PushTokenList {
            tokens: tokens.into_iter().map(push_token_to_proto).collect(),
        }))
    }

    async fn unregister_push_token(
        &self,
        request: Request<UnregisterPushTokenRequest>,
    ) -> Result<Response<()>, Status> {
        let ctx = self.require_session(request.metadata()).await?;
        self.state
            .push
            .unregister(ctx.identity_id(), &request.into_inner().token)
            .await
            .map_err(to_status)?;
        Ok(Response::new(()))
    }

    async fn server_get_flag(
        &self,
        request: Request<ServerGetFlagRequest>,
    ) -> Result<Response<Flag>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let ev = self
            .state
            .eval
            .flag(&identity, &req.name)
            .await
            .map_err(to_status)?;
        Ok(Response::new(evaluation_to_proto(ev)))
    }

    async fn server_explain_flag(
        &self,
        request: Request<ServerGetFlagRequest>,
    ) -> Result<Response<prost_types::Struct>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let doc = self
            .state
            .eval
            .explain_flag(&identity, &req.name)
            .await
            .map_err(to_status)?;
        Ok(Response::new(json_to_struct(doc)))
    }

    async fn server_upsert_identity(
        &self,
        request: Request<ServerUpsertIdentityRequest>,
    ) -> Result<Response<IdentityWithProperties>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let i = self
            .state
            .identity
            .upsert(
                &req.external_id,
                value_map_to_json(req.default),
                value_map_to_json(req.custom),
            )
            .await
            .map_err(to_status)?;
        self.state
            .events
            .system(&i, zanshin_core::events::EV_AUTHENTICATE, "", None, None)
            .await;
        Ok(Response::new(identity_with_properties(&i)))
    }

    async fn server_identify(
        &self,
        request: Request<ServerIdentifyRequest>,
    ) -> Result<Response<IdentityWithProperties>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let from = self
            .state
            .identity
            .get_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let (s, merged) = self
            .state
            .identity
            .identify(from.id, &req.id)
            .await
            .map_err(to_status)?;
        if let Some(src) = &merged {
            self.state
                .taxonomy
                .repo
                .record_merge(&zanshin_store::taxonomy::IdentityMerge {
                    id: uuid::Uuid::now_v7(),
                    from_identity: src.id,
                    from_external: src.external_id.clone(),
                    into_identity: s.identity.id,
                    into_external: s.identity.external_id.clone(),
                    merged_at: chrono::Utc::now(),
                })
                .await
                .ok();
        }
        Ok(Response::new(identity_with_properties(&s.identity)))
    }

    async fn server_update_properties(
        &self,
        request: Request<ServerUpdatePropertiesRequest>,
    ) -> Result<Response<Properties>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let i = self
            .state
            .identity
            .update_properties(
                identity.id,
                value_map_to_json(req.default),
                value_map_to_json(req.custom),
            )
            .await
            .map_err(to_status)?;
        Ok(Response::new(properties_to_proto(&i.properties)))
    }

    async fn server_join_live_event(
        &self,
        request: Request<ServerLiveEventRequest>,
    ) -> Result<Response<LiveEvent>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let view = self
            .state
            .eval
            .join_live_event(&identity, &req.key)
            .await
            .map_err(to_status)?;
        self.state
            .events
            .system(
                &identity,
                zanshin_core::events::EV_LIVE_EVENT_JOIN,
                &req.key,
                None,
                None,
            )
            .await;
        Ok(Response::new(live_event_to_proto(view)))
    }

    async fn server_leave_live_event(
        &self,
        request: Request<ServerLiveEventRequest>,
    ) -> Result<Response<LeaveLiveEventResponse>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let left = self
            .state
            .live_events
            .leave(identity.id, &req.key)
            .await
            .map_err(to_status)?;
        Ok(Response::new(LeaveLiveEventResponse { left }))
    }

    async fn server_get_messages(
        &self,
        request: Request<ServerGetMessagesRequest>,
    ) -> Result<Response<MessageList>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let limit = if req.limit > 0 { req.limit as i64 } else { 50 };
        let msgs = self
            .state
            .eval
            .inbox(&identity, limit)
            .await
            .map_err(to_status)?;
        Ok(Response::new(MessageList {
            messages: msgs.into_iter().map(inbox_to_proto).collect(),
        }))
    }

    async fn server_read_message(
        &self,
        request: Request<ServerMessageRequest>,
    ) -> Result<Response<Message>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let id = uuid::Uuid::parse_str(&req.id)
            .map_err(|_| Status::invalid_argument("id must be a uuid"))?;
        let m = self
            .state
            .messages
            .mark_read(&identity, id)
            .await
            .map_err(to_status)?;
        Ok(Response::new(inbox_to_proto(m)))
    }

    async fn server_consume_message(
        &self,
        request: Request<ServerMessageRequest>,
    ) -> Result<Response<()>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let id = uuid::Uuid::parse_str(&req.id)
            .map_err(|_| Status::invalid_argument("id must be a uuid"))?;
        self.state
            .messages
            .consume(&identity, id)
            .await
            .map_err(to_status)?;
        Ok(Response::new(()))
    }

    async fn server_remove_message(
        &self,
        request: Request<ServerMessageRequest>,
    ) -> Result<Response<()>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let identity = self
            .state
            .eval
            .identity_by_external_id(&req.external_id)
            .await
            .map_err(to_status)?;
        let id = uuid::Uuid::parse_str(&req.id)
            .map_err(|_| Status::invalid_argument("id must be a uuid"))?;
        self.state
            .messages
            .delete_inbox(&identity, id)
            .await
            .map_err(to_status)?;
        Ok(Response::new(()))
    }

    async fn server_update_audience_members(
        &self,
        request: Request<ServerAudienceMembersRequest>,
    ) -> Result<Response<ServerAudienceMembersResponse>, Status> {
        self.require_server_key(request.metadata()).await?;
        let req = request.into_inner();
        let mut added = 0;
        let mut removed = 0;
        if !req.add.is_empty() {
            let (n, _) = self
                .state
                .audiences
                .set_managed(&req.audience, &req.add, true)
                .await
                .map_err(to_status)?;
            added = n;
        }
        if !req.remove.is_empty() {
            let (n, _) = self
                .state
                .audiences
                .set_managed(&req.audience, &req.remove, false)
                .await
                .map_err(to_status)?;
            removed = n;
        }
        Ok(Response::new(ServerAudienceMembersResponse {
            added: added as i32,
            removed: removed as i32,
        }))
    }

    async fn healthcheck(&self, _request: Request<()>) -> Result<Response<()>, Status> {
        Ok(Response::new(()))
    }
}

/// Bridge an async producer onto a `Stream` through a bounded channel.
fn async_stream<T, F, Fut>(producer: F) -> tokio_stream::wrappers::ReceiverStream<T>
where
    T: Send + 'static,
    F: FnOnce(tokio::sync::mpsc::Sender<T>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    tokio::spawn(producer(tx));
    tokio_stream::wrappers::ReceiverStream::new(rx)
}

/// One project's gRPC server with the configured message size limits.
pub fn service(state: AppState) -> ZanshinServer<ZanshinGrpc> {
    let max_msg = state.cfg.server.grpc_max_message_bytes;
    ZanshinServer::new(ZanshinGrpc::new(state))
        .max_decoding_message_size(max_msg)
        .max_encoding_message_size(max_msg)
}

/// Serve a single project (tests / embedded use).
pub async fn serve(
    state: AppState,
    addr: SocketAddr,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let cfg = state.cfg.clone();
    serve_with(service(state), cfg, addr, shutdown).await
}

/// Serve every project: `svc` is the per-project dispatcher (`Projects::grpc_dispatch`).
pub async fn serve_with<S>(
    svc: S,
    cfg: std::sync::Arc<crate::config::Config>,
    addr: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()>
where
    S: tower::Service<
            http::Request<tonic::body::BoxBody>,
            Response = http::Response<tonic::body::BoxBody>,
            Error = std::convert::Infallible,
        > + tonic::server::NamedService
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<ZanshinServer<ZanshinGrpc>>()
        .await;
    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(zanshin_proto::FILE_DESCRIPTOR_SET)
        .register_encoded_file_descriptor_set(tonic_health::pb::FILE_DESCRIPTOR_SET)
        .build_v1()?;

    let cfg = &cfg.server;
    let max_msg = cfg.grpc_max_message_bytes;
    let concurrency = if cfg.grpc_concurrency == 0 {
        usize::MAX
    } else {
        cfg.grpc_concurrency
    };
    let layer = tower::ServiceBuilder::new()
        .layer(metrics::GrpcMetricsLayer)
        .concurrency_limit(concurrency)
        .into_inner();
    tracing::info!(%addr, max_message_bytes = max_msg, concurrency, "grpc listening");
    tonic::transport::Server::builder()
        .timeout(cfg.request_timeout)
        .tcp_keepalive(Some(std::time::Duration::from_secs(60)))
        .http2_keepalive_interval(Some(std::time::Duration::from_secs(30)))
        .http2_keepalive_timeout(Some(std::time::Duration::from_secs(20)))
        .layer(layer)
        .add_service(health_service)
        .add_service(reflection)
        .add_service(svc)
        .serve_with_shutdown(addr, async move {
            let _ = shutdown.wait_for(|v| *v).await;
        })
        .await?;
    tracing::info!("grpc listener closed");
    Ok(())
}
