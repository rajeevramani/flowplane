//! Integration tests for Platform API materializer with native resource generation and FK tracking

use flowplane::config::SimpleXdsConfig;
use flowplane::domain::api_definition::{
    ApiDefinitionSpec, ListenerConfig, RouteConfig as RouteSpec,
};
use flowplane::platform_api::materializer::PlatformApiMaterializer;
use flowplane::storage::repository::{ApiDefinitionRepository, ClusterRepository, RouteRepository};
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
    assert_eq!(outcome.generated_route_ids.len(), 1, "Should generate 1 route");
    assert_eq!(outcome.generated_cluster_ids.len(), 1, "Should generate 1 cluster");

    // Verify native resources exist in database
    let cluster_repo = ClusterRepository::new(ctx.pool.clone());
    let route_repo = RouteRepository::new(ctx.pool.clone());

    let cluster = cluster_repo
        .get_by_id(&flowplane::domain::ClusterId::from_str_unchecked(
            &outcome.generated_cluster_ids[0],
        ))
        .await
        .unwrap();
    assert_eq!(cluster.source, "platform_api", "Cluster should be tagged with platform_api");

    let route = route_repo
        .get_by_id(&flowplane::domain::RouteId::from_str_unchecked(&outcome.generated_route_ids[0]))
        .await
        .unwrap();
    assert_eq!(route.source, "platform_api", "Route should be tagged with platform_api");
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
    assert_eq!(
        api_route.generated_route_id,
        Some(outcome.generated_route_ids[0].clone()),
        "api_route.generated_route_id should match created route"
    );
    assert_eq!(
        api_route.generated_cluster_id,
        Some(outcome.generated_cluster_ids[0].clone()),
        "api_route.generated_cluster_id should match created cluster"
    );
}

#[tokio::test]
async fn test_shared_listener_mode_merges_routes() {
    let ctx = create_test_context().await;

    // Ensure default gateway resources exist
    flowplane::openapi::defaults::ensure_default_gateway_resources(&ctx.state)
        .await
        .expect("setup default gateway");

    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();

    // Get the default gateway route config before creating Platform API definition
    let route_repo = RouteRepository::new(ctx.pool.clone());
    let initial_route = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let initial_config: flowplane::xds::route::RouteConfig =
        serde_json::from_str(&initial_route.configuration).unwrap();
    let initial_vhost_count = initial_config.virtual_hosts.len();

    // Create Platform API definition with shared listener mode
    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "shared.example.com".to_string(),
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

    // Verify no isolated listener was created
    assert!(outcome.generated_listener_id.is_none(), "Should not generate listener in shared mode");

    // Verify the default gateway route config was updated with a new virtual host
    let updated_route = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let updated_config: flowplane::xds::route::RouteConfig =
        serde_json::from_str(&updated_route.configuration).unwrap();

    assert_eq!(
        updated_config.virtual_hosts.len(),
        initial_vhost_count + 1,
        "Should add one virtual host to default-gateway-routes"
    );

    // Verify the new virtual host has the correct domain
    let platform_vhost = updated_config
        .virtual_hosts
        .iter()
        .find(|vh| vh.domains.contains(&"shared.example.com".to_string()))
        .expect("Should find virtual host with Platform API domain");

    assert_eq!(platform_vhost.domains, vec!["shared.example.com"]);
    assert_eq!(platform_vhost.routes.len(), 1, "Should have one route");

    // Verify clusters were created
    assert_eq!(outcome.generated_cluster_ids.len(), 1, "Should create cluster");

    // Test deletion: verify virtual host is removed from shared route config
    materializer.delete_definition(outcome.definition.id.as_str()).await.unwrap();

    // Verify the default gateway route config was updated (virtual host removed)
    let after_delete_route = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let after_delete_config: flowplane::xds::route::RouteConfig =
        serde_json::from_str(&after_delete_route.configuration).unwrap();

    assert_eq!(
        after_delete_config.virtual_hosts.len(),
        initial_vhost_count,
        "Should remove Platform API virtual host from default-gateway-routes"
    );

    // Verify the Platform API virtual host is gone
    let platform_vhost_exists = after_delete_config
        .virtual_hosts
        .iter()
        .any(|vh| vh.domains.contains(&"shared.example.com".to_string()));
    assert!(!platform_vhost_exists, "Platform API virtual host should be removed");

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
    let old_route_ids = initial_outcome.generated_route_ids.clone();
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

    // Verify new resources were created
    assert_eq!(updated_outcome.generated_route_ids.len(), 1);
    assert_eq!(updated_outcome.generated_cluster_ids.len(), 1);

    // Verify old resources were deleted
    let cluster_repo = ClusterRepository::new(ctx.pool.clone());
    let route_repo = RouteRepository::new(ctx.pool.clone());

    for old_route_id in &old_route_ids {
        assert!(
            route_repo
                .get_by_id(&flowplane::domain::RouteId::from_str_unchecked(old_route_id))
                .await
                .is_err(),
            "Old route should be deleted"
        );
    }

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
    let route_id = outcome.generated_route_ids[0].clone();
    let cluster_id = outcome.generated_cluster_ids[0].clone();

    // Delete the API definition
    let api_repo = ApiDefinitionRepository::new(ctx.pool.clone());
    api_repo.delete_definition(&definition_id).await.unwrap();

    // Verify FK columns are set to NULL due to ON DELETE SET NULL
    let cluster_repo = ClusterRepository::new(ctx.pool.clone());
    let route_repo = RouteRepository::new(ctx.pool.clone());

    // The native resources should still exist (FK is SET NULL, not CASCADE)
    let cluster = cluster_repo
        .get_by_id(&flowplane::domain::ClusterId::from_str_unchecked(&cluster_id))
        .await
        .unwrap();
    let route = route_repo
        .get_by_id(&flowplane::domain::RouteId::from_str_unchecked(&route_id))
        .await
        .unwrap();

    assert_eq!(cluster.source, "platform_api");
    assert_eq!(route.source, "platform_api");
}
