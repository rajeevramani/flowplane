//! Service-level coverage for publishing IMPORTED spec versions (fpv2-dn7.1).
//!
//! These tests assert the publish gate's source-kind contract directly at the
//! `fp-core::services::api_lifecycle` boundary:
//!   * an imported spec is inert after `create_api` (no published pointer) and becomes
//!     servable only through the explicit `publish_spec_version` gate (constitution inv 16);
//!   * publishing an imported spec is idempotent over its generated tools;
//!   * `Manual` stays excluded by the allow-list (fails closed);
//!   * `reject_spec_version` stays learned-only (imported cannot be rejected).
//!
//! DB-backed: skip when `FLOWPLANE_TEST_DATABASE_URL` is unset. Parallel-safe: unique
//! org/team/api names per test, no global row-count assumptions (constitution inv 18).

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_core::services::api_lifecycle::{self as api_svc, CreateApiInput};
use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::api_lifecycle::{ApiDefinitionSpec, SpecFormat, SpecSourceKind, SpecVersionInput};
use fp_domain::authz::TeamRef;
use fp_domain::{ErrorCode, OrgRole, RequestId};
use fp_storage::repos::{api_lifecycle as storage_api_lifecycle, identity};
use serde_json::json;
use sqlx::PgPool;
use std::collections::BTreeSet;

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

fn imported_openapi() -> serde_json::Value {
    json!({
        "openapi": "3.0.3",
        "info": { "title": "Imported", "version": "1" },
        "paths": {
            "/items/{id}": { "get": { "operationId": "get_item" } }
        }
    })
}

fn import_input(name: String) -> CreateApiInput {
    CreateApiInput {
        name,
        definition: ApiDefinitionSpec {
            display_name: "Imported API".into(),
            description: String::new(),
        },
        imported_spec: Some(imported_openapi()),
        route_binding_name: None,
        route_binding: None,
    }
}

async fn tool_name_set(
    pool: &PgPool,
    team: TeamRef,
    api_id: fp_domain::ApiDefinitionId,
) -> BTreeSet<String> {
    storage_api_lifecycle::list_api_tools(pool, team.id, api_id)
        .await
        .expect("list tools")
        .into_iter()
        .map(|t| t.name)
        .collect()
}

#[tokio::test]
async fn imported_spec_is_inert_until_published_then_serves_tools() {
    let Some(w) = world().await else { return };
    let status = api_svc::create_api(
        &w.pool,
        &w.admin,
        w.team,
        import_input(unique("imported-api")),
        RequestId::generate(),
    )
    .await
    .expect("create imported api");

    // Inert by default: tools generated, but no published pointer (constitution inv 16).
    assert_eq!(status.tool_count, 1, "import generates the tool");
    assert!(
        status.api.published_spec_version_id.is_none(),
        "import must NOT auto-publish"
    );
    let spec = status.latest_spec.expect("imported spec version");
    assert_eq!(spec.source_kind, SpecSourceKind::Imported);
    assert_eq!(spec.version, 1, "imported APIs are single-version (v1)");

    let tools_before = tool_name_set(&w.pool, w.team, status.api.id).await;
    assert_eq!(tools_before.len(), 1);

    // Explicit publish gate: imported spec has no review events, so it publishes cleanly.
    let published = api_svc::publish_spec_version(
        &w.pool,
        &w.admin,
        w.team,
        api_svc::SpecReviewInput {
            api: status.api.name.clone(),
            version: spec.version,
            reason: "publish imported".into(),
        },
        RequestId::generate(),
    )
    .await
    .expect("imported spec publishes through the explicit gate");
    assert_eq!(published.spec.id, spec.id);
    assert_eq!(published.tool_count, 1);

    let after = api_svc::api_status(
        &w.pool,
        &w.admin,
        w.team,
        &status.api.name,
        RequestId::generate(),
    )
    .await
    .expect("status after publish");
    assert_eq!(
        after.api.published_spec_version_id,
        Some(spec.id),
        "publish flips the pointer to the imported spec"
    );

    // Regeneration is idempotent: delete-and-recreate yields the same tool rows.
    let tools_after = tool_name_set(&w.pool, w.team, status.api.id).await;
    assert_eq!(
        tools_before, tools_after,
        "publish must not change generated tool names/count"
    );
}

#[tokio::test]
async fn manual_spec_version_is_not_publishable() {
    let Some(w) = world().await else { return };
    // Create an API with no imported spec, then seed a Manual spec version directly via
    // storage (no product producer exists for Manual). The publish allow-list must reject it.
    let status = api_svc::create_api(
        &w.pool,
        &w.admin,
        w.team,
        CreateApiInput {
            name: unique("manual-api"),
            definition: ApiDefinitionSpec {
                display_name: "Manual API".into(),
                description: String::new(),
            },
            imported_spec: None,
            route_binding_name: None,
            route_binding: None,
        },
        RequestId::generate(),
    )
    .await
    .expect("create api");

    let mut tx = w.pool.begin().await.expect("manual tx");
    let manual = storage_api_lifecycle::create_spec_version(
        &mut tx,
        w.team,
        status.api.id,
        &SpecVersionInput {
            source_kind: SpecSourceKind::Manual,
            format: SpecFormat::OpenApi3,
            spec: imported_openapi(),
        },
    )
    .await
    .expect("seed manual spec");
    tx.commit().await.expect("manual commit");

    let err = api_svc::publish_spec_version(
        &w.pool,
        &w.admin,
        w.team,
        api_svc::SpecReviewInput {
            api: status.api.name.clone(),
            version: manual.version,
            reason: "should fail".into(),
        },
        RequestId::generate(),
    )
    .await
    .expect_err("manual spec versions are not publishable");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert!(
        err.message.contains("only learned or imported"),
        "unexpected message: {}",
        err.message
    );
}

#[tokio::test]
async fn imported_spec_cannot_be_rejected() {
    let Some(w) = world().await else { return };
    let status = api_svc::create_api(
        &w.pool,
        &w.admin,
        w.team,
        import_input(unique("reject-imported")),
        RequestId::generate(),
    )
    .await
    .expect("create imported api");
    let spec = status.latest_spec.expect("imported spec");

    let err = api_svc::reject_spec_version(
        &w.pool,
        &w.admin,
        w.team,
        api_svc::SpecReviewInput {
            api: status.api.name.clone(),
            version: spec.version,
            reason: "nope".into(),
        },
        RequestId::generate(),
    )
    .await
    .expect_err("imported specs have no review loop to reject");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert!(
        err.message
            .contains("only learned spec versions can be rejected"),
        "unexpected message: {}",
        err.message
    );
}
