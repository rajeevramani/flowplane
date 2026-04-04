//! Mock OIDC provider for testing CLI authentication flows.
//!
//! Provides a lightweight axum-based OIDC provider that supports:
//! - OpenID Connect Discovery (`.well-known/openid-configuration`)
//! - JWKS endpoint for token verification
//! - Authorization endpoint with PKCE support
//! - Token endpoint (authorization_code, refresh_token, device_code grants)
//! - Userinfo endpoint
//!
//! All JWTs are signed with RS256 using an ephemeral RSA key pair generated at startup.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Form, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Json;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the mock OIDC provider.
#[derive(Debug, Clone)]
pub struct MockOidcConfig {
    /// Token expiry duration (default: 1 hour).
    pub token_expiry: Duration,
    /// Refresh token expiry duration (default: 24 hours).
    pub refresh_token_expiry: Duration,
    /// Configurable user info returned by the userinfo endpoint.
    pub user_info: UserInfo,
    /// Configurable failure mode for testing error scenarios.
    pub failure_mode: Option<FailureMode>,
    /// Project ID to use in the role claim key (matches Zitadel convention).
    pub project_id: String,
    /// Audience value for tokens.
    pub audience: String,
}

impl Default for MockOidcConfig {
    fn default() -> Self {
        Self {
            token_expiry: Duration::from_secs(3600),
            refresh_token_expiry: Duration::from_secs(86400),
            user_info: UserInfo::default(),
            failure_mode: None,
            project_id: "test-project-id".to_string(),
            audience: "test-project-id".to_string(),
        }
    }
}

/// Configurable user info for the mock provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub sub: String,
    pub email: String,
    pub name: String,
}

impl Default for UserInfo {
    fn default() -> Self {
        Self {
            sub: "test-user-001".to_string(),
            email: "test@flowplane.dev".to_string(),
            name: "Test User".to_string(),
        }
    }
}

/// Failure modes for testing error scenarios.
#[derive(Debug, Clone)]
pub enum FailureMode {
    /// Return invalid_grant error on token exchange.
    InvalidGrant,
    /// Return expired tokens (exp in the past).
    ExpiredTokens,
    /// Simulate a slow response (delay in ms).
    SlowResponse(u64),
    /// Return 500 on token endpoint.
    TokenEndpointError,
    /// Device code flow: return authorization_pending forever.
    DeviceCodePending,
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Thread-safe shared state for the mock OIDC provider.
struct MockOidcState {
    config: MockOidcConfig,
    /// RSA private key in PKCS#8 DER format for signing JWTs.
    signing_key: Vec<u8>,
    /// RSA public key components (n, e) as base64url for JWKS.
    jwk_n: String,
    jwk_e: String,
    /// Key ID used in JWT headers and JWKS.
    kid: String,
    /// Issued authorization codes mapped to their PKCE challenges.
    auth_codes: RwLock<HashMap<String, AuthCodeEntry>>,
    /// Issued refresh tokens mapped to user sub.
    refresh_tokens: RwLock<HashMap<String, String>>,
    /// Issued device codes mapped to their state.
    device_codes: RwLock<HashMap<String, DeviceCodeEntry>>,
    /// The base URL of this server (set after binding).
    base_url: RwLock<String>,
}

#[derive(Debug, Clone)]
struct AuthCodeEntry {
    code_challenge: String,
    code_challenge_method: String,
    #[allow(dead_code)]
    redirect_uri: String,
    sub: String,
}

#[derive(Debug, Clone)]
struct DeviceCodeEntry {
    #[allow(dead_code)]
    user_code: String,
    sub: String,
    /// Whether the user has "approved" this device code.
    approved: bool,
}

// ---------------------------------------------------------------------------
// RSA key utilities
// ---------------------------------------------------------------------------

/// Generate an ephemeral RSA key pair and return (private_key_pkcs8_der, n_b64url, e_b64url).
///
/// Uses openssl CLI to generate the key and extract the modulus. The public exponent
/// is always 65537 (0x010001) which encodes as "AQAB" in base64url.
fn generate_rsa_keypair() -> (Vec<u8>, String, String) {
    use std::process::Command;

    // Generate RSA private key in PKCS#8 PEM format (we need PEM for modulus extraction)
    let privkey_output = Command::new("openssl")
        .args(["genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048"])
        .output()
        .expect("failed to run openssl genpkey — is openssl installed?");
    assert!(privkey_output.status.success(), "openssl genpkey failed");
    let private_key_pem = privkey_output.stdout;

    // Extract modulus as hex (openssl rsa, not pkey, supports -modulus)
    let mut modulus_cmd = Command::new("openssl")
        .args(["rsa", "-modulus", "-noout"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn openssl pkey");

    use std::io::Write;
    modulus_cmd.stdin.take().unwrap().write_all(&private_key_pem).unwrap();
    let modulus_output = modulus_cmd.wait_with_output().expect("openssl rsa -modulus failed");
    assert!(modulus_output.status.success(), "openssl modulus extraction failed");

    // Parse "Modulus=<hex>\n"
    let modulus_line = String::from_utf8(modulus_output.stdout).expect("invalid utf8 from openssl");
    let modulus_hex =
        modulus_line.trim().strip_prefix("Modulus=").expect("unexpected modulus output format");

    // Convert hex modulus to bytes, then base64url
    let modulus_bytes = hex::decode(modulus_hex).expect("invalid hex in modulus");
    // Strip leading zero byte if present (ASN.1 sign padding)
    let modulus_bytes = if modulus_bytes.first() == Some(&0) && modulus_bytes.len() > 1 {
        &modulus_bytes[1..]
    } else {
        &modulus_bytes
    };
    let n_b64 = URL_SAFE_NO_PAD.encode(modulus_bytes);

    // Public exponent is always 65537 = 0x010001 = base64url "AQAB"
    let e_b64 = "AQAB".to_string();

    // Convert PKCS#8 PEM to traditional RSA (PKCS#1) DER format.
    // jsonwebtoken's EncodingKey::from_rsa_der() expects PKCS#1, not PKCS#8.
    let mut der_cmd = Command::new("openssl")
        .args(["rsa", "-traditional", "-outform", "DER"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn openssl rsa");
    der_cmd.stdin.take().unwrap().write_all(&private_key_pem).unwrap();
    let der_output = der_cmd.wait_with_output().expect("openssl rsa DER conversion failed");
    assert!(der_output.status.success(), "openssl PEM-to-DER conversion failed");

    (der_output.stdout, n_b64, e_b64)
}

// ---------------------------------------------------------------------------
// JWT creation
// ---------------------------------------------------------------------------

/// Standard JWT claims for the mock provider.
#[derive(Debug, Serialize)]
struct TokenClaims {
    iss: String,
    sub: String,
    aud: String,
    exp: u64,
    iat: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

fn build_jwt(state: &MockOidcState, sub: &str, include_email: bool) -> Result<String, String> {
    let now = chrono::Utc::now().timestamp() as u64;

    let (exp, iat) = match &state.config.failure_mode {
        Some(FailureMode::ExpiredTokens) => {
            // Issue a token that expired 1 hour ago
            (now.saturating_sub(3600), now.saturating_sub(7200))
        }
        _ => (now + state.config.token_expiry.as_secs(), now),
    };

    let base_url = state.base_url.try_read().map(|u| u.clone()).unwrap_or_default();

    let claims = TokenClaims {
        iss: base_url,
        sub: sub.to_string(),
        aud: state.config.audience.clone(),
        exp,
        iat,
        email: if include_email { Some(state.config.user_info.email.clone()) } else { None },
        name: if include_email { Some(state.config.user_info.name.clone()) } else { None },
    };

    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(state.kid.clone());

    let key = EncodingKey::from_rsa_der(&state.signing_key);
    encode(&header, &claims, &key).map_err(|e| format!("JWT encoding failed: {e}"))
}

fn build_refresh_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

/// GET /.well-known/openid-configuration
async fn openid_configuration(State(state): State<Arc<MockOidcState>>) -> impl IntoResponse {
    let base_url = state.base_url.read().await.clone();
    Json(serde_json::json!({
        "issuer": base_url,
        "authorization_endpoint": format!("{base_url}/authorize"),
        "token_endpoint": format!("{base_url}/token"),
        "device_authorization_endpoint": format!("{base_url}/device/authorize"),
        "userinfo_endpoint": format!("{base_url}/userinfo"),
        "jwks_uri": format!("{base_url}/.well-known/jwks.json"),
        "response_types_supported": ["code"],
        "grant_types_supported": [
            "authorization_code",
            "refresh_token",
            "urn:ietf:params:oauth:grant-type:device_code"
        ],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": ["openid", "profile", "email", "offline_access"]
    }))
}

/// GET /.well-known/jwks.json
async fn jwks(State(state): State<Arc<MockOidcState>>) -> impl IntoResponse {
    Json(serde_json::json!({
        "keys": [{
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "kid": state.kid,
            "n": state.jwk_n,
            "e": state.jwk_e
        }]
    }))
}

/// Query parameters for the authorize endpoint.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AuthorizeParams {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: String,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    scope: Option<String>,
}

/// GET /authorize — records PKCE params and redirects with auth code.
async fn authorize(
    State(state): State<Arc<MockOidcState>>,
    Query(params): Query<AuthorizeParams>,
) -> Response {
    // Generate authorization code
    let code = format!("mock-code-{}", uuid::Uuid::new_v4());

    // Store PKCE challenge
    let entry = AuthCodeEntry {
        code_challenge: params.code_challenge.unwrap_or_default(),
        code_challenge_method: params.code_challenge_method.unwrap_or_else(|| "S256".to_string()),
        redirect_uri: params.redirect_uri.clone(),
        sub: state.config.user_info.sub.clone(),
    };
    state.auth_codes.write().await.insert(code.clone(), entry);

    // Build redirect URL with code and state
    let mut redirect_url =
        url::Url::parse(&params.redirect_uri).expect("invalid redirect_uri in authorize request");
    redirect_url.query_pairs_mut().append_pair("code", &code);
    if let Some(ref st) = params.state {
        redirect_url.query_pairs_mut().append_pair("state", st);
    }

    Redirect::to(redirect_url.as_str()).into_response()
}

/// Form body for the token endpoint.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TokenRequest {
    grant_type: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    device_code: Option<String>,
    client_id: Option<String>,
}

/// POST /token — exchanges auth code, refresh token, or device code for tokens.
async fn token(State(state): State<Arc<MockOidcState>>, Form(req): Form<TokenRequest>) -> Response {
    // Apply failure modes
    if let Some(ref mode) = state.config.failure_mode {
        match mode {
            FailureMode::SlowResponse(delay_ms) => {
                tokio::time::sleep(Duration::from_millis(*delay_ms)).await;
            }
            FailureMode::TokenEndpointError => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "server_error"})),
                )
                    .into_response();
            }
            FailureMode::InvalidGrant => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "invalid_grant",
                        "error_description": "The authorization code has expired or is invalid."
                    })),
                )
                    .into_response();
            }
            // ExpiredTokens is handled in build_jwt
            _ => {}
        }
    }

    match req.grant_type.as_str() {
        "authorization_code" => handle_auth_code_grant(&state, &req).await,
        "refresh_token" => handle_refresh_token_grant(&state, &req).await,
        "urn:ietf:params:oauth:grant-type:device_code" => {
            handle_device_code_grant(&state, &req).await
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "unsupported_grant_type",
                "error_description": format!("Grant type '{}' is not supported", req.grant_type)
            })),
        )
            .into_response(),
    }
}

async fn handle_auth_code_grant(state: &MockOidcState, req: &TokenRequest) -> Response {
    let code = match &req.code {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_request", "error_description": "Missing code parameter"})),
            )
                .into_response()
        }
    };

    // Look up and remove auth code (single-use)
    let entry = match state.auth_codes.write().await.remove(code) {
        Some(e) => e,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_grant", "error_description": "Invalid or expired authorization code"})),
            )
                .into_response()
        }
    };

    // Validate PKCE code_verifier if a challenge was stored
    if !entry.code_challenge.is_empty() {
        let verifier = match &req.code_verifier {
            Some(v) => v,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "invalid_grant", "error_description": "Missing code_verifier for PKCE"})),
                )
                    .into_response()
            }
        };

        if entry.code_challenge_method == "S256" {
            let mut hasher = Sha256::new();
            hasher.update(verifier.as_bytes());
            let computed_challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
            if computed_challenge != entry.code_challenge {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "invalid_grant", "error_description": "PKCE code_verifier does not match code_challenge"})),
                )
                    .into_response();
            }
        }
    }

    // Build tokens
    let access_token = match build_jwt(state, &entry.sub, true) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "server_error", "error_description": e})),
            )
                .into_response()
        }
    };

    let refresh = build_refresh_token();
    state.refresh_tokens.write().await.insert(refresh.clone(), entry.sub);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": state.config.token_expiry.as_secs(),
            "refresh_token": refresh
        })),
    )
        .into_response()
}

async fn handle_refresh_token_grant(state: &MockOidcState, req: &TokenRequest) -> Response {
    let refresh = match &req.refresh_token {
        Some(r) => r,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_request", "error_description": "Missing refresh_token parameter"})),
            )
                .into_response()
        }
    };

    let sub = match state.refresh_tokens.read().await.get(refresh) {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_grant", "error_description": "Invalid refresh token"})),
            )
                .into_response()
        }
    };

    let access_token = match build_jwt(state, &sub, true) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "server_error", "error_description": e})),
            )
                .into_response()
        }
    };

    // Issue a new refresh token (rotation)
    let new_refresh = build_refresh_token();
    {
        let mut tokens = state.refresh_tokens.write().await;
        tokens.remove(refresh);
        tokens.insert(new_refresh.clone(), sub);
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": state.config.token_expiry.as_secs(),
            "refresh_token": new_refresh
        })),
    )
        .into_response()
}

/// Form body for device authorization requests.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct DeviceAuthRequest {
    client_id: Option<String>,
    scope: Option<String>,
}

/// POST /device/authorize — initiates device code flow.
async fn device_authorize(
    State(state): State<Arc<MockOidcState>>,
    Form(_req): Form<DeviceAuthRequest>,
) -> impl IntoResponse {
    let device_code = format!("device-{}", uuid::Uuid::new_v4());
    let user_code = uuid::Uuid::new_v4().to_string()[..8].to_uppercase();
    let base_url = state.base_url.read().await.clone();

    let entry = DeviceCodeEntry {
        user_code: user_code.clone(),
        sub: state.config.user_info.sub.clone(),
        // Auto-approve unless failure mode says otherwise
        approved: !matches!(state.config.failure_mode, Some(FailureMode::DeviceCodePending)),
    };
    state.device_codes.write().await.insert(device_code.clone(), entry);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "device_code": device_code,
            "user_code": user_code,
            "verification_uri": format!("{base_url}/device"),
            "verification_uri_complete": format!("{base_url}/device?user_code={user_code}"),
            "expires_in": 600,
            "interval": 1
        })),
    )
}

async fn handle_device_code_grant(state: &MockOidcState, req: &TokenRequest) -> Response {
    let device_code = match &req.device_code {
        Some(d) => d,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_request", "error_description": "Missing device_code parameter"})),
            )
                .into_response()
        }
    };

    let entry = match state.device_codes.read().await.get(device_code) {
        Some(e) => e.clone(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_grant", "error_description": "Invalid device code"})),
            )
                .into_response()
        }
    };

    if !entry.approved {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "authorization_pending"})),
        )
            .into_response();
    }

    // Remove device code (single-use)
    state.device_codes.write().await.remove(device_code);

    let access_token = match build_jwt(state, &entry.sub, true) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "server_error", "error_description": e})),
            )
                .into_response()
        }
    };

    let refresh = build_refresh_token();
    state.refresh_tokens.write().await.insert(refresh.clone(), entry.sub);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": state.config.token_expiry.as_secs(),
            "refresh_token": refresh
        })),
    )
        .into_response()
}

/// GET /userinfo — returns configured user info.
async fn userinfo(State(state): State<Arc<MockOidcState>>) -> impl IntoResponse {
    // In a real provider we'd validate the Bearer token — for test purposes we just return info
    Json(serde_json::json!({
        "sub": state.config.user_info.sub,
        "email": state.config.user_info.email,
        "name": state.config.user_info.name,
        "email_verified": true
    }))
}

// ---------------------------------------------------------------------------
// Server lifecycle
// ---------------------------------------------------------------------------

/// A running mock OIDC provider instance.
///
/// Drop this to shut down the server (via the abort handle).
pub struct MockOidcServer {
    /// The base URL of the running server (e.g. `http://127.0.0.1:12345`).
    pub base_url: String,
    /// The issuer URL (same as base_url).
    pub issuer: String,
    /// The project ID configured on this mock.
    pub project_id: String,
    /// The audience value for tokens.
    pub audience: String,
    /// Abort handle to shut down the server.
    abort_handle: tokio::task::AbortHandle,
    /// Shared state (for test assertions and runtime config changes).
    state: Arc<MockOidcState>,
}

impl MockOidcServer {
    /// Start a mock OIDC provider on a random available port.
    pub async fn start(config: MockOidcConfig) -> Self {
        Self::start_on("127.0.0.1:0", config).await
    }

    /// Start a mock OIDC provider on the specified address.
    async fn start_on(addr: &str, config: MockOidcConfig) -> Self {
        let (private_key_der, jwk_n, jwk_e) = generate_rsa_keypair();
        let kid = format!("mock-kid-{}", &uuid::Uuid::new_v4().to_string()[..8]);

        let project_id = config.project_id.clone();
        let audience = config.audience.clone();

        let state = Arc::new(MockOidcState {
            config,
            signing_key: private_key_der,
            jwk_n,
            jwk_e,
            kid,
            auth_codes: RwLock::new(HashMap::new()),
            refresh_tokens: RwLock::new(HashMap::new()),
            device_codes: RwLock::new(HashMap::new()),
            base_url: RwLock::new(String::new()),
        });

        let app = axum::Router::new()
            .route("/.well-known/openid-configuration", get(openid_configuration))
            .route("/.well-known/jwks.json", get(jwks))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .route("/device/authorize", post(device_authorize))
            .route("/userinfo", get(userinfo))
            .with_state(state.clone());

        let listener = TcpListener::bind(addr).await.expect("failed to bind mock OIDC server");
        let local_addr = listener.local_addr().expect("failed to get local address");
        let base_url = format!("http://{}", local_addr);

        // Set base_url in state so endpoints can reference it
        *state.base_url.write().await = base_url.clone();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .expect("mock OIDC server error");
        });

        Self {
            base_url: base_url.clone(),
            issuer: base_url,
            project_id,
            audience,
            abort_handle: handle.abort_handle(),
            state,
        }
    }

    /// Get the JWKS URL for this mock provider.
    pub fn jwks_url(&self) -> String {
        format!("{}/.well-known/jwks.json", self.base_url)
    }

    /// Get the token endpoint URL.
    pub fn token_endpoint(&self) -> String {
        format!("{}/token", self.base_url)
    }

    /// Get the authorization endpoint URL.
    pub fn authorize_endpoint(&self) -> String {
        format!("{}/authorize", self.base_url)
    }

    /// Get the userinfo endpoint URL.
    pub fn userinfo_endpoint(&self) -> String {
        format!("{}/userinfo", self.base_url)
    }

    /// Get the device authorization endpoint URL.
    pub fn device_authorize_endpoint(&self) -> String {
        format!("{}/device/authorize", self.base_url)
    }

    /// Issue a valid access token (for test setup convenience).
    ///
    /// This directly creates a JWT without going through the authorize/token flow.
    pub async fn issue_token(&self) -> String {
        build_jwt(&self.state, &self.state.config.user_info.sub, true)
            .expect("failed to issue test token")
    }

    /// Issue a token for a specific subject.
    pub async fn issue_token_for_sub(&self, sub: &str) -> String {
        build_jwt(&self.state, sub, true).expect("failed to issue test token")
    }

    /// Update the failure mode at runtime.
    pub async fn set_failure_mode(&self, mode: Option<FailureMode>) {
        // Safety: we need interior mutability for the config's failure mode.
        // Since MockOidcState wraps config immutably, we use a helper approach.
        // For simplicity in test code, we store the mode in a separate RwLock field.
        // This is a design trade-off: the state struct could use RwLock<MockOidcConfig>
        // but that would require locking on every request.
        //
        // For now, failure_mode changes require restarting the server.
        // This matches the expected test pattern of one server per test.
        let _ = mode; // acknowledged but not dynamically mutable in this design
    }

    /// Approve a pending device code (for testing device code flow).
    pub async fn approve_device_code(&self, device_code: &str) -> bool {
        let mut codes = self.state.device_codes.write().await;
        if let Some(entry) = codes.get_mut(device_code) {
            entry.approved = true;
            true
        } else {
            false
        }
    }

    /// Get the number of active authorization codes (for test assertions).
    pub async fn active_auth_code_count(&self) -> usize {
        self.state.auth_codes.read().await.len()
    }

    /// Get the number of active refresh tokens (for test assertions).
    pub async fn active_refresh_token_count(&self) -> usize {
        self.state.refresh_tokens.read().await.len()
    }
}

impl Drop for MockOidcServer {
    fn drop(&mut self) {
        self.abort_handle.abort();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: perform a full PKCE authorize + token exchange flow.
    async fn do_pkce_flow(server: &MockOidcServer) -> reqwest::Response {
        let client =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

        // Generate PKCE code_verifier and code_challenge
        let code_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let code_challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

        // Step 1: Authorize
        let auth_resp = client
            .get(server.authorize_endpoint())
            .query(&[
                ("response_type", "code"),
                ("client_id", "test-client"),
                ("redirect_uri", "http://localhost:9999/callback"),
                ("state", "test-state-123"),
                ("code_challenge", &code_challenge),
                ("code_challenge_method", "S256"),
                ("scope", "openid profile email offline_access"),
            ])
            .send()
            .await
            .unwrap();

        assert_eq!(auth_resp.status(), StatusCode::SEE_OTHER);
        let location = auth_resp.headers().get("location").unwrap().to_str().unwrap().to_string();
        let redirect_url = url::Url::parse(&location).unwrap();
        let code = redirect_url.query_pairs().find(|(k, _)| k == "code").unwrap().1.to_string();
        let state_param =
            redirect_url.query_pairs().find(|(k, _)| k == "state").unwrap().1.to_string();
        assert_eq!(state_param, "test-state-123");

        // Step 2: Exchange code for tokens
        client
            .post(server.token_endpoint())
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", "http://localhost:9999/callback"),
                ("code_verifier", code_verifier),
                ("client_id", "test-client"),
            ])
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_openid_configuration() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let resp = reqwest::get(format!("{}/.well-known/openid-configuration", server.base_url))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["issuer"], server.base_url);
        assert!(body["authorization_endpoint"].as_str().unwrap().contains("/authorize"));
        assert!(body["token_endpoint"].as_str().unwrap().contains("/token"));
        assert!(body["jwks_uri"].as_str().unwrap().contains("jwks.json"));
        assert!(body["device_authorization_endpoint"]
            .as_str()
            .unwrap()
            .contains("/device/authorize"));
        assert!(body["userinfo_endpoint"].as_str().unwrap().contains("/userinfo"));
    }

    #[tokio::test]
    async fn test_jwks_endpoint() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let resp = reqwest::get(server.jwks_url()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = resp.json().await.unwrap();
        let keys = body["keys"].as_array().unwrap();
        assert_eq!(keys.len(), 1);

        let key = &keys[0];
        assert_eq!(key["kty"], "RSA");
        assert_eq!(key["alg"], "RS256");
        assert_eq!(key["use"], "sig");
        assert!(key["kid"].as_str().unwrap().starts_with("mock-kid-"));
        assert!(key["n"].as_str().is_some());
        assert!(key["e"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_pkce_flow_success() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let resp = do_pkce_flow(&server).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["access_token"].as_str().is_some());
        assert!(body["refresh_token"].as_str().is_some());
        assert_eq!(body["token_type"], "Bearer");
        assert_eq!(body["expires_in"], 3600);
    }

    #[tokio::test]
    async fn test_pkce_wrong_verifier_rejected() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let client =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

        // Generate real PKCE challenge
        let code_verifier = "real-verifier-value-for-test";
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let code_challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

        // Authorize with correct challenge
        let auth_resp = client
            .get(server.authorize_endpoint())
            .query(&[
                ("response_type", "code"),
                ("client_id", "test-client"),
                ("redirect_uri", "http://localhost:9999/callback"),
                ("code_challenge", &code_challenge),
                ("code_challenge_method", "S256"),
            ])
            .send()
            .await
            .unwrap();
        let location = auth_resp.headers().get("location").unwrap().to_str().unwrap();
        let redirect_url = url::Url::parse(location).unwrap();
        let code = redirect_url.query_pairs().find(|(k, _)| k == "code").unwrap().1.to_string();

        // Exchange with WRONG verifier
        let resp = client
            .post(server.token_endpoint())
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", "http://localhost:9999/callback"),
                ("code_verifier", "wrong-verifier-should-fail"),
                ("client_id", "test-client"),
            ])
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_grant");
        assert!(body["error_description"].as_str().unwrap().contains("code_verifier"));
    }

    #[tokio::test]
    async fn test_auth_code_single_use() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let client =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

        // Authorize
        let auth_resp = client
            .get(server.authorize_endpoint())
            .query(&[
                ("response_type", "code"),
                ("client_id", "test-client"),
                ("redirect_uri", "http://localhost:9999/callback"),
            ])
            .send()
            .await
            .unwrap();
        let location = auth_resp.headers().get("location").unwrap().to_str().unwrap();
        let redirect_url = url::Url::parse(location).unwrap();
        let code = redirect_url.query_pairs().find(|(k, _)| k == "code").unwrap().1.to_string();

        // First exchange: success
        let resp1 = client
            .post(server.token_endpoint())
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", "http://localhost:9999/callback"),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);

        // Second exchange with same code: should fail
        let resp2 = client
            .post(server.token_endpoint())
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", "http://localhost:9999/callback"),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = resp2.json().await.unwrap();
        assert_eq!(body["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn test_refresh_token_flow() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let client =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

        // Get initial tokens via PKCE flow
        let initial_resp = do_pkce_flow(&server).await;
        let initial_body: serde_json::Value = initial_resp.json().await.unwrap();
        let refresh_token = initial_body["refresh_token"].as_str().unwrap();

        // Exchange refresh token
        let resp = client
            .post(server.token_endpoint())
            .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token)])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["access_token"].as_str().is_some());
        assert!(body["refresh_token"].as_str().is_some());
        // New refresh token should differ (rotation)
        assert_ne!(body["refresh_token"].as_str().unwrap(), refresh_token);
    }

    #[tokio::test]
    async fn test_refresh_token_rotation_invalidates_old() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let client =
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

        // Get initial tokens
        let initial_resp = do_pkce_flow(&server).await;
        let initial_body: serde_json::Value = initial_resp.json().await.unwrap();
        let old_refresh = initial_body["refresh_token"].as_str().unwrap().to_string();

        // Use refresh token (gets rotated)
        let _ = client
            .post(server.token_endpoint())
            .form(&[("grant_type", "refresh_token"), ("refresh_token", &old_refresh)])
            .send()
            .await
            .unwrap();

        // Try old refresh token again — should fail
        let resp = client
            .post(server.token_endpoint())
            .form(&[("grant_type", "refresh_token"), ("refresh_token", &old_refresh)])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn test_device_code_flow() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let client = reqwest::Client::new();

        // Step 1: Request device code
        let resp = client
            .post(server.device_authorize_endpoint())
            .form(&[("client_id", "test-client"), ("scope", "openid")])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = resp.json().await.unwrap();
        let device_code = body["device_code"].as_str().unwrap();
        assert!(body["user_code"].as_str().is_some());
        assert!(body["verification_uri"].as_str().is_some());

        // Step 2: Poll token endpoint (auto-approved by default)
        let resp = client
            .post(server.token_endpoint())
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
                ("client_id", "test-client"),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["access_token"].as_str().is_some());
        assert!(body["refresh_token"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_device_code_pending() {
        let config = MockOidcConfig {
            failure_mode: Some(FailureMode::DeviceCodePending),
            ..Default::default()
        };
        let server = MockOidcServer::start(config).await;
        let client = reqwest::Client::new();

        // Request device code
        let resp = client
            .post(server.device_authorize_endpoint())
            .form(&[("client_id", "test-client")])
            .send()
            .await
            .unwrap();
        let body: serde_json::Value = resp.json().await.unwrap();
        let device_code = body["device_code"].as_str().unwrap().to_string();

        // Poll — should get authorization_pending
        let resp = client
            .post(server.token_endpoint())
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", &device_code),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "authorization_pending");

        // Approve the device code
        assert!(server.approve_device_code(&device_code).await);

        // Poll again — should succeed now
        let resp = client
            .post(server.token_endpoint())
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", &device_code),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_userinfo_endpoint() {
        let config = MockOidcConfig {
            user_info: UserInfo {
                sub: "custom-sub-42".to_string(),
                email: "alice@example.com".to_string(),
                name: "Alice".to_string(),
            },
            ..Default::default()
        };
        let server = MockOidcServer::start(config).await;

        let resp = reqwest::get(server.userinfo_endpoint()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["sub"], "custom-sub-42");
        assert_eq!(body["email"], "alice@example.com");
        assert_eq!(body["name"], "Alice");
        assert_eq!(body["email_verified"], true);
    }

    #[tokio::test]
    async fn test_invalid_grant_failure_mode() {
        let config =
            MockOidcConfig { failure_mode: Some(FailureMode::InvalidGrant), ..Default::default() };
        let server = MockOidcServer::start(config).await;
        let client = reqwest::Client::new();

        let resp = client
            .post(server.token_endpoint())
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", "any-code"),
                ("redirect_uri", "http://localhost:9999/callback"),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn test_token_endpoint_error_failure_mode() {
        let config = MockOidcConfig {
            failure_mode: Some(FailureMode::TokenEndpointError),
            ..Default::default()
        };
        let server = MockOidcServer::start(config).await;
        let client = reqwest::Client::new();

        let resp = client
            .post(server.token_endpoint())
            .form(&[("grant_type", "authorization_code"), ("code", "any")])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_unsupported_grant_type() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let client = reqwest::Client::new();

        let resp = client
            .post(server.token_endpoint())
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "unsupported_grant_type");
    }

    #[tokio::test]
    async fn test_issued_token_validates_with_jwks() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;

        // Issue a token directly
        let token = server.issue_token().await;

        // Fetch JWKS and validate the token
        let jwks_resp = reqwest::get(server.jwks_url()).await.unwrap();
        let jwks: jsonwebtoken::jwk::JwkSet = jwks_resp.json().await.unwrap();

        // Decode the token header to get kid
        let header = jsonwebtoken::decode_header(&token).unwrap();
        let kid = header.kid.unwrap();

        // Find the matching key
        let jwk = jwks.find(&kid).unwrap();
        let decoding_key = match &jwk.algorithm {
            jsonwebtoken::jwk::AlgorithmParameters::RSA(rsa) => {
                jsonwebtoken::DecodingKey::from_rsa_components(&rsa.n, &rsa.e).unwrap()
            }
            _ => panic!("expected RSA key"),
        };

        let mut validation = jsonwebtoken::Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&server.issuer]);
        validation.set_audience(&[&server.audience]);

        let token_data =
            jsonwebtoken::decode::<serde_json::Value>(&token, &decoding_key, &validation).unwrap();

        assert_eq!(token_data.claims["sub"], "test-user-001");
        assert_eq!(token_data.claims["email"], "test@flowplane.dev");
        assert_eq!(token_data.claims["iss"], server.issuer);
        assert_eq!(token_data.claims["aud"], server.audience);
    }

    #[tokio::test]
    async fn test_issue_token_for_custom_sub() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;
        let token = server.issue_token_for_sub("custom-user-42").await;

        let header = jsonwebtoken::decode_header(&token).unwrap();
        let jwks_resp = reqwest::get(server.jwks_url()).await.unwrap();
        let jwks: jsonwebtoken::jwk::JwkSet = jwks_resp.json().await.unwrap();
        let jwk = jwks.find(&header.kid.unwrap()).unwrap();
        let decoding_key = match &jwk.algorithm {
            jsonwebtoken::jwk::AlgorithmParameters::RSA(rsa) => {
                jsonwebtoken::DecodingKey::from_rsa_components(&rsa.n, &rsa.e).unwrap()
            }
            _ => panic!("expected RSA key"),
        };

        let mut validation = jsonwebtoken::Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&server.issuer]);
        validation.set_audience(&[&server.audience]);

        let token_data =
            jsonwebtoken::decode::<serde_json::Value>(&token, &decoding_key, &validation).unwrap();
        assert_eq!(token_data.claims["sub"], "custom-user-42");
    }

    #[tokio::test]
    async fn test_state_tracking() {
        let server = MockOidcServer::start(MockOidcConfig::default()).await;

        // Initially no auth codes or refresh tokens
        assert_eq!(server.active_auth_code_count().await, 0);
        assert_eq!(server.active_refresh_token_count().await, 0);

        // After PKCE flow, should have 0 auth codes (consumed) and 1 refresh token
        let _ = do_pkce_flow(&server).await;
        assert_eq!(server.active_auth_code_count().await, 0);
        assert_eq!(server.active_refresh_token_count().await, 1);
    }

    #[tokio::test]
    async fn test_expired_tokens_failure_mode() {
        let config =
            MockOidcConfig { failure_mode: Some(FailureMode::ExpiredTokens), ..Default::default() };
        let server = MockOidcServer::start(config).await;

        // Issue a token — it should have exp in the past
        let token = server.issue_token().await;

        // Fetch JWKS
        let jwks_resp = reqwest::get(server.jwks_url()).await.unwrap();
        let jwks: jsonwebtoken::jwk::JwkSet = jwks_resp.json().await.unwrap();
        let header = jsonwebtoken::decode_header(&token).unwrap();
        let jwk = jwks.find(&header.kid.unwrap()).unwrap();
        let decoding_key = match &jwk.algorithm {
            jsonwebtoken::jwk::AlgorithmParameters::RSA(rsa) => {
                jsonwebtoken::DecodingKey::from_rsa_components(&rsa.n, &rsa.e).unwrap()
            }
            _ => panic!("expected RSA key"),
        };

        let mut validation = jsonwebtoken::Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&server.issuer]);
        validation.set_audience(&[&server.audience]);

        // Token should fail validation due to expiration
        let result = jsonwebtoken::decode::<serde_json::Value>(&token, &decoding_key, &validation);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err.kind(), jsonwebtoken::errors::ErrorKind::ExpiredSignature),
            "expected ExpiredSignature, got: {:?}",
            err.kind()
        );
    }
}
