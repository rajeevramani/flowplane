#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::api_lifecycle::{
    ApiDefinitionSpec, ApiRouteBindingSpec, ApiToolSpec, HttpMethod, RetentionPolicySpec,
    SpecFormat, SpecSourceKind, SpecVersionInput,
};
use fp_domain::authz::TeamRef;
use fp_domain::{ErrorCode, ListenerId, RouteConfigId};
use fp_storage::repos::{api_lifecycle, identity};
use sqlx::{PgPool, Row};

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

struct World {
    pool: PgPool,
    team_a: TeamRef,
    team_b: TeamRef,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let org_a = identity::create_org(&pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let team_a = identity::create_team(&pool, org_a.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&pool, org_b.id, &unique("team-b"), "")
        .await
        .expect("team b");

    Some(World {
        pool,
        team_a: TeamRef {
            id: team_a.id,
            org_id: org_a.id,
        },
        team_b: TeamRef {
            id: team_b.id,
            org_id: org_b.id,
        },
    })
}

async fn insert_route_config(pool: &PgPool, team: TeamRef, name: &str) -> RouteConfigId {
    let id = RouteConfigId::generate();
    sqlx::query(
        "INSERT INTO route_configs (id, team_id, org_id, name, spec) \
         VALUES ($1, $2, $3, $4, '{}'::jsonb)",
    )
    .bind(id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .execute(pool)
    .await
    .expect("route config");
    id
}

async fn insert_listener(pool: &PgPool, team: TeamRef, name: &str) -> ListenerId {
    let id = ListenerId::generate();
    sqlx::query(
        "INSERT INTO listeners (id, team_id, org_id, name, spec) \
         VALUES ($1, $2, $3, $4, '{\"address\":\"0.0.0.0\",\"port\":18080}'::jsonb)",
    )
    .bind(id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .execute(pool)
    .await
    .expect("listener");
    id
}

fn api_spec(display: &str) -> ApiDefinitionSpec {
    ApiDefinitionSpec {
        display_name: display.into(),
        description: "test api".into(),
    }
}

fn openapi(title: &str) -> SpecVersionInput {
    SpecVersionInput {
        source_kind: SpecSourceKind::Imported,
        format: SpecFormat::OpenApi3,
        spec: serde_json::json!({
            "openapi": "3.0.3",
            "info": { "title": title, "version": "1.0.0" },
            "paths": {}
        }),
    }
}

#[tokio::test]
async fn api_definitions_are_named_per_team_not_globally() {
    let Some(w) = world().await else { return };
    let api_name = unique("payments");

    let mut tx = w.pool.begin().await.expect("tx a");
    let api_a = api_lifecycle::create_api_definition(&mut tx, w.team_a, &api_name, &api_spec("A"))
        .await
        .expect("api a");
    tx.commit().await.expect("commit a");

    let mut tx = w.pool.begin().await.expect("tx b");
    let api_b = api_lifecycle::create_api_definition(&mut tx, w.team_b, &api_name, &api_spec("B"))
        .await
        .expect("same name in another team is allowed");
    tx.commit().await.expect("commit b");
    assert_ne!(api_a.id, api_b.id);

    let mut tx = w.pool.begin().await.expect("tx dup");
    let dup = api_lifecycle::create_api_definition(&mut tx, w.team_a, &api_name, &api_spec("dup"))
        .await
        .expect_err("same-team duplicate rejected");
    assert_eq!(dup.code, ErrorCode::Conflict);
    tx.rollback().await.expect("rollback dup");

    let (items, total) = api_lifecycle::list_api_definitions(&w.pool, w.team_a.id, 50, 0)
        .await
        .expect("list");
    assert_eq!(total, 1);
    assert_eq!(items, vec![api_a]);
}

#[tokio::test]
async fn route_bindings_reject_cross_team_gateway_references() {
    let Some(w) = world().await else { return };
    let api_name = unique("orders");
    let route_b = insert_route_config(&w.pool, w.team_b, &unique("rc-b")).await;
    let listener_b = insert_listener(&w.pool, w.team_b, &unique("listener-b")).await;

    let mut tx = w.pool.begin().await.expect("tx");
    let api = api_lifecycle::create_api_definition(&mut tx, w.team_a, &api_name, &api_spec("A"))
        .await
        .expect("api");
    let err = api_lifecycle::create_route_binding(
        &mut tx,
        w.team_a,
        api.id,
        &unique("binding"),
        &ApiRouteBindingSpec {
            route_config_id: route_b,
            listener_id: Some(listener_b),
            virtual_host: Some("api".into()),
            route: Some("list".into()),
        },
    )
    .await
    .expect_err("cross-team route config rejected before insert");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    tx.rollback().await.expect("rollback");
}

#[tokio::test]
async fn spec_versions_are_append_only_and_tools_reference_same_api_spec() {
    let Some(w) = world().await else { return };
    let mut tx = w.pool.begin().await.expect("tx");
    let api = api_lifecycle::create_api_definition(
        &mut tx,
        w.team_a,
        &unique("catalog"),
        &api_spec("Catalog"),
    )
    .await
    .expect("api");
    let v1 = api_lifecycle::create_spec_version(&mut tx, w.team_a, api.id, &openapi("Catalog"))
        .await
        .expect("spec v1");
    let v2 = api_lifecycle::create_spec_version(
        &mut tx,
        w.team_a,
        api.id,
        &SpecVersionInput {
            spec: serde_json::json!({
                "openapi": "3.0.3",
                "info": { "title": "Catalog", "version": "1.1.0" },
                "paths": { "/items": {} }
            }),
            ..openapi("Catalog")
        },
    )
    .await
    .expect("spec v2");
    assert_eq!((v1.version, v2.version), (1, 2));
    assert_ne!(v1.spec_hash, v2.spec_hash);

    let tool = api_lifecycle::create_api_tool(
        &mut tx,
        w.team_a,
        api.id,
        v2.id,
        &unique("list-items"),
        &ApiToolSpec {
            operation_id: "listItems".into(),
            method: HttpMethod::Get,
            path: "/items".into(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            enabled: true,
        },
    )
    .await
    .expect("tool");
    tx.commit().await.expect("commit");

    let tools = api_lifecycle::list_api_tools(&w.pool, w.team_a.id, api.id)
        .await
        .expect("tools");
    assert_eq!(tools, vec![tool]);

    let err = sqlx::query("UPDATE spec_versions SET spec = '{}'::jsonb WHERE id = $1")
        .bind(v1.id.as_uuid())
        .execute(&w.pool)
        .await
        .expect_err("trigger rejects updates");
    let sqlstate = match err {
        sqlx::Error::Database(db) => db.code().map(|code| code.to_string()),
        other => panic!("expected database error, got {other:?}"),
    };
    assert_eq!(sqlstate.as_deref(), Some("45000"));
}

#[tokio::test]
async fn concurrent_spec_version_inserts_get_distinct_versions() {
    let Some(w) = world().await else { return };
    let mut tx = w.pool.begin().await.expect("tx");
    let api = api_lifecycle::create_api_definition(
        &mut tx,
        w.team_a,
        &unique("parallel"),
        &api_spec("Parallel"),
    )
    .await
    .expect("api");
    tx.commit().await.expect("commit api");

    let pool_a = w.pool.clone();
    let pool_b = w.pool.clone();
    let team = w.team_a;
    let api_id = api.id;
    let (a, b) = tokio::join!(
        async move {
            let mut tx = pool_a.begin().await.expect("tx a");
            let spec =
                api_lifecycle::create_spec_version(&mut tx, team, api_id, &openapi("Parallel A"))
                    .await
                    .expect("spec a");
            tx.commit().await.expect("commit a");
            spec.version
        },
        async move {
            let mut tx = pool_b.begin().await.expect("tx b");
            let spec =
                api_lifecycle::create_spec_version(&mut tx, team, api_id, &openapi("Parallel B"))
                    .await
                    .expect("spec b");
            tx.commit().await.expect("commit b");
            spec.version
        }
    );

    let mut versions = vec![a, b];
    versions.sort_unstable();
    assert_eq!(versions, vec![1, 2]);

    let count: i64 = sqlx::query("SELECT count(*) FROM spec_versions WHERE api_definition_id = $1")
        .bind(api.id.as_uuid())
        .fetch_one(&w.pool)
        .await
        .expect("count")
        .get(0);
    assert_eq!(count, 2);
}

#[tokio::test]
async fn retention_policy_can_be_scoped_to_api() {
    let Some(w) = world().await else { return };
    let mut tx = w.pool.begin().await.expect("tx");
    let api = api_lifecycle::create_api_definition(
        &mut tx,
        w.team_a,
        &unique("retained"),
        &api_spec("Retained"),
    )
    .await
    .expect("api");
    let policy = api_lifecycle::create_retention_policy(
        &mut tx,
        w.team_a,
        &unique("policy"),
        &RetentionPolicySpec {
            api_definition_id: Some(api.id),
            raw_observation_ttl_days: 30,
            max_spec_versions: 25,
        },
    )
    .await
    .expect("policy");
    tx.commit().await.expect("commit");

    assert_eq!(policy.api_definition_id, Some(api.id));
    assert_eq!(policy.raw_observation_ttl_days, 30);
    assert_eq!(policy.max_spec_versions, 25);
}
