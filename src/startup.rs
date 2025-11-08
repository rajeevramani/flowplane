//! Startup sequence for Flowplane control plane
//!
//! This module handles the initial bootstrap process, including:
//! - Auto-generation of setup tokens on first startup
//! - Display of formatted welcome banner with setup instructions
//! - Configuration of TTL and behavior via environment variables

use crate::auth::models::NewPersonalAccessToken;
use crate::auth::models::TokenStatus;
use crate::auth::setup_token::SetupToken;
use crate::domain::TokenId;
use crate::errors::{Error, Result};
use crate::storage::repository::{
    AuditEvent, AuditLogRepository, SqlxTokenRepository, TokenRepository,
};
use crate::storage::DbPool;
use std::sync::Arc;
use tracing::{info, warn};

/// Environment variable to skip auto-generation of setup tokens
const ENV_SKIP_SETUP_TOKEN: &str = "FLOWPLANE_SKIP_SETUP_TOKEN";

/// Environment variable to configure setup token TTL in days (default: 7)
const ENV_SETUP_TOKEN_TTL_DAYS: &str = "FLOWPLANE_SETUP_TOKEN_TTL_DAYS";

/// Environment variable to configure setup token max usage count (default: 1)
const ENV_SETUP_TOKEN_MAX_USAGE: &str = "FLOWPLANE_SETUP_TOKEN_MAX_USAGE";

/// Default TTL for setup tokens in days
const DEFAULT_SETUP_TOKEN_TTL_DAYS: i64 = 7;

/// Default max usage count for setup tokens
const DEFAULT_SETUP_TOKEN_MAX_USAGE: i64 = 1;

/// Check if setup token auto-generation should be skipped
fn should_skip_setup_token() -> bool {
    std::env::var(ENV_SKIP_SETUP_TOKEN)
        .map(|v| v.trim().eq_ignore_ascii_case("true") || v.trim() == "1")
        .unwrap_or(false)
}

/// Get setup token TTL from environment or use default
fn get_setup_token_ttl_days() -> i64 {
    std::env::var(ENV_SETUP_TOKEN_TTL_DAYS)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_SETUP_TOKEN_TTL_DAYS)
}

/// Get setup token max usage count from environment or use default
fn get_setup_token_max_usage() -> i64 {
    std::env::var(ENV_SETUP_TOKEN_MAX_USAGE)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_SETUP_TOKEN_MAX_USAGE)
}

/// Display formatted banner with setup token
fn display_setup_token_banner(setup_token: &str, expires_in_days: i64) {
    eprintln!();
    eprintln!("{}", "╔".to_string() + &"═".repeat(78) + "╗");
    eprintln!("║{:^78}║", "🚀 FLOWPLANE CONTROL PLANE - FIRST TIME SETUP");
    eprintln!("{}", "╠".to_string() + &"═".repeat(78) + "╣");
    eprintln!("║{:78}║", "");
    eprintln!("║  {:76}║", "A setup token has been automatically generated for initial bootstrap.");
    eprintln!("║  {:76}║", "Use this token to create your admin account via the bootstrap API.");
    eprintln!("║{:78}║", "");
    eprintln!("{}", "╠".to_string() + &"═".repeat(78) + "╣");
    eprintln!("║  {:76}║", "Setup Token:");
    eprintln!("║{:78}║", "");

    // Split token into chunks for better readability
    let token_display = format!("  {}", setup_token);
    eprintln!("║  \x1b[1;32m{:76}\x1b[0m║", token_display);

    eprintln!("║{:78}║", "");
    eprintln!("{}", "╠".to_string() + &"═".repeat(78) + "╣");
    eprintln!("║  {:76}║", "📋 Next Steps:");
    eprintln!("║{:78}║", "");
    eprintln!("║  {:76}║", "1. Use the setup token to create an admin account:");
    eprintln!("║{:78}║", "");
    eprintln!("║     {:73}║", "curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \\");
    eprintln!("║       {:71}║", "-H \"Content-Type: application/json\" \\");
    eprintln!("║       {:71}║", "-d '{");
    eprintln!("║         {:69}║", format!("\"setupToken\": \"{}\",", setup_token));
    eprintln!("║         {:69}║", "\"tokenName\": \"my-admin-token\"");
    eprintln!("║       {:71}║", "}'");
    eprintln!("║{:78}║", "");
    eprintln!("║  {:76}║", "2. Save the returned admin token securely");
    eprintln!("║{:78}║", "");
    eprintln!("║  {:76}║", "3. Use the admin token for all future API calls");
    eprintln!("║{:78}║", "");
    eprintln!("{}", "╠".to_string() + &"═".repeat(78) + "╣");
    eprintln!("║  {:76}║", "⚠️  Important:");
    eprintln!("║{:78}║", "");
    eprintln!("║  {:76}║", format!("• This token expires in {} days", expires_in_days));
    eprintln!("║  {:76}║", "• It can only be used once (for security)");
    eprintln!("║  {:76}║", "• Store it securely - it won't be shown again");
    eprintln!("║  {:76}║", "• After bootstrap, this token becomes invalid");
    eprintln!("║{:78}║", "");
    eprintln!("{}", "╚".to_string() + &"═".repeat(78) + "╝");
    eprintln!();
}

/// Display message when system is already bootstrapped
fn display_already_bootstrapped_message() {
    info!("System already bootstrapped - admin tokens exist");
    eprintln!();
    eprintln!("✅ Flowplane is ready - admin account already configured");
    eprintln!();
}

/// Handle first-time startup: auto-generate setup token if no admin tokens exist
///
/// This function:
/// 1. Checks if any tokens exist in the database
/// 2. If no tokens exist:
///    - Generates a cryptographically secure setup token
///    - Stores it in the database
///    - Displays a formatted banner with instructions
/// 3. If tokens exist:
///    - Displays a "ready" message
///    - Logs that the system is already bootstrapped
///
/// # Environment Variables
///
/// - `FLOWPLANE_SKIP_SETUP_TOKEN`: Set to "true" or "1" to skip auto-generation
/// - `FLOWPLANE_SETUP_TOKEN_TTL_DAYS`: TTL in days (default: 7)
/// - `FLOWPLANE_SETUP_TOKEN_MAX_USAGE`: Max usage count (default: 1)
///
/// # Returns
///
/// `Ok(())` on success, or an error if token generation/storage fails
pub async fn handle_first_time_startup(pool: DbPool) -> Result<()> {
    // Check if setup token generation should be skipped
    if should_skip_setup_token() {
        info!("Setup token auto-generation skipped via environment variable");
        return Ok(());
    }

    let token_repo = SqlxTokenRepository::new(pool.clone());
    let audit_repo = Arc::new(AuditLogRepository::new(pool.clone()));

    // Check if any active tokens exist
    let active_count = token_repo.count_active_tokens().await?;

    if active_count > 0 {
        // System is already bootstrapped
        display_already_bootstrapped_message();
        return Ok(());
    }

    // No tokens exist - this is first-time startup
    info!("First-time startup detected - generating setup token");

    // Get configuration from environment
    let ttl_days = get_setup_token_ttl_days();
    let max_usage = get_setup_token_max_usage();

    info!(ttl_days = ttl_days, max_usage = max_usage, "Generating setup token with configuration");

    // Generate setup token
    let setup_token_generator = SetupToken::new();
    let (token_value, hashed_secret, expires_at) =
        setup_token_generator.generate(Some(max_usage), Some(ttl_days))?;

    // Extract token ID from token value (format: fp_setup_{id}.{secret})
    let token_id = token_value
        .strip_prefix("fp_setup_")
        .and_then(|s| s.split('.').next())
        .ok_or_else(|| Error::internal("Failed to extract token ID from generated setup token"))?;

    // Store setup token in database
    let new_token = NewPersonalAccessToken {
        id: TokenId::from_string(token_id.to_string()),
        name: "auto-generated-setup-token".to_string(),
        description: Some(format!(
            "Automatically generated setup token for initial bootstrap (expires in {} days)",
            ttl_days
        )),
        hashed_secret,
        status: TokenStatus::Active,
        expires_at: Some(expires_at),
        created_by: Some("system".to_string()),
        scopes: vec!["bootstrap:initialize".to_string()],
        is_setup_token: true,
        max_usage_count: Some(max_usage),
        usage_count: 0,
    };

    token_repo.create_token(new_token).await?;

    // Log audit event
    audit_repo
        .record_auth_event(AuditEvent {
            action: "setup_token.auto_generated".to_string(),
            resource_id: Some(token_id.to_string()),
            resource_name: Some("auto-generated-setup-token".to_string()),
            metadata: serde_json::json!({
                "ttl_days": ttl_days,
                "max_usage": max_usage,
                "expires_at": expires_at,
            }),
        })
        .await?;

    // Display banner
    display_setup_token_banner(&token_value, ttl_days);

    // Also log for structured logging
    warn!(
        token = %token_value,
        expires_in_days = ttl_days,
        "Auto-generated setup token for first-time startup - save this token securely"
    );

    Ok(())
}
