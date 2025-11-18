//! Integration tests for Platform API materializer with native resource generation and FK tracking

use flowplane::auth::team::CreateTeamRequest;
use flowplane::config::SimpleXdsConfig;
use flowplane::domain::api_definition::{
    ApiDefinitionSpec, ListenerConfig, RouteConfig as RouteSpec,
};
use flowplane::platform_api::materializer::PlatformApiMaterializer;
use flowplane::storage::repositories::team::{SqlxTeamRepository, TeamRepository};
use flowplane::storage::repository::{ApiDefinitionRepository, ClusterRepository};
use flowplane::storage::{self, DbPool};
use flowplane::xds::XdsState;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;

struct TestContext {
    state: Arc<XdsState>,
    pool: DbPool,
}

async fn create_test_context() -> TestContext {
    // Set BOOTSTRAP_TOKEN for tests that need default gateway resources
    std::env::set_var(
        "BOOTSTRAP_TOKEN",
        "test-bootstrap-token-x8K9mP2nQ5rS7tU9vW1xY3zA4bC6dE8fG0hI2jK4L6m=",
    );

    // Use in-memory database without cache=shared to ensure test isolation
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:")
        .await
        .expect("create sqlite pool");

    storage::run_migrations(&pool).await.expect("run migrations for tests");

    // Create test team to satisfy FK constraints
    let team_repo = SqlxTeamRepository::new(pool.clone());
    let _ = team_repo
        .create_team(CreateTeamRequest {
            name: "test-team".to_string(),
            display_name: "Test Team".to_string(),
            description: Some("Team for materializer integration tests".to_string()),
            owner_user_id: None,
            settings: None,
        })
        .await;

    let state = Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool.clone()));

    TestContext { state, pool }
}

#[tokio::test]
async fn test_create_definition_generates_native_resources() {
    let ctx = create_test_context().await;
    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();

    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "test.example.com".to_string(),
        listener: ListenerConfig {
            name: None,
            bind_address: "0.0.0.0".to_string(),
            port: 8080,
            protocol: "HTTP".to_string(),
            tls_config: None,
            http_filters: None,
        },
        tls_config: None,
        routes: vec![RouteSpec {
            match_type: "prefix".to_string(),
            match_value: "/api/v1".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!({
                "targets": [
                    {"name": "backend", "endpoint": "backend.svc:8080", "weight": 100}
                ]
            }),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    let outcome = materializer.create_definition(spec).await.unwrap();

    // Verify API definition was created
    assert!(!outcome.definition.id.as_str().is_empty());
    assert_eq!(outcome.definition.team, "test-team");
    assert_eq!(outcome.definition.domain, "test.example.com");

    // Verify native resources were generated
    // Note: Platform API no longer creates native routes (routes are generated dynamically via xDS)
    // Only clusters are created
    assert_eq!(outcome.generated_route_ids.len(), 0, "Should not create native routes");
    assert_eq!(outcome.generated_cluster_ids.len(), 1, "Should generate 1 cluster");

    // Verify cluster exists in database
    let cluster_repo = ClusterRepository::new(ctx.pool.clone());

    let cluster = cluster_repo
        .get_by_id(&flowplane::domain::ClusterId::from_str_unchecked(
            &outcome.generated_cluster_ids[0],
        ))
        .await
        .unwrap();
    assert_eq!(cluster.source, "platform_api", "Cluster should be tagged with platform_api");
}

#[tokio::test]
async fn test_fk_relationships_stored_correctly() {
    let ctx = create_test_context().await;
    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();

    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "test.example.com".to_string(),
        listener: ListenerConfig {
            name: None,
            bind_address: "0.0.0.0".to_string(),
            port: 8080,
            protocol: "HTTP".to_string(),
            tls_config: None,
            http_filters: None,
        },
        tls_config: None,
        routes: vec![RouteSpec {
            match_type: "prefix".to_string(),
            match_value: "/api".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!({
                "targets": [{
                    "name": "test-backend",
                    "endpoint": "backend.svc:8080",
                    "weight": 100
                }]
            }),
            timeout_seconds: None,
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    let outcome = materializer.create_definition(spec).await.unwrap();
    let api_repo = ApiDefinitionRepository::new(ctx.pool.clone());

    // Verify FK relationships in api_routes table
    let api_routes = api_repo.list_routes(&outcome.definition.id).await.unwrap();
    assert_eq!(api_routes.len(), 1);

    let api_route = &api_routes[0];
    // Note: Platform API no longer creates native routes, so generated_route_id should be None
    assert_eq!(
        api_route.generated_route_id, None,
        "api_route.generated_route_id should be None (no native routes created)"
    );
    assert_eq!(
        api_route.generated_cluster_id,
        Some(outcome.generated_cluster_ids[0].clone()),
        "api_route.generated_cluster_id should match created cluster"
    );
}

#[tokio::test]
async fn test_isolated_listener_mode_creates_listener() {
    let ctx = create_test_context().await;
    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();

    // Create Platform API definition (now all definitions use isolated listeners)
    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "isolated.example.com".to_string(),
        listener: ListenerConfig {
            name: None,
            bind_address: "0.0.0.0".to_string(),
            port: 8081,
            protocol: "HTTP".to_string(),
            tls_config: None,
            http_filters: None,
        },
        tls_config: None,
        routes: vec![RouteSpec {
            match_type: "prefix".to_string(),
            match_value: "/api".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!({
                "targets": [{
                    "name": "test-backend",
                    "endpoint": "backend.svc:8080",
                    "weight": 100
                }]
            }),
            timeout_seconds: None,
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    let outcome = materializer.create_definition(spec).await.unwrap();

    // Verify isolated listener was created
    assert!(outcome.generated_listener_id.is_some(), "Should generate isolated listener");

    // Verify clusters were created
    assert_eq!(outcome.generated_cluster_ids.len(), 1, "Should create cluster");

    // Verify no native routes were created (routes are dynamically generated via xDS)
    assert_eq!(outcome.generated_route_ids.len(), 0, "Should not create native routes");

    // Test deletion: verify listener is removed
    materializer.delete_definition(outcome.definition.id.as_str()).await.unwrap();

    // Verify the definition was deleted from the database
    let api_repo = ApiDefinitionRepository::new(ctx.pool.clone());
    let definition_result = api_repo.get_definition(&outcome.definition.id).await;
    assert!(definition_result.is_err(), "Definition should be deleted from database");
}

#[tokio::test]
async fn test_update_definition_cleans_up_orphaned_resources() {
    let ctx = create_test_context().await;
    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();

    // Create initial definition with 2 routes
    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "update-test.example.com".to_string(),
        listener: ListenerConfig {
            name: None,
            bind_address: "0.0.0.0".to_string(),
            port: 8082,
            protocol: "HTTP".to_string(),
            tls_config: None,
            http_filters: None,
        },
        tls_config: None,
        routes: vec![
            RouteSpec {
                match_type: "prefix".to_string(),
                match_value: "/api/v1".to_string(),
                case_sensitive: true,
                headers: None,
                rewrite_prefix: None,
                rewrite_regex: None,
                rewrite_substitution: None,
                upstream_targets: serde_json::json!({
                    "targets": [{
                        "name": "test-backend",
                        "endpoint": "backend.svc:8080",
                        "weight": 100
                    }]
                }),
                timeout_seconds: None,
                override_config: None,
                deployment_note: None,
                route_order: Some(0),
            },
            RouteSpec {
                match_type: "prefix".to_string(),
                match_value: "/api/v2".to_string(),
                case_sensitive: true,
                headers: None,
                rewrite_prefix: None,
                rewrite_regex: None,
                rewrite_substitution: None,
                upstream_targets: serde_json::json!({
                    "targets": [{
                        "name": "test-backend",
                        "endpoint": "backend.svc:8080",
                        "weight": 100
                    }]
                }),
                timeout_seconds: None,
                override_config: None,
                deployment_note: None,
                route_order: Some(1),
            },
        ],
    };

    let initial_outcome = materializer.create_definition(spec).await.unwrap();
    let old_cluster_ids = initial_outcome.generated_cluster_ids.clone();

    // Update with only 1 route
    let updated_routes = vec![RouteSpec {
        match_type: "prefix".to_string(),
        match_value: "/api/v3".to_string(),
        case_sensitive: true,
        headers: None,
        rewrite_prefix: None,
        rewrite_regex: None,
        rewrite_substitution: None,
        upstream_targets: serde_json::json!({
            "targets": [{
                "name": "test-backend-v3",
                "endpoint": "backend-v3.svc:8080",
                "weight": 100
            }]
        }),
        timeout_seconds: None,
        override_config: None,
        deployment_note: None,
        route_order: Some(0),
    }];

    let updated_outcome = materializer
        .update_definition(initial_outcome.definition.id.as_str(), updated_routes)
        .await
        .unwrap();

    // Verify new clusters were created
    assert_eq!(updated_outcome.generated_route_ids.len(), 0, "Should not create native routes");
    assert_eq!(updated_outcome.generated_cluster_ids.len(), 1);

    // Verify old clusters were deleted
    let cluster_repo = ClusterRepository::new(ctx.pool.clone());

    for old_cluster_id in &old_cluster_ids {
        assert!(
            cluster_repo
                .get_by_id(&flowplane::domain::ClusterId::from_str_unchecked(old_cluster_id))
                .await
                .is_err(),
            "Old cluster should be deleted"
        );
    }
}

#[tokio::test]
async fn test_cascading_delete_removes_native_resources() {
    let ctx = create_test_context().await;
    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();

    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "delete-test.example.com".to_string(),
        listener: ListenerConfig {
            name: None,
            bind_address: "0.0.0.0".to_string(),
            port: 8083,
            protocol: "HTTP".to_string(),
            tls_config: None,
            http_filters: None,
        },
        tls_config: None,
        routes: vec![RouteSpec {
            match_type: "prefix".to_string(),
            match_value: "/api".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!({
                "targets": [{
                    "name": "test-backend",
                    "endpoint": "backend.svc:8080",
                    "weight": 100
                }]
            }),
            timeout_seconds: None,
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    let outcome = materializer.create_definition(spec).await.unwrap();
    let definition_id = outcome.definition.id.clone();
    let cluster_id = outcome.generated_cluster_ids[0].clone();

    // Delete the API definition
    let api_repo = ApiDefinitionRepository::new(ctx.pool.clone());
    api_repo.delete_definition(&definition_id).await.unwrap();

    // Verify the cluster still exists (FK is SET NULL, not CASCADE)
    let cluster_repo = ClusterRepository::new(ctx.pool.clone());
    let cluster = cluster_repo
        .get_by_id(&flowplane::domain::ClusterId::from_str_unchecked(&cluster_id))
        .await
        .unwrap();

    assert_eq!(cluster.source, "platform_api");
}
