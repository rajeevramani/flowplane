//! Zitadel testcontainer management and OIDC token acquisition for E2E tests.
//!
//! Provides:
//! - Zitadel container lifecycle (start, health check, bootstrap)
//! - Human token acquisition via Session API + OIDC finalize flow
//! - Agent token acquisition via OAuth2 client_credentials grant
//! - Test user creation in Zitadel

use std::path::PathBuf;
use std::time::Duration;

use reqwest::Client;
use serde_json::{json, Value};
use testcontainers::core::{IntoContainerPort, Mount};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tracing::{info, warn};

/// Zitadel configuration discovered after bootstrap
#[derive(Debug, Clone)]
pub struct ZitadelTestConfig {
    /// Base URL for Zitadel API (e.g. http://localhost:PORT)
    pub base_url: String,
    /// Admin PAT for Management API calls
    pub admin_pat: String,
    /// Project ID created during bootstrap
    pub project_id: String,
    /// SPA client ID for OIDC flows
    pub spa_client_id: String,
}

/// Master key used for Zitadel encryption in test mode
const ZITADEL_MASTER_KEY: &str = "3nG1pz-cttTXaQapJLoDNSkiOQX4vPvK";

/// Default admin credentials created by Zitadel's start-from-init
const _ZITADEL_ADMIN_EMAIL: &str = "zitadel-admin@zitadel.localhost";
const ZITADEL_ADMIN_PASSWORD: &str = "Password1!"; // pragma: allowlist secret — dev-only default

/// Test superadmin credentials
pub const SUPERADMIN_EMAIL: &str = "admin@flowplane.local";
pub const SUPERADMIN_PASSWORD: &str = "Flowplane1!"; // pragma: allowlist secret — dev-only default

/// Start a Zitadel container that shares the given PostgreSQL instance.
///
/// Creates a `zitadel` database on the shared PostgreSQL, then starts Zitadel
/// configured to use it. Returns the container, its mapped HTTP port, and the
/// host-side path to the machinekey directory (for reading the PAT file).
pub async fn start_zitadel_container(
    pg_host: &str,
    pg_port: u16,
) -> anyhow::Result<(ContainerAsync<GenericImage>, u16, PathBuf)> {
    info!("Creating 'zitadel' database on shared PostgreSQL...");

    // Create the zitadel database on the shared PostgreSQL
    let admin_url = format!("postgresql://postgres:postgres@{}:{}/postgres", pg_host, pg_port);
    let pool = sqlx::PgPool::connect(&admin_url).await?;
    // Create database if not exists (idempotent — ignore error if already exists)
    let _ = sqlx::query("CREATE DATABASE zitadel").execute(&pool).await;
    pool.close().await;
    info!("Zitadel database ready on shared PostgreSQL");

    // Create a host temp directory for Zitadel to write the PAT file.
    // The Zitadel container is scratch-based (no shell, no /tmp), so we
    // bind-mount a host directory to /machinekey (matching docker-compose).
    let machinekey_dir = std::env::temp_dir().join("flowplane-e2e-machinekey");
    std::fs::create_dir_all(&machinekey_dir)?;
    let machinekey_host_path = machinekey_dir.to_string_lossy().to_string();
    info!(path = %machinekey_host_path, "Created machinekey directory");

    // Start Zitadel container
    info!("Starting Zitadel container...");
    let zitadel_image = GenericImage::new("ghcr.io/zitadel/zitadel", "latest")
        .with_exposed_port(8080.tcp())
        .with_wait_for(testcontainers::core::WaitFor::Nothing)
        .with_cmd(["start-from-init", "--masterkeyFromEnv", "--tlsMode", "disabled"])
        .with_mount(Mount::bind_mount(&machinekey_host_path, "/machinekey"))
        .with_env_var("ZITADEL_MASTERKEY", ZITADEL_MASTER_KEY)
        .with_env_var("ZITADEL_DATABASE_POSTGRES_HOST", "host.docker.internal")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_PORT", pg_port.to_string())
        .with_env_var("ZITADEL_DATABASE_POSTGRES_DATABASE", "zitadel")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_USER_USERNAME", "postgres")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_USER_PASSWORD", "postgres")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_USER_SSL_MODE", "disable")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_ADMIN_USERNAME", "postgres")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_ADMIN_PASSWORD", "postgres")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_ADMIN_SSL_MODE", "disable")
        .with_env_var("ZITADEL_EXTERNALDOMAIN", "localhost")
        .with_env_var("ZITADEL_EXTERNALSECURE", "false")
        .with_env_var("ZITADEL_DEFAULTINSTANCE_FEATURES_LOGINV2_REQUIRED", "false")
        // First instance admin user
        .with_env_var("ZITADEL_FIRSTINSTANCE_ORG_HUMAN_USERNAME", "zitadel-admin")
        .with_env_var("ZITADEL_FIRSTINSTANCE_ORG_HUMAN_PASSWORD", ZITADEL_ADMIN_PASSWORD)
        // Auto-create machine user with PAT
        .with_env_var("ZITADEL_FIRSTINSTANCE_ORG_MACHINE_MACHINE_USERNAME", "zitadel-admin-sa")
        .with_env_var("ZITADEL_FIRSTINSTANCE_ORG_MACHINE_MACHINE_NAME", "Admin Service Account")
        .with_env_var("ZITADEL_FIRSTINSTANCE_ORG_MACHINE_MACHINEKEY_TYPE", "1")
        .with_env_var(
            "ZITADEL_FIRSTINSTANCE_ORG_MACHINE_PAT_EXPIRATIONDATE",
            "2030-01-01T00:00:00Z",
        )
        .with_env_var("ZITADEL_FIRSTINSTANCE_PATPATH", "/machinekey/admin-pat.txt")
        .with_env_var("ZITADEL_FIRSTINSTANCE_MACHINEKEYPATH", "/machinekey/admin-sa.json");

    let container = zitadel_image
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start Zitadel container: {}", e))?;

    let zitadel_port = container
        .get_host_port_ipv4(8080)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get Zitadel port: {}", e))?;

    info!(port = zitadel_port, "Zitadel container started");

    Ok((container, zitadel_port, machinekey_dir))
}

/// Wait for Zitadel to be ready via /debug/ready health check.
pub async fn wait_for_zitadel_ready(base_url: &str) -> anyhow::Result<()> {
    let client = Client::new();
    let ready_url = format!("{}/debug/ready", base_url);

    info!(url = %ready_url, "Waiting for Zitadel to be ready...");

    let mut attempts = 0;
    loop {
        match client.get(&ready_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!("Zitadel is ready");
                return Ok(());
            }
            Ok(resp) => {
                if attempts % 10 == 0 {
                    warn!(status = %resp.status(), attempt = attempts, "Zitadel not ready yet");
                }
            }
            Err(_) => {
                if attempts % 10 == 0 {
                    warn!(attempt = attempts, "Zitadel not reachable yet");
                }
            }
        }

        attempts += 1;
        if attempts >= 120 {
            anyhow::bail!("Zitadel not ready after 120 seconds");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Read the admin PAT from the host-side machinekey directory.
///
/// Zitadel writes the PAT to the bind-mounted `/machinekey/admin-pat.txt`.
/// We poll the host filesystem until the file appears.
pub async fn read_admin_pat(machinekey_dir: &std::path::Path) -> anyhow::Result<String> {
    let pat_path = machinekey_dir.join("admin-pat.txt");
    info!(path = %pat_path.display(), "Waiting for admin PAT to be written...");

    let mut attempts = 0;
    loop {
        if pat_path.exists() {
            let pat = tokio::fs::read_to_string(&pat_path).await?.trim().to_string();
            if !pat.is_empty() {
                info!(pat_len = pat.len(), "Admin PAT loaded from host filesystem");
                return Ok(pat);
            }
        }

        attempts += 1;
        if attempts >= 120 {
            anyhow::bail!("Admin PAT not written after 120 seconds");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Validate that the PAT works against the Zitadel Management API.
pub async fn validate_pat(base_url: &str, pat: &str) -> anyhow::Result<()> {
    let client = Client::new();
    let mut attempts = 0;

    loop {
        let resp = client
            .post(format!("{}/management/v1/projects/_search", base_url))
            .header("Authorization", format!("Bearer {}", pat))
            .header("Content-Type", "application/json")
            .body(r#"{"queries":[]}"#)
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                info!("PAT validated successfully");
                return Ok(());
            }
            _ => {}
        }

        attempts += 1;
        if attempts >= 30 {
            anyhow::bail!("PAT validation failed after 30 attempts");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Grant IAM_LOGIN_CLIENT role to the admin service account.
/// Required for the Session API token flow.
async fn grant_login_client_role(base_url: &str, pat: &str) -> anyhow::Result<()> {
    let client = Client::new();

    // Get PAT owner's user ID
    let resp = client
        .get(format!("{}/oidc/v1/userinfo", base_url))
        .header("Authorization", format!("Bearer {}", pat))
        .send()
        .await?;

    let userinfo: Value = resp.json().await?;
    let user_id = userinfo["sub"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Could not determine PAT owner user ID"))?;

    // Grant IAM_LOGIN_CLIENT + IAM_OWNER
    let resp = client
        .put(format!("{}/admin/v1/members/{}", base_url, user_id))
        .header("Authorization", format!("Bearer {}", pat))
        .header("Content-Type", "application/json")
        .json(&json!({"roles": ["IAM_OWNER", "IAM_LOGIN_CLIENT"]}))
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Failed to grant IAM_LOGIN_CLIENT: {}", text);
    }

    info!("IAM_LOGIN_CLIENT role granted");
    Ok(())
}

/// Zitadel API helper — calls with admin PAT and returns (status, body).
async fn zitadel_api(
    client: &Client,
    base_url: &str,
    pat: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> anyhow::Result<(u16, Value)> {
    let url = format!("{}{}", base_url, path);

    let mut req = match method {
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "GET" => client.get(&url),
        "DELETE" => client.delete(&url),
        _ => anyhow::bail!("Unsupported method: {}", method),
    };

    req = req
        .header("Authorization", format!("Bearer {}", pat))
        .header("Content-Type", "application/json");

    if let Some(b) = body {
        req = req.json(&b);
    }

    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(json!(null));
    Ok((status, body))
}

/// Bootstrap Zitadel: create project, SPA app, email action.
/// Returns ZitadelTestConfig with all IDs needed for the CP.
pub async fn bootstrap_zitadel(
    base_url: &str,
    pat: &str,
    zitadel_port: u16,
) -> anyhow::Result<ZitadelTestConfig> {
    let client = Client::new();

    // Grant IAM_LOGIN_CLIENT for Session API token flow
    grant_login_client_role(base_url, pat).await?;

    // Create project
    let (status, body) = zitadel_api(
        &client,
        base_url,
        pat,
        "POST",
        "/management/v1/projects",
        Some(json!({"name": "Flowplane", "projectRoleAssertion": true})),
    )
    .await?;

    let project_id = if status == 200 || status == 201 {
        body["id"].as_str().ok_or_else(|| anyhow::anyhow!("No project ID in response"))?.to_string()
    } else {
        // Project exists, search for it
        let (_, search_body) = zitadel_api(
            &client,
            base_url,
            pat,
            "POST",
            "/management/v1/projects/_search",
            Some(json!({"queries": []})),
        )
        .await?;

        search_body["result"]
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|p| p["name"].as_str() == Some("Flowplane"))
                    .and_then(|p| p["id"].as_str())
                    .map(|s| s.to_string())
            })
            .ok_or_else(|| anyhow::anyhow!("Could not find existing Flowplane project"))?
    };
    info!(project_id = %project_id, "Project ready");

    // Create SPA application
    let redirect_uri = "http://localhost:8080/auth/callback";
    let post_logout_uri = "http://localhost:8080/login";
    let (status, body) = zitadel_api(
        &client,
        base_url,
        pat,
        "POST",
        &format!("/management/v1/projects/{}/apps/oidc", project_id),
        Some(json!({
            "name": "Flowplane UI",
            "redirectUris": [redirect_uri],
            "postLogoutRedirectUris": [post_logout_uri],
            "responseTypes": ["OIDC_RESPONSE_TYPE_CODE"],
            "grantTypes": ["OIDC_GRANT_TYPE_AUTHORIZATION_CODE"],
            "appType": "OIDC_APP_TYPE_USER_AGENT",
            "authMethodType": "OIDC_AUTH_METHOD_TYPE_NONE",
            "accessTokenType": "OIDC_TOKEN_TYPE_JWT",
            "devMode": true
        })),
    )
    .await?;

    let spa_client_id = if status == 200 || status == 201 {
        body["clientId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No clientId in SPA app response"))?
            .to_string()
    } else {
        // App exists, search for it
        let (_, search_body) = zitadel_api(
            &client,
            base_url,
            pat,
            "POST",
            &format!("/management/v1/projects/{}/apps/_search", project_id),
            Some(json!({"queries": []})),
        )
        .await?;

        search_body["result"]
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|a| a["name"].as_str() == Some("Flowplane UI"))
                    .and_then(|a| a["oidcConfig"]["clientId"].as_str())
                    .map(|s| s.to_string())
            })
            .ok_or_else(|| anyhow::anyhow!("Could not find existing SPA app client ID"))?
    };
    info!(spa_client_id = %spa_client_id, "SPA app ready");

    // Create email action (adds email claim to access tokens)
    let action_script = r#"function addEmailToAccessToken(ctx, api) { if (ctx.v1.user.human && ctx.v1.user.human.email) { api.v1.claims.setClaim("email", ctx.v1.user.human.email); if (ctx.v1.user.human.profile && ctx.v1.user.human.profile.displayName) { api.v1.claims.setClaim("name", ctx.v1.user.human.profile.displayName); } } }"#;

    let (status, body) = zitadel_api(
        &client,
        base_url,
        pat,
        "POST",
        "/management/v1/actions",
        Some(json!({
            "name": "addEmailToAccessToken",
            "script": action_script,
            "timeout": "10s",
            "allowedToFail": true
        })),
    )
    .await?;

    let action_id = if status == 200 || status == 201 {
        body["id"].as_str().map(|s| s.to_string())
    } else {
        // Search for existing action
        let (_, search_body) = zitadel_api(
            &client,
            base_url,
            pat,
            "POST",
            "/management/v1/actions/_search",
            Some(json!({"queries": []})),
        )
        .await?;

        search_body["result"].as_array().and_then(|arr| {
            arr.iter()
                .find(|a| a["name"].as_str() == Some("addEmailToAccessToken"))
                .and_then(|a| a["id"].as_str())
                .map(|s| s.to_string())
        })
    };

    if let Some(ref aid) = action_id {
        // Set action on Complement Token flow (type 2), Pre Access Token trigger (type 5)
        let _ = zitadel_api(
            &client,
            base_url,
            pat,
            "POST",
            "/management/v1/flows/2/trigger/5",
            Some(json!({"actionIds": [aid]})),
        )
        .await;
        info!(action_id = %aid, "Email action set on token flow");
    }

    // Set the external port so Zitadel generates correct issuer URLs
    // (the container maps 8080 to a random host port)
    info!(zitadel_port = zitadel_port, "Zitadel bootstrap complete");

    Ok(ZitadelTestConfig {
        base_url: base_url.to_string(),
        admin_pat: pat.to_string(),
        project_id,
        spa_client_id,
    })
}

/// Create a human user in Zitadel.
/// Returns the user ID.
pub async fn create_human_user(
    base_url: &str,
    pat: &str,
    email: &str,
    first_name: &str,
    last_name: &str,
    password: &str,
) -> anyhow::Result<String> {
    let client = Client::new();

    let (status, body) = zitadel_api(
        &client,
        base_url,
        pat,
        "POST",
        "/v2/users/human",
        Some(json!({
            "username": email,
            "profile": {
                "givenName": first_name,
                "familyName": last_name,
                "displayName": format!("{} {}", first_name, last_name)
            },
            "email": {
                "email": email,
                "isVerified": true
            },
            "password": {
                "password": password,
                "changeRequired": false
            }
        })),
    )
    .await?;

    if status == 200 || status == 201 {
        let user_id = body["userId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No userId in create user response: {:?}", body))?
            .to_string();
        info!(user_id = %user_id, email = %email, "Human user created");
        Ok(user_id)
    } else if status == 409 {
        // User exists, look them up
        let (_, search_body) = zitadel_api(
            &client,
            base_url,
            pat,
            "POST",
            "/v2/users",
            Some(json!({"queries": [{"emailQuery": {"emailAddress": email}}]})),
        )
        .await?;

        let user_id = search_body["result"]
            .as_array()
            .and_then(|arr| {
                arr.first()
                    .and_then(|u| u["userId"].as_str().or(u["id"].as_str()))
                    .map(|s| s.to_string())
            })
            .ok_or_else(|| anyhow::anyhow!("User {} exists but could not find ID", email))?;
        info!(user_id = %user_id, email = %email, "Human user already exists");
        Ok(user_id)
    } else {
        anyhow::bail!("Create user {} failed (HTTP {}): {:?}", email, status, body);
    }
}

/// Obtain an OIDC access token for a human user via the Session API flow.
///
/// This implements the same flow as `scripts/lib/zitadel-auth.sh`:
/// 1. Start OIDC authorize with x-zitadel-login-client header
/// 2. Create authenticated session via Session API
/// 3. Finalize auth request with session token
/// 4. Exchange authorization code for tokens
pub async fn obtain_human_token(
    config: &ZitadelTestConfig,
    email: &str,
    password: &str,
) -> anyhow::Result<String> {
    let client = Client::builder().redirect(reqwest::redirect::Policy::none()).build()?;

    let base_url = &config.base_url;
    let client_id = &config.spa_client_id;
    let project_id = &config.project_id;
    let redirect_uri = "http://localhost:8080/auth/callback";

    // 1. Generate PKCE code verifier and challenge
    use base64::Engine;
    use sha2::Digest;

    let code_verifier: String = {
        let mut rng_bytes = [0u8; 96];
        getrandom::fill(&mut rng_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to generate random bytes: {}", e))?;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rng_bytes)
    };
    let code_challenge = {
        let hash = sha2::Sha256::digest(code_verifier.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
    };

    // 2. Start OIDC authorize flow with x-zitadel-login-client header
    let scope = format!("openid profile email urn:zitadel:iam:org:project:id:{}:aud", project_id);
    let auth_url = format!(
        "{}/oauth/v2/authorize?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state=e2e-test",
        base_url,
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(&scope),
        code_challenge,
    );

    let resp =
        client.get(&auth_url).header("x-zitadel-login-client", &config.admin_pat).send().await?;

    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("No Location header in authorize redirect"))?
        .to_string();

    // Extract auth request ID from Location header
    let auth_request_id = extract_param(&location, "authRequest")
        .or_else(|| extract_param(&location, "authRequestID"))
        .ok_or_else(|| {
            anyhow::anyhow!("Could not extract authRequestId from Location: {}", location)
        })?;

    // 3. Create authenticated session via Session API v2
    let session_resp = client
        .post(format!("{}/v2/sessions", base_url))
        .header("Authorization", format!("Bearer {}", config.admin_pat))
        .header("Content-Type", "application/json")
        .json(&json!({
            "checks": {
                "user": {"loginName": email},
                "password": {"password": password}
            }
        }))
        .send()
        .await?;

    if !session_resp.status().is_success() {
        let text = session_resp.text().await.unwrap_or_default();
        anyhow::bail!("Session creation failed for {}: {}", email, text);
    }

    let session_body: Value = session_resp.json().await?;
    let session_id = session_body["sessionId"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No sessionId in session response"))?;
    let session_token = session_body["sessionToken"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No sessionToken in session response"))?;

    // 4. Finalize OIDC auth request with session
    let finalize_resp = client
        .post(format!("{}/v2/oidc/auth_requests/{}", base_url, auth_request_id))
        .header("Authorization", format!("Bearer {}", config.admin_pat))
        .header("Content-Type", "application/json")
        .json(&json!({
            "session": {
                "sessionId": session_id,
                "sessionToken": session_token
            }
        }))
        .send()
        .await?;

    if !finalize_resp.status().is_success() {
        let text = finalize_resp.text().await.unwrap_or_default();
        anyhow::bail!("Auth request finalize failed: {}", text);
    }

    let finalize_body: Value = finalize_resp.json().await?;
    let callback_url = finalize_body["callbackUrl"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No callbackUrl in finalize response"))?;

    let auth_code = extract_param(callback_url, "code")
        .ok_or_else(|| anyhow::anyhow!("No code in callbackUrl: {}", callback_url))?;

    // 5. Exchange authorization code for tokens
    let token_resp = client
        .post(format!("{}/oauth/v2/token", base_url))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
            urlencoding::encode(&auth_code),
            urlencoding::encode(redirect_uri),
            urlencoding::encode(client_id),
            urlencoding::encode(&code_verifier),
        ))
        .send()
        .await?;

    if !token_resp.status().is_success() {
        let text = token_resp.text().await.unwrap_or_default();
        anyhow::bail!("Token exchange failed: {}", text);
    }

    let token_body: Value = token_resp.json().await?;
    let access_token = token_body["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in token response"))?
        .to_string();

    info!(email = %email, token_len = access_token.len(), "Human OIDC token obtained");
    Ok(access_token)
}

/// Obtain an access token for a machine user via OAuth2 client_credentials grant.
///
/// Includes the project audience scope so the JWT's `aud` claim contains the
/// project ID, which the Flowplane CP requires for JWT validation.
pub async fn obtain_agent_token(
    config: &ZitadelTestConfig,
    client_id: &str,
    client_secret: &str,
) -> anyhow::Result<String> {
    let client = Client::new();
    let scope = format!("openid urn:zitadel:iam:org:project:id:{}:aud", config.project_id);

    let resp = client
        .post(format!("{}/oauth/v2/token", config.base_url))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=client_credentials&client_id={}&client_secret={}&scope={}",
            urlencoding::encode(client_id),
            urlencoding::encode(client_secret),
            urlencoding::encode(&scope),
        ))
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("client_credentials token request failed for {}: {}", client_id, text);
    }

    let body: Value = resp.json().await?;
    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in client_credentials response"))?
        .to_string();

    info!(client_id = %client_id, "Agent token obtained via client_credentials");
    Ok(access_token)
}

/// Extract a query parameter from a URL string.
fn extract_param(url: &str, param: &str) -> Option<String> {
    let query_start = url.find('?')?;
    let query = &url[query_start + 1..];
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            if key == param {
                return Some(urlencoding::decode(value).unwrap_or_default().to_string());
            }
        }
    }
    None
}

/// Set environment variables needed by the Flowplane CP to use this Zitadel instance.
pub fn set_cp_env_vars(config: &ZitadelTestConfig) {
    std::env::set_var("FLOWPLANE_ZITADEL_ISSUER", &config.base_url);
    std::env::set_var("FLOWPLANE_ZITADEL_PROJECT_ID", &config.project_id);
    std::env::set_var("FLOWPLANE_ZITADEL_JWKS_URL", format!("{}/oauth/v2/keys", config.base_url));
    std::env::set_var("FLOWPLANE_ZITADEL_SPA_CLIENT_ID", &config.spa_client_id);
    std::env::set_var("FLOWPLANE_ZITADEL_ADMIN_PAT", &config.admin_pat);
    std::env::set_var("FLOWPLANE_ZITADEL_ADMIN_URL", &config.base_url);
    std::env::set_var("FLOWPLANE_SUPERADMIN_EMAIL", SUPERADMIN_EMAIL);
    std::env::set_var("FLOWPLANE_SUPERADMIN_INITIAL_PASSWORD", SUPERADMIN_PASSWORD);
    info!("CP environment variables set for Zitadel at {}", config.base_url);
}
