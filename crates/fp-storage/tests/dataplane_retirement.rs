//! RED PostgreSQL schema contract for fpv2-7f3.7 dataplane retirement.
//!
//! Authored from the approved design/plan without reading production or migration source. Every
//! fixture uses UUIDv7 identifiers and unique names; this target never truncates or globally cleans
//! the shared database.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use sqlx::PgPool;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7().simple())
}

async fn pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 6)
        .await
        .expect("connect real PostgreSQL");
    fp_storage::migrate(&pool)
        .await
        .expect("apply checked-in migrations");
    Some(pool)
}

async fn seed_team(pool: &PgPool) -> (uuid::Uuid, uuid::Uuid) {
    let org_id = uuid::Uuid::now_v7();
    let team_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(unique("retirement-schema-org"))
        .execute(pool)
        .await
        .expect("organization fixture");
    sqlx::query("INSERT INTO teams (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(org_id)
        .bind(unique("retirement-schema-team"))
        .execute(pool)
        .await
        .expect("team fixture");
    (org_id, team_id)
}

#[tokio::test]
async fn migration_0036_adds_retirement_tombstone_and_active_only_name_uniqueness() {
    let Some(pool) = pool().await else { return };

    let columns: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT column_name, data_type, is_nullable FROM information_schema.columns \
         WHERE table_schema = current_schema() AND table_name = 'dataplanes' \
           AND column_name IN ('retired_at', 'retired_reason') ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .expect("inspect dataplane lifecycle columns");
    assert_eq!(
        columns,
        vec![
            (
                "retired_at".to_owned(),
                "timestamp with time zone".to_owned(),
                "YES".to_owned()
            ),
            (
                "retired_reason".to_owned(),
                "text".to_owned(),
                "YES".to_owned()
            ),
        ],
        "migration 0036 must add nullable retired_at and retired_reason columns"
    );

    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes WHERE schemaname = current_schema() \
         AND tablename = 'dataplanes' AND indexdef ILIKE '%team_id%' \
         AND indexdef ILIKE '%name%'",
    )
    .fetch_all(&pool)
    .await
    .expect("inspect dataplane name indexes");
    assert!(
        indexes.iter().any(|definition| {
            definition.contains("UNIQUE INDEX")
                && definition.contains("retired_at IS NULL")
                && definition.contains("team_id")
                && definition.contains("name")
        }),
        "active dataplane names require a partial unique index: {indexes:?}"
    );
    assert!(
        indexes.iter().all(|definition| {
            !definition.contains("UNIQUE INDEX") || definition.contains("retired_at IS NULL")
        }),
        "permanent team/name uniqueness must be removed: {indexes:?}"
    );
}

#[tokio::test]
async fn active_name_collision_is_rejected_but_a_retired_row_does_not_reserve_the_name() {
    let Some(pool) = pool().await else { return };
    let (org_id, team_id) = seed_team(&pool).await;
    let name = unique("active-only-name");

    sqlx::query("INSERT INTO dataplanes (id, team_id, org_id, name) VALUES ($1, $2, $3, $4)")
        .bind(uuid::Uuid::now_v7())
        .bind(team_id)
        .bind(org_id)
        .bind(&name)
        .execute(&pool)
        .await
        .expect("first active dataplane");
    let collision =
        sqlx::query("INSERT INTO dataplanes (id, team_id, org_id, name) VALUES ($1, $2, $3, $4)")
            .bind(uuid::Uuid::now_v7())
            .bind(team_id)
            .bind(org_id)
            .bind(&name)
            .execute(&pool)
            .await
            .expect_err("two active dataplanes with one team/name must conflict");
    assert_eq!(
        collision
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23505")
    );

    sqlx::query(
        "UPDATE dataplanes SET retired_at = now(), retired_reason = $2 WHERE team_id = $1 AND name = $3",
    )
    .bind(team_id)
    .bind("schema contract fixture")
    .bind(&name)
    .execute(&pool)
    .await
    .expect("retire first row directly for index contract");
    sqlx::query("INSERT INTO dataplanes (id, team_id, org_id, name) VALUES ($1, $2, $3, $4)")
        .bind(uuid::Uuid::now_v7())
        .bind(team_id)
        .bind(org_id)
        .bind(&name)
        .execute(&pool)
        .await
        .expect("a retired row must not reserve the active name");
}
