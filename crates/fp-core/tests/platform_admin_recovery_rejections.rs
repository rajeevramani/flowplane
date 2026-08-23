//! Real-PostgreSQL fail-closed matrix for platform-admin recovery planning.

#![allow(clippy::expect_used)]

use fp_core::services::platform_admin_recovery::plan;
use fp_domain::{EntityStatus, ErrorCode};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;
use uuid::Uuid;

#[tokio::test]
async fn plan_rejects_unsafe_identity_and_membership_states_without_mutation() {
    let Some(database_url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL").ok() else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let admin_pool = fp_storage::connect(&database_url, 2)
        .await
        .expect("test database admin connection");
    let database_name = format!("fp_recovery_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE DATABASE \"{database_name}\""))
        .execute(&admin_pool)
        .await
        .expect("create isolated test database");
    let options = PgConnectOptions::from_str(&database_url)
        .expect("parse test database URL")
        .database(&database_name);
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_with(options)
        .await
        .expect("connect isolated test database");
    fp_storage::migrate(&pool).await.expect("migrations");
    let mut test_lock = pool.acquire().await.expect("test lock connection");
    sqlx::query("SELECT pg_advisory_lock(420001)")
        .execute(&mut *test_lock)
        .await
        .expect("identity-fixture lock");

    let platform_org = Uuid::now_v7();
    let tenant_org = Uuid::now_v7();
    let source_user = Uuid::now_v7();
    let replacement_user = Uuid::now_v7();
    let second_owner = Uuid::now_v7();
    let platform_membership = Uuid::now_v7();
    let tenant_membership = Uuid::now_v7();
    let suffix = &platform_org.simple().to_string()[..12];
    let platform_name = format!("recovery-platform-{suffix}");
    let tenant_name = format!("recovery-tenant-{suffix}");
    let source_subject = format!("recovery-source-{suffix}");
    let replacement_subject = format!("recovery-target-{suffix}");
    let missing_subject = format!("recovery-missing-{suffix}");

    sqlx::query(
        "INSERT INTO organizations (id, name, display_name, status) VALUES ($1, $2, $2, 'active')",
    )
    .bind(platform_org)
    .bind(&platform_name)
    .execute(&pool)
    .await
    .expect("insert platform organization");
    sqlx::query("INSERT INTO users (id, subject, name, status) VALUES ($1, $2, $2, 'active')")
        .bind(source_user)
        .bind(&source_subject)
        .execute(&pool)
        .await
        .expect("insert source user");

    let uninitialized = plan(&pool, &missing_subject, &[]).await.err();

    sqlx::query(
        "INSERT INTO instance_meta (key, value) VALUES ('platform_org_id', $1) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(platform_org.to_string())
    .execute(&pool)
    .await
    .expect("set platform marker");
    let missing_replacement = plan(&pool, &missing_subject, &[]).await.err();

    sqlx::query("INSERT INTO users (id, subject, name, status) VALUES ($1, $2, $2, 'suspended')")
        .bind(replacement_user)
        .bind(&replacement_subject)
        .execute(&pool)
        .await
        .expect("insert suspended replacement");
    let suspended_replacement = plan(&pool, &replacement_subject, &[]).await.err();

    sqlx::query("UPDATE users SET status = 'active' WHERE id = $1")
        .bind(replacement_user)
        .execute(&pool)
        .await
        .expect("activate replacement fixture");
    let zero_owners = plan(&pool, &replacement_subject, &[]).await.err();

    sqlx::query(
        "INSERT INTO org_memberships (id, org_id, user_id, role) VALUES ($1, $2, $3, 'owner')",
    )
    .bind(platform_membership)
    .bind(platform_org)
    .bind(source_user)
    .execute(&pool)
    .await
    .expect("insert sole owner");
    let same_user = plan(&pool, &source_subject, &[]).await.err();

    sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
        .bind(source_user)
        .execute(&pool)
        .await
        .expect("suspend source fixture");
    let suspended_source = plan(&pool, &replacement_subject, &[]).await.ok();
    sqlx::query("UPDATE users SET status = 'active' WHERE id = $1")
        .bind(source_user)
        .execute(&pool)
        .await
        .expect("restore source fixture");

    sqlx::query("INSERT INTO users (id, subject, name, status) VALUES ($1, $2, $2, 'active')")
        .bind(second_owner)
        .bind(format!("recovery-second-{suffix}"))
        .execute(&pool)
        .await
        .expect("insert second owner user");
    let second_membership = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO org_memberships (id, org_id, user_id, role) VALUES ($1, $2, $3, 'owner')",
    )
    .bind(second_membership)
    .bind(platform_org)
    .bind(second_owner)
    .execute(&pool)
    .await
    .expect("insert second owner membership");
    let multiple_owners = plan(&pool, &replacement_subject, &[]).await.err();
    sqlx::query("DELETE FROM org_memberships WHERE id = $1")
        .bind(second_membership)
        .execute(&pool)
        .await
        .expect("remove second owner membership");

    let replacement_platform_membership = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO org_memberships (id, org_id, user_id, role) VALUES ($1, $2, $3, 'viewer')",
    )
    .bind(replacement_platform_membership)
    .bind(platform_org)
    .bind(replacement_user)
    .execute(&pool)
    .await
    .expect("insert replacement platform membership");
    let existing_platform_membership = plan(&pool, &replacement_subject, &[]).await.err();
    sqlx::query("DELETE FROM org_memberships WHERE id = $1")
        .bind(replacement_platform_membership)
        .execute(&pool)
        .await
        .expect("remove replacement platform membership");

    let invalid_tenant = plan(
        &pool,
        &replacement_subject,
        &[format!("missing-tenant-{suffix}")],
    )
    .await
    .err();
    let platform_tenant = plan(&pool, &replacement_subject, &[platform_org.to_string()])
        .await
        .err();

    sqlx::query(
        "INSERT INTO organizations (id, name, display_name, status) VALUES ($1, $2, $2, 'active')",
    )
    .bind(tenant_org)
    .bind(&tenant_name)
    .execute(&pool)
    .await
    .expect("insert tenant organization");
    let ambiguous_org = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO organizations (id, name, display_name, status) VALUES ($1, $2, $2, 'active')",
    )
    .bind(ambiguous_org)
    .bind(tenant_org.to_string())
    .execute(&pool)
    .await
    .expect("insert UUID-shaped ambiguous tenant name");
    let ambiguous_tenant = plan(&pool, &replacement_subject, &[tenant_org.to_string()])
        .await
        .err();
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(ambiguous_org)
        .execute(&pool)
        .await
        .expect("remove ambiguous tenant fixture");
    sqlx::query(
        "INSERT INTO org_memberships (id, org_id, user_id, role) VALUES ($1, $2, $3, 'member')",
    )
    .bind(tenant_membership)
    .bind(tenant_org)
    .bind(source_user)
    .execute(&pool)
    .await
    .expect("insert source tenant membership");
    let source_not_tenant_owner = plan(
        &pool,
        &replacement_subject,
        std::slice::from_ref(&tenant_name),
    )
    .await
    .err();
    sqlx::query("UPDATE org_memberships SET role = 'owner' WHERE id = $1")
        .bind(tenant_membership)
        .execute(&pool)
        .await
        .expect("promote source fixture to tenant owner");

    let mut guarded_tx = pool.begin().await.expect("begin guarded mismatch check");
    let guarded_mismatch = fp_storage::repos::identity::transfer_recovery_membership_in_tx(
        &mut guarded_tx,
        platform_membership,
        fp_domain::OrgId::from(platform_org),
        fp_domain::UserId::from(replacement_user),
        fp_domain::UserId::from(source_user),
    )
    .await;
    guarded_tx
        .rollback()
        .await
        .expect("rollback guarded mismatch check");
    assert!(
        guarded_mismatch.is_err(),
        "guarded row mismatch must conflict"
    );

    let grant_team = Uuid::now_v7();
    let source_grant = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO teams (id, org_id, name, display_name, status) \
         VALUES ($1, $2, $3, $3, 'active')",
    )
    .bind(grant_team)
    .bind(tenant_org)
    .bind(format!("recovery-grant-team-{suffix}"))
    .execute(&pool)
    .await
    .expect("insert grant team fixture");
    sqlx::query(
        "INSERT INTO user_grants (id, user_id, org_id, team_id, resource, action, created_by) \
         VALUES ($1, $2, $3, $4, 'clusters', 'read', $2)",
    )
    .bind(source_grant)
    .bind(source_user)
    .bind(tenant_org)
    .bind(grant_team)
    .execute(&pool)
    .await
    .expect("insert source grant fixture");
    let selected_tenant_with_source_grant = plan(
        &pool,
        &replacement_subject,
        std::slice::from_ref(&tenant_name),
    )
    .await
    .err();
    sqlx::query("DELETE FROM user_grants WHERE id = $1")
        .bind(source_grant)
        .execute(&pool)
        .await
        .expect("remove source grant fixture");
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(grant_team)
        .execute(&pool)
        .await
        .expect("remove grant team fixture");

    let replacement_tenant_membership = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO org_memberships (id, org_id, user_id, role) VALUES ($1, $2, $3, 'viewer')",
    )
    .bind(replacement_tenant_membership)
    .bind(tenant_org)
    .bind(replacement_user)
    .execute(&pool)
    .await
    .expect("insert replacement tenant membership");
    let existing_tenant_membership = plan(
        &pool,
        &replacement_subject,
        std::slice::from_ref(&tenant_name),
    )
    .await
    .err();
    sqlx::query("DELETE FROM org_memberships WHERE id = $1")
        .bind(replacement_tenant_membership)
        .execute(&pool)
        .await
        .expect("remove replacement tenant membership");

    let duplicate_tenant = plan(
        &pool,
        &replacement_subject,
        &[tenant_name.clone(), tenant_org.to_string()],
    )
    .await
    .err();
    let final_membership_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM org_memberships WHERE org_id = ANY($1)")
            .bind(vec![platform_org, tenant_org])
            .fetch_one(&pool)
            .await
            .expect("count unchanged memberships");

    drop(test_lock);
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE \"{database_name}\" WITH (FORCE)"))
        .execute(&admin_pool)
        .await
        .expect("drop isolated test database");

    let cases = [
        ("uninitialized", uninitialized),
        ("missing replacement", missing_replacement),
        ("suspended replacement", suspended_replacement),
        ("zero owners", zero_owners),
        ("same replacement and source", same_user),
        ("multiple owners", multiple_owners),
        ("existing platform membership", existing_platform_membership),
        ("invalid tenant", invalid_tenant),
        ("platform tenant selector", platform_tenant),
        ("ambiguous tenant selector", ambiguous_tenant),
        ("source not tenant owner", source_not_tenant_owner),
        (
            "selected tenant has source grants",
            selected_tenant_with_source_grant,
        ),
        ("existing tenant membership", existing_tenant_membership),
        ("duplicate tenant", duplicate_tenant),
    ];
    for (name, error) in cases {
        assert!(error.is_some(), "{name} must fail closed");
        let error = error.expect("checked fail-closed error");
        assert_eq!(error.code, ErrorCode::Conflict, "{name}");
        assert!(!error.message.contains(&source_subject), "{name}");
        assert!(!error.message.contains(&replacement_subject), "{name}");
        assert!(!error.message.contains(&missing_subject), "{name}");
    }
    let suspended_source = suspended_source.expect("suspended source remains recoverable");
    assert_eq!(suspended_source.source_user_status, EntityStatus::Suspended);
    assert_eq!(
        final_membership_count, 2,
        "plan must not mutate memberships"
    );
}
