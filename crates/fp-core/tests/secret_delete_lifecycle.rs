#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::secrets::{self as secret_svc, SecretWrite};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::{ErrorCode, OrgRole, RequestId, Secret, SecretSpec};
use fp_storage::repos::identity;
use sqlx::PgPool;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

struct World {
    pool: PgPool,
    team: TeamRef,
    admin: PrincipalCtx,
    member: PrincipalCtx,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    std::env::set_var(
        "FLOWPLANE_SECRET_ENCRYPTION_KEY",
        "12345678901234567890123456789012",
    );
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    let org = identity::create_org(&pool, &unique("delete-org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("delete-team"), "")
        .await
        .expect("team");
    let admin = identity::upsert_user_by_subject(
        &pool,
        &unique("admin-sub"),
        &format!("{}@test", unique("admin")),
        "Admin",
    )
    .await
    .expect("admin");
    identity::add_org_membership(&pool, admin, org.id, OrgRole::Admin)
        .await
        .expect("admin membership");
    let member = identity::upsert_user_by_subject(
        &pool,
        &unique("member-sub"),
        &format!("{}@test", unique("member")),
        "Member",
    )
    .await
    .expect("member");
    identity::add_org_membership(&pool, member, org.id, OrgRole::Member)
        .await
        .expect("member org membership");
    identity::add_team_membership(&pool, member, team.id)
        .await
        .expect("member team membership");
    let team_ref = TeamRef {
        id: team.id,
        org_id: org.id,
    };
    Some(World {
        pool,
        team: team_ref,
        admin: PrincipalCtx::User {
            user_id: admin,
            platform_admin: false,
            org_selector_required: false,
            org: Some((org.id, OrgRole::Admin)),
            grants: GrantSet::default(),
        },
        member: PrincipalCtx::User {
            user_id: member,
            platform_admin: false,
            org_selector_required: false,
            org: Some((org.id, OrgRole::Member)),
            grants: GrantSet::default(),
        },
    })
}

async fn create_secret(w: &World, name: &str, spec: SecretSpec) -> Secret {
    secret_svc::create_secret(
        &w.pool,
        &w.admin,
        w.team,
        SecretWrite {
            name,
            description: "dependency fixture",
            spec,
            expires_at: None,
        },
        RequestId::generate(),
    )
    .await
    .expect("create secret")
}

async fn success_evidence_count(w: &World, request_id: RequestId, name: &str) -> (i64, i64) {
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log \
         WHERE request_id = $1 AND action = 'secret.delete' AND outcome = 'success'",
    )
    .bind(request_id.as_uuid())
    .fetch_one(&w.pool)
    .await
    .expect("audit count");
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events \
         WHERE team_id = $1 AND event_type = 'secret.deleted' AND payload->>'name' = $2",
    )
    .bind(w.team.id.as_uuid())
    .bind(name)
    .fetch_one(&w.pool)
    .await
    .expect("event count");
    (audit, events)
}

#[tokio::test]
async fn listener_conflict_is_bounded_sorted_and_never_discloses_foreign_team() {
    let Some(w) = world().await else { return };
    let secret_name = unique("listener-secret");
    let secret = create_secret(
        &w,
        &secret_name,
        SecretSpec::TlsCertificate {
            certificate_chain: "certificate-canary".into(),
            private_key: "private-key-canary".into(),
            password: None,
            ocsp_staple: None,
        },
    )
    .await;
    let run = unique("run");
    for index in (0..12).rev() {
        let listener_id = Uuid::now_v7();
        let name = format!("listener-{index:02}-{run}");
        sqlx::query(
            "INSERT INTO listeners (id, team_id, org_id, name, spec) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(listener_id)
        .bind(w.team.id.as_uuid())
        .bind(w.team.org_id.as_uuid())
        .bind(&name)
        .bind(serde_json::json!({"address":"127.0.0.1","port":20000 + index}))
        .execute(&w.pool)
        .await
        .expect("listener");
        sqlx::query(
            "INSERT INTO listener_secret_refs (listener_id, team_id, secret_id, usage) \
             VALUES ($1, $2, $3, 'tls_certificate')",
        )
        .bind(listener_id)
        .bind(w.team.id.as_uuid())
        .bind(secret.id.as_uuid())
        .execute(&w.pool)
        .await
        .expect("listener ref");
    }

    let foreign_org = identity::create_org(&w.pool, &unique("foreign-org"), "")
        .await
        .expect("foreign org");
    let foreign_team = identity::create_team(&w.pool, foreign_org.id, &unique("foreign-team"), "")
        .await
        .expect("foreign team");
    let foreign_secret = Uuid::now_v7();
    let foreign_listener = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO secrets \
         (id, team_id, org_id, name, description, secret_type, configuration_encrypted, nonce, encryption_key_id) \
         VALUES ($1, $2, $3, $4, '', 'tls_certificate', $5, $6, 'test')",
    )
    .bind(foreign_secret)
    .bind(foreign_team.id.as_uuid())
    .bind(foreign_org.id.as_uuid())
    .bind(&secret_name)
    .bind(vec![1_u8])
    .bind(vec![0_u8; 12])
    .execute(&w.pool)
    .await
    .expect("foreign secret");
    sqlx::query(
        "INSERT INTO listeners (id, team_id, org_id, name, spec) VALUES ($1, $2, $3, 'foreign-leak', $4)",
    )
    .bind(foreign_listener)
    .bind(foreign_team.id.as_uuid())
    .bind(foreign_org.id.as_uuid())
    .bind(serde_json::json!({"address":"127.0.0.1","port":31000}))
    .execute(&w.pool)
    .await
    .expect("foreign listener");
    sqlx::query(
        "INSERT INTO listener_secret_refs (listener_id, team_id, secret_id, usage) \
         VALUES ($1, $2, $3, 'tls_certificate')",
    )
    .bind(foreign_listener)
    .bind(foreign_team.id.as_uuid())
    .bind(foreign_secret)
    .execute(&w.pool)
    .await
    .expect("foreign ref");

    let request_id = RequestId::generate();
    let err = secret_svc::delete_secret(
        &w.pool,
        &w.admin,
        w.team,
        &secret_name,
        secret.version,
        request_id,
    )
    .await
    .expect_err("referenced secret must conflict");
    assert_eq!(err.code, ErrorCode::Conflict);
    let details = err.details.expect("bounded dependency details");
    let listeners = &details["dependencies"]["listeners"];
    assert_eq!(listeners["total"], 12);
    assert_eq!(listeners["truncated"], true);
    let names = listeners["names"].as_array().expect("listener names");
    assert_eq!(names.len(), 10);
    let rendered: Vec<&str> = names.iter().filter_map(serde_json::Value::as_str).collect();
    assert!(rendered.windows(2).all(|pair| pair[0] < pair[1]));
    let text = details.to_string();
    assert!(!text.contains("foreign-leak"));
    assert!(!text.contains("private-key-canary"));
    assert!(
        fp_storage::repos::secrets::get_secret(&w.pool, w.team.id, &secret_name)
            .await
            .expect("get target")
            .is_some()
    );
    assert_eq!(
        success_evidence_count(&w, request_id, &secret_name).await,
        (0, 0)
    );
}

#[tokio::test]
async fn cluster_and_ai_provider_dependencies_conflict_without_values() {
    let Some(w) = world().await else { return };
    let ca_name = unique("cluster-ca");
    let ca = create_secret(
        &w,
        &ca_name,
        SecretSpec::CertificateValidationContext {
            trusted_ca: "trusted-ca-canary".into(),
            match_subject_alt_names: Vec::new(),
            crl: None,
            only_verify_leaf_cert_crl: false,
        },
    )
    .await;
    let cluster_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO clusters (id, team_id, org_id, name, spec) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(cluster_id)
    .bind(w.team.id.as_uuid())
    .bind(w.team.org_id.as_uuid())
    .bind("cluster-dependent")
    .bind(serde_json::json!({"endpoints":[]}))
    .execute(&w.pool)
    .await
    .expect("cluster");
    sqlx::query(
        "INSERT INTO cluster_secret_refs (cluster_id, team_id, secret_id, usage) \
         VALUES ($1, $2, $3, 'validation_context')",
    )
    .bind(cluster_id)
    .bind(w.team.id.as_uuid())
    .bind(ca.id.as_uuid())
    .execute(&w.pool)
    .await
    .expect("cluster ref");
    let err = secret_svc::delete_secret(
        &w.pool,
        &w.admin,
        w.team,
        &ca_name,
        ca.version,
        RequestId::generate(),
    )
    .await
    .expect_err("cluster dependency");
    assert_eq!(
        err.details.expect("details")["dependencies"]["clusters"]["names"][0],
        "cluster-dependent"
    );

    let generic_name = unique("ai-key");
    let generic = create_secret(
        &w,
        &generic_name,
        SecretSpec::GenericSecret {
            secret: "YWktY2FuYXJ5".into(),
        },
    )
    .await;
    sqlx::query(
        "INSERT INTO ai_providers \
         (id, team_id, org_id, name, kind, base_url, credential_secret_id) \
         VALUES ($1, $2, $3, 'provider-dependent', 'openai', 'https://example.test', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(w.team.id.as_uuid())
    .bind(w.team.org_id.as_uuid())
    .bind(generic.id.as_uuid())
    .execute(&w.pool)
    .await
    .expect("provider");
    let err = secret_svc::delete_secret(
        &w.pool,
        &w.admin,
        w.team,
        &generic_name,
        generic.version,
        RequestId::generate(),
    )
    .await
    .expect_err("AI dependency");
    let details = err.details.expect("details");
    assert_eq!(
        details["dependencies"]["ai_providers"]["names"][0],
        "provider-dependent"
    );
    assert!(!details.to_string().contains("YWktY2FuYXJ5"));
}

#[tokio::test]
async fn missing_stale_and_unauthorized_delete_fail_without_success_evidence() {
    let Some(w) = world().await else { return };
    let name = unique("fail-closed");
    let secret = create_secret(
        &w,
        &name,
        SecretSpec::GenericSecret {
            secret: "ZmFpbC1jbG9zZWQ=".into(),
        },
    )
    .await;

    for (ctx, target, revision, expected) in [
        (
            &w.member,
            name.as_str(),
            secret.version,
            ErrorCode::Forbidden,
        ),
        (
            &w.admin,
            name.as_str(),
            secret.version + 1,
            ErrorCode::RevisionMismatch,
        ),
        (&w.admin, "missing-secret", 1, ErrorCode::NotFound),
    ] {
        let request_id = RequestId::generate();
        let err = secret_svc::delete_secret(&w.pool, ctx, w.team, target, revision, request_id)
            .await
            .expect_err("delete must fail closed");
        assert_eq!(err.code, expected);
        assert_eq!(success_evidence_count(&w, request_id, target).await, (0, 0));
    }
    assert!(
        fp_storage::repos::secrets::get_secret(&w.pool, w.team.id, &name)
            .await
            .expect("get target")
            .is_some()
    );
}
