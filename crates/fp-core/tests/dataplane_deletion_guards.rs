//! RED service/PostgreSQL deletion-guard contract for fpv2-7f3.8.
//!
//! UUIDv7-isolated fixtures only; no cleanup is required or performed. Both active and retired
//! dataplane history must be classified before a hard-delete can reach a foreign-key error.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::{ErrorCode, OrgRole, RequestId};
use fp_storage::repos::identity;
use sqlx::PgPool;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7().simple())
}

async fn pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8)
        .await
        .expect("connect real PostgreSQL");
    fp_storage::migrate(&pool)
        .await
        .expect("migrate PostgreSQL");
    Some(pool)
}

struct Fixture {
    team: TeamRef,
    ctx: PrincipalCtx,
    dataplane_id: uuid::Uuid,
    certificate_id: uuid::Uuid,
    org_id: fp_domain::OrgId,
}

async fn fixture(pool: &PgPool, retired: bool) -> Fixture {
    let org = identity::create_org(pool, &unique("delete-guard-org"), "")
        .await
        .expect("org fixture");
    let team_row = identity::create_team(pool, org.id, &unique("delete-guard-team"), "")
        .await
        .expect("team fixture");
    let user = identity::upsert_user_by_subject(
        pool,
        &unique("delete-guard-user"),
        "delete-guard@test.invalid",
        "Delete Guard",
    )
    .await
    .expect("user fixture");
    identity::add_org_membership(pool, user, org.id, OrgRole::Admin)
        .await
        .expect("membership fixture");
    let ctx = PrincipalCtx::User {
        user_id: user,
        platform_admin: false,
        org_selector_required: false,
        org: Some((org.id, OrgRole::Admin)),
        grants: GrantSet::default(),
    };
    let team = TeamRef {
        id: team_row.id,
        org_id: org.id,
    };
    let dataplane_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO dataplanes \
         (id, team_id, org_id, name, retired_at, retired_reason) \
         VALUES ($1, $2, $3, $4, \
                 CASE WHEN $5 THEN now() ELSE NULL END, \
                 CASE WHEN $5 THEN 'retained deletion blocker' ELSE NULL END)",
    )
    .bind(dataplane_id)
    .bind(team.id.as_uuid())
    .bind(org.id.as_uuid())
    .bind(unique("delete-guard-dataplane"))
    .bind(retired)
    .execute(pool)
    .await
    .expect("dataplane fixture");
    let certificate_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO proxy_certificates \
         (id, team_id, dataplane_id, spiffe_uri, serial_number, fingerprint_sha256, expires_at, \
          revoked_at, revoked_reason) \
         VALUES ($1, $2, $3, $4, $5, $6, now() + interval '1 hour', \
                 CASE WHEN $7 THEN now() ELSE NULL END, \
                 CASE WHEN $7 THEN 'dataplane retired' ELSE NULL END)",
    )
    .bind(certificate_id)
    .bind(team.id.as_uuid())
    .bind(dataplane_id)
    .bind(format!("spiffe://flowplane.test/dataplane/{dataplane_id}"))
    .bind(uuid::Uuid::now_v7().simple().to_string())
    .bind(format!(
        "{}{}",
        uuid::Uuid::now_v7().simple(),
        uuid::Uuid::now_v7().simple()
    ))
    .bind(retired)
    .execute(pool)
    .await
    .expect("certificate history fixture");
    Fixture {
        team,
        ctx,
        dataplane_id,
        certificate_id,
        org_id: org.id,
    }
}

struct EmptyTeamFixture {
    team: TeamRef,
    ctx: PrincipalCtx,
}

async fn empty_team_fixture(pool: &PgPool) -> EmptyTeamFixture {
    let org = identity::create_org(pool, &unique("delete-race-org"), "")
        .await
        .expect("org fixture");
    let team_row = identity::create_team(pool, org.id, &unique("delete-race-team"), "")
        .await
        .expect("team fixture");
    let user = identity::upsert_user_by_subject(
        pool,
        &unique("delete-race-user"),
        "delete-race@test.invalid",
        "Delete Race",
    )
    .await
    .expect("user fixture");
    identity::add_org_membership(pool, user, org.id, OrgRole::Admin)
        .await
        .expect("membership fixture");
    EmptyTeamFixture {
        team: TeamRef {
            id: team_row.id,
            org_id: org.id,
        },
        ctx: PrincipalCtx::User {
            user_id: user,
            platform_admin: false,
            org_selector_required: false,
            org: Some((org.id, OrgRole::Admin)),
            grants: GrantSet::default(),
        },
    }
}

async fn empty_org_fixture(pool: &PgPool) -> (fp_domain::OrgId, PrincipalCtx) {
    let org = identity::create_org(pool, &unique("org-delete-race"), "")
        .await
        .expect("org fixture");
    let user = identity::upsert_user_by_subject(
        pool,
        &unique("org-delete-race-user"),
        "org-delete-race@test.invalid",
        "Org Delete Race",
    )
    .await
    .expect("user fixture");
    (
        org.id,
        PrincipalCtx::User {
            user_id: user,
            platform_admin: true,
            org_selector_required: false,
            org: None,
            grants: GrantSet::default(),
        },
    )
}

async fn single_connection_pool() -> PgPool {
    let url = std::env::var("FLOWPLANE_TEST_DATABASE_URL").expect("database URL remains set");
    fp_storage::connect(&url, 1)
        .await
        .expect("dedicated race connection")
}

async fn backend_pid(pool: &PgPool) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(pool)
        .await
        .expect("dedicated backend pid")
}

async fn wait_for_lock_wait(observer: &PgPool, pid: i32) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity \
                 WHERE pid = $1 AND state = 'active' AND wait_event_type = 'Lock')",
            )
            .bind(pid)
            .fetch_one(observer)
            .await
            .expect("observe race backend");
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("race operation reached a PostgreSQL lock wait");
}

async fn insert_certificate_history(
    pool: &PgPool,
    team_id: uuid::Uuid,
    dataplane_id: uuid::Uuid,
) -> uuid::Uuid {
    let certificate_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO proxy_certificates \
         (id, team_id, dataplane_id, spiffe_uri, serial_number, fingerprint_sha256, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now() + interval '1 hour')",
    )
    .bind(certificate_id)
    .bind(team_id)
    .bind(dataplane_id)
    .bind(format!("spiffe://flowplane.test/dataplane/{dataplane_id}"))
    .bind(uuid::Uuid::now_v7().simple().to_string())
    .bind(format!(
        "{}{}",
        uuid::Uuid::now_v7().simple(),
        uuid::Uuid::now_v7().simple()
    ))
    .execute(pool)
    .await
    .expect("certificate history fixture");
    certificate_id
}

async fn assert_history_intact(pool: &PgPool, fixture: &Fixture) {
    let team_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM teams WHERE id = $1)")
        .bind(fixture.team.id.as_uuid())
        .fetch_one(pool)
        .await
        .expect("team existence");
    let org_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM organizations WHERE id = $1)")
            .bind(fixture.org_id.as_uuid())
            .fetch_one(pool)
            .await
            .expect("org existence");
    let dataplane_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM dataplanes WHERE id = $1)")
            .bind(fixture.dataplane_id)
            .fetch_one(pool)
            .await
            .expect("dataplane existence");
    let certificate_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM proxy_certificates WHERE id = $1 AND dataplane_id = $2)",
    )
    .bind(fixture.certificate_id)
    .bind(fixture.dataplane_id)
    .fetch_one(pool)
    .await
    .expect("certificate existence");
    assert!(team_exists && org_exists && dataplane_exists && certificate_exists);
}

#[tokio::test]
async fn team_delete_classifies_active_and_retired_dataplanes_without_purging_history() {
    let Some(pool) = pool().await else { return };
    for retired in [false, true] {
        let fixture = fixture(&pool, retired).await;
        let error = fp_core::services::teams::delete_team(
            &pool,
            &fixture.ctx,
            fixture.team,
            RequestId::generate(),
        )
        .await
        .expect_err("dataplane history must block hard team deletion");
        assert_eq!(
            error.code,
            ErrorCode::Conflict,
            "guard must classify before any internal/FK error: {error:?}"
        );
        assert_history_intact(&pool, &fixture).await;
    }
}

#[tokio::test]
async fn org_delete_classifies_active_and_retired_dataplane_history_without_purging_it() {
    let Some(pool) = pool().await else { return };
    for retired in [false, true] {
        let fixture = fixture(&pool, retired).await;
        let platform_ctx = match &fixture.ctx {
            PrincipalCtx::User { user_id, .. } => PrincipalCtx::User {
                user_id: *user_id,
                platform_admin: true,
                org_selector_required: false,
                org: None,
                grants: GrantSet::default(),
            },
            PrincipalCtx::Agent { .. } => unreachable!("fixture creates a user principal"),
        };
        let error = fp_core::services::orgs::delete_org(
            &pool,
            &platform_ctx,
            fixture.org_id,
            RequestId::generate(),
        )
        .await
        .expect_err("dataplane-bearing org must not hard-delete");
        assert_eq!(
            error.code,
            ErrorCode::Conflict,
            "guard must classify before any internal/FK error: {error:?}"
        );
        assert_history_intact(&pool, &fixture).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dataplane_create_committing_during_team_delete_forces_conflict_and_preserves_history() {
    let Some(pool) = pool().await else { return };
    let fixture = empty_team_fixture(&pool).await;
    let delete_pool = single_connection_pool().await;
    let delete_pid = backend_pid(&delete_pool).await;
    let mut blocker = pool.begin().await.expect("begin team race blocker");
    sqlx::query("SELECT id FROM teams WHERE id = $1 FOR NO KEY UPDATE")
        .bind(fixture.team.id.as_uuid())
        .fetch_one(&mut *blocker)
        .await
        .expect("lock team parent row");
    let delete_ctx = fixture.ctx.clone();
    let team = fixture.team;
    let delete_task = tokio::spawn(async move {
        fp_core::services::teams::delete_team(
            &delete_pool,
            &delete_ctx,
            team,
            RequestId::generate(),
        )
        .await
    });
    wait_for_lock_wait(&pool, delete_pid).await;
    let dataplane = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fp_core::services::dataplanes::create_dataplane(
            &pool,
            &fixture.ctx,
            fixture.team,
            &unique("team-delete-race-dataplane"),
            "committed while deletion is waiting",
            RequestId::generate(),
        ),
    )
    .await
    .expect("dataplane create is not blocked")
    .expect("concurrent dataplane commits");
    let dataplane_id = dataplane.id.as_uuid();
    let certificate_id =
        insert_certificate_history(&pool, fixture.team.id.as_uuid(), dataplane_id).await;
    blocker.commit().await.expect("release team race blocker");
    let delete = tokio::time::timeout(std::time::Duration::from_secs(5), delete_task)
        .await
        .expect("team delete completed")
        .expect("team delete task");
    let error = delete.expect_err("committed dataplane must make team deletion lose");
    assert_eq!(error.code, ErrorCode::Conflict, "no Internal/FK: {error:?}");
    let retained: (bool, bool, bool) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM teams WHERE id = $1), \
                EXISTS(SELECT 1 FROM dataplanes WHERE id = $2), \
                EXISTS(SELECT 1 FROM proxy_certificates WHERE id = $3 AND dataplane_id = $2)",
    )
    .bind(fixture.team.id.as_uuid())
    .bind(dataplane_id)
    .bind(certificate_id)
    .fetch_one(&pool)
    .await
    .expect("retained history");
    assert_eq!(retained, (true, true, true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn team_create_committing_during_org_delete_forces_conflict_and_preserves_all_rows() {
    let Some(pool) = pool().await else { return };
    let (org_id, platform_ctx) = empty_org_fixture(&pool).await;
    let delete_pool = single_connection_pool().await;
    let delete_pid = backend_pid(&delete_pool).await;
    let mut blocker = pool.begin().await.expect("begin org race blocker");
    sqlx::query("SELECT id FROM organizations WHERE id = $1 FOR NO KEY UPDATE")
        .bind(org_id.as_uuid())
        .fetch_one(&mut *blocker)
        .await
        .expect("lock org parent row");
    let delete_task = tokio::spawn(async move {
        fp_core::services::orgs::delete_org(
            &delete_pool,
            &platform_ctx,
            org_id,
            RequestId::generate(),
        )
        .await
    });
    wait_for_lock_wait(&pool, delete_pid).await;
    let team = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        identity::create_team(&pool, org_id, &unique("org-delete-race-team"), ""),
    )
    .await
    .expect("team create is not blocked")
    .expect("concurrent team commits");
    let dataplane_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO dataplanes (id, team_id, org_id, name) VALUES ($1, $2, $3, $4)")
        .bind(dataplane_id)
        .bind(team.id.as_uuid())
        .bind(org_id.as_uuid())
        .bind(unique("org-delete-race-dataplane"))
        .execute(&pool)
        .await
        .expect("committed dataplane under concurrent team");
    let certificate_id = insert_certificate_history(&pool, team.id.as_uuid(), dataplane_id).await;
    blocker.commit().await.expect("release org race blocker");
    let delete = tokio::time::timeout(std::time::Duration::from_secs(5), delete_task)
        .await
        .expect("org delete completed")
        .expect("org delete task");
    let error = delete.expect_err("committed team must make org deletion lose");
    assert_eq!(error.code, ErrorCode::Conflict, "no Internal/FK: {error:?}");
    let retained: (bool, bool, bool, bool) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM organizations WHERE id = $1), \
                EXISTS(SELECT 1 FROM teams WHERE id = $2), \
                EXISTS(SELECT 1 FROM dataplanes WHERE id = $3), \
                EXISTS(SELECT 1 FROM proxy_certificates WHERE id = $4 AND dataplane_id = $3)",
    )
    .bind(org_id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(dataplane_id)
    .bind(certificate_id)
    .fetch_one(&pool)
    .await
    .expect("retained rows");
    assert_eq!(retained, (true, true, true, true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn team_delete_committing_first_makes_waiting_dataplane_create_conflict_without_orphans() {
    let Some(pool) = pool().await else { return };
    let fixture = empty_team_fixture(&pool).await;
    let create_pool = single_connection_pool().await;
    let create_pid = backend_pid(&create_pool).await;
    let dataplane_name = unique("team-delete-wins-dataplane");

    let mut delete = pool.begin().await.expect("begin authoritative team delete");
    sqlx::query("SELECT id FROM teams WHERE id = $1 FOR UPDATE")
        .bind(fixture.team.id.as_uuid())
        .fetch_one(&mut *delete)
        .await
        .expect("team delete holds authoritative row lock");

    let create_ctx = fixture.ctx.clone();
    let team = fixture.team;
    let create_name = dataplane_name.clone();
    let create_task = tokio::spawn(async move {
        fp_core::services::dataplanes::create_dataplane(
            &create_pool,
            &create_ctx,
            team,
            &create_name,
            "must lose to committed team deletion",
            RequestId::generate(),
        )
        .await
    });
    wait_for_lock_wait(&pool, create_pid).await;

    let deleted = sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(fixture.team.id.as_uuid())
        .execute(&mut *delete)
        .await
        .expect("delete locked team row");
    assert_eq!(deleted.rows_affected(), 1);
    delete.commit().await.expect("team deletion commits first");

    let create = tokio::time::timeout(std::time::Duration::from_secs(5), create_task)
        .await
        .expect("waiting dataplane create completed")
        .expect("dataplane create task");
    let error = create.expect_err("dataplane create must lose after team deletion commits");
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "deleted parent must be classified, never Internal/FK: {error:?}"
    );

    let orphan_counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM dataplanes WHERE team_id = $1 OR name = $2), \
                (SELECT count(*) FROM proxy_certificates WHERE team_id = $1)",
    )
    .bind(fixture.team.id.as_uuid())
    .bind(&dataplane_name)
    .fetch_one(&pool)
    .await
    .expect("query dataplane and credential evidence");
    assert_eq!(orphan_counts, (0, 0), "losing create left orphan evidence");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn org_delete_committing_first_makes_waiting_team_create_conflict_without_orphans() {
    let Some(pool) = pool().await else { return };
    let (org_id, platform_ctx) = empty_org_fixture(&pool).await;
    let create_pool = single_connection_pool().await;
    let create_pid = backend_pid(&create_pool).await;
    let team_name = unique("org-delete-wins-team");

    let mut delete = pool.begin().await.expect("begin authoritative org delete");
    sqlx::query("SELECT id FROM organizations WHERE id = $1 FOR UPDATE")
        .bind(org_id.as_uuid())
        .fetch_one(&mut *delete)
        .await
        .expect("org delete holds authoritative row lock");

    let create_name = team_name.clone();
    let create_ctx = match &platform_ctx {
        PrincipalCtx::User { user_id, .. } => PrincipalCtx::User {
            user_id: *user_id,
            platform_admin: true,
            org_selector_required: false,
            org: Some((org_id, OrgRole::Admin)),
            grants: GrantSet::default(),
        },
        PrincipalCtx::Agent { .. } => unreachable!("fixture creates a user principal"),
    };
    let create_task = tokio::spawn(async move {
        fp_core::services::teams::create_team(
            &create_pool,
            &create_ctx,
            &create_name,
            "must lose to committed org deletion",
            RequestId::generate(),
        )
        .await
    });
    wait_for_lock_wait(&pool, create_pid).await;

    let deleted = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org_id.as_uuid())
        .execute(&mut *delete)
        .await
        .expect("delete locked org row");
    assert_eq!(deleted.rows_affected(), 1);
    delete.commit().await.expect("org deletion commits first");

    let create = tokio::time::timeout(std::time::Duration::from_secs(5), create_task)
        .await
        .expect("waiting team create completed")
        .expect("team create task");
    let error = create.expect_err("team create must lose after org deletion commits");
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "deleted parent must be classified, never Internal/FK: {error:?}"
    );

    let orphan_evidence: (bool, i64, i64, i64) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM organizations WHERE id = $1), \
                (SELECT count(*) FROM teams WHERE org_id = $1 OR name = $2), \
                (SELECT count(*) FROM dataplanes WHERE org_id = $1), \
                (SELECT count(*) FROM proxy_certificates \
                 WHERE team_id IN (SELECT id FROM teams WHERE org_id = $1 OR name = $2) \
                    OR dataplane_id IN (SELECT id FROM dataplanes WHERE org_id = $1))",
    )
    .bind(org_id.as_uuid())
    .bind(&team_name)
    .fetch_one(&pool)
    .await
    .expect("query deleted org and team/dataplane/certificate orphan evidence");
    assert_eq!(
        orphan_evidence,
        (false, 0, 0, 0),
        "losing team create left orphan evidence"
    );
}
