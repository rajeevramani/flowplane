#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::route_generation;
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::api_lifecycle::{
    ApiDefinitionSpec, SpecFormat, SpecReviewDecision, SpecSourceKind, SpecVersionInput,
};
use fp_domain::authz::TeamRef;
use fp_domain::gateway::listener::{ListenerProtocol, ListenerSpec};
use fp_domain::{ErrorCode, OrgRole, RequestId};
use fp_storage::repos::{api_lifecycle, identity};
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
async fn route_plan_apply_replays_persisted_preview() {
    let Some(w) = world().await else { return };
    let spec_id = reviewed_spec(&w, &unique("learned-api")).await;
    let plan = route_generation::create_plan(
        &w.pool,
        &w.admin,
        w.team,
        route_generation::CreateRoutePlanInput {
            spec_version_id: spec_id,
            listener_port: 19190,
        },
        RequestId::generate(),
    )
    .await
    .expect("dry-run plan");

    assert!(plan.plan.conflicts.is_empty());
    let applied =
        route_generation::apply_plan(&w.pool, &w.admin, w.team, plan.id, RequestId::generate())
            .await
            .expect("apply");

    assert_eq!(applied.cluster.spec, plan.plan.cluster_spec);
    assert_eq!(applied.route_config.spec, plan.plan.route_config_spec);
    assert_eq!(applied.listener.spec, plan.plan.listener_spec);
    assert_eq!(applied.plan.status.as_str(), "applied");
}

#[tokio::test]
async fn route_plan_apply_fails_on_intervening_conflict() {
    let Some(w) = world().await else { return };
    let api_name = unique("learned-api");
    let spec_id = reviewed_spec(&w, &api_name).await;
    let plan = route_generation::create_plan(
        &w.pool,
        &w.admin,
        w.team,
        route_generation::CreateRoutePlanInput {
            spec_version_id: spec_id,
            listener_port: 19191,
        },
        RequestId::generate(),
    )
    .await
    .expect("dry-run plan");

    fp_core::services::gateway::create_listener(
        &w.pool,
        &w.admin,
        w.team,
        &plan.plan.listener_name,
        ListenerSpec {
            address: "0.0.0.0".into(),
            port: 19192,
            public_base_url: None,
            protocol: ListenerProtocol::Http,
            route_config: None,
            http_filters: Vec::new(),
            access_logs: Vec::new(),
            tls_context: None,
        },
        RequestId::generate(),
        false,
    )
    .await
    .expect("intervening listener");

    let err =
        route_generation::apply_plan(&w.pool, &w.admin, w.team, plan.id, RequestId::generate())
            .await
            .expect_err("apply conflict");

    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(
        fp_core::services::clusters::get_cluster(
            &w.pool,
            &w.admin,
            w.team,
            &plan.plan.cluster_name,
            RequestId::generate(),
        )
        .await
        .expect_err("cluster cleaned up")
        .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn route_plan_create_rejects_unreviewed_spec() {
    let Some(w) = world().await else { return };
    let spec_id = learned_spec(&w, &unique("learned-api"), None).await;

    let err = route_generation::create_plan(
        &w.pool,
        &w.admin,
        w.team,
        route_generation::CreateRoutePlanInput {
            spec_version_id: spec_id,
            listener_port: 19193,
        },
        RequestId::generate(),
    )
    .await
    .expect_err("unreviewed spec cannot create a route plan");

    assert_eq!(err.code, ErrorCode::Conflict);
}

#[tokio::test]
async fn route_plan_create_rejects_rejected_spec() {
    let Some(w) = world().await else { return };
    let spec_id = learned_spec(
        &w,
        &unique("learned-api"),
        Some(SpecReviewDecision::Rejected),
    )
    .await;

    let err = route_generation::create_plan(
        &w.pool,
        &w.admin,
        w.team,
        route_generation::CreateRoutePlanInput {
            spec_version_id: spec_id,
            listener_port: 19194,
        },
        RequestId::generate(),
    )
    .await
    .expect_err("rejected spec cannot create a route plan");

    assert_eq!(err.code, ErrorCode::Conflict);
}

#[tokio::test]
async fn route_plan_apply_rechecks_review_state() {
    let Some(w) = world().await else { return };
    let api_name = unique("learned-api");
    let spec_id = reviewed_spec(&w, &api_name).await;
    let plan = route_generation::create_plan(
        &w.pool,
        &w.admin,
        w.team,
        route_generation::CreateRoutePlanInput {
            spec_version_id: spec_id,
            listener_port: 19195,
        },
        RequestId::generate(),
    )
    .await
    .expect("dry-run plan");

    append_decision(&w, spec_id, SpecReviewDecision::Rejected).await;

    let err =
        route_generation::apply_plan(&w.pool, &w.admin, w.team, plan.id, RequestId::generate())
            .await
            .expect_err("rejected spec cannot apply");

    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(
        fp_core::services::clusters::get_cluster(
            &w.pool,
            &w.admin,
            w.team,
            &plan.plan.cluster_name,
            RequestId::generate(),
        )
        .await
        .expect_err("no gateway mutation")
        .code,
        ErrorCode::NotFound
    );
}

async fn reviewed_spec(w: &World, api_name: &str) -> fp_domain::SpecVersionId {
    learned_spec(w, api_name, Some(SpecReviewDecision::Reviewed)).await
}

async fn learned_spec(
    w: &World,
    api_name: &str,
    decision: Option<SpecReviewDecision>,
) -> fp_domain::SpecVersionId {
    let mut tx = w.pool.begin().await.expect("tx");
    let api = api_lifecycle::create_api_definition(
        &mut tx,
        w.team,
        api_name,
        &ApiDefinitionSpec {
            display_name: api_name.into(),
            description: String::new(),
        },
    )
    .await
    .expect("api");
    let spec = api_lifecycle::create_spec_version(
        &mut tx,
        w.team,
        api.id,
        &SpecVersionInput {
            source_kind: SpecSourceKind::Learned,
            format: SpecFormat::OpenApi3,
            spec: serde_json::json!({
                "openapi": "3.1.0",
                "info": {"title": api_name, "version": "1.0.0"},
                "x-flowplane-learning-source": {
                    "observed_host": "api.example.test",
                    "forwarded_upstream_host": "upstream.example.test",
                    "forwarded_upstream_port": 443,
                    "forwarded_upstream_tls": true
                },
                "paths": {
                    "/v1/items/{id}": {
                        "get": {"operationId": "getItem", "responses": {"200": {"description": "ok"}}}
                    }
                }
            }),
        },
    )
    .await
    .expect("spec");
    if let Some(decision) = decision {
        api_lifecycle::append_spec_review_event(
            &mut tx,
            w.team,
            api_lifecycle::SpecReviewEventInsert {
                api_id: api.id,
                spec_version_id: spec.id,
                decision,
                actor_type: "user",
                actor_id: None,
                reason: "test",
                metadata: serde_json::json!({}),
            },
        )
        .await
        .expect("review");
    }
    tx.commit().await.expect("commit");
    spec.id
}

async fn append_decision(
    w: &World,
    spec_id: fp_domain::SpecVersionId,
    decision: SpecReviewDecision,
) {
    let mut tx = w.pool.begin().await.expect("tx");
    let spec = api_lifecycle::get_spec_version_by_id(&mut tx, w.team.id, spec_id)
        .await
        .expect("spec");
    api_lifecycle::append_spec_review_event(
        &mut tx,
        w.team,
        api_lifecycle::SpecReviewEventInsert {
            api_id: spec.api_definition_id,
            spec_version_id: spec.id,
            decision,
            actor_type: "user",
            actor_id: None,
            reason: "test",
            metadata: serde_json::json!({}),
        },
    )
    .await
    .expect("decision");
    tx.commit().await.expect("commit");
}
