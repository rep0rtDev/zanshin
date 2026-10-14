//! gRPC parity: the RPCs added for SDK wave 2 against a live server. Run only with
//! ZANSHIN_TEST_DATABASE_URL / ZANSHIN_TEST_REDIS_URL.

use std::{net::SocketAddr, sync::Arc};

use serde_json::{json, Value};
use tonic::{metadata::MetadataValue, Request};
use zanshin_proto::api::v1::{
    zanshin_client::ZanshinClient, AuthenticateRequest, GetFlagsRequest, IdentifyRequest,
    ServerAudienceMembersRequest, ServerGetFlagRequest, ServerUpsertIdentityRequest,
};
use zanshin_server::{config::Config, grpc, http, state::AppState};

const ADMIN_KEY: &str = "test-bootstrap-admin-key-grpc";

async fn start() -> Option<(String, String, reqwest::Client)> {
    let db = std::env::var("ZANSHIN_TEST_DATABASE_URL").ok()?;
    let redis = std::env::var("ZANSHIN_TEST_REDIS_URL").ok()?;
    std::env::set_var("ZANSHIN__DATABASE__URL", db);
    std::env::set_var("ZANSHIN__REDIS__URL", redis);
    std::env::set_var(
        "ZANSHIN__AUTH__JWT_SECRET",
        "test-secret-test-secret-test-secret-0123",
    );
    std::env::set_var("ZANSHIN__ADMIN__BOOTSTRAP_KEY", ADMIN_KEY);
    std::env::set_var("ZANSHIN__SERVER__HTTP_ADDR", "127.0.0.1:0");
    std::env::set_var("ZANSHIN__PUSH__PROVIDER", "log");
    let grpc_addr: SocketAddr = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    std::env::set_var("ZANSHIN__SERVER__GRPC_ADDR", grpc_addr.to_string());
    let cfg = Config::load(None).expect("config");
    let state = AppState::build(Arc::new(cfg)).await.expect("state");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_addr: SocketAddr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    tokio::spawn(grpc::serve(state.clone(), grpc_addr, rx));
    tokio::spawn(async move {
        axum::serve(
            listener,
            http::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(grpc_addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Some((
        format!("http://{http_addr}"),
        format!("http://{grpc_addr}"),
        reqwest::Client::new(),
    ))
}

async fn mint_key(c: &reqwest::Client, base: &str, kind: &str) -> String {
    let v: Value = c
        .post(format!("{base}/admin/v1/api-keys"))
        .bearer_auth(ADMIN_KEY)
        .json(&json!({"name": format!("grpc-{kind}"), "kind": kind}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["secret"].as_str().expect("key secret").to_string()
}

fn with_key<T>(mut req: Request<T>, key: &str) -> Request<T> {
    req.metadata_mut()
        .insert("x-api-key", MetadataValue::try_from(key).unwrap());
    req
}

fn with_token<T>(mut req: Request<T>, token: &str) -> Request<T> {
    req.metadata_mut().insert(
        "authorization",
        MetadataValue::try_from(format!("Bearer {token}")).unwrap(),
    );
    req
}

#[tokio::test]
async fn grpc_parity_flow() {
    let Some((base, grpc_url, c)) = start().await else {
        return;
    };
    let sfx = uuid::Uuid::now_v7().simple().to_string();
    let client_key = mint_key(&c, &base, "client").await;
    let server_key = mint_key(&c, &base, "server").await;
    let fkey = format!("gf_{sfx}");
    let st = c
        .post(format!("{base}/admin/v1/flags"))
        .bearer_auth(ADMIN_KEY)
        .json(&json!({"key": fkey, "value_type": "number", "default_value": 7}))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st, 201);

    let mut g = ZanshinClient::connect(grpc_url)
        .await
        .expect("grpc connect");

    // Client surface: authenticate → flags → overrides → identify to a new id.
    let ext = format!("grpc_{sfx}");
    let s = g
        .authenticate(with_key(
            Request::new(AuthenticateRequest {
                id: ext.clone(),
                default: Default::default(),
                custom: Default::default(),
            }),
            &client_key,
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(s.identity.as_ref().unwrap().external_id, ext);
    let token = s.token.clone();

    let flags = g
        .get_flags(with_token(
            Request::new(GetFlagsRequest {
                names: vec![fkey.clone()],
            }),
            &token,
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(flags.flags.len(), 1);
    assert_eq!(flags.flags[0].name, fkey);

    let ov = g
        .get_flag_overrides(with_token(Request::new(()), &token))
        .await
        .unwrap()
        .into_inner();
    assert!(ov.overrides.is_empty(), "{ov:?}");

    let new_ext = format!("grpc_acct_{sfx}");
    let s2 = g
        .identify(with_token(
            Request::new(IdentifyRequest {
                id: new_ext.clone(),
            }),
            &token,
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(s2.identity.as_ref().unwrap().external_id, new_ext);
    assert_ne!(s2.token, token);

    // Server surface: upsert identity, single flag, explain, audience members on a missing audience.
    let other = format!("grpc_srv_{sfx}");
    let up = g
        .server_upsert_identity(with_key(
            Request::new(ServerUpsertIdentityRequest {
                external_id: other.clone(),
                default: Default::default(),
                custom: Default::default(),
            }),
            &server_key,
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(up.identity.unwrap().external_id, other);

    let f = g
        .server_get_flag(with_key(
            Request::new(ServerGetFlagRequest {
                external_id: other.clone(),
                name: fkey.clone(),
            }),
            &server_key,
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(f.name, fkey);
    assert_eq!(
        f.value.and_then(|v| v.kind),
        Some(prost_types::value::Kind::NumberValue(7.0))
    );

    let explain = g
        .server_explain_flag(with_key(
            Request::new(ServerGetFlagRequest {
                external_id: other.clone(),
                name: fkey.clone(),
            }),
            &server_key,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(!explain.fields.is_empty(), "{explain:?}");

    let err = g
        .server_update_audience_members(with_key(
            Request::new(ServerAudienceMembersRequest {
                audience: format!("missing_{sfx}"),
                add: vec![other.clone()],
                remove: vec![],
            }),
            &server_key,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound, "{err:?}");

    // A client key must not reach server RPCs.
    let err = g
        .server_get_flag(with_key(
            Request::new(ServerGetFlagRequest {
                external_id: other,
                name: fkey,
            }),
            &client_key,
        ))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err.code(),
            tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
        ),
        "{err:?}"
    );
}
