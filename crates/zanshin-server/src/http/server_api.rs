//! Trusted-backend API, `/server/v1`. Requires a `server` or `admin` API key and addresses
//! identities by external id — this is what the Nakama module and game servers call.

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use zanshin_core::Error;

use crate::state::AppState;

use super::{
    auth::ServerAuth,
    error::ApiResult,
    v1::{split_names, FlagList},
};

pub fn router() -> Router<AppState> {
    Router::new().route("/flags", get(flags))
}

#[derive(Debug, Deserialize)]
pub struct ServerFlagsQuery {
    pub external_id: String,
    #[serde(default)]
    pub names: Option<String>,
}

async fn flags(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Query(q): Query<ServerFlagsQuery>,
) -> ApiResult<Json<FlagList>> {
    if q.external_id.is_empty() {
        return Err(Error::invalid("external_id is required").into());
    }
    let flags = state
        .flags
        .evaluate_by_external_id(&q.external_id, &split_names(q.names))
        .await?;
    Ok(Json(FlagList { flags }))
}
