//! Startup sequence for Flowplane control plane
//!
//! Displays a setup banner when the Zitadel project is not configured,
//! and seeds dev-mode resources when running in `AuthMode::Dev`.

use crate::auth::dev_token::{DEV_USER_EMAIL, DEV_USER_SUB};
use crate::domain::UserId;
use crate::errors::Result;
use crate::storage::repositories::app_ids;
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

/// Seed dev-mode resources: org, team, user, memberships, and default dataplane.
///
/// All inserts are idempotent (`ON CONFLICT DO NOTHING`). Safe to call on every
/// startup — rows that already exist are silently skipped.
///
/// Returns the dev user's `UserId` (matches the `DevAuthState` identity).
pub async fn seed_dev_resources(pool: &sqlx::PgPool) -> Result<UserId> {
    let dev_user_id = UserId::from_str_unchecked("dev-user-id");
    let dev_org_id = "dev-org-id";
    let dev_email = DEV_USER_EMAIL;
    let now = chrono::Utc::now();

    // 1. Create dev org (idempotent)
    sqlx::query(
        "INSERT INTO organizations (id, name, display_name, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(dev_org_id)
    .bind("dev-org")
    .bind("Dev Organization")
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| crate::Error::database(e, "seed dev org".to_string()))?;

    // 2. Create default team in dev-org (idempotent)
    //    Use a deterministic ID so the team reference is stable across restarts.
    let dev_team_id = "dev-default-team-id";
    sqlx::query(
        "INSERT INTO teams (id, name, display_name, org_id, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(dev_team_id)
    .bind("default")
    .bind("Default")
    .bind(dev_org_id)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| crate::Error::database(e, "seed dev team".to_string()))?;

    // 3. Create dev user (idempotent)
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, name, status, is_admin, user_type, zitadel_sub, created_at, updated_at) \
         VALUES ($1, $2, '', 'Dev User', 'active', false, 'human', $3, $4, $5) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(dev_user_id.to_string())
    .bind(dev_email)
    .bind(DEV_USER_SUB)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| crate::Error::database(e, "seed dev user".to_string()))?;

    // 4. Create org membership for dev user as admin (idempotent)
    sqlx::query(
        "INSERT INTO organization_memberships (id, user_id, org_id, role, created_at) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (user_id, org_id) DO NOTHING",
    )
    .bind("dev-org-membership-id")
    .bind(dev_user_id.to_string())
    .bind(dev_org_id)
    .bind("admin")
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| crate::Error::database(e, "seed dev org membership".to_string()))?;

    // 5. Create team membership for dev user in default team (idempotent)
    sqlx::query(
        "INSERT INTO user_team_memberships (id, user_id, team, created_at) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (user_id, team) DO NOTHING",
    )
    .bind("dev-team-membership-id")
    .bind(dev_user_id.to_string())
    .bind(dev_team_id)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| crate::Error::database(e, "seed dev team membership".to_string()))?;

    // 6. Create default dataplane for the default team (idempotent)
    sqlx::query(
        "INSERT INTO dataplanes (id, team, name, description, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind("dev-dataplane-id")
    .bind(dev_team_id)
    .bind("dev-dataplane")
    .bind("Default dev dataplane")
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| crate::Error::database(e, "seed dev dataplane".to_string()))?;

    // 7. Enable stats dashboard app in dev mode (idempotent)
    sqlx::query(
        "INSERT INTO instance_apps (app_id, enabled, enabled_by, enabled_at, created_at, updated_at) \
         VALUES ($1, 1, $2, $3, $4, $4) \
         ON CONFLICT (app_id) DO NOTHING",
    )
    .bind(app_ids::STATS_DASHBOARD)
    .bind(dev_email)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| crate::Error::database(e, "seed dev stats_dashboard app".to_string()))?;

    info!(
        user_id = %dev_user_id,
        org = "dev-org",
        team = "default",
        "Dev resources seeded"
    );

    Ok(dev_user_id)
}
