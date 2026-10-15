//! SCIM 2.0 provisioning of console users (`/scim/v2`), bearer-token authenticated. Users map to
//! console users (active ↔ not disabled), Groups map to archetypes via `admin.scim.group_archetypes`.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use zanshin_core::{Error, Result};
use zanshin_store::console_users::{ConsoleUserRow, UserPatch};

use crate::state::AppState;

const SCHEMA_USER: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
const SCHEMA_GROUP: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
const SCHEMA_LIST: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";
const SCHEMA_PATCH: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";
const SCHEMA_ERROR: &str = "urn:ietf:params:scim:api:messages:2.0:Error";
const ACTOR: &str = "scim";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/scim/v2/ServiceProviderConfig", get(sp_config))
        .route("/scim/v2/ResourceTypes", get(resource_types))
        .route("/scim/v2/Schemas", get(schemas))
        .route("/scim/v2/Users", get(list_users).post(create_user))
        .route(
            "/scim/v2/Users/{id}",
            get(get_user)
                .put(replace_user)
                .patch(patch_user)
                .delete(delete_user),
        )
        .route("/scim/v2/Groups", get(list_groups))
        .route(
            "/scim/v2/Groups/{id}",
            get(get_group).patch(patch_group).put(patch_group),
        )
}

struct ScimError(StatusCode, String);

impl IntoResponse for ScimError {
    fn into_response(self) -> Response {
        let body = json!({ "schemas": [SCHEMA_ERROR], "status": self.0.as_u16().to_string(), "detail": self.1 });
        (
            self.0,
            [(header::CONTENT_TYPE, "application/scim+json")],
            Json(body),
        )
            .into_response()
    }
}

impl From<Error> for ScimError {
    fn from(e: Error) -> Self {
        let st = match &e {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::InvalidArgument(_) => StatusCode::BAD_REQUEST,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::Unauthenticated(_) => StatusCode::UNAUTHORIZED,
            Error::PermissionDenied(_) => StatusCode::FORBIDDEN,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ScimError(st, e.to_string())
    }
}

type ScimResult<T> = std::result::Result<T, ScimError>;

fn scim_json(status: StatusCode, v: Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/scim+json")],
        Json(v),
    )
        .into_response()
}

/// Bearer token check (constant time). 404 when SCIM is not configured so the surface is invisible.
fn authorize(state: &AppState, headers: &HeaderMap) -> ScimResult<()> {
    let Some(cfg) = state.cfg.admin.scim.as_ref() else {
        return Err(ScimError(
            StatusCode::NOT_FOUND,
            "scim is not enabled".into(),
        ));
    };
    let expected = cfg.token();
    if expected.is_empty() {
        return Err(ScimError(
            StatusCode::NOT_FOUND,
            "scim is not enabled".into(),
        ));
    }
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let a = presented.as_bytes();
    let b = expected.as_bytes();
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= (*a.get(i).unwrap_or(&0) ^ *b.get(i).unwrap_or(&0)) as usize;
    }
    if diff != 0 {
        return Err(ScimError(
            StatusCode::UNAUTHORIZED,
            "invalid scim token".into(),
        ));
    }
    Ok(())
}

fn user_json(base: &str, u: &ConsoleUserRow) -> Value {
    let (given, family) = match u.name.split_once(' ') {
        Some((g, f)) => (g.to_string(), f.to_string()),
        None => (u.name.clone(), String::new()),
    };
    json!({
        "schemas": [SCHEMA_USER],
        "id": u.id,
        "userName": u.email,
        "displayName": u.name,
        "name": { "formatted": u.name, "givenName": given, "familyName": family },
        "emails": [{ "value": u.email, "primary": true, "type": "work" }],
        "active": !u.disabled,
        "groups": u.archetype.as_ref().map(|a| vec![json!({ "value": a, "display": a })]).unwrap_or_default(),
        "meta": {
            "resourceType": "User",
            "created": u.created_at,
            "lastModified": u.updated_at,
            "location": format!("{base}/scim/v2/Users/{}", u.id),
        }
    })
}

fn base_url(state: &AppState, headers: &HeaderMap) -> String {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or(if state.cfg.server.production {
            "https"
        } else {
            "http"
        });
    format!("{proto}://{host}")
}

#[derive(Deserialize, Default)]
struct ListQuery {
    #[serde(default)]
    filter: Option<String>,
    #[serde(default, rename = "startIndex")]
    start_index: Option<usize>,
    #[serde(default)]
    count: Option<usize>,
}

/// Supports `userName eq "x"`, `emails.value eq "x"`, `externalId eq "x"` and `displayName eq "x"`.
fn filter_eq(filter: &str) -> Option<(String, String)> {
    let mut parts = filter.splitn(3, ' ');
    let attr = parts.next()?.to_string();
    if !parts.next()?.eq_ignore_ascii_case("eq") {
        return None;
    }
    let v = parts.next()?.trim().trim_matches('"').to_string();
    Some((attr, v))
}

fn list_response(items: Vec<Value>, start: usize, total: usize) -> Value {
    json!({
        "schemas": [SCHEMA_LIST],
        "totalResults": total,
        "startIndex": start,
        "itemsPerPage": items.len(),
        "Resources": items,
    })
}

async fn sp_config(State(state): State<AppState>, headers: HeaderMap) -> ScimResult<Response> {
    authorize(&state, &headers)?;
    Ok(scim_json(
        StatusCode::OK,
        json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig"],
            "documentationUri": "https://github.com/rep0rtDev/zanshin/blob/main/docs/api.md",
            "patch": { "supported": true },
            "bulk": { "supported": false, "maxOperations": 0, "maxPayloadSize": 0 },
            "filter": { "supported": true, "maxResults": 200 },
            "changePassword": { "supported": false },
            "sort": { "supported": false },
            "etag": { "supported": false },
            "authenticationSchemes": [{ "type": "oauthbearertoken", "name": "Bearer token", "description": "Static bearer token from admin.scim.token" }],
        }),
    ))
}

async fn resource_types(State(state): State<AppState>, headers: HeaderMap) -> ScimResult<Response> {
    authorize(&state, &headers)?;
    Ok(scim_json(
        StatusCode::OK,
        json!({
            "schemas": [SCHEMA_LIST], "totalResults": 2, "startIndex": 1, "itemsPerPage": 2,
            "Resources": [
                { "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ResourceType"], "id": "User", "name": "User", "endpoint": "/Users", "schema": SCHEMA_USER },
                { "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ResourceType"], "id": "Group", "name": "Group", "endpoint": "/Groups", "schema": SCHEMA_GROUP }
            ]
        }),
    ))
}

async fn schemas(State(state): State<AppState>, headers: HeaderMap) -> ScimResult<Response> {
    authorize(&state, &headers)?;
    Ok(scim_json(
        StatusCode::OK,
        json!({
            "schemas": [SCHEMA_LIST], "totalResults": 2, "startIndex": 1, "itemsPerPage": 2,
            "Resources": [
                { "id": SCHEMA_USER, "name": "User", "attributes": [
                    { "name": "userName", "type": "string", "required": true, "uniqueness": "server" },
                    { "name": "displayName", "type": "string" },
                    { "name": "name", "type": "complex", "subAttributes": [{ "name": "formatted", "type": "string" }, { "name": "givenName", "type": "string" }, { "name": "familyName", "type": "string" }] },
                    { "name": "emails", "type": "complex", "multiValued": true },
                    { "name": "active", "type": "boolean" }
                ] },
                { "id": SCHEMA_GROUP, "name": "Group", "attributes": [
                    { "name": "displayName", "type": "string", "required": true },
                    { "name": "members", "type": "complex", "multiValued": true }
                ] }
            ]
        }),
    ))
}

fn page(q: &ListQuery, total: usize) -> (usize, usize) {
    let start = q.start_index.unwrap_or(1).max(1);
    let count = q.count.unwrap_or(100).clamp(0, 200);
    let _ = total;
    (start, count)
}

async fn list_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> ScimResult<Response> {
    authorize(&state, &headers)?;
    let base = base_url(&state, &headers);
    let mut users = state.console_users.repo().list().await?;
    if let Some(f) = &q.filter {
        let Some((attr, v)) = filter_eq(f) else {
            return Err(ScimError(
                StatusCode::BAD_REQUEST,
                "unsupported filter; use `attr eq \"value\"`".into(),
            ));
        };
        let v = v.to_lowercase();
        users.retain(|u| match attr.to_ascii_lowercase().as_str() {
            "username" | "emails.value" | "emails" | "externalid" => u.email.to_lowercase() == v,
            "displayname" | "name.formatted" => u.name.to_lowercase() == v,
            "id" => u.id.to_string() == v,
            "active" => (!u.disabled).to_string() == v,
            _ => false,
        });
    }
    let total = users.len();
    let (start, count) = page(&q, total);
    let items: Vec<Value> = users
        .iter()
        .skip(start - 1)
        .take(count)
        .map(|u| user_json(&base, u))
        .collect();
    Ok(scim_json(
        StatusCode::OK,
        list_response(items, start, total),
    ))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ScimUser {
    #[serde(default)]
    user_name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    name: Option<Value>,
    #[serde(default)]
    emails: Vec<Value>,
    #[serde(default)]
    active: Option<bool>,
    #[serde(default)]
    groups: Vec<Value>,
}

impl ScimUser {
    fn email(&self) -> Option<String> {
        self.emails
            .iter()
            .find(|e| e["primary"] == true)
            .or(self.emails.first())
            .and_then(|e| e["value"].as_str().map(str::to_string))
            .or_else(|| self.user_name.clone().filter(|u| u.contains('@')))
            .map(|e| e.trim().to_lowercase())
    }
    fn full_name(&self) -> Option<String> {
        let formatted = self
            .name
            .as_ref()
            .and_then(|n| n["formatted"].as_str())
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty());
        formatted
            .or_else(|| self.display_name.clone().filter(|s| !s.trim().is_empty()))
            .or_else(|| {
                let n = self.name.as_ref()?;
                let g = n["givenName"].as_str().unwrap_or("").trim();
                let f = n["familyName"].as_str().unwrap_or("").trim();
                let s = format!("{g} {f}").trim().to_string();
                (!s.is_empty()).then_some(s)
            })
            .or_else(|| self.user_name.clone())
    }
    fn archetype(&self, state: &AppState) -> Option<String> {
        let cfg = state.cfg.admin.scim.as_ref()?;
        self.groups
            .iter()
            .filter_map(|g| g["display"].as_str().or(g["value"].as_str()))
            .find_map(|g| cfg.group_archetypes.get(g).cloned())
            .or_else(|| cfg.default_archetype.clone())
    }
}

async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ScimUser>,
) -> ScimResult<Response> {
    authorize(&state, &headers)?;
    let base = base_url(&state, &headers);
    let email = body.email().ok_or_else(|| {
        ScimError(
            StatusCode::BAD_REQUEST,
            "userName or emails[].value must be an email".into(),
        )
    })?;
    if let Some(existing) = state.console_users.repo().get_by_email(&email).await? {
        return Err(ScimError(
            StatusCode::CONFLICT,
            format!("user {} already exists ({})", email, existing.id),
        ));
    }
    let name = body.full_name().unwrap_or_else(|| email.clone());
    let archetype = body.archetype(&state);
    let permissions = if archetype.is_none() {
        Some(zanshin_core::permissions::Permissions::read_all())
    } else {
        None
    };
    let user = state
        .console_users
        .create(
            &email,
            &name,
            None,
            archetype.as_deref(),
            permissions,
            ACTOR,
        )
        .await?;
    let user = if body.active == Some(false) {
        state
            .console_users
            .update(
                user.id,
                UserPatch {
                    disabled: Some(true),
                    ..Default::default()
                },
                None,
            )
            .await?
    } else {
        user
    };
    tracing::info!(email = %user.email, "scim: user provisioned");
    Ok(scim_json(StatusCode::CREATED, user_json(&base, &user)))
}

async fn get_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ScimResult<Response> {
    authorize(&state, &headers)?;
    let base = base_url(&state, &headers);
    let u = state.console_users.repo().get(id).await?;
    Ok(scim_json(StatusCode::OK, user_json(&base, &u)))
}

async fn apply_user(state: &AppState, id: Uuid, body: &ScimUser) -> Result<ConsoleUserRow> {
    let current = state.console_users.repo().get(id).await?;
    let mut patch = UserPatch::default();
    if let Some(n) = body.full_name() {
        if n != current.name {
            patch.name = Some(n);
        }
    }
    if let Some(a) = body.active {
        if a == current.disabled {
            patch.disabled = Some(!a);
        }
    }
    let archetype = if body.groups.is_empty() {
        None
    } else {
        body.archetype(state).map(Some)
    };
    let archetype_ref = archetype.as_ref().map(|a| a.as_deref());
    if patch.name.is_none() && patch.disabled.is_none() && archetype_ref.is_none() {
        return Ok(current);
    }
    state.console_users.update(id, patch, archetype_ref).await
}

async fn replace_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ScimUser>,
) -> ScimResult<Response> {
    authorize(&state, &headers)?;
    let base = base_url(&state, &headers);
    let u = apply_user(&state, id, &body).await?;
    Ok(scim_json(StatusCode::OK, user_json(&base, &u)))
}

#[derive(Deserialize)]
struct PatchOp {
    #[serde(default, rename = "Operations")]
    operations: Vec<Operation>,
}

#[derive(Deserialize)]
struct Operation {
    op: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    value: Value,
}

/// RFC 7644 PatchOp: `replace` / `add` with or without `path` for `active`, `displayName`,
/// `name.*`, `userName`, `emails`; other paths are ignored (not an error — IdPs send many).
async fn patch_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchOp>,
) -> ScimResult<Response> {
    authorize(&state, &headers)?;
    let base = base_url(&state, &headers);
    let mut merged = ScimUser::default();
    let mut touched = false;
    for op in &body.operations {
        if !matches!(op.op.to_ascii_lowercase().as_str(), "replace" | "add") {
            continue;
        }
        match op.path.as_deref().map(|p| p.to_ascii_lowercase()) {
            None => {
                if let Ok(u) = serde_json::from_value::<ScimUser>(op.value.clone()) {
                    if u.active.is_some() {
                        merged.active = u.active;
                    }
                    if u.full_name().is_some() {
                        merged.display_name = u.full_name();
                    }
                    if !u.groups.is_empty() {
                        merged.groups = u.groups;
                    }
                    touched = true;
                }
            }
            Some(p) if p == "active" => {
                merged.active = match &op.value {
                    Value::Bool(b) => Some(*b),
                    Value::String(s) => Some(s.eq_ignore_ascii_case("true")),
                    _ => None,
                };
                touched = true;
            }
            Some(p) if p == "displayname" || p == "name.formatted" => {
                merged.display_name = op.value.as_str().map(str::to_string);
                touched = true;
            }
            Some(p) if p == "name.givenname" || p == "name.familyname" => {
                let current = state.console_users.repo().get(id).await?;
                let (g, f) = current
                    .name
                    .split_once(' ')
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .unwrap_or((current.name.clone(), String::new()));
                let v = op.value.as_str().unwrap_or("").to_string();
                let (g, f) = if p == "name.givenname" {
                    (v, f)
                } else {
                    (g, v)
                };
                merged.display_name = Some(format!("{g} {f}").trim().to_string());
                touched = true;
            }
            Some(p) if p == "groups" => {
                merged.groups = op.value.as_array().cloned().unwrap_or_default();
                touched = true;
            }
            _ => {}
        }
    }
    let u = if touched {
        apply_user(&state, id, &merged).await?
    } else {
        state.console_users.repo().get(id).await?
    };
    Ok(scim_json(StatusCode::OK, user_json(&base, &u)))
}

async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ScimResult<StatusCode> {
    authorize(&state, &headers)?;
    state.console_users.repo().get(id).await?;
    state.console_users.repo().revoke_all_sessions(id).await?;
    state.console_users.repo().delete(id).await?;
    tracing::info!(user = %id, "scim: user deprovisioned");
    Ok(StatusCode::NO_CONTENT)
}

fn group_json(base: &str, name: &str, members: &[&ConsoleUserRow]) -> Value {
    json!({
        "schemas": [SCHEMA_GROUP],
        "id": name,
        "displayName": name,
        "members": members.iter().map(|u| json!({ "value": u.id, "display": u.email, "$ref": format!("{base}/scim/v2/Users/{}", u.id) })).collect::<Vec<_>>(),
        "meta": { "resourceType": "Group", "location": format!("{base}/scim/v2/Groups/{name}") }
    })
}

/// Groups are the configured `group_archetypes` keys; membership = users with that archetype.
async fn list_groups(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> ScimResult<Response> {
    authorize(&state, &headers)?;
    let base = base_url(&state, &headers);
    let cfg = state
        .cfg
        .admin
        .scim
        .clone()
        .unwrap_or_else(|| unreachable!());
    let users = state.console_users.repo().list().await?;
    let mut names: Vec<&String> = cfg.group_archetypes.keys().collect();
    names.sort();
    if let Some((attr, v)) = q.filter.as_deref().and_then(filter_eq) {
        if attr.eq_ignore_ascii_case("displayName") || attr.eq_ignore_ascii_case("id") {
            names.retain(|n| **n == v);
        }
    }
    let items: Vec<Value> = names
        .iter()
        .map(|n| {
            let arch = &cfg.group_archetypes[*n];
            let members: Vec<&ConsoleUserRow> = users
                .iter()
                .filter(|u| u.archetype.as_deref() == Some(arch.as_str()))
                .collect();
            group_json(&base, n, &members)
        })
        .collect();
    let total = items.len();
    Ok(scim_json(StatusCode::OK, list_response(items, 1, total)))
}

async fn get_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ScimResult<Response> {
    authorize(&state, &headers)?;
    let base = base_url(&state, &headers);
    let cfg = state
        .cfg
        .admin
        .scim
        .clone()
        .unwrap_or_else(|| unreachable!());
    let arch = cfg
        .group_archetypes
        .get(&id)
        .ok_or_else(|| ScimError(StatusCode::NOT_FOUND, format!("group {id:?} is not mapped")))?;
    let users = state.console_users.repo().list().await?;
    let members: Vec<&ConsoleUserRow> = users
        .iter()
        .filter(|u| u.archetype.as_deref() == Some(arch.as_str()))
        .collect();
    Ok(scim_json(StatusCode::OK, group_json(&base, &id, &members)))
}

/// `add` / `remove` members (`path: "members"`) or a full `replace` of members → set / clear the
/// mapped archetype on those users.
async fn patch_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ScimResult<Response> {
    authorize(&state, &headers)?;
    let cfg = state
        .cfg
        .admin
        .scim
        .clone()
        .unwrap_or_else(|| unreachable!());
    let arch =
        cfg.group_archetypes.get(&id).cloned().ok_or_else(|| {
            ScimError(StatusCode::NOT_FOUND, format!("group {id:?} is not mapped"))
        })?;
    let member_ids = |v: &Value| -> Vec<Uuid> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["value"].as_str().and_then(|s| s.parse().ok()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let ops: Vec<(String, Option<String>, Value)> = if body["schemas"]
        .as_array()
        .is_some_and(|s| s.iter().any(|x| x == SCHEMA_PATCH))
    {
        body["Operations"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|o| {
                        (
                            o["op"].as_str().unwrap_or("").to_ascii_lowercase(),
                            o["path"].as_str().map(str::to_string),
                            o["value"].clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![(
            "replace".into(),
            Some("members".into()),
            body["members"].clone(),
        )]
    };
    for (op, path, value) in ops {
        let is_members = path
            .as_deref()
            .map(|p| p.to_ascii_lowercase().starts_with("members"))
            .unwrap_or(true);
        if !is_members {
            continue;
        }
        let ids = if path.is_none() {
            member_ids(&value["members"])
        } else {
            member_ids(&value)
        };
        match op.as_str() {
            "add" => {
                for uid in ids {
                    state
                        .console_users
                        .update(uid, UserPatch::default(), Some(Some(arch.as_str())))
                        .await?;
                }
            }
            "remove" => {
                let targets = if ids.is_empty() {
                    // path like members[value eq "uuid"]
                    path.as_deref()
                        .and_then(|p| p.split('"').nth(1))
                        .and_then(|s| s.parse::<Uuid>().ok())
                        .into_iter()
                        .collect()
                } else {
                    ids
                };
                for uid in targets {
                    let u = state.console_users.repo().get(uid).await?;
                    if u.archetype.as_deref() == Some(arch.as_str()) {
                        state
                            .console_users
                            .update(uid, UserPatch::default(), Some(None))
                            .await?;
                    }
                }
            }
            "replace" => {
                let users = state.console_users.repo().list().await?;
                for u in users.iter().filter(|u| {
                    u.archetype.as_deref() == Some(arch.as_str()) && !ids.contains(&u.id)
                }) {
                    state
                        .console_users
                        .update(u.id, UserPatch::default(), Some(None))
                        .await?;
                }
                for uid in ids {
                    state
                        .console_users
                        .update(uid, UserPatch::default(), Some(Some(arch.as_str())))
                        .await?;
                }
            }
            _ => {}
        }
    }
    let base = base_url(&state, &headers);
    let users = state.console_users.repo().list().await?;
    let members: Vec<&ConsoleUserRow> = users
        .iter()
        .filter(|u| u.archetype.as_deref() == Some(arch.as_str()))
        .collect();
    Ok(scim_json(StatusCode::OK, group_json(&base, &id, &members)))
}
