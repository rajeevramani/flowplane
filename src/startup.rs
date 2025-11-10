//! Startup sequence for Flowplane control plane
//!
//! This module handles the initial bootstrap process, including:
//! - Display of formatted welcome banner with bootstrap instructions
//! - Detection of first-time startup (no tokens in database)
//! - Environment variable configuration for startup behavior

use crate::errors::Result;
use crate::storage::repository::{SqlxTokenRepository, TokenRepository};
use crate::storage::DbPool;
use tracing::info;

/// Environment variable to skip first-time startup banner
const ENV_SKIP_SETUP_TOKEN: &str = "FLOWPLANE_SKIP_SETUP_TOKEN";

/// Check if first-time startup banner should be skipped
fn should_skip_setup_token() -> bool {
    std::env::var(ENV_SKIP_SETUP_TOKEN)
        .map(|v| v.trim().eq_ignore_ascii_case("true") || v.trim() == "1")
        .unwrap_or(false)
}

/// Display formatted banner for first-time setup
fn display_first_time_setup_banner() {
    eprintln!();
    eprintln!("{}", "╔".to_string() + &"═".repeat(78) + "╗");
    eprintln!("║{:^78}║", "🚀 FLOWPLANE CONTROL PLANE - FIRST TIME SETUP");
    eprintln!("{}", "╠".to_string() + &"═".repeat(78) + "╣");
    eprintln!("║{:78}║", "");
    eprintln!("║  {:76}║", "Welcome! This appears to be your first time running Flowplane.");
    eprintln!("║  {:76}║", "To get started, you'll need to initialize the system.");
    eprintln!("║{:78}║", "");
    eprintln!("{}", "╠".to_string() + &"═".repeat(78) + "╣");
    eprintln!("║  {:76}║", "📋 Next Steps:");
    eprintln!("║{:78}║", "");
    eprintln!("║  {:76}║", "1. Call the bootstrap endpoint to generate your setup token:");
    eprintln!("║{:78}║", "");
    eprintln!("║     {:73}║", "curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \\");
    eprintln!("║       {:71}║", "-H \"Content-Type: application/json\" \\");
    eprintln!("║       {:71}║", "-d '{\"adminEmail\": \"admin@example.com\"}'");
    eprintln!("║{:78}║", "");
    eprintln!("║  {:76}║", "2. The response will include a setup token");
    eprintln!("║{:78}║", "");
    eprintln!("║  {:76}║", "3. Use that setup token to create a session:");
    eprintln!("║{:78}║", "");
    eprintln!("║     {:73}║", "curl -X POST http://localhost:8080/api/v1/auth/sessions \\");
    eprintln!("║       {:71}║", "-H \"Content-Type: application/json\" \\");
    eprintln!("║       {:71}║", "-d '{\"setupToken\": \"<your-setup-token>\"}'");
    eprintln!("║{:78}║", "");
    eprintln!("{}", "╠".to_string() + &"═".repeat(78) + "╣");
    eprintln!("║  {:76}║", "🔒 Security:");
    eprintln!("║{:78}║", "");
    eprintln!("║  {:76}║", "• Setup tokens are single-use and expire quickly");
    eprintln!("║  {:76}║", "• The bootstrap endpoint can only be called when uninitialized");
    eprintln!("║  {:76}║", "• No sensitive data is logged to stdout/stderr");
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

/// Handle first-time startup: display instructions for bootstrapping
///
/// This function:
/// 1. Checks if any tokens exist in the database
/// 2. If no tokens exist:
///    - Displays a formatted banner with bootstrap instructions
///    - Instructs user to call /api/v1/bootstrap/initialize
/// 3. If tokens exist:
///    - Displays a "ready" message
///    - Logs that the system is already bootstrapped
///
/// # Environment Variables
///
/// - `FLOWPLANE_SKIP_SETUP_TOKEN`: Set to "true" or "1" to skip first-time banner
///
/// # Returns
///
/// `Ok(())` on success, or an error if database check fails
pub async fn handle_first_time_startup(pool: DbPool) -> Result<()> {
    // Check if startup banner should be skipped
    if should_skip_setup_token() {
        info!("First-time startup banner skipped via environment variable");
        return Ok(());
    }

    let token_repo = SqlxTokenRepository::new(pool.clone());

    // Check if any active tokens exist
    let active_count = token_repo.count_active_tokens().await?;

    if active_count > 0 {
        // System is already bootstrapped
        display_already_bootstrapped_message();
        return Ok(());
    }

    // No tokens exist - this is first-time startup
    info!("First-time startup detected - displaying bootstrap instructions");
    display_first_time_setup_banner();

    Ok(())
}
