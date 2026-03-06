//! Dynamic Client Registration (RFC 7591) proxy handler.
//!
//! Receives RFC 7591 registration requests, translates them into Zitadel
//! Management API calls, and returns RFC 7591-compliant responses.
//!
//! # Endpoint
//!
//! `POST /api/v1/oauth/register`
//!
//! # Breaking change (auth-v3 Phase C)
//!
//! This endpoint now requires authentication. The caller must be an org admin.
//! The previous unauthenticated flow with Zitadel role grants has been replaced
//! with DB permission creation (org + team memberships) via the shared
//! `provision_machine_user()` helper, consistent with C.2.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::{authorization::require_org_admin_only, models::AuthContext},
    storage::{
        repositories::{
            OrganizationRepository, SqlxOrganizationRepository, SqlxTeamRepository, TeamRepository,
        },
        DbPool,
    },
};

use super::organizations::provision_machine_user;

/// Extract the database pool from ApiState.
fn dcr_pool(state: &ApiState) -> Result<DbPool, (&'static str, String)> {
    state
        .xds_state
        .cluster_repository
        .as_ref()
        .map(|r| r.pool().clone())
        .ok_or_else(|| ("server_error", "Database unavailable".to_string()))
}

/// RFC 7591 Dynamic Client Registration request.
#[derive(Debug, Deserialize)]
pub struct DcrRequest {
    /// Human-readable client name (required).
    pub client_name: String,
    /// Grant types — must include `client_credentials`.
    pub grant_types: Vec<String>,
    /// Space-separated scopes (e.g., `"team:t1:clusters:read team:t1:routes:read"`).
    #[serde(default)]
    pub scope: Option<String>,
    /// Auth method for the token endpoint. Defaults to `client_secret_basic`.
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
}

/// RFC 7591 Dynamic Client Registration response.
#[derive(Debug, Serialize)]
pub struct DcrResponse {
    pub client_id: String,
    pub client_secret: String,
    pub client_name: String,
    pub grant_types: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub token_endpoint: String,
}

/// RFC 7591 error response.
#[derive(Debug, Serialize)]
pub struct DcrErrorResponse {
    pub error: &'static str,
    pub error_description: String,
}

/// Validate a DCR request.
///
/// Returns an RFC 7591 error string if validation fails.
fn validate_dcr_request(req: &DcrRequest) -> Result<(), (&'static str, String)> {
    if req.client_name.is_empty() {
        return Err(("invalid_client_metadata", "client_name is required".to_string()));
    }

    if req.client_name.len() > 128 {
        return Err((
            "invalid_client_metadata",
            "client_name must be 128 characters or fewer".to_string(),
        ));
    }

    // RFC 7591: grant_types is required and must include client_credentials
    if req.grant_types.is_empty() {
        return Err(("invalid_client_metadata", "grant_types is required".to_string()));
    }

    if !req.grant_types.iter().any(|g| g == "client_credentials") {
        return Err((
            "invalid_client_metadata",
            "grant_types must include client_credentials".to_string(),
        ));
    }

    // Only client_secret_basic and client_secret_post are supported
    if let Some(method) = &req.token_endpoint_auth_method {
        if method != "client_secret_basic" && method != "client_secret_post" {
            return Err((
                "invalid_client_metadata",
                format!("unsupported token_endpoint_auth_method: {method}"),
            ));
        }
    }

    Ok(())
}

/// Parse RFC 7591 scope string into per-team scope groups.
///
/// Input: `"team:eng:clusters:read team:eng:routes:create team:frontend:clusters:read"`
/// Output: `{ "eng": ["team:eng:clusters:read", "team:eng:routes:create"], "frontend": [...] }`
///
/// Scopes that don't match `team:{name}:{resource}:{action}` are silently skipped.
fn parse_scopes_by_team(scope: &str) -> HashMap<String, Vec<String>> {
    let mut by_team: HashMap<String, Vec<String>> = HashMap::new();
    for s in scope.split_whitespace() {
        if let Some(rest) = s.strip_prefix("team:") {
            if let Some(team_name) = rest.split(':').next() {
                if !team_name.is_empty() {
                    by_team.entry(team_name.to_string()).or_default().push(s.to_string());
                }
            }
        }
    }
    by_team
}

/// Convert an ApiError to an RFC 7591 error response.
fn api_err_to_dcr(e: ApiError) -> axum::response::Response {
    let body = DcrErrorResponse { error: "server_error", error_description: format!("{e:?}") };
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
}

/// Handle Dynamic Client Registration (RFC 7591).
///
/// `POST /api/v1/oauth/register`
///
/// Requires authentication: caller must be an org admin. Creates a machine user
/// in Zitadel and provisions DB permissions (user, org membership, team memberships).
/// No Zitadel role grants are issued — DB is the single source of truth for permissions.
#[instrument(skip(state, payload), fields(client_name = %payload.client_name, user_id = ?context.user_id))]
pub async fn dcr_register_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<DcrRequest>,
) -> Result<(StatusCode, Json<DcrResponse>), axum::response::Response> {
    // Validate request format first
    if let Err((error, error_description)) = validate_dcr_request(&payload) {
        let body = DcrErrorResponse { error, error_description };
        return Err((StatusCode::BAD_REQUEST, Json(body)).into_response());
    }

    // Org admin authentication: require org admin role, determine org from context
    let org_name = context.org_name.clone().ok_or_else(|| {
        let body = DcrErrorResponse {
            error: "access_denied",
            error_description: "Must be authenticated as an org admin to register clients"
                .to_string(),
        };
        (StatusCode::FORBIDDEN, Json(body)).into_response()
    })?;

    if let Err(e) = require_org_admin_only(&context, &org_name) {
        let body = DcrErrorResponse { error: "access_denied", error_description: format!("{e:?}") };
        return Err((StatusCode::FORBIDDEN, Json(body)).into_response());
    }

    // Ensure Zitadel admin client is configured
    let zitadel_client = state.zitadel_admin.as_deref().ok_or_else(|| {
        let body = DcrErrorResponse {
            error: "server_error",
            error_description: "Dynamic Client Registration is not configured".to_string(),
        };
        (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
    })?;

    // Get database pool
    let pool = dcr_pool(&state).map_err(|(error, error_description)| {
        (StatusCode::SERVICE_UNAVAILABLE, Json(DcrErrorResponse { error, error_description }))
            .into_response()
    })?;

    // Resolve org by name
    let org_repo = SqlxOrganizationRepository::new(pool.clone());
    let org = org_repo
        .get_organization_by_name(&org_name)
        .await
        .map_err(|e| api_err_to_dcr(ApiError::from(e)))?
        .ok_or_else(|| {
            let body = DcrErrorResponse {
                error: "invalid_request",
                error_description: format!("Organization '{}' not found", org_name),
            };
            (StatusCode::BAD_REQUEST, Json(body)).into_response()
        })?;
    let org_id_str = org.id.to_string();

    // Parse scopes and validate teams
    let scope_by_team = match &payload.scope {
        Some(s) if !s.trim().is_empty() => parse_scopes_by_team(s),
        _ => HashMap::new(),
    };

    let team_repo = Arc::new(SqlxTeamRepository::new(pool.clone()));
    let mut team_entries: Vec<(String, Vec<String>)> = Vec::new();
    for (team_name_str, scopes) in &scope_by_team {
        let team = team_repo
            .get_team_by_org_and_name(&org.id, team_name_str)
            .await
            .map_err(|e| api_err_to_dcr(ApiError::from(e)))?
            .ok_or_else(|| {
                let body = DcrErrorResponse {
                    error: "invalid_scope",
                    error_description: format!(
                        "Team '{}' not found in org '{}'",
                        team_name_str, org_name
                    ),
                };
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            })?;
        team_entries.push((team.id.to_string(), scopes.clone()));
    }

    // Build username in the same format as the agent provisioning endpoint (C.2)
    let username = format!("{}--{}", org_name, payload.client_name);

    // Build token endpoint URL
    let token_endpoint = std::env::var("FLOWPLANE_ZITADEL_ISSUER")
        .map_err(|_| {
            let body = DcrErrorResponse {
                error: "server_error",
                error_description: "Token endpoint not configured".to_string(),
            };
            (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
        })
        .map(|issuer| format!("{}/oauth/v2/token", issuer.trim_end_matches('/')))?;

    // Idempotency: check if machine user already exists in Zitadel
    if let Some(zitadel_sub) =
        zitadel_client.search_user_by_username(&username).await.map_err(api_err_to_dcr)?
    {
        // Machine user already exists — evict cache and return without credentials
        if let Some(ref cache) = state.permission_cache {
            cache.evict(&zitadel_sub).await;
        }
        tracing::info!(
            username = %username,
            org_name = %org_name,
            "DCR: client already exists — returning idempotent 200"
        );
        let auth_method = payload
            .token_endpoint_auth_method
            .as_deref()
            .unwrap_or("client_secret_basic")
            .to_string();
        return Ok((
            StatusCode::OK,
            Json(DcrResponse {
                client_id: String::new(),
                client_secret: String::new(),
                client_name: payload.client_name,
                grant_types: payload.grant_types,
                token_endpoint_auth_method: auth_method,
                token_endpoint,
            }),
        ));
    }

    // Provision new machine user via shared helper (Zitadel + DB)
    let (_local_user_id, client_id, client_secret) = provision_machine_user(
        zitadel_client,
        &pool,
        state.permission_cache.as_deref(),
        &org_id_str,
        &username,
        &payload.client_name,
        &team_entries,
        crate::auth::models::AgentContext::CpTool, // TODO(E.3): make configurable from DCR metadata
    )
    .await
    .map_err(api_err_to_dcr)?;

    let auth_method =
        payload.token_endpoint_auth_method.as_deref().unwrap_or("client_secret_basic").to_string();

    tracing::info!(
        client_name = %payload.client_name,
        client_id = %client_id,
        org_name = %org_name,
        "DCR: client registered successfully"
    );

    Ok((
        StatusCode::CREATED,
        Json(DcrResponse {
            client_id,
            client_secret,
            client_name: payload.client_name,
            grant_types: payload.grant_types,
            token_endpoint_auth_method: auth_method,
            token_endpoint,
        }),
    ))
}

// ===== OAuth Metadata Endpoints =====

/// OpenID Connect Discovery metadata (RFC 8414 / OpenID Connect Discovery 1.0).
///
/// Returns a document pointing agents to Zitadel's token, authorization,
/// JWKS, and registration endpoints. Flowplane proxies DCR (`/api/v1/oauth/register`)
/// while all other OAuth flows go directly to Zitadel.
#[derive(Debug, Serialize)]
pub struct OAuthMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub registration_endpoint: String,
    pub scopes_supported: Vec<&'static str>,
    pub response_types_supported: Vec<&'static str>,
    pub grant_types_supported: Vec<&'static str>,
    pub token_endpoint_auth_methods_supported: Vec<&'static str>,
}

/// Shared state for the metadata endpoint.
#[derive(Clone)]
pub struct OAuthMetadataState {
    pub issuer: String,
    pub registration_endpoint: String,
}

impl OAuthMetadataState {
    /// Build metadata state from environment.
    pub fn from_env() -> Option<Self> {
        let issuer = std::env::var("FLOWPLANE_ZITADEL_ISSUER").ok()?;
        if issuer.is_empty() {
            return None;
        }

        // Registration endpoint is our DCR proxy
        let api_base = std::env::var("FLOWPLANE_API_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:8080".to_string());
        let registration_endpoint =
            format!("{}/api/v1/oauth/register", api_base.trim_end_matches('/'));

        Some(Self { issuer, registration_endpoint })
    }
}

/// `GET /.well-known/openid-configuration`
///
/// Returns OAuth/OIDC metadata combining Zitadel endpoints with
/// Flowplane's DCR proxy.
#[instrument(skip(state))]
pub async fn openid_configuration_handler(
    State(state): State<OAuthMetadataState>,
) -> Json<OAuthMetadata> {
    let base = state.issuer.trim_end_matches('/');

    Json(OAuthMetadata {
        issuer: state.issuer.clone(),
        authorization_endpoint: format!("{base}/oauth/v2/authorize"),
        token_endpoint: format!("{base}/oauth/v2/token"),
        jwks_uri: format!("{base}/oauth/v2/keys"),
        registration_endpoint: state.registration_endpoint,
        scopes_supported: vec!["openid", "profile", "email", "offline_access"],
        response_types_supported: vec!["code", "id_token", "id_token token"],
        grant_types_supported: vec!["authorization_code", "client_credentials", "refresh_token"],
        token_endpoint_auth_methods_supported: vec!["client_secret_basic", "client_secret_post"],
    })
}

// ===== SPA Auth Config Endpoint =====

/// Response for `GET /api/v1/auth/config`.
///
/// Returns OIDC configuration so the SPA can initialize at runtime
/// without needing values baked in at build time.
#[derive(Debug, Serialize)]
pub struct AuthConfigResponse {
    pub issuer: String,
    pub client_id: String,
    pub app_url: String,
}

/// Shared state for the auth config endpoint.
#[derive(Clone)]
pub struct AuthConfigState {
    pub issuer: String,
    pub client_id: String,
    pub app_url: String,
}

impl AuthConfigState {
    /// Build auth config state from environment.
    ///
    /// Requires `FLOWPLANE_ZITADEL_ISSUER` and `FLOWPLANE_ZITADEL_SPA_CLIENT_ID`.
    /// `FLOWPLANE_APP_URL` defaults to `http://localhost:8080`.
    pub fn from_env() -> Option<Self> {
        let issuer = std::env::var("FLOWPLANE_ZITADEL_ISSUER").ok()?;
        if issuer.is_empty() {
            return None;
        }
        let client_id = std::env::var("FLOWPLANE_ZITADEL_SPA_CLIENT_ID").ok()?;
        if client_id.is_empty() {
            return None;
        }
        let app_url = std::env::var("FLOWPLANE_APP_URL")
            .unwrap_or_else(|_| "http://localhost:8080".to_string());
        Some(Self { issuer, client_id, app_url })
    }
}

/// `GET /api/v1/auth/config`
///
/// Returns OIDC configuration for the SPA to initialize at runtime.
#[instrument(skip(state))]
pub async fn auth_config_handler(State(state): State<AuthConfigState>) -> Json<AuthConfigResponse> {
    Json(AuthConfigResponse {
        issuer: state.issuer.clone(),
        client_id: state.client_id.clone(),
        app_url: state.app_url.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Validation tests ---

    #[test]
    fn validate_valid_request() {
        let req = DcrRequest {
            client_name: "my-agent".to_string(),
            grant_types: vec!["client_credentials".to_string()],
            scope: Some("team:t1:clusters:read".to_string()),
            token_endpoint_auth_method: Some("client_secret_basic".to_string()),
        };
        assert!(validate_dcr_request(&req).is_ok());
    }

    #[test]
    fn validate_missing_client_name() {
        let req = DcrRequest {
            client_name: String::new(),
            grant_types: vec!["client_credentials".to_string()],
            scope: None,
            token_endpoint_auth_method: None,
        };
        let err = validate_dcr_request(&req).unwrap_err();
        assert_eq!(err.0, "invalid_client_metadata");
        assert!(err.1.contains("client_name"));
    }

    #[test]
    fn validate_client_name_too_long() {
        let req = DcrRequest {
            client_name: "a".repeat(129),
            grant_types: vec!["client_credentials".to_string()],
            scope: None,
            token_endpoint_auth_method: None,
        };
        let err = validate_dcr_request(&req).unwrap_err();
        assert_eq!(err.0, "invalid_client_metadata");
        assert!(err.1.contains("128"));
    }

    #[test]
    fn validate_empty_grant_types() {
        let req = DcrRequest {
            client_name: "agent".to_string(),
            grant_types: vec![],
            scope: None,
            token_endpoint_auth_method: None,
        };
        let err = validate_dcr_request(&req).unwrap_err();
        assert_eq!(err.0, "invalid_client_metadata");
        assert!(err.1.contains("grant_types"));
    }

    #[test]
    fn validate_missing_client_credentials_grant() {
        let req = DcrRequest {
            client_name: "agent".to_string(),
            grant_types: vec!["authorization_code".to_string()],
            scope: None,
            token_endpoint_auth_method: None,
        };
        let err = validate_dcr_request(&req).unwrap_err();
        assert_eq!(err.0, "invalid_client_metadata");
        assert!(err.1.contains("client_credentials"));
    }

    #[test]
    fn validate_unsupported_auth_method() {
        let req = DcrRequest {
            client_name: "agent".to_string(),
            grant_types: vec!["client_credentials".to_string()],
            scope: None,
            token_endpoint_auth_method: Some("private_key_jwt".to_string()),
        };
        let err = validate_dcr_request(&req).unwrap_err();
        assert_eq!(err.0, "invalid_client_metadata");
        assert!(err.1.contains("private_key_jwt"));
    }

    #[test]
    fn validate_client_secret_post_allowed() {
        let req = DcrRequest {
            client_name: "agent".to_string(),
            grant_types: vec!["client_credentials".to_string()],
            scope: None,
            token_endpoint_auth_method: Some("client_secret_post".to_string()),
        };
        assert!(validate_dcr_request(&req).is_ok());
    }

    #[test]
    fn validate_no_auth_method_defaults_ok() {
        let req = DcrRequest {
            client_name: "agent".to_string(),
            grant_types: vec!["client_credentials".to_string()],
            scope: None,
            token_endpoint_auth_method: None,
        };
        assert!(validate_dcr_request(&req).is_ok());
    }

    // --- Scope parsing tests ---

    #[test]
    fn parse_scopes_single_team() {
        let by_team = parse_scopes_by_team("team:t1:clusters:read");
        assert_eq!(by_team.len(), 1);
        assert_eq!(by_team["t1"], vec!["team:t1:clusters:read"]);
    }

    #[test]
    fn parse_scopes_multiple_teams() {
        let by_team = parse_scopes_by_team("team:t1:clusters:read team:t2:routes:create");
        assert_eq!(by_team.len(), 2);
        assert_eq!(by_team["t1"], vec!["team:t1:clusters:read"]);
        assert_eq!(by_team["t2"], vec!["team:t2:routes:create"]);
    }

    #[test]
    fn parse_scopes_same_team_multiple_scopes() {
        let by_team = parse_scopes_by_team("team:eng:clusters:read team:eng:routes:update");
        assert_eq!(by_team.len(), 1);
        let mut scopes = by_team["eng"].clone();
        scopes.sort();
        assert_eq!(scopes, vec!["team:eng:clusters:read", "team:eng:routes:update"]);
    }

    #[test]
    fn parse_scopes_empty() {
        let by_team = parse_scopes_by_team("");
        assert!(by_team.is_empty());
    }

    #[test]
    fn parse_scopes_skips_non_team() {
        let by_team = parse_scopes_by_team("openid profile team:t1:clusters:read");
        assert_eq!(by_team.len(), 1);
        assert!(by_team.contains_key("t1"));
    }

    // --- DcrResponse serialization tests ---

    #[test]
    fn dcr_response_serializes_correctly() {
        let resp = DcrResponse {
            client_id: "cid-123".to_string(),
            client_secret: "secret-456".to_string(),
            client_name: "test-agent".to_string(),
            grant_types: vec!["client_credentials".to_string()],
            token_endpoint_auth_method: "client_secret_basic".to_string(),
            token_endpoint: "http://localhost:8081/oauth/v2/token".to_string(),
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["client_id"], "cid-123");
        assert_eq!(json["client_secret"], "secret-456");
        assert_eq!(json["client_name"], "test-agent");
        assert_eq!(json["grant_types"], serde_json::json!(["client_credentials"]));
        assert_eq!(json["token_endpoint_auth_method"], "client_secret_basic");
        assert_eq!(json["token_endpoint"], "http://localhost:8081/oauth/v2/token");
    }

    // --- Metadata tests ---

    #[test]
    fn metadata_state_requires_issuer() {
        // No env vars set → None
        std::env::remove_var("FLOWPLANE_ZITADEL_ISSUER");
        assert!(OAuthMetadataState::from_env().is_none());
    }

    #[test]
    fn metadata_response_has_required_fields() {
        let metadata = OAuthMetadata {
            issuer: "https://auth.example.com".to_string(),
            authorization_endpoint: "https://auth.example.com/oauth/v2/authorize".to_string(),
            token_endpoint: "https://auth.example.com/oauth/v2/token".to_string(),
            jwks_uri: "https://auth.example.com/oauth/v2/keys".to_string(),
            registration_endpoint: "http://localhost:8080/api/v1/oauth/register".to_string(),
            scopes_supported: vec!["openid", "profile", "email", "offline_access"],
            response_types_supported: vec!["code", "id_token", "id_token token"],
            grant_types_supported: vec![
                "authorization_code",
                "client_credentials",
                "refresh_token",
            ],
            token_endpoint_auth_methods_supported: vec![
                "client_secret_basic",
                "client_secret_post",
            ],
        };

        let json = serde_json::to_value(&metadata).unwrap();
        assert_eq!(json["issuer"], "https://auth.example.com");
        assert_eq!(json["token_endpoint"], "https://auth.example.com/oauth/v2/token");
        assert_eq!(json["jwks_uri"], "https://auth.example.com/oauth/v2/keys");
        assert_eq!(json["registration_endpoint"], "http://localhost:8080/api/v1/oauth/register");
        assert!(json["grant_types_supported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "client_credentials"));
    }

    #[test]
    fn dcr_error_response_serializes_correctly() {
        let resp = DcrErrorResponse {
            error: "invalid_client_metadata",
            error_description: "client_name is required".to_string(),
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"], "invalid_client_metadata");
        assert_eq!(json["error_description"], "client_name is required");
    }

    // --- Auth config tests ---

    #[test]
    fn auth_config_requires_issuer() {
        std::env::remove_var("FLOWPLANE_ZITADEL_ISSUER");
        std::env::set_var("FLOWPLANE_ZITADEL_SPA_CLIENT_ID", "test-client");
        assert!(AuthConfigState::from_env().is_none());
        std::env::remove_var("FLOWPLANE_ZITADEL_SPA_CLIENT_ID");
    }

    #[test]
    fn auth_config_requires_client_id() {
        std::env::set_var("FLOWPLANE_ZITADEL_ISSUER", "http://localhost:8081");
        std::env::remove_var("FLOWPLANE_ZITADEL_SPA_CLIENT_ID");
        assert!(AuthConfigState::from_env().is_none());
        std::env::remove_var("FLOWPLANE_ZITADEL_ISSUER");
    }

    #[test]
    fn auth_config_response_serializes_correctly() {
        let resp = AuthConfigResponse {
            issuer: "http://localhost:8081".to_string(),
            client_id: "123456789".to_string(),
            app_url: "http://localhost:8080".to_string(),
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["issuer"], "http://localhost:8081");
        assert_eq!(json["client_id"], "123456789");
        assert_eq!(json["app_url"], "http://localhost:8080");
    }
}
