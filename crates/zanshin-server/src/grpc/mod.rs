#![allow(clippy::result_large_err)]

//! gRPC surface (tonic) mirroring the REST API.

mod convert;

use std::net::SocketAddr;

use tokio::sync::watch;
use tonic::{metadata::MetadataMap, Request, Response, Status};
use zanshin_core::Error;
use zanshin_proto::api::v1::{
    zanshin_server::{Zanshin, ZanshinServer},
    AuthenticateLogoutRequest, AuthenticateRefreshRequest, AuthenticateRequest, Identity,
    Properties, Session, UpdatePropertiesRequest,
};

use crate::{services::identity::SessionContext, state::AppState};

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

    async fn require_client_key(&self, meta: &MetadataMap) -> Result<(), Status> {
        match Self::api_key(meta) {
            Some(k) => {
                self.state
                    .api_keys
                    .authenticate(&k)
                    .await
                    .map_err(to_status)?;
                Ok(())
            }
            None if self.state.cfg.auth.allow_anonymous_authenticate => Ok(()),
            None => Err(Status::unauthenticated("api key required")),
        }
    }

    async fn require_session(&self, meta: &MetadataMap) -> Result<SessionContext, Status> {
        let auth = meta
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("session token required"))?;
        let token = auth
            .strip_prefix("Bearer ")
            .ok_or_else(|| Status::unauthenticated("session token required"))?;
        self.state
            .identity
            .verify_access(token.trim())
            .await
            .map_err(to_status)
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
        self.state
            .identity
            .logout(token, refresh)
            .await
            .map_err(to_status)?;
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

    async fn healthcheck(&self, _request: Request<()>) -> Result<Response<()>, Status> {
        Ok(Response::new(()))
    }
}

pub async fn serve(
    state: AppState,
    addr: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<ZanshinServer<ZanshinGrpc>>()
        .await;
    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(zanshin_proto::FILE_DESCRIPTOR_SET)
        .register_encoded_file_descriptor_set(tonic_health::pb::FILE_DESCRIPTOR_SET)
        .build_v1()?;

    tracing::info!(%addr, "grpc listening");
    tonic::transport::Server::builder()
        .add_service(health_service)
        .add_service(reflection)
        .add_service(ZanshinServer::new(ZanshinGrpc::new(state)))
        .serve_with_shutdown(addr, async move {
            let _ = shutdown.wait_for(|v| *v).await;
        })
        .await?;
    Ok(())
}
