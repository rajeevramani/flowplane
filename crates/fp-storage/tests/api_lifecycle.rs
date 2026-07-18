#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::api_lifecycle::{
    ApiDefinitionSpec, ApiRouteBindingSpec, ApiToolSpec, CaptureSessionSpec, CaptureSessionStatus,
    HttpMethod, ObservationIngest, RetentionPolicySpec, SpecFormat, SpecSourceKind,
    SpecVersionInput,
};
use fp_domain::authz::TeamRef;
use fp_domain::{ErrorCode, ListenerId, RouteConfigId};
use fp_storage::repos::{api_lifecycle, identity};
use sqlx::types::chrono::Utc;
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
         VALUES ($1, $2, $3, $4, '{\"virtual_hosts\":[]}'::jsonb)",
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

fn observation(request_id: &str, path: &str) -> ObservationIngest {
    ObservationIngest {
        request_id: request_id.into(),
        method: "GET".into(),
        path: path.into(),
        response_status: Some(200),
        request_headers: serde_json::Map::new(),
        response_headers: serde_json::Map::new(),
        request_body: None,
        response_body: None,
        request_body_truncated: false,
        response_body_truncated: false,
        request_body_bytes: None,
        response_body_bytes: None,
        metadata_seen: true,
        body_seen: false,
        observed_at: Utc::now(),
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
async fn route_bindings_allow_only_one_unscoped_binding_per_api_route_config() {
    let Some(w) = world().await else { return };
    let route = insert_route_config(&w.pool, w.team_a, &unique("rc")).await;
    let mut tx = w.pool.begin().await.expect("tx");
    let api =
        api_lifecycle::create_api_definition(&mut tx, w.team_a, &unique("api"), &api_spec("API"))
            .await
            .expect("api");
    api_lifecycle::create_route_binding(
        &mut tx,
        w.team_a,
        api.id,
        &unique("whole-rc"),
        &ApiRouteBindingSpec {
            route_config_id: route,
            listener_id: None,
            virtual_host: None,
            route: None,
        },
    )
    .await
    .expect("first unscoped binding");

    let err = api_lifecycle::create_route_binding(
        &mut tx,
        w.team_a,
        api.id,
        &unique("whole-rc-duplicate"),
        &ApiRouteBindingSpec {
            route_config_id: route,
            listener_id: None,
            virtual_host: None,
            route: None,
        },
    )
    .await
    .expect_err("duplicate unscoped binding");
    assert_eq!(err.code, ErrorCode::Conflict);

    api_lifecycle::create_route_binding(
        &mut tx,
        w.team_a,
        api.id,
        &unique("scoped"),
        &ApiRouteBindingSpec {
            route_config_id: route,
            listener_id: None,
            virtual_host: Some("default".into()),
            route: Some("all".into()),
        },
    )
    .await
    .expect("scoped binding remains allowed");
    tx.commit().await.expect("commit");
}

#[tokio::test]
async fn route_bindings_allow_only_one_vhost_binding_per_api_route_config() {
    let Some(w) = world().await else { return };
    let route = insert_route_config(&w.pool, w.team_a, &unique("rc")).await;
    let mut tx = w.pool.begin().await.expect("tx");
    let api =
        api_lifecycle::create_api_definition(&mut tx, w.team_a, &unique("api"), &api_spec("API"))
            .await
            .expect("api");
    api_lifecycle::create_route_binding(
        &mut tx,
        w.team_a,
        api.id,
        &unique("vhost"),
        &ApiRouteBindingSpec {
            route_config_id: route,
            listener_id: None,
            virtual_host: Some("default".into()),
            route: None,
        },
    )
    .await
    .expect("first vhost binding");

    let err = api_lifecycle::create_route_binding(
        &mut tx,
        w.team_a,
        api.id,
        &unique("vhost-duplicate"),
        &ApiRouteBindingSpec {
            route_config_id: route,
            listener_id: None,
            virtual_host: Some("default".into()),
            route: None,
        },
    )
    .await
    .expect_err("duplicate vhost binding");
    assert_eq!(err.code, ErrorCode::Conflict);

    let err = api_lifecycle::create_route_binding(
        &mut tx,
        w.team_a,
        api.id,
        &unique("route-without-vhost"),
        &ApiRouteBindingSpec {
            route_config_id: route,
            listener_id: None,
            virtual_host: None,
            route: Some("all".into()),
        },
    )
    .await
    .expect_err("route selector requires virtual host");
    assert_eq!(err.code, ErrorCode::ValidationFailed);

    api_lifecycle::create_route_binding(
        &mut tx,
        w.team_a,
        api.id,
        &unique("route-scoped"),
        &ApiRouteBindingSpec {
            route_config_id: route,
            listener_id: None,
            virtual_host: Some("default".into()),
            route: Some("all".into()),
        },
    )
    .await
    .expect("route-scoped binding remains allowed");
    tx.commit().await.expect("commit");
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

#[tokio::test]
async fn retention_policy_allows_one_team_default_per_team() {
    let Some(w) = world().await else { return };
    let mut tx = w.pool.begin().await.expect("tx");
    let default = api_lifecycle::create_retention_policy(
        &mut tx,
        w.team_a,
        &unique("team-default"),
        &RetentionPolicySpec {
            api_definition_id: None,
            raw_observation_ttl_days: 14,
            max_spec_versions: 25,
        },
    )
    .await
    .expect("first default");
    assert_eq!(default.api_definition_id, None);

    let err = api_lifecycle::create_retention_policy(
        &mut tx,
        w.team_a,
        &unique("team-default-2"),
        &RetentionPolicySpec {
            api_definition_id: None,
            raw_observation_ttl_days: 7,
            max_spec_versions: 10,
        },
    )
    .await
    .expect_err("second default in same team");
    assert_eq!(err.code, ErrorCode::Conflict);
    tx.commit().await.expect("commit");

    let mut tx = w.pool.begin().await.expect("other team tx");
    api_lifecycle::create_retention_policy(
        &mut tx,
        w.team_b,
        &unique("team-default"),
        &RetentionPolicySpec {
            api_definition_id: None,
            raw_observation_ttl_days: 21,
            max_spec_versions: 30,
        },
    )
    .await
    .expect("other team default");
    tx.commit().await.expect("other team commit");
}

#[tokio::test]
async fn capture_sessions_are_bounded_and_transitioned_per_team() {
    let Some(w) = world().await else { return };
    let mut tx = w.pool.begin().await.expect("tx");
    let api = api_lifecycle::create_api_definition(
        &mut tx,
        w.team_a,
        &unique("learn-api"),
        &api_spec("Learn"),
    )
    .await
    .expect("api");
    let session = api_lifecycle::create_capture_session(
        &mut tx,
        w.team_a,
        &unique("learn"),
        &CaptureSessionSpec {
            api_definition_id: Some(api.id),
            route_config_id: None,
            listener_id: None,
            virtual_host: None,
            route: None,
            target_sample_count: 25,
            max_duration_seconds: Some(60),
            max_bytes: 4096,
            max_distinct_paths: 20,
        },
    )
    .await
    .expect("session");
    assert_eq!(session.status, CaptureSessionStatus::Capturing);
    assert_eq!(session.api_definition_id, Some(api.id));
    tx.commit().await.expect("commit");

    let (items, total) = api_lifecycle::list_capture_sessions(
        &w.pool,
        w.team_a.id,
        Some(CaptureSessionStatus::Capturing),
        50,
        0,
    )
    .await
    .expect("list");
    assert_eq!(total, 1);
    assert_eq!(items[0].id, session.id);

    let mut tx = w.pool.begin().await.expect("transition tx");
    let completed = api_lifecycle::transition_capture_session(
        &mut tx,
        w.team_a.id,
        &session.name,
        CaptureSessionStatus::Completed,
    )
    .await
    .expect("complete");
    assert_eq!(completed.status, CaptureSessionStatus::Completed);
    assert!(completed.completed_at.is_some());
    let err = api_lifecycle::transition_capture_session(
        &mut tx,
        w.team_a.id,
        &session.name,
        CaptureSessionStatus::Cancelled,
    )
    .await
    .expect_err("terminal session rejects second transition");
    assert_eq!(err.code, ErrorCode::Conflict);
    tx.rollback().await.expect("rollback");
}

#[tokio::test]
async fn capture_sessions_reject_cross_team_route_scope() {
    let Some(w) = world().await else { return };
    let route_b = insert_route_config(&w.pool, w.team_b, &unique("rc-b")).await;
    let listener_b = insert_listener(&w.pool, w.team_b, &unique("listener-b")).await;

    let mut tx = w.pool.begin().await.expect("tx");
    let err = api_lifecycle::create_capture_session(
        &mut tx,
        w.team_a,
        &unique("learn-route"),
        &CaptureSessionSpec {
            api_definition_id: None,
            route_config_id: Some(route_b),
            listener_id: Some(listener_b),
            virtual_host: Some("api".into()),
            route: Some("list".into()),
            target_sample_count: 10,
            max_duration_seconds: None,
            max_bytes: 1024,
            max_distinct_paths: 10,
        },
    )
    .await
    .expect_err("cross-team route scope rejected before insert");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    tx.rollback().await.expect("rollback");
}

#[tokio::test]
async fn capture_ingest_binding_validates_active_team_and_scope() {
    let Some(w) = world().await else { return };
    let route_a = insert_route_config(&w.pool, w.team_a, &unique("rc-a")).await;
    let route_b = insert_route_config(&w.pool, w.team_b, &unique("rc-b")).await;
    let listener_a = insert_listener(&w.pool, w.team_a, &unique("listener-a")).await;
    let mut tx = w.pool.begin().await.expect("tx");
    let api = api_lifecycle::create_api_definition(
        &mut tx,
        w.team_a,
        &unique("ingest-api"),
        &api_spec("Ingest"),
    )
    .await
    .expect("api");
    api_lifecycle::create_route_binding(
        &mut tx,
        w.team_a,
        api.id,
        &unique("binding"),
        &ApiRouteBindingSpec {
            route_config_id: route_a,
            listener_id: Some(listener_a),
            virtual_host: Some("default".into()),
            route: Some("all".into()),
        },
    )
    .await
    .expect("binding");
    let session = api_lifecycle::create_capture_session(
        &mut tx,
        w.team_a,
        &unique("ingest-session"),
        &CaptureSessionSpec {
            api_definition_id: Some(api.id),
            route_config_id: None,
            listener_id: None,
            virtual_host: None,
            route: None,
            target_sample_count: 25,
            max_duration_seconds: Some(60),
            max_bytes: 4096,
            max_distinct_paths: 20,
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit");

    let valid = api_lifecycle::validate_capture_ingest_binding(
        &w.pool,
        w.team_a.id,
        session.id,
        Some(api.id),
        route_a,
        Some(listener_a),
    )
    .await
    .expect("valid binding");
    assert_eq!(valid.id, session.id);

    let err = api_lifecycle::validate_capture_ingest_binding(
        &w.pool,
        w.team_a.id,
        session.id,
        Some(api.id),
        route_b,
        Some(listener_a),
    )
    .await
    .expect_err("wrong route rejected");
    assert_eq!(err.code, ErrorCode::NotFound);

    let err = api_lifecycle::validate_capture_ingest_binding(
        &w.pool,
        w.team_b.id,
        session.id,
        Some(api.id),
        route_b,
        None,
    )
    .await
    .expect_err("wrong team rejected");
    assert_eq!(err.code, ErrorCode::NotFound);

    let mut tx = w.pool.begin().await.expect("transition tx");
    api_lifecycle::transition_capture_session(
        &mut tx,
        w.team_a.id,
        &session.name,
        CaptureSessionStatus::Completed,
    )
    .await
    .expect("complete");
    tx.commit().await.expect("commit complete");
    let err = api_lifecycle::validate_capture_ingest_binding(
        &w.pool,
        w.team_a.id,
        session.id,
        Some(api.id),
        route_a,
        Some(listener_a),
    )
    .await
    .expect_err("terminal session rejected");
    assert_eq!(err.code, ErrorCode::Conflict);
}

#[tokio::test]
async fn raw_observation_ingest_redacts_and_counts_accepted_rows() {
    let Some(w) = world().await else { return };
    let route = insert_route_config(&w.pool, w.team_a, &unique("rc")).await;
    let listener = insert_listener(&w.pool, w.team_a, &unique("listener")).await;
    let mut tx = w.pool.begin().await.expect("tx");
    let session = api_lifecycle::create_capture_session(
        &mut tx,
        w.team_a,
        &unique("capture"),
        &CaptureSessionSpec {
            api_definition_id: None,
            route_config_id: Some(route),
            listener_id: Some(listener),
            virtual_host: Some("default".into()),
            route: Some("all".into()),
            target_sample_count: 10,
            max_duration_seconds: Some(60),
            max_bytes: 4096,
            max_distinct_paths: 10,
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit session");

    let mut input = observation("req-redact", "/orders");
    input
        .request_headers
        .insert("authorization".into(), serde_json::json!("Bearer secret"));
    input.request_headers.insert(
        "proxy-authorization".into(),
        serde_json::json!("Basic secret"),
    );
    input
        .request_headers
        .insert("x-api-key".into(), serde_json::json!("key-secret"));
    input
        .request_headers
        .insert("x-auth-token".into(), serde_json::json!("token-secret"));
    input
        .request_headers
        .insert("x-envoy-internal".into(), serde_json::json!("true"));
    input
        .request_headers
        .insert("accept".into(), serde_json::json!("application/json"));
    input.response_body = Some("{\"ok\":true}".into());
    input.body_seen = true;

    let mut tx = w.pool.begin().await.expect("ingest tx");
    let row = api_lifecycle::ingest_raw_observation(
        &mut tx,
        w.team_a,
        session.id,
        None,
        route,
        Some(listener),
        &input,
    )
    .await
    .expect("ingest");
    tx.commit().await.expect("commit ingest");

    assert_eq!(row.request_headers["authorization"], "[REDACTED]");
    assert_eq!(row.request_headers["proxy-authorization"], "[REDACTED]");
    assert_eq!(row.request_headers["x-api-key"], "[REDACTED]");
    assert_eq!(row.request_headers["x-auth-token"], "[REDACTED]");
    assert!(row.request_headers.get("x-envoy-internal").is_none());
    assert_eq!(row.request_headers["accept"], "application/json");

    let refreshed = api_lifecycle::get_capture_session(&w.pool, w.team_a.id, &session.name)
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.sample_count, 1);
    assert_eq!(refreshed.path_count, 1);
    assert_eq!(refreshed.byte_count, row.response_body_bytes);
    assert_eq!(refreshed.drop_count, 0);
}

#[tokio::test]
async fn raw_observation_body_merge_does_not_increment_sample_count() {
    let Some(w) = world().await else { return };
    let route = insert_route_config(&w.pool, w.team_a, &unique("rc")).await;
    let mut tx = w.pool.begin().await.expect("tx");
    let session = api_lifecycle::create_capture_session(
        &mut tx,
        w.team_a,
        &unique("merge"),
        &CaptureSessionSpec {
            api_definition_id: None,
            route_config_id: Some(route),
            listener_id: None,
            virtual_host: None,
            route: None,
            target_sample_count: 10,
            max_duration_seconds: Some(60),
            max_bytes: 4096,
            max_distinct_paths: 10,
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit session");

    let metadata = observation("req-merge", "/items");
    let mut tx = w.pool.begin().await.expect("metadata tx");
    api_lifecycle::ingest_raw_observation(
        &mut tx, w.team_a, session.id, None, route, None, &metadata,
    )
    .await
    .expect("metadata ingest");
    tx.commit().await.expect("commit metadata");

    let mut body = observation("req-merge", "/items");
    body.request_headers.clear();
    body.response_headers.clear();
    body.response_status = None;
    body.metadata_seen = false;
    body.body_seen = true;
    body.request_body = Some("hello".into());
    body.response_body = Some("world".into());
    let mut tx = w.pool.begin().await.expect("body tx");
    let merged = api_lifecycle::ingest_raw_observation(
        &mut tx, w.team_a, session.id, None, route, None, &body,
    )
    .await
    .expect("body ingest");
    tx.commit().await.expect("commit body");

    assert_eq!(merged.request_body.as_deref(), Some("hello"));
    assert_eq!(merged.response_body.as_deref(), Some("world"));
    let refreshed = api_lifecycle::get_capture_session(&w.pool, w.team_a.id, &session.name)
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.sample_count, 1);
    assert_eq!(refreshed.path_count, 1);
    assert_eq!(refreshed.byte_count, 10);
}

#[tokio::test]
async fn raw_observation_duplicate_merges_and_same_path_counts_are_incremental() {
    let Some(w) = world().await else { return };
    let route = insert_route_config(&w.pool, w.team_a, &unique("rc")).await;
    let mut tx = w.pool.begin().await.expect("tx");
    let session = api_lifecycle::create_capture_session(
        &mut tx,
        w.team_a,
        &unique("incremental"),
        &CaptureSessionSpec {
            api_definition_id: None,
            route_config_id: Some(route),
            listener_id: None,
            virtual_host: None,
            route: None,
            target_sample_count: 3,
            max_duration_seconds: Some(60),
            max_bytes: 4096,
            max_distinct_paths: 1,
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit session");

    let first_metadata = observation("req-incremental-a", "/same");
    let mut tx = w.pool.begin().await.expect("first metadata tx");
    api_lifecycle::ingest_raw_observation(
        &mut tx,
        w.team_a,
        session.id,
        None,
        route,
        None,
        &first_metadata,
    )
    .await
    .expect("first metadata ingest");
    tx.commit().await.expect("commit first metadata");

    let mut first_body = observation("req-incremental-a", "/same");
    first_body.metadata_seen = false;
    first_body.body_seen = true;
    first_body.request_body = Some("hello".into());
    first_body.response_body = Some("world".into());
    let mut tx = w.pool.begin().await.expect("first body tx");
    api_lifecycle::ingest_raw_observation(
        &mut tx,
        w.team_a,
        session.id,
        None,
        route,
        None,
        &first_body,
    )
    .await
    .expect("first body ingest");
    tx.commit().await.expect("commit first body");

    let second_metadata = observation("req-incremental-b", "/same");
    let mut tx = w.pool.begin().await.expect("second metadata tx");
    api_lifecycle::ingest_raw_observation(
        &mut tx,
        w.team_a,
        session.id,
        None,
        route,
        None,
        &second_metadata,
    )
    .await
    .expect("second metadata ingest");
    tx.commit().await.expect("commit second metadata");

    let refreshed = api_lifecycle::get_capture_session(&w.pool, w.team_a.id, &session.name)
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.sample_count, 2);
    assert_eq!(refreshed.path_count, 1);
    assert_eq!(refreshed.byte_count, 10);
    assert_eq!(refreshed.status, CaptureSessionStatus::Capturing);
}

#[tokio::test]
async fn raw_observation_quota_drop_does_not_insert_or_move_sample_count() {
    let Some(w) = world().await else { return };
    let route = insert_route_config(&w.pool, w.team_a, &unique("rc")).await;
    let mut tx = w.pool.begin().await.expect("tx");
    let session = api_lifecycle::create_capture_session(
        &mut tx,
        w.team_a,
        &unique("quota"),
        &CaptureSessionSpec {
            api_definition_id: None,
            route_config_id: Some(route),
            listener_id: None,
            virtual_host: None,
            route: None,
            target_sample_count: 10,
            max_duration_seconds: Some(60),
            max_bytes: 5,
            max_distinct_paths: 10,
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit session");

    let mut too_large = observation("req-large", "/large");
    too_large.body_seen = true;
    too_large.response_body = Some("too-large".into());
    let mut tx = w.pool.begin().await.expect("ingest tx");
    let err = api_lifecycle::ingest_raw_observation(
        &mut tx, w.team_a, session.id, None, route, None, &too_large,
    )
    .await
    .expect_err("quota drop");
    assert_eq!(err.code, ErrorCode::QuotaExceeded);
    tx.commit().await.expect("commit drop count");

    let refreshed = api_lifecycle::get_capture_session(&w.pool, w.team_a.id, &session.name)
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.sample_count, 0);
    assert_eq!(refreshed.byte_count, 0);
    assert_eq!(refreshed.drop_count, 1);
    let raw_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM raw_observations WHERE capture_session_id = $1")
            .bind(session.id.as_uuid())
            .fetch_one(&w.pool)
            .await
            .expect("raw count");
    assert_eq!(raw_count, 0);
}

#[tokio::test]
async fn raw_observation_truncated_body_quota_uses_reported_original_bytes() {
    let Some(w) = world().await else { return };
    let route = insert_route_config(&w.pool, w.team_a, &unique("rc")).await;
    let mut tx = w.pool.begin().await.expect("tx");
    let session = api_lifecycle::create_capture_session(
        &mut tx,
        w.team_a,
        &unique("truncated-quota"),
        &CaptureSessionSpec {
            api_definition_id: None,
            route_config_id: Some(route),
            listener_id: None,
            virtual_host: None,
            route: None,
            target_sample_count: 10,
            max_duration_seconds: Some(60),
            max_bytes: 5,
            max_distinct_paths: 10,
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit session");

    let mut truncated = observation("req-truncated", "/truncated");
    truncated.body_seen = true;
    truncated.request_body = Some("abc".into());
    truncated.request_body_truncated = true;
    truncated.request_body_bytes = Some(10);
    let mut tx = w.pool.begin().await.expect("ingest tx");
    let err = api_lifecycle::ingest_raw_observation(
        &mut tx, w.team_a, session.id, None, route, None, &truncated,
    )
    .await
    .expect_err("reported original size exceeds quota");
    assert_eq!(err.code, ErrorCode::QuotaExceeded);
    tx.commit().await.expect("commit drop count");

    let refreshed = api_lifecycle::get_capture_session(&w.pool, w.team_a.id, &session.name)
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.sample_count, 0);
    assert_eq!(refreshed.byte_count, 0);
    assert_eq!(refreshed.drop_count, 1);
}

#[tokio::test]
async fn raw_observation_target_sample_count_completes_session() {
    let Some(w) = world().await else { return };
    let route = insert_route_config(&w.pool, w.team_a, &unique("rc")).await;
    let mut tx = w.pool.begin().await.expect("tx");
    let session = api_lifecycle::create_capture_session(
        &mut tx,
        w.team_a,
        &unique("complete"),
        &CaptureSessionSpec {
            api_definition_id: None,
            route_config_id: Some(route),
            listener_id: None,
            virtual_host: None,
            route: None,
            target_sample_count: 1,
            max_duration_seconds: Some(60),
            max_bytes: 4096,
            max_distinct_paths: 10,
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit session");

    let mut tx = w.pool.begin().await.expect("ingest tx");
    api_lifecycle::ingest_raw_observation(
        &mut tx,
        w.team_a,
        session.id,
        None,
        route,
        None,
        &observation("req-complete", "/done"),
    )
    .await
    .expect("ingest");
    tx.commit().await.expect("commit ingest");

    let refreshed = api_lifecycle::get_capture_session(&w.pool, w.team_a.id, &session.name)
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.status, CaptureSessionStatus::Completed);
    assert!(refreshed.completed_at.is_some());
    assert_eq!(refreshed.sample_count, 1);
}

#[tokio::test]
async fn raw_observation_body_can_merge_after_target_completion() {
    let Some(w) = world().await else { return };
    let route = insert_route_config(&w.pool, w.team_a, &unique("rc")).await;
    let mut tx = w.pool.begin().await.expect("tx");
    let session = api_lifecycle::create_capture_session(
        &mut tx,
        w.team_a,
        &unique("late-body"),
        &CaptureSessionSpec {
            api_definition_id: None,
            route_config_id: Some(route),
            listener_id: None,
            virtual_host: None,
            route: None,
            target_sample_count: 1,
            max_duration_seconds: Some(60),
            max_bytes: 4096,
            max_distinct_paths: 10,
        },
    )
    .await
    .expect("session");
    tx.commit().await.expect("commit session");

    let metadata = observation("req-late", "/late");
    let mut tx = w.pool.begin().await.expect("metadata tx");
    api_lifecycle::ingest_raw_observation(
        &mut tx, w.team_a, session.id, None, route, None, &metadata,
    )
    .await
    .expect("metadata ingest");
    tx.commit().await.expect("commit metadata");
    let completed = api_lifecycle::get_capture_session(&w.pool, w.team_a.id, &session.name)
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(completed.status, CaptureSessionStatus::Completed);

    let mut body = observation("req-late", "/late");
    body.metadata_seen = false;
    body.body_seen = true;
    body.response_status = None;
    body.response_body = Some("late-body".into());
    let mut tx = w.pool.begin().await.expect("body tx");
    let merged = api_lifecycle::ingest_raw_observation(
        &mut tx, w.team_a, session.id, None, route, None, &body,
    )
    .await
    .expect("late body merge");
    tx.commit().await.expect("commit body");

    assert!(merged.body_seen);
    assert_eq!(merged.response_body.as_deref(), Some("late-body"));
    let refreshed = api_lifecycle::get_capture_session(&w.pool, w.team_a.id, &session.name)
        .await
        .expect("get session")
        .expect("session");
    assert_eq!(refreshed.status, CaptureSessionStatus::Completed);
    assert_eq!(refreshed.sample_count, 1);
    assert_eq!(refreshed.byte_count, 9);
}

// -- ui-f4 S1: paginated spec-version metadata + batched latest review decisions --

async fn commit_spec_version(
    pool: &PgPool,
    team: TeamRef,
    api_id: fp_domain::ApiDefinitionId,
    title: &str,
) -> fp_domain::api_lifecycle::SpecVersion {
    let mut tx = pool.begin().await.expect("spec tx");
    let spec = api_lifecycle::create_spec_version(&mut tx, team, api_id, &openapi(title))
        .await
        .expect("spec version");
    tx.commit().await.expect("commit spec");
    spec
}

async fn commit_review_event(
    pool: &PgPool,
    team: TeamRef,
    api_id: fp_domain::ApiDefinitionId,
    spec_version_id: fp_domain::SpecVersionId,
    decision: fp_domain::api_lifecycle::SpecReviewDecision,
) {
    let mut tx = pool.begin().await.expect("event tx");
    api_lifecycle::append_spec_review_event(
        &mut tx,
        team,
        api_lifecycle::SpecReviewEventInsert {
            api_id,
            spec_version_id,
            decision,
            actor_type: "user",
            actor_id: None,
            reason: "",
            metadata: serde_json::json!({}),
        },
    )
    .await
    .expect("append event");
    tx.commit().await.expect("commit event");
}

#[tokio::test]
async fn spec_version_meta_list_pages_newest_first_with_batched_latest_decisions() {
    use fp_domain::api_lifecycle::SpecReviewDecision as D;
    let Some(w) = world().await else { return };
    let mut tx = w.pool.begin().await.expect("tx");
    let api =
        api_lifecycle::create_api_definition(&mut tx, w.team_a, &unique("api"), &api_spec("A"))
            .await
            .expect("api");
    tx.commit().await.expect("commit api");

    let v1 = commit_spec_version(&w.pool, w.team_a, api.id, "v1").await;
    let v2 = commit_spec_version(&w.pool, w.team_a, api.id, "v2").await;
    let v3 = commit_spec_version(&w.pool, w.team_a, api.id, "v3").await;
    // v1 has no events; v2 ends published; v3 ends rejected.
    commit_review_event(&w.pool, w.team_a, api.id, v2.id, D::Submitted).await;
    commit_review_event(&w.pool, w.team_a, api.id, v2.id, D::Published).await;
    commit_review_event(&w.pool, w.team_a, api.id, v3.id, D::Submitted).await;
    commit_review_event(&w.pool, w.team_a, api.id, v3.id, D::Rejected).await;

    let (page, total) = api_lifecycle::list_spec_versions_meta(&w.pool, w.team_a.id, api.id, 2, 0)
        .await
        .expect("page 1");
    assert_eq!(total, 3);
    assert_eq!(
        page.iter().map(|m| m.version).collect::<Vec<_>>(),
        vec![v3.version, v2.version]
    );
    assert_eq!(page[0].id, v3.id);
    assert_eq!(page[0].spec_hash, v3.spec_hash);
    assert_eq!(page[0].source_kind, SpecSourceKind::Imported);

    let ids: Vec<_> = page.iter().map(|m| m.id).collect();
    let decisions = api_lifecycle::latest_spec_review_decisions(&w.pool, w.team_a.id, &ids)
        .await
        .expect("decisions");
    let of = |id| decisions.iter().find(|(d, _)| *d == id).map(|(_, d)| *d);
    assert_eq!(of(v3.id), Some(D::Rejected));
    assert_eq!(of(v2.id), Some(D::Published));

    let (page2, total2) =
        api_lifecycle::list_spec_versions_meta(&w.pool, w.team_a.id, api.id, 2, 2)
            .await
            .expect("page 2");
    assert_eq!(total2, 3);
    assert_eq!(page2.len(), 1);
    assert_eq!(page2[0].version, v1.version);
    let decisions2 =
        api_lifecycle::latest_spec_review_decisions(&w.pool, w.team_a.id, &[page2[0].id])
            .await
            .expect("decisions 2");
    assert!(decisions2.is_empty(), "v1 has no review events");
}

#[tokio::test]
async fn latest_spec_review_decisions_scope_by_team_and_break_ties_deterministically() {
    use fp_domain::api_lifecycle::SpecReviewDecision as D;
    let Some(w) = world().await else { return };
    let mut tx = w.pool.begin().await.expect("tx");
    let api =
        api_lifecycle::create_api_definition(&mut tx, w.team_a, &unique("api"), &api_spec("A"))
            .await
            .expect("api");
    tx.commit().await.expect("commit api");
    let v1 = commit_spec_version(&w.pool, w.team_a, api.id, "v1").await;

    // Empty input short-circuits without touching the DB.
    let none = api_lifecycle::latest_spec_review_decisions(&w.pool, w.team_a.id, &[])
        .await
        .expect("empty");
    assert!(none.is_empty());

    // Two events with IDENTICAL created_at: the id tie-break must decide, matching the
    // single-version query's `created_at DESC, id DESC`.
    let ts = Utc::now();
    // Random per-run ids (fixed ids collide across suite runs on the shared DB); both Rust
    // Uuid and PostgreSQL uuid compare byte-wise, so sorting picks the same winner.
    let (low, high) = {
        let (a, b) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        (a.min(b), a.max(b))
    };
    for (id, decision) in [(low, "published"), (high, "rejected")] {
        sqlx::query(
            "INSERT INTO spec_version_review_events \
             (id, team_id, org_id, api_definition_id, spec_version_id, decision, actor_type, \
              actor_id, reason, metadata, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, 'user', NULL, '', '{}'::jsonb, $7)",
        )
        .bind(id)
        .bind(w.team_a.id.as_uuid())
        .bind(w.team_a.org_id.as_uuid())
        .bind(api.id.as_uuid())
        .bind(v1.id.as_uuid())
        .bind(decision)
        .bind(ts)
        .execute(&w.pool)
        .await
        .expect("seed event");
    }
    let batch = api_lifecycle::latest_spec_review_decisions(&w.pool, w.team_a.id, &[v1.id])
        .await
        .expect("batch");
    assert_eq!(batch, vec![(v1.id, D::Rejected)], "highest id wins the tie");
    let mut tx = w.pool.begin().await.expect("tx single");
    let single = api_lifecycle::latest_spec_review_decision(&mut tx, w.team_a.id, v1.id)
        .await
        .expect("single");
    tx.rollback().await.expect("rollback");
    assert_eq!(single, Some(D::Rejected), "batch and single queries agree");

    // Cross-team: team_b sees nothing for team_a's versions.
    let cross = api_lifecycle::latest_spec_review_decisions(&w.pool, w.team_b.id, &[v1.id])
        .await
        .expect("cross");
    assert!(cross.is_empty());
    let (cross_list, cross_total) =
        api_lifecycle::list_spec_versions_meta(&w.pool, w.team_b.id, api.id, 50, 0)
            .await
            .expect("cross list");
    assert!(cross_list.is_empty());
    assert_eq!(cross_total, 0);
}
