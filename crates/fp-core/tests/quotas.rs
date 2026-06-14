//! Service-level quota coverage for resources beyond the original gateway vertical.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::api_lifecycle::{self as api_svc, CreateApiInput};
use fp_core::services::learning::{self as learning_svc, StartLearningSessionInput};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::api_lifecycle::{ApiDefinitionSpec, CaptureSessionSpec};
use fp_domain::authz::TeamRef;
use fp_domain::{ErrorCode, OrgRole, RequestId};
use fp_storage::repos::identity;
use sqlx::PgPool;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

struct World {
    pool: PgPool,
    team: TeamRef,
    admin: PrincipalCtx,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let user = identity::upsert_user_by_subject(&pool, &unique("sub"), "admin@example.test", "A")
        .await
        .expect("user");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("membership");
    Some(World {
        pool,
        team: TeamRef {
            id: team.id,
            org_id: org.id,
        },
        admin: PrincipalCtx::User {
            user_id: user,
            platform_admin: false,
            org_selector_required: false,
            org: Some((org.id, OrgRole::Admin)),
            grants: GrantSet::default(),
        },
    })
}

#[tokio::test]
async fn learning_session_create_path_enforces_default_quota() {
    let Some(w) = world().await else { return };
    let api = api_svc::create_api(
        &w.pool,
        &w.admin,
        w.team,
        CreateApiInput {
            name: unique("api"),
            definition: ApiDefinitionSpec {
                display_name: "Quota API".into(),
                description: String::new(),
            },
            imported_spec: None,
            route_binding_name: None,
            route_binding: None,
        },
        RequestId::generate(),
    )
    .await
    .expect("api");

    for i in 0..5 {
        learning_svc::start_session(
            &w.pool,
            &w.admin,
            w.team,
            StartLearningSessionInput {
                name: unique(&format!("learn-{i}")),
                api: None,
                spec: CaptureSessionSpec {
                    api_definition_id: Some(api.api.id),
                    route_config_id: None,
                    listener_id: None,
                    virtual_host: None,
                    route: None,
                    target_sample_count: 10,
                    max_duration_seconds: Some(60),
                    max_bytes: 4096,
                    max_distinct_paths: 10,
                },
            },
            RequestId::generate(),
        )
        .await
        .expect("within learning-session quota");
    }

    let err = learning_svc::start_session(
        &w.pool,
        &w.admin,
        w.team,
        StartLearningSessionInput {
            name: unique("learn-over"),
            api: None,
            spec: CaptureSessionSpec {
                api_definition_id: Some(api.api.id),
                route_config_id: None,
                listener_id: None,
                virtual_host: None,
                route: None,
                target_sample_count: 10,
                max_duration_seconds: Some(60),
                max_bytes: 4096,
                max_distinct_paths: 10,
            },
        },
        RequestId::generate(),
    )
    .await
    .expect_err("sixth learning session must trip quota");
    assert_eq!(err.code, ErrorCode::QuotaExceeded);
}
