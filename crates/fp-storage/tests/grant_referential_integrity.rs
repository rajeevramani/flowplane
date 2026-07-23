#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

//! Storage-level tests for migration 0033 (`grant_referential_integrity`).
//!
//! These assert schema behaviour, not repository behaviour, so fixtures are inserted with
//! direct SQL: the subject under test is the DDL itself (the composite foreign keys and the
//! carry-forward `INSERT ... SELECT`), and going through repositories would only obscure which
//! constraint fired.

use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

async fn pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    Some(pool)
}

/// SQLSTATE of a failed statement. Assertions use the code, never the message text, so a
/// PostgreSQL wording change cannot silently turn these tests green.
fn sqlstate(err: &sqlx::Error) -> String {
    err.as_database_error()
        .and_then(|e| e.code())
        .map(|c| c.to_string())
        .unwrap_or_else(|| format!("<no sqlstate: {err}>"))
}

// ---------------------------------------------------------------------------
// fixtures (shared external PostgreSQL, tests run in parallel -> unique names only)
// ---------------------------------------------------------------------------

async fn mk_org<'e, E>(ex: E) -> uuid::Uuid
where
    E: sqlx::PgExecutor<'e>,
{
    let id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(unique("org"))
        .execute(ex)
        .await
        .expect("insert org");
    id
}

async fn mk_team<'e, E>(ex: E, org_id: uuid::Uuid) -> uuid::Uuid
where
    E: sqlx::PgExecutor<'e>,
{
    let id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO teams (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(org_id)
        .bind(unique("team"))
        .execute(ex)
        .await
        .expect("insert team");
    id
}

async fn mk_user<'e, E>(ex: E) -> uuid::Uuid
where
    E: sqlx::PgExecutor<'e>,
{
    let id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, subject) VALUES ($1, $2)")
        .bind(id)
        .bind(unique("subject"))
        .execute(ex)
        .await
        .expect("insert user");
    id
}

async fn mk_agent<'e, E>(ex: E, org_id: uuid::Uuid) -> uuid::Uuid
where
    E: sqlx::PgExecutor<'e>,
{
    let id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, name, kind, token_hash) VALUES ($1, $2, $3, 'cp-tool', $4)")
        .bind(id)
        .bind(org_id)
        .bind(unique("agent"))
        .bind(unique("hash"))
        .execute(ex)
        .await
        .expect("insert agent");
    id
}

async fn mk_membership<'e, E>(ex: E, user_id: uuid::Uuid, org_id: uuid::Uuid)
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query(
        "INSERT INTO org_memberships (id, user_id, org_id, role) VALUES ($1, $2, $3, 'member')",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(user_id)
    .bind(org_id)
    .execute(ex)
    .await
    .expect("insert membership");
}

async fn insert_user_grant(
    pool: &PgPool,
    id: uuid::Uuid,
    user_id: uuid::Uuid,
    org_id: uuid::Uuid,
    team_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO user_grants (id, user_id, org_id, team_id, resource, action) \
         VALUES ($1, $2, $3, $4, 'clusters', 'read')",
    )
    .bind(id)
    .bind(user_id)
    .bind(org_id)
    .bind(team_id)
    .execute(pool)
    .await
    .map(|_| ())
}

async fn insert_agent_grant(
    pool: &PgPool,
    id: uuid::Uuid,
    agent_id: uuid::Uuid,
    org_id: uuid::Uuid,
    team_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO agent_grants (id, agent_id, org_id, team_id, resource, action) \
         VALUES ($1, $2, $3, $4, 'clusters', 'read')",
    )
    .bind(id)
    .bind(agent_id)
    .bind(org_id)
    .bind(team_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Row-scoped existence check by primary key — never a global count, since the test database
/// is shared with concurrently running tests.
async fn exists(pool: &PgPool, table: &str, id: uuid::Uuid) -> bool {
    let sql = format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE id = $1)");
    sqlx::query_scalar::<_, bool>(&sql)
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("exists probe")
}

// ---------------------------------------------------------------------------
// 1. removing an org membership cascades that org's grants, and only that org's
// ---------------------------------------------------------------------------

#[tokio::test]
async fn membership_removal_cascades_only_that_orgs_grants() {
    let Some(pool) = pool().await else { return };

    let org_a = mk_org(&pool).await;
    let org_b = mk_org(&pool).await;
    let team_a = mk_team(&pool, org_a).await;
    let team_b = mk_team(&pool, org_b).await;
    let user = mk_user(&pool).await;
    mk_membership(&pool, user, org_a).await;
    mk_membership(&pool, user, org_b).await;

    let grant_a = uuid::Uuid::now_v7();
    let grant_b = uuid::Uuid::now_v7();
    insert_user_grant(&pool, grant_a, user, org_a, team_a)
        .await
        .expect("grant a");
    insert_user_grant(&pool, grant_b, user, org_b, team_b)
        .await
        .expect("grant b");

    sqlx::query("DELETE FROM org_memberships WHERE user_id = $1 AND org_id = $2")
        .bind(user)
        .bind(org_a)
        .execute(&pool)
        .await
        .expect("delete membership in org a");

    assert!(
        !exists(&pool, "user_grants", grant_a).await,
        "grant in org A must be revoked with the membership it was contingent on"
    );
    assert!(
        exists(&pool, "user_grants", grant_b).await,
        "grant in org B must be untouched — the cascade is scoped by (user_id, org_id)"
    );
}

// ---------------------------------------------------------------------------
// 2. a grant for a user with no membership in that org is unrepresentable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_grant_without_membership_is_rejected() {
    let Some(pool) = pool().await else { return };

    let org = mk_org(&pool).await;
    let team = mk_team(&pool, org).await;
    let user = mk_user(&pool).await; // deliberately NOT a member of `org`

    let err = insert_user_grant(&pool, uuid::Uuid::now_v7(), user, org, team)
        .await
        .expect_err("membership-less user grant must be rejected");

    assert_eq!(
        sqlstate(&err),
        "23503",
        "expected foreign_key_violation, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 3. a grant whose team lives in another org is unrepresentable, on both tables
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cross_org_team_is_rejected_on_both_grant_tables() {
    let Some(pool) = pool().await else { return };

    let org_a = mk_org(&pool).await;
    let org_b = mk_org(&pool).await;
    let team_b = mk_team(&pool, org_b).await;

    // (a) user_grants: membership in A exists, but the team belongs to B.
    let user = mk_user(&pool).await;
    mk_membership(&pool, user, org_a).await;
    let err = insert_user_grant(&pool, uuid::Uuid::now_v7(), user, org_a, team_b)
        .await
        .expect_err("cross-org team on user_grants must be rejected");
    assert_eq!(
        sqlstate(&err),
        "23503",
        "user_grants: expected foreign_key_violation, got: {err}"
    );

    // (b) agent_grants: agent belongs to A, but the team belongs to B.
    let agent = mk_agent(&pool, org_a).await;
    let err = insert_agent_grant(&pool, uuid::Uuid::now_v7(), agent, org_a, team_b)
        .await
        .expect_err("cross-org team on agent_grants must be rejected");
    assert_eq!(
        sqlstate(&err),
        "23503",
        "agent_grants: expected foreign_key_violation, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 4. deleting an agent cascades its grants; suspending one does not
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_deletion_cascades_but_suspension_does_not() {
    let Some(pool) = pool().await else { return };

    let org = mk_org(&pool).await;
    let team = mk_team(&pool, org).await;

    // (a) Deletion. This uses direct SQL because NO production agent-deletion path exists —
    // nothing in the service or repository layer deletes an `agents` row today. The cascade is
    // deliberately defensive: it protects a path that has not been built yet, so that whenever
    // agent deletion is implemented it cannot leave dangling authority behind.
    let deleted_agent = mk_agent(&pool, org).await;
    let deleted_grant = uuid::Uuid::now_v7();
    insert_agent_grant(&pool, deleted_grant, deleted_agent, org, team)
        .await
        .expect("grant");

    sqlx::query("DELETE FROM agents WHERE id = $1")
        .bind(deleted_agent)
        .execute(&pool)
        .await
        .expect("delete agent");

    assert!(
        !exists(&pool, "agent_grants", deleted_grant).await,
        "deleting an agent must cascade its grants"
    );

    // (b) Suspension. Suspension is a status change, not a deletion — the grants must survive
    // so that re-activating the agent restores it intact.
    let suspended_agent = mk_agent(&pool, org).await;
    let suspended_grant = uuid::Uuid::now_v7();
    insert_agent_grant(&pool, suspended_grant, suspended_agent, org, team)
        .await
        .expect("grant");

    sqlx::query("UPDATE agents SET status = 'suspended' WHERE id = $1")
        .bind(suspended_agent)
        .execute(&pool)
        .await
        .expect("suspend agent");

    assert!(
        exists(&pool, "agent_grants", suspended_grant).await,
        "suspending an agent must NOT drop its grants — the asymmetry with deletion is intended"
    );
}

// ---------------------------------------------------------------------------
// 5. an agent grant naming an agent in another org is unrepresentable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_grant_cross_org_principal_is_rejected() {
    let Some(pool) = pool().await else { return };

    let org_a = mk_org(&pool).await;
    let org_b = mk_org(&pool).await;
    let team_a = mk_team(&pool, org_a).await;
    let foreign_agent = mk_agent(&pool, org_b).await;

    // org_id and team_id are mutually consistent (both org A) — the only thing wrong is that the
    // principal belongs to org B. Before 0033 this row was perfectly insertable: the legacy
    // `grants` table's `principal_id` carried no foreign key at all, so nothing tied the
    // principal to the org the row claimed.
    let err = insert_agent_grant(&pool, uuid::Uuid::now_v7(), foreign_agent, org_a, team_a)
        .await
        .expect_err("cross-org principal must be rejected");

    assert_eq!(
        sqlstate(&err),
        "23503",
        "expected foreign_key_violation, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 6. the shipped migration file carries valid grants forward and drops orphans
// ---------------------------------------------------------------------------

/// Split a PostgreSQL URL into (server-prefix, database-name), ignoring any query string.
fn split_db_url(url: &str) -> (String, String) {
    let (base, _query) = url.split_once('?').unwrap_or((url, ""));
    let (prefix, db) = base
        .rsplit_once('/')
        .expect("database url has a path segment");
    (prefix.to_string(), db.to_string())
}

#[tokio::test]
async fn migration_0033_carries_valid_grants_forward_and_drops_orphans() {
    use sqlx::{Connection, Executor, PgConnection};

    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };

    // A dedicated, uniquely-named database on the same server. Because this test owns the whole
    // database, unscoped assertions (bare `SELECT count(*) FROM user_grants`) are safe here and
    // ONLY here — every other test in this file shares one database with parallel tests.
    let (prefix, maintenance_db) = split_db_url(&url);
    let scratch_db = format!("fp_mig_{}", uuid::Uuid::now_v7().simple());
    let scratch_url = format!("{prefix}/{scratch_db}");

    let mut admin = PgConnection::connect(&url)
        .await
        .expect("maintenance connect");
    admin
        .execute(format!("CREATE DATABASE {scratch_db}").as_str())
        .await
        .unwrap_or_else(|e| panic!("create database {scratch_db} on {maintenance_db}: {e}"));
    admin.close().await.ok();

    let outcome = run_carry_forward_case(&scratch_url).await;

    // Best-effort cleanup; a leaked scratch database must not mask the assertion outcome.
    if let Ok(mut admin) = PgConnection::connect(&url).await {
        let _ = admin
            .execute(format!("DROP DATABASE IF EXISTS {scratch_db} WITH (FORCE)").as_str())
            .await;
        let _ = admin.close().await;
    }

    if let Err(msg) = outcome {
        panic!("{msg}");
    }
}

async fn run_carry_forward_case(scratch_url: &str) -> Result<(), String> {
    use sqlx::migrate::Migrate;
    use sqlx::{Connection, PgConnection};

    let mut conn = PgConnection::connect(scratch_url)
        .await
        .expect("scratch connect");
    conn.ensure_migrations_table().await.unwrap();

    // --- schema as of 0032: the polymorphic `grants` table, pre-split ---
    for m in fp_storage::MIGRATOR.iter().filter(|m| m.version <= 32) {
        conn.apply(m).await.unwrap();
    }

    let org_a = mk_org(&mut conn).await;
    let org_b = mk_org(&mut conn).await;
    let team_a = mk_team(&mut conn, org_a).await;
    let member = mk_user(&mut conn).await;
    mk_membership(&mut conn, member, org_a).await;
    let stranger = mk_user(&mut conn).await; // no membership anywhere
    let own_agent = mk_agent(&mut conn, org_a).await;
    let foreign_agent = mk_agent(&mut conn, org_b).await;

    let valid_user_grant = uuid::Uuid::now_v7();
    let valid_agent_grant = uuid::Uuid::now_v7();
    let orphan_user_grant = uuid::Uuid::now_v7();
    let orphan_agent_grant = uuid::Uuid::now_v7();

    // Four legacy rows, every one of them representable under 0001–0032. Note that a
    // cross-org TEAM grant is NOT seedable even pre-0033 — 0002's composite FK on
    // (team_id, org_id) already forbade it — so no such case appears here.
    let seed = [
        // (i) valid user grant: the user has an org_memberships row, team is in the same org
        (valid_user_grant, "user", member, "clusters"),
        // (ii) valid agent grant: the agent belongs to that org, team is in the same org
        (valid_agent_grant, "agent", own_agent, "clusters"),
        // (iii) orphan user grant: no org_memberships row for (user, org)
        (orphan_user_grant, "user", stranger, "listeners"),
        // (iv) orphan agent grant: principal_id names an agent in a DIFFERENT org
        (orphan_agent_grant, "agent", foreign_agent, "listeners"),
    ];
    for (id, principal_type, principal_id, resource) in seed {
        sqlx::query(
            "INSERT INTO grants (id, principal_type, principal_id, org_id, team_id, resource, action) \
             VALUES ($1, $2, $3, $4, $5, $6, 'read')",
        )
        .bind(id)
        .bind(principal_type)
        .bind(principal_id)
        .bind(org_a)
        .bind(team_a)
        .bind(resource)
        .execute(&mut conn)
        .await
        .unwrap_or_else(|e| panic!("seed legacy grant {id}: {e}"));
    }

    let before: Vec<(uuid::Uuid, DateTime<Utc>)> =
        sqlx::query("SELECT id, created_at FROM grants ORDER BY id")
            .fetch_all(&mut conn)
            .await
            .expect("read legacy grants")
            .into_iter()
            .map(|r| (r.get("id"), r.get("created_at")))
            .collect();
    let created_at_of = |id: uuid::Uuid| -> DateTime<Utc> {
        before.iter().find(|(i, _)| *i == id).expect("seeded row").1
    };

    // --- apply the shipped 0033 itself; nothing below transcribes its SQL ---
    let m0033 = fp_storage::MIGRATOR
        .iter()
        .find(|m| m.version == 33)
        .expect("migration 0033 present");
    conn.apply(m0033).await.unwrap();

    let user_rows: Vec<(uuid::Uuid, DateTime<Utc>)> =
        sqlx::query("SELECT id, created_at FROM user_grants")
            .fetch_all(&mut conn)
            .await
            .expect("read user_grants")
            .into_iter()
            .map(|r| (r.get("id"), r.get("created_at")))
            .collect();
    let agent_rows: Vec<(uuid::Uuid, DateTime<Utc>)> =
        sqlx::query("SELECT id, created_at FROM agent_grants")
            .fetch_all(&mut conn)
            .await
            .expect("read agent_grants")
            .into_iter()
            .map(|r| (r.get("id"), r.get("created_at")))
            .collect();
    let legacy_gone: bool = sqlx::query_scalar("SELECT to_regclass('grants_legacy') IS NULL")
        .fetch_one(&mut conn)
        .await
        .expect("probe grants_legacy");

    conn.close().await.ok();

    if user_rows != vec![(valid_user_grant, created_at_of(valid_user_grant))] {
        return Err(format!(
            "user_grants should hold exactly the valid user grant with its original id and created_at; got {user_rows:?}"
        ));
    }
    if agent_rows != vec![(valid_agent_grant, created_at_of(valid_agent_grant))] {
        return Err(format!(
            "agent_grants should hold exactly the valid agent grant with its original id and created_at; got {agent_rows:?}"
        ));
    }
    if !legacy_gone {
        return Err("grants_legacy must not survive the migration".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 7. re-adding a membership does not resurrect the grants it cascaded away
// ---------------------------------------------------------------------------

#[tokio::test]
async fn re_adding_membership_does_not_restore_grants() {
    let Some(pool) = pool().await else { return };

    let org = mk_org(&pool).await;
    let team = mk_team(&pool, org).await;
    let user = mk_user(&pool).await;
    mk_membership(&pool, user, org).await;

    let grant = uuid::Uuid::now_v7();
    insert_user_grant(&pool, grant, user, org, team)
        .await
        .expect("grant");

    sqlx::query("DELETE FROM org_memberships WHERE user_id = $1 AND org_id = $2")
        .bind(user)
        .bind(org)
        .execute(&pool)
        .await
        .expect("delete membership");
    assert!(
        !exists(&pool, "user_grants", grant).await,
        "grant should cascade away"
    );

    mk_membership(&pool, user, org).await;

    // Grants are destroyed, not archived, when a membership is removed. Re-adding the person to
    // the org gives them a membership and nothing else — their previous authority must be
    // re-granted explicitly. This is the intentional, documented release behaviour, not a bug:
    // silently resurrecting revoked authority would be the security defect.
    assert!(
        !exists(&pool, "user_grants", grant).await,
        "re-adding the membership must NOT restore previously cascaded grants"
    );
}
