//! Startup sequence for Flowplane control plane
//!
//! Displays a setup banner when the Zitadel project is not configured.

use crate::errors::Result;
use tracing::info;

/// Check whether the Zitadel project has been configured.
///
/// Logs a warning if `FLOWPLANE_ZITADEL_PROJECT_ID` is unset,
/// otherwise confirms readiness.
pub async fn handle_first_time_startup() -> Result<()> {
    let project_id = std::env::var("FLOWPLANE_ZITADEL_PROJECT_ID").unwrap_or_default();

    if project_id.is_empty() {
        info!("FLOWPLANE_ZITADEL_PROJECT_ID not set — system needs configuration");
        eprintln!();
        eprintln!("Flowplane requires Zitadel for authentication.");
        eprintln!(
            "Set FLOWPLANE_ZITADEL_PROJECT_ID after creating a project in the Zitadel console."
        );
        eprintln!();
    } else {
        info!(project_id, "Zitadel project configured — ready");
    }

    Ok(())
}
