//! Dev-mode resource seeding (v2 of v1's `startup.rs::seed_dev_resources`).
//!
//! Inserts a dev org, default team, dev user, and owner membership — all idempotent
//! (`ON CONFLICT DO NOTHING`) with fixed UUIDs so references are stable across restarts.
//! Deliberately does NOT mark anything as the platform org: dev mode has no platform admin,
//! matching v1 (spec/05 §2). Only invoked from the dev-mode startup path.

use fp_domain::{DomainError, DomainResult, OrgId, TeamId, UserId};
use sqlx::PgPool;
use uuid::Uuid;

/// Stable dev identifiers (UUIDv8-style constants; never collide with v7-generated ids).
pub fn dev_org_id() -> OrgId {
    OrgId::from(Uuid::from_u128(0x000F_1071_0000_0000_0001))
}
pub fn dev_team_id() -> TeamId {
    TeamId::from(Uuid::from_u128(0x000F_1071_0000_0000_0002))
}
pub fn dev_user_id() -> UserId {
    UserId::from(Uuid::from_u128(0x000F_1071_0000_0000_0003))
}

const DEV_SUBJECT: &str = "dev-user";

/// Seed dev org/team/user/membership. Safe to call on every dev-mode boot.
pub async fn seed_dev(pool: &PgPool) -> DomainResult<UserId> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DomainError::internal(format!("seed: begin transaction: {e}")))?;

    sqlx::query(
        "INSERT INTO organizations (id, name, display_name) \
         VALUES ($1, 'dev-org', 'Dev Organization') ON CONFLICT (id) DO NOTHING",
    )
    .bind(dev_org_id().as_uuid())
    .execute(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("seed dev org: {e}")))?;

    sqlx::query(
        "INSERT INTO teams (id, org_id, name, display_name) \
         VALUES ($1, $2, 'default', 'Default') ON CONFLICT (id) DO NOTHING",
    )
    .bind(dev_team_id().as_uuid())
    .bind(dev_org_id().as_uuid())
    .execute(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("seed dev team: {e}")))?;

    sqlx::query(
        "INSERT INTO users (id, subject, email, name) \
         VALUES ($1, $2, 'dev@flowplane.local', 'Dev User') ON CONFLICT (id) DO NOTHING",
    )
    .bind(dev_user_id().as_uuid())
    .bind(DEV_SUBJECT)
    .execute(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("seed dev user: {e}")))?;

    sqlx::query(
        "INSERT INTO org_memberships (id, user_id, org_id, role) \
         VALUES (gen_random_uuid(), $1, $2, 'owner') ON CONFLICT (user_id, org_id) DO NOTHING",
    )
    .bind(dev_user_id().as_uuid())
    .bind(dev_org_id().as_uuid())
    .execute(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("seed dev membership: {e}")))?;

    sqlx::query(
        "INSERT INTO team_memberships (id, user_id, team_id) \
         VALUES (gen_random_uuid(), $1, $2) ON CONFLICT (user_id, team_id) DO NOTHING",
    )
    .bind(dev_user_id().as_uuid())
    .bind(dev_team_id().as_uuid())
    .execute(&mut *tx)
    .await
    .map_err(|e| DomainError::internal(format!("seed dev team membership: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| DomainError::internal(format!("seed: commit: {e}")))?;
    tracing::info!(
        org = "dev-org",
        team = "default",
        user = DEV_SUBJECT,
        "dev resources seeded"
    );
    Ok(dev_user_id())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn seeding_is_idempotent_and_links_membership() {
        let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return;
        };
        let pool = crate::connect(&url, 2).await.expect("connect");
        crate::migrate(&pool).await.expect("migrate");

        let first = seed_dev(&pool).await.expect("first seed");
        let second = seed_dev(&pool).await.expect("second seed must be a no-op");
        assert_eq!(first, second);

        let (role,): (String,) =
            sqlx::query_as("SELECT role FROM org_memberships WHERE user_id = $1 AND org_id = $2")
                .bind(dev_user_id().as_uuid())
                .bind(dev_org_id().as_uuid())
                .fetch_one(&pool)
                .await
                .expect("membership row exists");
        assert_eq!(role, "owner");

        let (count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM org_memberships WHERE user_id = $1")
                .bind(dev_user_id().as_uuid())
                .fetch_one(&pool)
                .await
                .expect("count");
        assert_eq!(count, 1, "re-seeding must not duplicate memberships");
    }
}
