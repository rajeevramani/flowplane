//! Integration tests for Platform API materializer with native resource generation and FK tracking

use flowplane::config::SimpleXdsConfig;
use flowplane::platform_api::materializer::{
    ApiDefinitionSpec, ListenerInput, PlatformApiMaterializer, RouteSpec,
};
use flowplane::storage::repository::{
    ApiDefinitionRepository, ClusterRepository, ListenerRepository, RouteRepository,
};
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
        listener_isolation: false,
        isolation_listener: None,
        target_listeners: None,
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

    let cluster = cluster_repo.get_by_id(&outcome.generated_cluster_ids[0]).await.unwrap();
    assert_eq!(cluster.source, "platform_api", "Cluster should be tagged with platform_api");

    let route = route_repo.get_by_id(&outcome.generated_route_ids[0]).await.unwrap();
    assert_eq!(route.source, "platform_api", "Route should be tagged with platform_api");
}

#[tokio::test]
async fn test_fk_relationships_stored_correctly() {
    let ctx = create_test_context().await;
    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();

    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "test.example.com".to_string(),
        listener_isolation: false,
        isolation_listener: None,
        target_listeners: None,
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
async fn test_listener_isolation_creates_listener() {
    let ctx = create_test_context().await;
    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();

    // Use a random port to avoid conflicts
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(20000);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "isolated.example.com".to_string(),
        listener_isolation: true,
        isolation_listener: Some(ListenerInput {
            name: Some(format!("test-isolated-listener-{}", port)),
            bind_address: "127.0.0.1".to_string(),
            port: port as u32,
            protocol: "HTTP".to_string(),
            tls_config: None,
            http_filters: None,
        }),
        target_listeners: None,
        tls_config: None,
        routes: vec![RouteSpec {
            match_type: "prefix".to_string(),
            match_value: "/".to_string(),
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

    // Verify listener was created
    assert!(outcome.generated_listener_id.is_some(), "Should generate listener in isolation mode");

    let listener_repo = ListenerRepository::new(ctx.pool.clone());
    let listener =
        listener_repo.get_by_id(outcome.generated_listener_id.as_ref().unwrap()).await.unwrap();

    assert_eq!(listener.source, "platform_api", "Listener should be tagged with platform_api");
    assert!(
        listener.name.starts_with("test-isolated-listener-"),
        "Listener name should match pattern"
    );

    // Verify FK relationship in api_definitions table
    let api_repo = ApiDefinitionRepository::new(ctx.pool.clone());
    let updated_def = api_repo.get_definition(&outcome.definition.id).await.unwrap();
    assert_eq!(
        updated_def.generated_listener_id, outcome.generated_listener_id,
        "api_definition.generated_listener_id should match created listener"
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

    // Create Platform API definition with listenerIsolation=false
    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "shared.example.com".to_string(),
        listener_isolation: false,
        isolation_listener: None,
        target_listeners: None, // Should default to default-gateway-listener
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
    materializer.delete_definition(&outcome.definition.id).await.unwrap();

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
async fn test_isolated_listener_deletion() {
    let ctx = create_test_context().await;
    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();
    let listener_repo = ListenerRepository::new(ctx.pool.clone());
    let cluster_repo = ClusterRepository::new(ctx.pool.clone());
    let route_repo = RouteRepository::new(ctx.pool.clone());

    // Use a random port to avoid conflicts
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER_DELETE: AtomicU16 = AtomicU16::new(21000);
    let port = PORT_COUNTER_DELETE.fetch_add(1, Ordering::SeqCst);

    // Create Platform API definition with isolated listener
    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "isolated-delete.example.com".to_string(),
        listener_isolation: true,
        isolation_listener: Some(ListenerInput {
            name: Some(format!("test-isolated-delete-listener-{}", port)),
            bind_address: "127.0.0.1".to_string(),
            port: port as u32,
            protocol: "HTTP".to_string(),
            tls_config: None,
            http_filters: None,
        }),
        target_listeners: None,
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
    let listener_id = outcome.generated_listener_id.clone().expect("Should create listener");
    let cluster_ids = outcome.generated_cluster_ids.clone();
    let route_ids = outcome.generated_route_ids.clone();

    // Verify resources were created
    assert!(listener_repo.get_by_id(&listener_id).await.is_ok(), "Listener should exist");
    assert!(cluster_repo.get_by_id(&cluster_ids[0]).await.is_ok(), "Cluster should exist");
    assert!(route_repo.get_by_id(&route_ids[0]).await.is_ok(), "Route should exist");

    // Delete the definition
    materializer.delete_definition(&definition_id).await.unwrap();

    // Verify all resources were deleted
    assert!(listener_repo.get_by_id(&listener_id).await.is_err(), "Listener should be deleted");
    assert!(cluster_repo.get_by_id(&cluster_ids[0]).await.is_err(), "Cluster should be deleted");
    assert!(route_repo.get_by_id(&route_ids[0]).await.is_err(), "Route should be deleted");

    // Verify the definition was deleted from the database
    let api_repo = ApiDefinitionRepository::new(ctx.pool.clone());
    let definition_result = api_repo.get_definition(&definition_id).await;
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
        listener_isolation: false,
        isolation_listener: None,
        target_listeners: None,
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
        .update_definition(&initial_outcome.definition.id, updated_routes)
        .await
        .unwrap();

    // Verify new resources were created
    assert_eq!(updated_outcome.generated_route_ids.len(), 1);
    assert_eq!(updated_outcome.generated_cluster_ids.len(), 1);

    // Verify old resources were deleted
    let cluster_repo = ClusterRepository::new(ctx.pool.clone());
    let route_repo = RouteRepository::new(ctx.pool.clone());

    for old_route_id in &old_route_ids {
        assert!(route_repo.get_by_id(old_route_id).await.is_err(), "Old route should be deleted");
    }

    for old_cluster_id in &old_cluster_ids {
        assert!(
            cluster_repo.get_by_id(old_cluster_id).await.is_err(),
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
        listener_isolation: false,
        isolation_listener: None,
        target_listeners: None,
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
    let cluster = cluster_repo.get_by_id(&cluster_id).await.unwrap();
    let route = route_repo.get_by_id(&route_id).await.unwrap();

    assert_eq!(cluster.source, "platform_api");
    assert_eq!(route.source, "platform_api");
}
