//! Core contract tests for the redacted platform-admin recovery plan.

#![allow(clippy::expect_used)]

use fp_core::services::platform_admin_recovery::{
    apply, plan, PlanDigest, RecoveryPlan, TenantTransfer,
};
use fp_domain::{EntityStatus, ErrorCode, OrgId, OrgRole, UserId};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

fn id(raw: &str) -> Uuid {
    Uuid::parse_str(raw).expect("fixed UUID")
}

#[test]
fn canonical_v1_digest_matches_the_pinned_vector() {
    let plan = RecoveryPlan::new(
        OrgId::from(id("11111111-1111-4111-8111-111111111111")),
        UserId::from(id("22222222-2222-4222-8222-222222222222")),
        EntityStatus::Suspended,
        UserId::from(id("33333333-3333-4333-8333-333333333333")),
        EntityStatus::Active,
        id("44444444-4444-4444-8444-444444444444"),
        OrgRole::Owner,
        vec![TenantTransfer {
            org_id: OrgId::from(id("55555555-5555-4555-8555-555555555555")),
            org_name: "tenant-a".to_string(),
            membership_id: id("66666666-6666-4666-8666-666666666666"),
            role: OrgRole::Owner,
        }],
    )
    .expect("valid canonical plan");

    assert_eq!(
        plan.digest.as_str(),
        "sha256:11363c55dd994c6f33c9eb9489c83247c741d66b88a551237a504b3ed1d9a558"
    );
    assert_eq!(
        plan.canonical_json().expect("canonical JSON"),
        "{\"platform_membership_id\":\"44444444-4444-4444-8444-444444444444\",\"platform_org_id\":\"11111111-1111-4111-8111-111111111111\",\"platform_role\":\"owner\",\"replacement_user_id\":\"33333333-3333-4333-8333-333333333333\",\"replacement_user_status\":\"active\",\"source_user_id\":\"22222222-2222-4222-8222-222222222222\",\"source_user_status\":\"suspended\",\"tenant_transfers\":[{\"membership_id\":\"66666666-6666-4666-8666-666666666666\",\"org_id\":\"55555555-5555-4555-8555-555555555555\",\"org_name\":\"tenant-a\",\"role\":\"owner\"}]}"
    );
}

#[test]
fn expected_plan_digest_rejects_legacy_malformed_and_noncanonical_forms() {
    assert!(PlanDigest::parse(
        "sha256:11363c55dd994c6f33c9eb9489c83247c741d66b88a551237a504b3ed1d9a558"
    )
    .is_ok());
    for invalid in [
        "sha1:11363c55dd994c6f33c9eb9489c83247c741d66b88a551237a504b3ed1d9a558",
        "v0:11363c55dd994c6f33c9eb9489c83247c741d66b88a551237a504b3ed1d9a558",
        "sha256:11363c55",
        "sha256:11363c55dd994c6f33c9eb9489c83247c741d66b88a551237a504b3ed1d9a55G",
        "sha256:11363C55DD994C6F33C9EB9489C83247C741D66B88A551237A504B3ED1D9A558",
    ] {
        assert!(PlanDigest::parse(invalid).is_err());
    }
}

#[allow(clippy::too_many_arguments)]
fn recovery_plan(
    platform_org: u128,
    source_user: u128,
    source_status: EntityStatus,
    replacement_user: u128,
    membership: u128,
    tenants: Vec<TenantTransfer>,
) -> RecoveryPlan {
    RecoveryPlan::new(
        OrgId::from(Uuid::from_u128(platform_org)),
        UserId::from(Uuid::from_u128(source_user)),
        source_status,
        UserId::from(Uuid::from_u128(replacement_user)),
        EntityStatus::Active,
        Uuid::from_u128(membership),
        OrgRole::Owner,
        tenants,
    )
    .expect("valid recovery plan")
}

fn tenant(org: u128, name: &str, membership: u128) -> TenantTransfer {
    TenantTransfer {
        org_id: OrgId::from(Uuid::from_u128(org)),
        org_name: name.to_string(),
        membership_id: Uuid::from_u128(membership),
        role: OrgRole::Owner,
    }
}

#[test]
fn tenant_order_is_canonical_and_every_admissible_field_is_digest_sensitive() {
    let base_tenants = vec![tenant(20, "tenant-b", 200), tenant(10, "tenant-a", 100)];
    let base = recovery_plan(1, 2, EntityStatus::Active, 3, 4, base_tenants.clone());
    let reversed = recovery_plan(
        1,
        2,
        EntityStatus::Active,
        3,
        4,
        base_tenants.into_iter().rev().collect(),
    );
    assert_eq!(base.digest, reversed.digest);
    assert_eq!(
        base.tenant_transfers[0].org_id,
        OrgId::from(Uuid::from_u128(10))
    );

    let variants = [
        recovery_plan(
            9,
            2,
            EntityStatus::Active,
            3,
            4,
            base.tenant_transfers.clone(),
        ),
        recovery_plan(
            1,
            9,
            EntityStatus::Active,
            3,
            4,
            base.tenant_transfers.clone(),
        ),
        recovery_plan(
            1,
            2,
            EntityStatus::Suspended,
            3,
            4,
            base.tenant_transfers.clone(),
        ),
        recovery_plan(
            1,
            2,
            EntityStatus::Active,
            9,
            4,
            base.tenant_transfers.clone(),
        ),
        recovery_plan(
            1,
            2,
            EntityStatus::Active,
            3,
            9,
            base.tenant_transfers.clone(),
        ),
        recovery_plan(
            1,
            2,
            EntityStatus::Active,
            3,
            4,
            vec![tenant(11, "tenant-a", 100), tenant(20, "tenant-b", 200)],
        ),
        recovery_plan(
            1,
            2,
            EntityStatus::Active,
            3,
            4,
            vec![
                tenant(10, "tenant-renamed", 100),
                tenant(20, "tenant-b", 200),
            ],
        ),
        recovery_plan(
            1,
            2,
            EntityStatus::Active,
            3,
            4,
            vec![tenant(10, "tenant-a", 101), tenant(20, "tenant-b", 200)],
        ),
    ];
    for variant in variants {
        assert_ne!(base.digest, variant.digest);
    }

    assert!(RecoveryPlan::new(
        base.platform_org_id,
        base.source_user_id,
        base.source_user_status,
        base.replacement_user_id,
        EntityStatus::Suspended,
        base.platform_membership_id,
        OrgRole::Owner,
        vec![],
    )
    .is_err());
    assert!(RecoveryPlan::new(
        base.platform_org_id,
        base.source_user_id,
        base.source_user_status,
        base.replacement_user_id,
        EntityStatus::Active,
        base.platform_membership_id,
        OrgRole::Admin,
        vec![],
    )
    .is_err());
    let mut invalid_tenant = tenant(10, "tenant-a", 100);
    invalid_tenant.role = OrgRole::Admin;
    assert!(RecoveryPlan::new(
        base.platform_org_id,
        base.source_user_id,
        base.source_user_status,
        base.replacement_user_id,
        EntityStatus::Active,
        base.platform_membership_id,
        OrgRole::Owner,
        vec![invalid_tenant],
    )
    .is_err());
}

#[tokio::test]
async fn plan_reads_one_sole_owner_and_one_explicit_tenant_owner_from_real_postgres() {
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
        .expect("global identity-fixture lock");

    let platform_org = Uuid::now_v7();
    let tenant_org = Uuid::now_v7();
    let source_user = Uuid::now_v7();
    let replacement_user = Uuid::now_v7();
    let platform_membership = Uuid::now_v7();
    let tenant_membership = Uuid::now_v7();
    let suffix = &platform_org.simple().to_string()[..12];
    let platform_name = format!("recovery-platform-{suffix}");
    let tenant_name = format!("recovery-tenant-{suffix}");
    let source_subject = format!("recovery-source-{suffix}");
    let replacement_subject = format!("recovery-target-{suffix}");
    let old_marker: Option<String> =
        sqlx::query_scalar("SELECT value FROM instance_meta WHERE key = 'platform_org_id'")
            .fetch_optional(&pool)
            .await
            .expect("read old marker");

    for (org_id, name) in [(platform_org, &platform_name), (tenant_org, &tenant_name)] {
        sqlx::query(
            "INSERT INTO organizations (id, name, display_name, status) VALUES ($1, $2, $2, 'active')",
        )
        .bind(org_id)
        .bind(name)
        .execute(&pool)
        .await
        .expect("insert organization");
    }
    for (user_id, subject) in [
        (source_user, &source_subject),
        (replacement_user, &replacement_subject),
    ] {
        sqlx::query("INSERT INTO users (id, subject, name, status) VALUES ($1, $2, $2, 'active')")
            .bind(user_id)
            .bind(subject)
            .execute(&pool)
            .await
            .expect("insert user");
    }
    for (membership_id, org_id) in [
        (platform_membership, platform_org),
        (tenant_membership, tenant_org),
    ] {
        sqlx::query(
            "INSERT INTO org_memberships (id, org_id, user_id, role) VALUES ($1, $2, $3, 'owner')",
        )
        .bind(membership_id)
        .bind(org_id)
        .bind(source_user)
        .execute(&pool)
        .await
        .expect("insert owner membership");
    }
    sqlx::query(
        "INSERT INTO instance_meta (key, value) VALUES ('platform_org_id', $1) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(platform_org.to_string())
    .execute(&pool)
    .await
    .expect("set platform marker");

    let mut blocker = pool.begin().await.expect("begin blocking transaction");
    fp_storage::repos::bootstrap::lock_platform_identity_in_tx(&mut blocker)
        .await
        .expect("hold platform identity lock");
    let busy = tokio::time::timeout(
        Duration::from_millis(250),
        plan(
            &pool,
            &replacement_subject,
            std::slice::from_ref(&tenant_name),
        ),
    )
    .await;
    blocker
        .rollback()
        .await
        .expect("release platform identity lock");

    let result = plan(
        &pool,
        &replacement_subject,
        std::slice::from_ref(&tenant_name),
    )
    .await;
    let reviewed_digest = result.as_ref().expect("eligible plan").digest.clone();
    let mut apply_blocker = pool.begin().await.expect("begin apply blocker");
    assert!(
        fp_storage::repos::bootstrap::try_lock_platform_identity_in_tx(&mut apply_blocker)
            .await
            .expect("hold apply platform identity lock")
    );
    let busy_apply = tokio::time::timeout(
        Duration::from_millis(250),
        apply(
            &pool,
            &replacement_subject,
            std::slice::from_ref(&tenant_name),
            &reviewed_digest,
        ),
    )
    .await;
    apply_blocker
        .rollback()
        .await
        .expect("release apply platform identity lock");

    if let Some(marker) = old_marker {
        sqlx::query(
            "UPDATE instance_meta SET value = $1, updated_at = now() WHERE key = 'platform_org_id'",
        )
        .bind(marker)
        .execute(&pool)
        .await
        .expect("restore platform marker");
    } else {
        sqlx::query("DELETE FROM instance_meta WHERE key = 'platform_org_id'")
            .execute(&pool)
            .await
            .expect("remove platform marker");
    }
    sqlx::query("DELETE FROM organizations WHERE id = ANY($1)")
        .bind(vec![platform_org, tenant_org])
        .execute(&pool)
        .await
        .expect("delete fixture organizations");
    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(vec![source_user, replacement_user])
        .execute(&pool)
        .await
        .expect("delete fixture users");
    drop(test_lock);
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE \"{database_name}\" WITH (FORCE)"))
        .execute(&admin_pool)
        .await
        .expect("drop isolated test database");

    let plan = result.expect("eligible plan");
    let busy = busy
        .expect("busy plan must fail closed without waiting")
        .expect_err("busy plan must not return a stale plan");
    assert_eq!(busy.code, ErrorCode::Conflict);
    let busy_apply = busy_apply
        .expect("busy apply must fail closed without waiting")
        .expect_err("busy apply must not mutate");
    assert_eq!(busy_apply.code, ErrorCode::Conflict);
    assert_eq!(plan.platform_org_id, OrgId::from(platform_org));
    assert_eq!(plan.source_user_id, UserId::from(source_user));
    assert_eq!(plan.replacement_user_id, UserId::from(replacement_user));
    assert_eq!(plan.platform_membership_id, platform_membership);
    assert_eq!(plan.tenant_transfers.len(), 1);
    assert_eq!(plan.tenant_transfers[0].membership_id, tenant_membership);
    let output = serde_json::to_string(&plan).expect("serialize redacted plan");
    assert!(!output.contains(&source_subject));
    assert!(!output.contains(&replacement_subject));
}
