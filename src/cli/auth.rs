//! CLI authentication commands — OIDC PKCE login, token display, and user info.
//!
//! Supports two authentication modes:
//! - **Dev mode**: Uses a dev token (no OIDC). The server generates and stores it.
//! - **Prod mode**: Uses OIDC Authorization Code with PKCE (S256) or device code flow.
//!
//! Mode detection is via `GET /api/v1/auth/mode` on the control plane.

use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use clap::{Args, Subcommand};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use super::config::CliConfig;

/// Default loopback callback URL for PKCE flow.
const DEFAULT_CALLBACK_HOST: &str = "127.0.0.1";
/// Default timeout for OIDC login flows (5 minutes).
const LOGIN_TIMEOUT_SECS: u64 = 300;
/// Device code polling interval.
const DEVICE_CODE_POLL_INTERVAL_SECS: u64 = 5;

// ---------------------------------------------------------------------------
// CLI definitions
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum AuthCommands {
    /// Log in to Flowplane (OIDC PKCE or dev mode)
    Login(LoginArgs),
    /// Print the current access token
    Token,
    /// Show the current authenticated user
    Whoami,
    /// Clear stored credentials and log out
    Logout,
}

#[derive(Args, Debug)]
pub struct LoginArgs {
    /// Use device code flow instead of browser-based PKCE
    #[arg(long)]
    pub device_code: bool,

    /// Override the callback URL for PKCE flow
    #[arg(long)]
    pub callback_url: Option<String>,

    /// OIDC issuer URL (overrides config/env)
    #[arg(long)]
    pub issuer: Option<String>,

    /// OIDC client ID (overrides config/env)
    #[arg(long)]
    pub client_id: Option<String>,
}

// ---------------------------------------------------------------------------
// OIDC credential storage
// ---------------------------------------------------------------------------

/// Stored OIDC credentials in ~/.flowplane/credentials (JSON format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcCredentials {
    /// The access token (JWT).
    pub access_token: String,
    /// The refresh token for obtaining new access tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Unix timestamp when the access token expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// The OIDC issuer that issued these tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
}

impl OidcCredentials {
    /// Check if the access token has expired (with 60s buffer).
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(exp) => {
                let now = chrono::Utc::now().timestamp() as u64;
                now + 60 >= exp
            }
            None => false, // No expiry info → assume valid
        }
    }

    /// Save credentials to ~/.flowplane/credentials as JSON.
    pub fn save(&self) -> Result<()> {
        let home = home_dir()?;
        let dir = home.join(".flowplane");
        ensure_dir(&dir)?;

        let path = dir.join("credentials");
        let json = serde_json::to_string_pretty(self).context("failed to serialize credentials")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)
                .with_context(|| {
                    format!("failed to create credentials file: {}", path.display())
                })?;
            file.write_all(json.as_bytes())?;
            file.flush()?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&path, &json)
                .with_context(|| format!("failed to write credentials: {}", path.display()))?;
        }

        Ok(())
    }

    /// Load credentials from ~/.flowplane/credentials.
    ///
    /// Supports both JSON format (OIDC) and plain-text format (legacy dev token).
    pub fn load() -> Result<Self> {
        let home = home_dir()?;
        let path = home.join(".flowplane").join("credentials");

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read credentials: {}", path.display()))?;

        let trimmed = content.trim();
        if trimmed.is_empty() {
            anyhow::bail!("credentials file is empty");
        }

        // Try JSON format first (OIDC credentials)
        if trimmed.starts_with('{') {
            serde_json::from_str(trimmed).context("failed to parse OIDC credentials")
        } else {
            // Legacy plain-text token format
            Ok(Self {
                access_token: trimmed.to_string(),
                refresh_token: None,
                expires_at: None,
                issuer: None,
            })
        }
    }

    /// Delete credentials file.
    pub fn delete() -> Result<()> {
        let home = home_dir()?;
        let path = home.join(".flowplane").join("credentials");
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to delete credentials: {}", path.display()))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Auth mode detection
// ---------------------------------------------------------------------------

/// Response from GET /api/v1/auth/mode.
#[derive(Debug, Deserialize)]
struct AuthModeResponse {
    auth_mode: String,
    oidc_issuer: Option<String>,
    oidc_client_id: Option<String>,
}

/// Detect the auth mode from the control plane.
async fn detect_auth_mode(base_url: &str) -> Result<AuthModeResponse> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build()?;

    let url = format!("{}/api/v1/auth/mode", base_url.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to reach control plane at {url}"))?;

    if !resp.status().is_success() {
        anyhow::bail!("auth mode endpoint returned HTTP {}", resp.status());
    }

    resp.json::<AuthModeResponse>().await.context("failed to parse auth mode response")
}

// ---------------------------------------------------------------------------
// OIDC Discovery
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OidcDiscovery {
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    device_authorization_endpoint: Option<String>,
    #[allow(dead_code)]
    userinfo_endpoint: Option<String>,
}

async fn discover_oidc(issuer: &str) -> Result<OidcDiscovery> {
    let url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));

    let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?;

    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to fetch OIDC discovery at {url}"))?;

    if !resp.status().is_success() {
        anyhow::bail!("OIDC discovery endpoint returned HTTP {}", resp.status());
    }

    resp.json::<OidcDiscovery>().await.context("failed to parse OIDC discovery document")
}

// ---------------------------------------------------------------------------
// PKCE helpers
// ---------------------------------------------------------------------------

/// Generate a cryptographically random code_verifier (43-128 chars, URL-safe).
fn generate_code_verifier() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Compute S256 code_challenge from code_verifier.
fn compute_code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

/// Generate a random CSRF state parameter.
fn generate_state() -> String {
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

// ---------------------------------------------------------------------------
// Token response
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    #[allow(dead_code)]
    token_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
    #[allow(dead_code)]
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeviceAuthResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[allow(dead_code)]
    verification_uri_complete: Option<String>,
    expires_in: u64,
    #[allow(dead_code)]
    interval: Option<u64>,
}

// ---------------------------------------------------------------------------
// Command handler
// ---------------------------------------------------------------------------

pub async fn handle_auth_command(command: AuthCommands) -> Result<()> {
    match command {
        AuthCommands::Login(args) => handle_login(args).await,
        AuthCommands::Token => handle_token().await,
        AuthCommands::Whoami => handle_whoami().await,
        AuthCommands::Logout => handle_logout(),
    }
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

async fn handle_login(args: LoginArgs) -> Result<()> {
    let config = CliConfig::load().unwrap_or_default();
    let base_url = super::config::resolve_base_url(None);

    // Detect auth mode from the control plane
    let mode = match detect_auth_mode(&base_url).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Warning: Could not reach control plane: {e}");
            eprintln!("Proceeding with OIDC login using local configuration.");
            AuthModeResponse {
                auth_mode: "prod".to_string(),
                oidc_issuer: None,
                oidc_client_id: None,
            }
        }
    };

    if mode.auth_mode == "dev" {
        println!("Control plane is running in dev mode.");
        println!("No OIDC login needed — use the dev token from 'flowplane init'.");
        println!();
        // Try to show the current dev token if available
        match OidcCredentials::load() {
            Ok(creds) => {
                println!(
                    "Current token: {}",
                    &creds.access_token[..20.min(creds.access_token.len())]
                );
                println!("(stored in ~/.flowplane/credentials)");
            }
            Err(_) => {
                println!("No credentials found. Run 'flowplane init' to generate a dev token.");
            }
        }
        return Ok(());
    }

    // Resolve OIDC parameters: args > config > auth_mode response > env
    let issuer = args
        .issuer
        .or(config.oidc_issuer)
        .or(mode.oidc_issuer)
        .or_else(|| std::env::var("FLOWPLANE_OIDC_ISSUER").ok())
        .context(
            "OIDC issuer URL not configured. Set via --issuer, config.toml (oidc_issuer), or FLOWPLANE_OIDC_ISSUER",
        )?;

    let client_id = args
        .client_id
        .or(config.oidc_client_id)
        .or(mode.oidc_client_id)
        .or_else(|| std::env::var("FLOWPLANE_OIDC_CLIENT_ID").ok())
        .context(
            "OIDC client ID not configured. Set via --client-id, config.toml (oidc_client_id), or FLOWPLANE_OIDC_CLIENT_ID",
        )?;

    // Discover OIDC endpoints
    let discovery = discover_oidc(&issuer).await?;

    let creds = if args.device_code {
        do_device_code_flow(&discovery, &client_id, &issuer).await?
    } else {
        let callback_url = args.callback_url.or(config.callback_url);
        do_pkce_flow(&discovery, &client_id, &issuer, callback_url.as_deref()).await?
    };

    // Save credentials
    creds.save()?;

    // Update config with OIDC settings for future use
    let mut config = CliConfig::load().unwrap_or_default();
    config.oidc_issuer = Some(issuer);
    config.oidc_client_id = Some(client_id);
    config.save()?;

    println!("Login successful! Credentials saved to ~/.flowplane/credentials");

    Ok(())
}

// ---------------------------------------------------------------------------
// PKCE browser flow
// ---------------------------------------------------------------------------

async fn do_pkce_flow(
    discovery: &OidcDiscovery,
    client_id: &str,
    issuer: &str,
    callback_url_override: Option<&str>,
) -> Result<OidcCredentials> {
    let code_verifier = generate_code_verifier();
    let code_challenge = compute_code_challenge(&code_verifier);
    let state = generate_state();

    // Start a local HTTP server to receive the callback
    let listener = TcpListener::bind(format!("{DEFAULT_CALLBACK_HOST}:0")).await?;
    let local_addr = listener.local_addr()?;
    let callback_url = callback_url_override
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("http://{local_addr}/callback"));

    // Build authorization URL
    let mut auth_url = url::Url::parse(&discovery.authorization_endpoint)
        .context("invalid authorization_endpoint URL")?;
    auth_url
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", &callback_url)
        .append_pair("state", &state)
        .append_pair("code_challenge", &code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("scope", "openid profile email offline_access");

    println!("Opening browser for authentication...");
    println!();
    println!("If the browser doesn't open, visit this URL:");
    println!("  {auth_url}");
    println!();

    // Try to open browser
    let _ = open_url(auth_url.as_str());

    // Wait for the callback with a timeout
    let (tx, rx) = oneshot::channel::<CallbackResult>();

    let expected_state = state.clone();
    let server_handle = tokio::spawn(async move {
        wait_for_callback(listener, tx, &expected_state).await;
    });

    let callback_result = tokio::time::timeout(Duration::from_secs(LOGIN_TIMEOUT_SECS), rx)
        .await
        .context("Login timed out after 5 minutes. Try --device-code for headless environments.")?
        .context("callback channel closed")?;

    server_handle.abort();

    let auth_code = callback_result?;

    // Exchange auth code for tokens
    let client = reqwest::Client::new();
    let resp = client
        .post(&discovery.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &auth_code),
            ("redirect_uri", &callback_url),
            ("code_verifier", &code_verifier),
            ("client_id", client_id),
        ])
        .send()
        .await
        .context("failed to exchange authorization code")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Token exchange failed: {body}");
    }

    let token_resp: TokenResponse = resp.json().await.context("failed to parse token response")?;

    let expires_at = token_resp.expires_in.map(|secs| chrono::Utc::now().timestamp() as u64 + secs);

    Ok(OidcCredentials {
        access_token: token_resp.access_token,
        refresh_token: token_resp.refresh_token,
        expires_at,
        issuer: Some(issuer.to_string()),
    })
}

type CallbackResult = Result<String>;

/// Wait for the OIDC callback on the loopback listener.
///
/// Extracts the `code` query parameter and validates the `state` parameter.
async fn wait_for_callback(
    listener: TcpListener,
    tx: oneshot::Sender<CallbackResult>,
    expected_state: &str,
) {
    // Accept a single connection
    let (mut stream, _addr) = match listener.accept().await {
        Ok(conn) => conn,
        Err(e) => {
            let _ = tx.send(Err(anyhow::anyhow!("failed to accept callback connection: {e}")));
            return;
        }
    };

    // Read the HTTP request
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = vec![0u8; 4096];
    let n = match stream.read(&mut buf).await {
        Ok(n) => n,
        Err(e) => {
            let _ = tx.send(Err(anyhow::anyhow!("failed to read callback: {e}")));
            return;
        }
    };

    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse the request line to extract the path
    let path =
        request.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("/");

    // Parse query parameters from the path
    let query_string = path.split('?').nth(1).unwrap_or("");
    let params: HashMap<&str, &str> = query_string
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            match (parts.next(), parts.next()) {
                (Some(k), Some(v)) => Some((k, v)),
                _ => None,
            }
        })
        .collect();

    // Validate state
    let state = params.get("state").copied().unwrap_or("");
    if state != expected_state {
        let resp = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<html><body><h1>Login failed</h1><p>Invalid state parameter (possible CSRF attack).</p></body></html>";
        let _ = stream.write_all(resp.as_bytes()).await;
        let _ = tx.send(Err(anyhow::anyhow!("CSRF state mismatch")));
        return;
    }

    // Check for error
    if let Some(error) = params.get("error") {
        let desc = params.get("error_description").unwrap_or(&"unknown error");
        let resp = format!("HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<html><body><h1>Login failed</h1><p>{}: {}</p></body></html>", error, desc);
        let _ = stream.write_all(resp.as_bytes()).await;
        let _ = tx.send(Err(anyhow::anyhow!("OIDC error: {}: {}", error, desc)));
        return;
    }

    // Extract authorization code
    let code = match params.get("code") {
        Some(c) => c.to_string(),
        None => {
            let resp = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n<html><body><h1>Login failed</h1><p>Missing authorization code.</p></body></html>";
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = tx.send(Err(anyhow::anyhow!("missing authorization code in callback")));
            return;
        }
    };

    // Send success response to browser
    let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body><h1>Login successful!</h1><p>You can close this tab and return to the terminal.</p></body></html>";
    let _ = stream.write_all(resp.as_bytes()).await;

    let _ = tx.send(Ok(code));
}

// ---------------------------------------------------------------------------
// Device code flow
// ---------------------------------------------------------------------------

async fn do_device_code_flow(
    discovery: &OidcDiscovery,
    client_id: &str,
    issuer: &str,
) -> Result<OidcCredentials> {
    let device_endpoint = discovery
        .device_authorization_endpoint
        .as_deref()
        .context("OIDC provider does not support device code flow")?;

    let client = reqwest::Client::new();

    // Step 1: Request device code
    let resp = client
        .post(device_endpoint)
        .form(&[("client_id", client_id), ("scope", "openid profile email offline_access")])
        .send()
        .await
        .context("failed to request device code")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Device authorization failed: {body}");
    }

    let device_resp: DeviceAuthResponse =
        resp.json().await.context("failed to parse device authorization response")?;

    println!("To authenticate, visit:");
    println!();
    println!("  {}", device_resp.verification_uri);
    println!();
    println!("And enter the code: {}", device_resp.user_code);
    println!();
    println!("Waiting for approval...");

    // Step 2: Poll token endpoint
    let deadline = std::time::Instant::now() + Duration::from_secs(device_resp.expires_in);
    let poll_interval = Duration::from_secs(DEVICE_CODE_POLL_INTERVAL_SECS);

    loop {
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("Device code flow timed out. Please try again.");
        }

        tokio::time::sleep(poll_interval).await;

        let resp = client
            .post(&discovery.token_endpoint)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", &device_resp.device_code),
                ("client_id", client_id),
            ])
            .send()
            .await
            .context("failed to poll token endpoint")?;

        if resp.status().is_success() {
            let token_resp: TokenResponse =
                resp.json().await.context("failed to parse token response")?;

            let expires_at =
                token_resp.expires_in.map(|secs| chrono::Utc::now().timestamp() as u64 + secs);

            return Ok(OidcCredentials {
                access_token: token_resp.access_token,
                refresh_token: token_resp.refresh_token,
                expires_at,
                issuer: Some(issuer.to_string()),
            });
        }

        // Check error type
        let body = resp.text().await.unwrap_or_default();
        if let Ok(err) = serde_json::from_str::<TokenErrorResponse>(&body) {
            match err.error.as_str() {
                "authorization_pending" => {
                    // Still waiting — continue polling
                    print!(".");
                    std::io::stdout().flush().ok();
                }
                "slow_down" => {
                    // Increase polling interval
                    tokio::time::sleep(poll_interval).await;
                }
                "expired_token" => {
                    anyhow::bail!("Device code expired. Please try again.");
                }
                "access_denied" => {
                    anyhow::bail!("Authentication was denied by the user.");
                }
                _ => {
                    anyhow::bail!("Device code flow error: {}", err.error);
                }
            }
        } else {
            anyhow::bail!("Unexpected response from token endpoint: {body}");
        }
    }
}

// ---------------------------------------------------------------------------
// Token refresh
// ---------------------------------------------------------------------------

/// Refresh an expired access token using the stored refresh token.
pub async fn refresh_token_if_needed(creds: &mut OidcCredentials) -> Result<bool> {
    if !creds.is_expired() {
        return Ok(false);
    }

    let refresh_token = creds
        .refresh_token
        .as_deref()
        .context("access token expired and no refresh token available — please login again")?;

    let issuer =
        creds.issuer.as_deref().context("no issuer stored in credentials — please login again")?;

    let config = CliConfig::load().unwrap_or_default();
    let client_id =
        config.oidc_client_id.context("no OIDC client ID configured — please login again")?;

    let discovery = discover_oidc(issuer).await?;

    let client = reqwest::Client::new();
    let resp = client
        .post(&discovery.token_endpoint)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &client_id),
        ])
        .send()
        .await
        .context("failed to refresh token")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "Token refresh failed: {body}\nPlease login again with 'flowplane auth login'"
        );
    }

    let token_resp: TokenResponse =
        resp.json().await.context("failed to parse refresh response")?;

    creds.access_token = token_resp.access_token;
    if let Some(new_refresh) = token_resp.refresh_token {
        creds.refresh_token = Some(new_refresh);
    }
    creds.expires_at =
        token_resp.expires_in.map(|secs| chrono::Utc::now().timestamp() as u64 + secs);

    creds.save()?;

    Ok(true)
}

// ---------------------------------------------------------------------------
// Token display
// ---------------------------------------------------------------------------

async fn handle_token() -> Result<()> {
    let mut creds = OidcCredentials::load()
        .context("No credentials found. Run 'flowplane auth login' or 'flowplane init' first.")?;

    // Try to refresh if expired
    if creds.is_expired() {
        match refresh_token_if_needed(&mut creds).await {
            Ok(true) => {
                eprintln!("Token refreshed.");
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("Warning: {e}");
            }
        }
    }

    println!("{}", creds.access_token);
    Ok(())
}

// ---------------------------------------------------------------------------
// Whoami
// ---------------------------------------------------------------------------

/// Decoded JWT payload (minimal fields).
#[derive(Debug, Deserialize)]
struct JwtPayload {
    sub: Option<String>,
    email: Option<String>,
    name: Option<String>,
    iss: Option<String>,
    exp: Option<u64>,
}

async fn handle_whoami() -> Result<()> {
    let mut creds = OidcCredentials::load()
        .context("No credentials found. Run 'flowplane auth login' or 'flowplane init' first.")?;

    // Try to refresh if expired
    if creds.is_expired() {
        match refresh_token_if_needed(&mut creds).await {
            Ok(true) => {
                eprintln!("Token refreshed.");
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("Warning: {e}");
            }
        }
    }

    // Decode JWT payload without validation (just for display)
    let parts: Vec<&str> = creds.access_token.split('.').collect();
    if parts.len() >= 2 {
        // JWT payload is base64url-encoded (may need padding)
        let payload_b64 = parts[1];
        // Add padding if needed
        let padded = match payload_b64.len() % 4 {
            2 => format!("{payload_b64}=="),
            3 => format!("{payload_b64}="),
            _ => payload_b64.to_string(),
        };

        if let Ok(payload_bytes) = URL_SAFE_NO_PAD.decode(padded.trim_end_matches('=')) {
            if let Ok(payload) = serde_json::from_slice::<JwtPayload>(&payload_bytes) {
                // If email/name are missing from the JWT (typical for Zitadel access tokens),
                // fetch them from the userinfo endpoint.
                let (email, name) = if payload.email.is_some() || payload.name.is_some() {
                    (payload.email.clone(), payload.name.clone())
                } else if let Some(issuer) = payload.iss.as_deref() {
                    fetch_userinfo(&creds.access_token, issuer)
                        .await
                        .map(|(e, n)| (Some(e), n))
                        .unwrap_or((None, None))
                } else {
                    (None, None)
                };

                println!("Subject:  {}", payload.sub.as_deref().unwrap_or("<unknown>"));
                println!("Email:    {}", email.as_deref().unwrap_or("<not set>"));
                println!("Name:     {}", name.as_deref().unwrap_or("<not set>"));
                println!("Issuer:   {}", payload.iss.as_deref().unwrap_or("<unknown>"));

                if let Some(exp) = payload.exp {
                    let exp_dt = chrono::DateTime::from_timestamp(exp as i64, 0);
                    if let Some(dt) = exp_dt {
                        let now = chrono::Utc::now();
                        if dt > now {
                            let remaining = dt - now;
                            println!(
                                "Expires:  {} ({} remaining)",
                                dt.format("%Y-%m-%d %H:%M:%S UTC"),
                                format_duration(remaining.num_seconds())
                            );
                        } else {
                            println!("Expires:  {} (EXPIRED)", dt.format("%Y-%m-%d %H:%M:%S UTC"));
                        }
                    }
                }

                return Ok(());
            }
        }
    }

    // Not a JWT — just show the token type
    println!("Token type: opaque (dev mode token)");
    println!("Token:      {}...", &creds.access_token[..20.min(creds.access_token.len())]);

    Ok(())
}

/// Fetch email and name from the Zitadel userinfo endpoint.
///
/// Derives the userinfo URL from the JWT issuer claim.
async fn fetch_userinfo(token: &str, issuer: &str) -> Option<(String, Option<String>)> {
    let userinfo_url = format!("{issuer}/oidc/v1/userinfo");
    let client = reqwest::Client::new();
    let resp = client
        .get(&userinfo_url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    let email = body.get("email").and_then(|v| v.as_str()).map(|s| s.to_string())?;
    let name = body.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());
    Some((email, name))
}

fn format_duration(total_seconds: i64) -> String {
    if total_seconds < 60 {
        format!("{total_seconds}s")
    } else if total_seconds < 3600 {
        format!("{}m {}s", total_seconds / 60, total_seconds % 60)
    } else {
        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        format!("{hours}h {minutes}m")
    }
}

// ---------------------------------------------------------------------------
// Logout
// ---------------------------------------------------------------------------

fn handle_logout() -> Result<()> {
    OidcCredentials::delete()?;
    println!("Logged out. Credentials removed.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn home_dir() -> Result<std::path::PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("unable to determine home directory")?;
    Ok(std::path::PathBuf::from(home))
}

fn ensure_dir(dir: &std::path::Path) -> Result<()> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create directory: {}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Try to open a URL in the default browser.
fn open_url(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd").args(["/c", "start", url]).spawn()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_generate_code_verifier_length() {
        let verifier = generate_code_verifier();
        // 32 bytes base64url = 43 chars
        assert_eq!(verifier.len(), 43);
    }

    #[test]
    fn test_generate_code_verifier_uniqueness() {
        let v1 = generate_code_verifier();
        let v2 = generate_code_verifier();
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_code_challenge_s256() {
        // Known test vector from RFC 7636 Appendix B
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = compute_code_challenge(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn test_generate_state_length() {
        let state = generate_state();
        // 16 bytes base64url = 22 chars
        assert_eq!(state.len(), 22);
    }

    #[test]
    fn test_oidc_credentials_save_and_load() {
        let tmp = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let creds = OidcCredentials {
            access_token: "test-access-token".to_string(),
            refresh_token: Some("test-refresh-token".to_string()),
            expires_at: Some(9999999999),
            issuer: Some("https://auth.example.com".to_string()),
        };

        creds.save().unwrap();
        let loaded = OidcCredentials::load().unwrap();

        assert_eq!(loaded.access_token, "test-access-token");
        assert_eq!(loaded.refresh_token.as_deref(), Some("test-refresh-token"));
        assert_eq!(loaded.expires_at, Some(9999999999));
        assert_eq!(loaded.issuer.as_deref(), Some("https://auth.example.com"));

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        }
    }

    #[test]
    fn test_oidc_credentials_load_legacy_format() {
        let tmp = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        // Write a plain-text token (legacy format)
        let dir = tmp.path().join(".flowplane");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("credentials"), "legacy-dev-token\n").unwrap();

        let loaded = OidcCredentials::load().unwrap();
        assert_eq!(loaded.access_token, "legacy-dev-token");
        assert!(loaded.refresh_token.is_none());
        assert!(loaded.expires_at.is_none());

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        }
    }

    #[test]
    fn test_oidc_credentials_is_expired() {
        let past = OidcCredentials {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: Some(1000),
            issuer: None,
        };
        assert!(past.is_expired());

        let future = OidcCredentials {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: Some(9999999999),
            issuer: None,
        };
        assert!(!future.is_expired());

        let no_expiry = OidcCredentials {
            access_token: "token".to_string(),
            refresh_token: None,
            expires_at: None,
            issuer: None,
        };
        assert!(!no_expiry.is_expired());
    }

    #[test]
    fn test_oidc_credentials_delete() {
        let tmp = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let creds = OidcCredentials {
            access_token: "to-delete".to_string(),
            refresh_token: None,
            expires_at: None,
            issuer: None,
        };
        creds.save().unwrap();

        // Verify saved
        assert!(OidcCredentials::load().is_ok());

        // Delete
        OidcCredentials::delete().unwrap();

        // Verify deleted
        assert!(OidcCredentials::load().is_err());

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        }
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(90), "1m 30s");
        assert_eq!(format_duration(3661), "1h 1m");
        assert_eq!(format_duration(7200), "2h 0m");
    }

    #[test]
    fn test_auth_commands_parse() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            cmd: AuthCommands,
        }

        // login
        let cli = TestCli::try_parse_from(["test", "login"]);
        assert!(cli.is_ok());

        // login --device-code
        let cli = TestCli::try_parse_from(["test", "login", "--device-code"]);
        assert!(cli.is_ok());
        if let Ok(c) = cli {
            if let AuthCommands::Login(args) = c.cmd {
                assert!(args.device_code);
            }
        }

        // token
        let cli = TestCli::try_parse_from(["test", "token"]);
        assert!(cli.is_ok());

        // whoami
        let cli = TestCli::try_parse_from(["test", "whoami"]);
        assert!(cli.is_ok());

        // logout
        let cli = TestCli::try_parse_from(["test", "logout"]);
        assert!(cli.is_ok());
    }

    #[test]
    fn test_login_args_callback_url() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            cmd: AuthCommands,
        }

        let cli = TestCli::try_parse_from([
            "test",
            "login",
            "--callback-url",
            "http://localhost:9999/cb",
        ]);
        assert!(cli.is_ok());
        if let Ok(c) = cli {
            if let AuthCommands::Login(args) = c.cmd {
                assert_eq!(args.callback_url.as_deref(), Some("http://localhost:9999/cb"));
            }
        }
    }

    #[test]
    fn test_login_args_issuer_and_client_id() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            cmd: AuthCommands,
        }

        let cli = TestCli::try_parse_from([
            "test",
            "login",
            "--issuer",
            "https://auth.example.com",
            "--client-id",
            "my-client",
        ]);
        assert!(cli.is_ok());
        if let Ok(c) = cli {
            if let AuthCommands::Login(args) = c.cmd {
                assert_eq!(args.issuer.as_deref(), Some("https://auth.example.com"));
                assert_eq!(args.client_id.as_deref(), Some("my-client"));
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_oidc_credentials_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let creds = OidcCredentials {
            access_token: "perm-test".to_string(),
            refresh_token: None,
            expires_at: None,
            issuer: None,
        };
        creds.save().unwrap();

        let path = tmp.path().join(".flowplane").join("credentials");
        let perms = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(perms & 0o777, 0o600, "credentials should be 0600, got {:o}", perms & 0o777);

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        }
    }
}
