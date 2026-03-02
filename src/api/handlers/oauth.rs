//! Dynamic Client Registration (RFC 7591) proxy handler.
//!
//! Receives RFC 7591 registration requests, translates them into Zitadel
//! Management API calls, and returns RFC 7591-compliant responses.
//!
//! # Endpoint
//!
//! `POST /api/v1/oauth/register`
//!
//! # Flow
//!
//! 1. Agent sends RFC 7591 registration request
//! 2. Flowplane validates the request
//! 3. Flowplane creates a machine user in Zitadel via Management API
//! 4. Flowplane generates client credentials for that user
//! 5. Flowplane assigns role grants based on requested scopes
//! 6. Flowplane returns RFC 7591 response with credentials

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::api::rate_limit::RateLimiter;
use crate::auth::zitadel_admin::ZitadelAdminClient;

/// Shared state for the DCR handler.
#[derive(Clone)]
pub struct DcrState {
    /// Zitadel Management API client (None if DCR is disabled).
    pub admin_client: Option<ZitadelAdminClient>,
    /// Rate limiter for DCR registrations.
    pub rate_limiter: Arc<RateLimiter>,
    /// Zitadel project ID for role grants.
    pub project_id: String,
    /// Token endpoint URL returned in DCR responses.
    pub token_endpoint: String,
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

/// Parse RFC 7591 scope string into Zitadel role keys.
///
/// Scope format: `"team:t1:clusters:read team:t1:routes:read"`
/// Role key format: `"t1:clusters:read"` (strip the leading `team:` prefix)
///
/// Scopes that don't start with `team:` are silently skipped since they
/// don't map to Zitadel project roles.
pub fn parse_scopes_to_role_keys(scope: &str) -> Vec<String> {
    scope
        .split_whitespace()
        .filter_map(|s| s.strip_prefix("team:"))
        .map(|s| s.to_string())
        .collect()
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

/// Sanitize client_name into a valid Zitadel username.
///
/// Zitadel usernames must be alphanumeric with hyphens/underscores.
/// Prefixes with `dcr-` to avoid collisions with human users.
fn sanitize_username(client_name: &str) -> String {
    let sanitized: String = client_name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    format!("dcr-{sanitized}")
}

/// Extract client IP from request headers for rate limiting.
///
/// Prefers X-Forwarded-For (for proxied requests), falls back to X-Real-IP.
fn extract_client_ip(headers: &axum::http::HeaderMap) -> String {
    // Try X-Forwarded-For header first (for proxied requests)
    if let Some(forwarded) = headers.get("x-forwarded-for") {
        if let Ok(value) = forwarded.to_str() {
            if let Some(ip) = value.split(',').next().map(|s| s.trim().to_string()) {
                if !ip.is_empty() {
                    return ip;
                }
            }
        }
    }

    // Try X-Real-IP header
    if let Some(real_ip) = headers.get("x-real-ip") {
        if let Ok(ip) = real_ip.to_str() {
            let ip = ip.trim();
            if !ip.is_empty() {
                return ip.to_string();
            }
        }
    }

    "unknown".to_string()
}

/// Handle Dynamic Client Registration (RFC 7591).
///
/// `POST /api/v1/oauth/register`
#[instrument(skip(state, headers, payload), fields(client_name = %payload.client_name))]
pub async fn dcr_register_handler(
    State(state): State<DcrState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<DcrRequest>,
) -> Result<(StatusCode, Json<DcrResponse>), axum::response::Response> {
    // Rate limit by IP address
    let ip = extract_client_ip(&headers);
    if let Err(retry_after) = state.rate_limiter.check_rate_limit(&ip).await {
        let body = DcrErrorResponse {
            error: "too_many_requests",
            error_description: format!(
                "Registration rate limit exceeded. Retry after {retry_after} seconds."
            ),
        };
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            [(axum::http::header::RETRY_AFTER, retry_after.to_string())],
            Json(body),
        )
            .into_response());
    }

    // Validate request
    if let Err((error, error_description)) = validate_dcr_request(&payload) {
        let body = DcrErrorResponse { error, error_description };
        return Err((StatusCode::BAD_REQUEST, Json(body)).into_response());
    }

    // Ensure DCR is enabled (admin client configured)
    let admin_client = state.admin_client.as_ref().ok_or_else(|| {
        let body = DcrErrorResponse {
            error: "server_error",
            error_description: "Dynamic Client Registration is not configured".to_string(),
        };
        (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
    })?;

    // Step 1: Create machine user
    let username = sanitize_username(&payload.client_name);
    let user_id =
        admin_client.create_machine_user(&username, &payload.client_name).await.map_err(|e| {
            tracing::error!(error = ?e, "DCR: failed to create machine user");
            let body = DcrErrorResponse {
                error: "server_error",
                error_description: "Failed to create client".to_string(),
            };
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
        })?;

    // Step 2: Generate client secret
    let (client_id, client_secret) =
        admin_client.create_client_secret(&user_id).await.map_err(|e| {
            tracing::error!(error = ?e, "DCR: failed to create client secret");
            let body = DcrErrorResponse {
                error: "server_error",
                error_description: "Failed to generate client credentials".to_string(),
            };
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
        })?;

    // Step 3: Assign role grants from requested scopes
    if let Some(scope) = &payload.scope {
        let role_keys = parse_scopes_to_role_keys(scope);
        if !role_keys.is_empty() {
            admin_client.add_user_grant(&user_id, &state.project_id, role_keys).await.map_err(
                |e| {
                    tracing::error!(error = ?e, "DCR: failed to assign role grants");
                    let body = DcrErrorResponse {
                        error: "server_error",
                        error_description: "Failed to assign requested scopes".to_string(),
                    };
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
                },
            )?;
        }
    }

    let auth_method =
        payload.token_endpoint_auth_method.as_deref().unwrap_or("client_secret_basic").to_string();

    tracing::info!(
        client_name = %payload.client_name,
        client_id = %client_id,
        "DCR: client registered successfully"
    );

    let response = DcrResponse {
        client_id,
        client_secret,
        client_name: payload.client_name,
        grant_types: payload.grant_types,
        token_endpoint_auth_method: auth_method,
        token_endpoint: state.token_endpoint.clone(),
    };

    Ok((StatusCode::CREATED, Json(response)))
}

impl DcrState {
    /// Create DCR state from environment variables.
    ///
    /// Returns `None` if Zitadel admin client is not configured.
    pub fn from_env() -> Option<Self> {
        let project_id = std::env::var("FLOWPLANE_ZITADEL_PROJECT_ID").ok()?;
        if project_id.is_empty() {
            return None;
        }

        let issuer = std::env::var("FLOWPLANE_ZITADEL_ISSUER").ok()?;
        let token_endpoint = format!("{}/oauth/v2/token", issuer.trim_end_matches('/'));

        // DCR rate limit: defaults to 10 registrations per hour per IP
        let max_registrations = std::env::var("FLOWPLANE_RATE_LIMIT_DCR_PER_HOUR")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(10);

        let rate_limiter =
            Arc::new(RateLimiter::new(max_registrations, std::time::Duration::from_secs(3600)));

        Some(Self {
            admin_client: ZitadelAdminClient::from_env(),
            rate_limiter,
            project_id,
            token_endpoint,
        })
    }
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
    fn parse_scopes_single() {
        let roles = parse_scopes_to_role_keys("team:t1:clusters:read");
        assert_eq!(roles, vec!["t1:clusters:read"]);
    }

    #[test]
    fn parse_scopes_multiple() {
        let roles = parse_scopes_to_role_keys("team:t1:clusters:read team:t2:routes:write");
        assert_eq!(roles, vec!["t1:clusters:read", "t2:routes:write"]);
    }

    #[test]
    fn parse_scopes_empty() {
        let roles = parse_scopes_to_role_keys("");
        assert!(roles.is_empty());
    }

    #[test]
    fn parse_scopes_skips_non_team() {
        let roles = parse_scopes_to_role_keys("openid profile team:t1:clusters:read");
        assert_eq!(roles, vec!["t1:clusters:read"]);
    }

    #[test]
    fn parse_scopes_admin() {
        let roles = parse_scopes_to_role_keys("team:t1:admin");
        assert_eq!(roles, vec!["t1:admin"]);
    }

    // --- Username sanitization tests ---

    #[test]
    fn sanitize_simple_name() {
        assert_eq!(sanitize_username("my-agent"), "dcr-my-agent");
    }

    #[test]
    fn sanitize_name_with_spaces() {
        assert_eq!(sanitize_username("my agent"), "dcr-my-agent");
    }

    #[test]
    fn sanitize_name_with_special_chars() {
        assert_eq!(sanitize_username("agent@v2!"), "dcr-agent-v2-");
    }

    #[test]
    fn sanitize_name_with_underscores() {
        assert_eq!(sanitize_username("my_agent_v2"), "dcr-my_agent_v2");
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
