use axum::extract::Extension;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::{body::Body, Json};
use fp_core::PrincipalCtx;
use fp_domain::{AgentKind, RequestId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const PREFERRED_VERSION: &str = "2025-11-25";
const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-03-26"];
const SESSION_TTL: Duration = Duration::from_secs(3600);

static SESSIONS: LazyLock<Mutex<HashMap<String, McpSession>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
struct McpSession {
    principal: String,
    last_seen: Instant,
}

#[derive(Deserialize)]
pub struct JsonRpcRequest {
    jsonrpc: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    id: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    data: Value,
}

pub async fn post(
    headers: HeaderMap,
    Extension(ctx): Extension<PrincipalCtx>,
    Extension(rid): Extension<RequestId>,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    if let Err(error) = check_origin(&headers) {
        return rpc_error(req.id, -32600, error, rid, "origin").into_response();
    }
    if req.jsonrpc.as_deref() != Some("2.0") {
        return rpc_error(req.id, -32600, "jsonrpc must be \"2.0\"", rid, "validation")
            .into_response();
    }
    let version = match requested_version(&headers, &req) {
        Ok(version) => version,
        Err(error) => {
            return rpc_error(req.id, -32600, error, rid, "protocol").into_response();
        }
    };
    let principal = principal_key(&ctx);
    cleanup_sessions();
    let mut response: Response = match req.method.as_str() {
        "initialize"
            if matches!(
                ctx,
                PrincipalCtx::Agent {
                    kind: AgentKind::ApiConsumer,
                    ..
                }
            ) =>
        {
            rpc_error(
                req.id,
                -32600,
                "api-consumer agents do not use the MCP endpoint",
                rid,
                "authz",
            )
            .into_response()
        }
        "initialize" => initialize(req.id, &principal, version.clone()),
        "notifications/initialized" | "initialized" => notification(req.id),
        "ping" => with_session(&headers, &principal, req.id, rid, || json!({})),
        _ => rpc_error(req.id, -32601, "method not found", rid, "method").into_response(),
    };
    if let Ok(value) = HeaderValue::from_str(&version) {
        response.headers_mut().insert("mcp-protocol-version", value);
    }
    response
}

fn initialize(id: Option<Value>, principal: &str, protocol_version: String) -> Response {
    let session_id = format!("mcp-{}", uuid::Uuid::new_v4());
    sessions().insert(
        session_id.clone(),
        McpSession {
            principal: principal.to_string(),
            last_seen: Instant::now(),
        },
    );
    let mut response = rpc_result(
        id,
        json!({
            "protocolVersion": protocol_version,
            "serverInfo": {
                "name": "flowplane-mcp",
                "version": env!("CARGO_PKG_VERSION"),
                "title": "Flowplane MCP"
            },
            "capabilities": {
                "tools": { "listChanged": false }
            }
        }),
    )
    .into_response();
    if let Ok(value) = HeaderValue::from_str(&session_id) {
        response.headers_mut().insert("mcp-session-id", value);
    }
    response
}

fn notification(id: Option<Value>) -> Response {
    match id {
        Some(id) => rpc_result(Some(id), json!({})).into_response(),
        None => axum::http::Response::builder()
            .status(axum::http::StatusCode::ACCEPTED)
            .body(Body::empty())
            .unwrap_or_else(|_| Body::empty().into_response()),
    }
}

fn with_session(
    headers: &HeaderMap,
    principal: &str,
    id: Option<Value>,
    rid: RequestId,
    result: impl FnOnce() -> Value,
) -> Response {
    let Some(session_id) = headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| valid_session_id(s))
    else {
        return rpc_error(
            id,
            -32600,
            "missing or invalid MCP-Session-Id",
            rid,
            "session",
        )
        .into_response();
    };
    let mut sessions = sessions();
    let Some(session) = sessions.get_mut(session_id) else {
        return rpc_error(id, -32600, "unknown MCP session", rid, "session").into_response();
    };
    if session.principal != principal {
        return rpc_error(id, -32600, "MCP session principal mismatch", rid, "authz")
            .into_response();
    }
    session.last_seen = Instant::now();
    rpc_result(id, result()).into_response()
}

fn requested_version(headers: &HeaderMap, req: &JsonRpcRequest) -> Result<String, &'static str> {
    let header_version = headers
        .get("mcp-protocol-version")
        .and_then(|v| v.to_str().ok());
    let param_version = req.params.get("protocolVersion").and_then(|v| v.as_str());
    let requested = header_version
        .or(param_version)
        .unwrap_or(PREFERRED_VERSION);
    if SUPPORTED_VERSIONS.contains(&requested) {
        Ok(requested.to_string())
    } else {
        Err("unsupported MCP protocol version")
    }
}

fn check_origin(headers: &HeaderMap) -> Result<(), &'static str> {
    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
        return Ok(());
    };
    let allowed = std::env::var("FLOWPLANE_MCP_ALLOWED_ORIGINS")
        .unwrap_or_else(|_| "http://localhost,http://127.0.0.1,http://[::1]".to_string());
    if allowed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|allowed| origin_matches(allowed, origin))
    {
        Ok(())
    } else {
        Err("origin is not allowed")
    }
}

fn origin_matches(allowed: &str, origin: &str) -> bool {
    fn scheme_host(value: &str) -> Option<(&str, &str)> {
        let (scheme, rest) = value.split_once("://")?;
        let host = rest
            .strip_prefix('[')
            .and_then(|v| v.split_once(']').map(|(host, _)| host))
            .or_else(|| rest.split(':').next())
            .unwrap_or(rest);
        Some((scheme, host))
    }
    scheme_host(allowed) == scheme_host(origin)
}

fn principal_key(ctx: &PrincipalCtx) -> String {
    match ctx {
        PrincipalCtx::User { user_id, org, .. } => {
            format!(
                "user:{user_id}:org:{:?}",
                org.map(|(id, role)| (id, role.as_str()))
            )
        }
        PrincipalCtx::Agent {
            agent_id, org_id, ..
        } => format!("agent:{agent_id}:org:{org_id}"),
    }
}

fn cleanup_sessions() {
    let now = Instant::now();
    sessions().retain(|_, session| now.duration_since(session.last_seen) <= SESSION_TTL);
}

fn sessions() -> MutexGuard<'static, HashMap<String, McpSession>> {
    SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn valid_session_id(session_id: &str) -> bool {
    session_id
        .strip_prefix("mcp-")
        .and_then(|uuid| uuid::Uuid::parse_str(uuid).ok())
        .is_some()
}

fn rpc_result(id: Option<Value>, result: Value) -> Json<JsonRpcResponse> {
    Json(JsonRpcResponse {
        jsonrpc: "2.0",
        result: Some(result),
        error: None,
        id,
    })
}

fn rpc_error(
    id: Option<Value>,
    code: i64,
    message: impl Into<String>,
    rid: RequestId,
    kind: &'static str,
) -> Json<JsonRpcResponse> {
    Json(JsonRpcResponse {
        jsonrpc: "2.0",
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
            data: json!({
                "kind": kind,
                "requestId": rid.to_string(),
                "fix": "check the MCP request, session, protocol version, and bearer token",
            }),
        }),
        id,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::body::Body;
    use fp_core::GrantSet;
    use fp_domain::{OrgId, OrgRole, UserId};
    use http_body_util::BodyExt;

    #[test]
    fn origin_match_ignores_port() {
        assert!(origin_matches("http://localhost", "http://localhost:3000"));
        assert!(origin_matches("http://[::1]", "http://[::1]:3000"));
        assert!(!origin_matches("https://localhost", "http://localhost"));
        assert!(!origin_matches("http://localhost", "http://example.com"));
    }

    fn user(user_id: UserId, org_id: OrgId) -> PrincipalCtx {
        PrincipalCtx::User {
            user_id,
            platform_admin: false,
            org: Some((org_id, OrgRole::Admin)),
            org_selector_required: false,
            grants: GrantSet::default(),
        }
    }

    fn initialize_request() -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: Some("2.0".into()),
            method: "initialize".into(),
            params: json!({ "protocolVersion": PREFERRED_VERSION }),
            id: Some(json!(1)),
        }
    }

    fn ping_request() -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: Some("2.0".into()),
            method: "ping".into(),
            params: json!({}),
            id: Some(json!(2)),
        }
    }

    async fn json_body(response: Response<Body>) -> Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("json")
    }

    #[tokio::test]
    async fn initialize_and_ping_work_without_origin_header() {
        let ctx = user(UserId::generate(), OrgId::generate());
        let response = post(
            HeaderMap::new(),
            Extension(ctx.clone()),
            Extension(RequestId::generate()),
            Json(initialize_request()),
        )
        .await;
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .expect("session")
            .to_string();
        let body = json_body(response).await;
        assert_eq!(body["result"]["protocolVersion"], PREFERRED_VERSION);

        let mut headers = HeaderMap::new();
        headers.insert(
            "mcp-session-id",
            HeaderValue::from_str(&session).expect("session header"),
        );
        let response = post(
            headers,
            Extension(ctx),
            Extension(RequestId::generate()),
            Json(ping_request()),
        )
        .await;
        let body = json_body(response).await;
        assert_eq!(body["result"], json!({}));
    }

    #[tokio::test]
    async fn session_rejects_different_reauthenticated_principal() {
        let org_id = OrgId::generate();
        let response = post(
            HeaderMap::new(),
            Extension(user(UserId::generate(), org_id)),
            Extension(RequestId::generate()),
            Json(initialize_request()),
        )
        .await;
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .expect("session")
            .to_string();

        let mut headers = HeaderMap::new();
        headers.insert(
            "mcp-session-id",
            HeaderValue::from_str(&session).expect("session header"),
        );
        let response = post(
            headers,
            Extension(user(UserId::generate(), org_id)),
            Extension(RequestId::generate()),
            Json(ping_request()),
        )
        .await;
        let body = json_body(response).await;
        assert_eq!(body["error"]["data"]["kind"], "authz");
    }

    #[tokio::test]
    async fn bad_origin_and_protocol_fail_closed() {
        let ctx = user(UserId::generate(), OrgId::generate());
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("https://denied.example"));
        let response = post(
            headers,
            Extension(ctx.clone()),
            Extension(RequestId::generate()),
            Json(initialize_request()),
        )
        .await;
        let body = json_body(response).await;
        assert_eq!(body["error"]["data"]["kind"], "origin");

        let mut headers = HeaderMap::new();
        headers.insert(
            "mcp-protocol-version",
            HeaderValue::from_static("1999-01-01"),
        );
        let response = post(
            headers,
            Extension(ctx),
            Extension(RequestId::generate()),
            Json(initialize_request()),
        )
        .await;
        let body = json_body(response).await;
        assert_eq!(body["error"]["data"]["kind"], "protocol");
    }
}
