//! Integration tests verifying Native API and Platform API can coexist in shared listeners
//! without conflicts. Tests backward compatibility and stability.

use flowplane::{
    auth::team::CreateTeamRequest,
    domain::api_definition::{ApiDefinitionSpec, ListenerConfig, RouteConfig as RouteSpec},
    platform_api::materializer::PlatformApiMaterializer,
    storage::repositories::team::{SqlxTeamRepository, TeamRepository},
    storage::repository::{ClusterRepository, CreateClusterRequest, RouteRepository},
    xds::{
        route::{
            PathMatch, RouteActionConfig, RouteConfig, RouteMatchConfig, RouteRule,
            VirtualHostConfig,
        },
        ClusterSpec, EndpointSpec, XdsState,
    },
};
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;

struct TestContext {
    state: Arc<XdsState>,
    pool: sqlx::SqlitePool,
}

async fn create_test_context() -> TestContext {
    // Set BOOTSTRAP_TOKEN for tests that need default gateway resources
    std::env::set_var(
        "BOOTSTRAP_TOKEN",
        "test-bootstrap-token-x8K9mP2nQ5rS7tU9vW1xY3zA4bC6dE8fG0hI2jK4L6m=",
    );

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:")
        .await
        .expect("create sqlite pool");

    flowplane::storage::run_migrations(&pool).await.expect("run migrations");

    // Create test teams to satisfy FK constraints
    let team_repo = SqlxTeamRepository::new(pool.clone());
    for team_name in &["test", "test-team", "platform-team", "team-a", "team-b", "native-test"] {
        let _ = team_repo
            .create_team(CreateTeamRequest {
                name: team_name.to_string(),
                display_name: format!("Test Team {}", team_name),
                description: Some("Team for native platform compatibility tests".to_string()),
                owner_user_id: None,
                settings: None,
            })
            .await;
    }

    let state = Arc::new(XdsState::with_database(
        flowplane::config::SimpleXdsConfig::default(),
        pool.clone(),
    ));

    // Ensure default gateway resources exist
    flowplane::openapi::defaults::ensure_default_gateway_resources(&state)
        .await
        .expect("setup default gateway");

    TestContext { state, pool }
}

#[tokio::test]
async fn test_native_api_and_platform_api_work_independently() {
    let ctx = create_test_context().await;
    let route_repo = RouteRepository::new(ctx.pool.clone());
    let cluster_repo = ClusterRepository::new(ctx.pool.clone());
    let listener_repo = flowplane::storage::repository::ListenerRepository::new(ctx.pool.clone());

    // Get initial state of default-gateway resources
    let initial_route = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let initial_config: RouteConfig = serde_json::from_str(&initial_route.configuration).unwrap();
    let initial_vhost_count = initial_config.virtual_hosts.len();

    // Create a Native API cluster
    let native_cluster_spec = ClusterSpec {
        connect_timeout_seconds: Some(5),
        endpoints: vec![EndpointSpec::Address {
            host: "native-backend.svc".to_string(),
            port: 8080,
        }],
        use_tls: Some(false),
        tls_server_name: None,
        dns_lookup_family: None,
        lb_policy: None,
        least_request: None,
        ring_hash: None,
        maglev: None,
        circuit_breakers: None,
        health_checks: Vec::new(),
        outlier_detection: None,
    };

    let _native_cluster = cluster_repo
        .create(CreateClusterRequest {
            name: "native-test-cluster".to_string(),
            service_name: "native-backend".to_string(),
            configuration: serde_json::to_value(&native_cluster_spec).unwrap(),
            team: Some("test".into()),
        })
        .await
        .unwrap();

    // Create a Native API route that adds to default-gateway-routes
    let mut native_route_config: RouteConfig =
        serde_json::from_str(&initial_route.configuration).unwrap();

    // Add a new virtual host for Native API
    native_route_config.virtual_hosts.push(VirtualHostConfig {
        name: "native-test-vhost".to_string(),
        domains: vec!["native.test.local".to_string()],
        routes: vec![RouteRule {
            name: Some("native-test-route".to_string()),
            r#match: RouteMatchConfig {
                path: PathMatch::Prefix("/native".to_string()),
                headers: None,
                query_parameters: None,
            },
            action: RouteActionConfig::Cluster {
                name: "native-test-cluster".to_string(),
                timeout: Some(30),
                prefix_rewrite: None,
                path_template_rewrite: None,
            },
            typed_per_filter_config: Default::default(),
        }],
        typed_per_filter_config: Default::default(),
    });

    // Update the route configuration
    route_repo
        .update(
            &initial_route.id,
            flowplane::storage::repository::UpdateRouteRequest {
                path_prefix: None,
                cluster_name: None,
                configuration: Some(serde_json::to_value(&native_route_config).unwrap()),
                team: None,
            },
        )
        .await
        .unwrap();

    // Verify Native API route was added
    let after_native = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let after_native_config: RouteConfig =
        serde_json::from_str(&after_native.configuration).unwrap();
    assert_eq!(
        after_native_config.virtual_hosts.len(),
        initial_vhost_count + 1,
        "Native API should add one virtual host"
    );

    // Now create a Platform API definition with its own isolated listener
    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();
    let platform_spec = ApiDefinitionSpec {
        team: "platform-team".to_string(),
        domain: "platform.test.local".to_string(),
        listener: ListenerConfig {
            name: None,
            bind_address: "0.0.0.0".to_string(),
            port: 8081, // Different port than default gateway
            protocol: "HTTP".to_string(),
            tls_config: None,
            http_filters: None,
        },
        tls_config: None,
        routes: vec![RouteSpec {
            match_type: "prefix".to_string(),
            match_value: "/platform".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!({
                "targets": [{
                    "name": "platform-backend",
                    "endpoint": "platform-backend.svc:8080",
                    "weight": 100
                }]
            }),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    let outcome = materializer.create_definition(platform_spec).await.unwrap();

    // Verify Native API routes are unchanged (Platform API doesn't affect default-gateway-routes)
    let after_platform = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let after_platform_config: RouteConfig =
        serde_json::from_str(&after_platform.configuration).unwrap();

    assert_eq!(
        after_platform_config.virtual_hosts.len(),
        initial_vhost_count + 1,
        "Native API virtual host count should be unchanged"
    );

    // Verify Native API virtual host still exists unchanged
    let native_vhost = after_platform_config
        .virtual_hosts
        .iter()
        .find(|vh| vh.domains.contains(&"native.test.local".to_string()))
        .expect("Native API virtual host should still exist");
    assert_eq!(native_vhost.name, "native-test-vhost");
    assert_eq!(native_vhost.routes.len(), 1);

    // Verify Platform API created its own isolated listener
    assert!(
        outcome.generated_listener_id.is_some(),
        "Platform API should create isolated listener"
    );
    let listener_id_str = outcome.generated_listener_id.unwrap();
    let listener_id = flowplane::domain::ListenerId::from_str_unchecked(&listener_id_str);
    let platform_listener = listener_repo.get_by_id(&listener_id).await.unwrap();
    assert_eq!(platform_listener.port, Some(8081), "Platform API listener should be on port 8081");
}

#[tokio::test]
async fn test_native_api_update_does_not_affect_platform_api() {
    let ctx = create_test_context().await;
    let route_repo = RouteRepository::new(ctx.pool.clone());
    let listener_repo = flowplane::storage::repository::ListenerRepository::new(ctx.pool.clone());

    // Create Platform API definition first
    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();
    let platform_spec = ApiDefinitionSpec {
        team: "platform-team".to_string(),
        domain: "platform.test.local".to_string(),
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
            match_value: "/platform".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!({
                "targets": [{
                    "name": "platform-backend",
                    "endpoint": "platform-backend.svc:8080",
                    "weight": 100
                }]
            }),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    let outcome = materializer.create_definition(platform_spec).await.unwrap();
    let platform_listener_id_str = outcome.generated_listener_id.clone().unwrap();
    let platform_listener_id =
        flowplane::domain::ListenerId::from_str_unchecked(&platform_listener_id_str);

    // Get the Platform API listener state
    let platform_listener_before = listener_repo.get_by_id(&platform_listener_id).await.unwrap();

    // Now update Native API (modify default-gateway-routes)
    let default_route = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let mut route_config: RouteConfig = serde_json::from_str(&default_route.configuration).unwrap();

    // Add a new route to the default virtual host
    let default_vhost = route_config
        .virtual_hosts
        .iter_mut()
        .find(|vh| vh.name == "default-gateway-vhost")
        .expect("Default vhost should exist");

    default_vhost.routes.push(RouteRule {
        name: Some("native-updated-route".to_string()),
        r#match: RouteMatchConfig {
            path: PathMatch::Prefix("/updated".to_string()),
            headers: None,
            query_parameters: None,
        },
        action: RouteActionConfig::Cluster {
            name: "default-gateway-cluster".to_string(),
            timeout: Some(30),
            prefix_rewrite: None,
            path_template_rewrite: None,
        },
        typed_per_filter_config: Default::default(),
    });

    // Update via Native API (direct repository call)
    route_repo
        .update(
            &default_route.id,
            flowplane::storage::repository::UpdateRouteRequest {
                path_prefix: None,
                cluster_name: None,
                configuration: Some(serde_json::to_value(&route_config).unwrap()),
                team: None,
            },
        )
        .await
        .unwrap();

    // Verify Platform API listener is unchanged
    let platform_listener_after = listener_repo.get_by_id(&platform_listener_id).await.unwrap();
    assert_eq!(
        platform_listener_before.id, platform_listener_after.id,
        "Platform API listener should be unchanged"
    );
    assert_eq!(
        platform_listener_before.port, platform_listener_after.port,
        "Platform API listener port should be unchanged"
    );

    // Verify the native route was added to default-gateway-routes
    let after_update = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let updated_config: RouteConfig = serde_json::from_str(&after_update.configuration).unwrap();
    let default_vhost = updated_config
        .virtual_hosts
        .iter()
        .find(|vh| vh.name == "default-gateway-vhost")
        .expect("Default vhost should exist");
    assert!(
        default_vhost.routes.iter().any(|r| r
            .name
            .as_ref()
            .map(|n| n == "native-updated-route")
            .unwrap_or(false)),
        "Native updated route should exist"
    );
}

#[tokio::test]
async fn test_platform_api_delete_does_not_affect_native_api() {
    let ctx = create_test_context().await;
    let route_repo = RouteRepository::new(ctx.pool.clone());
    let listener_repo = flowplane::storage::repository::ListenerRepository::new(ctx.pool.clone());

    // Get initial state of default-gateway-routes
    let initial_route = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let initial_config: RouteConfig = serde_json::from_str(&initial_route.configuration).unwrap();
    let initial_vhost_count = initial_config.virtual_hosts.len();

    // Create Platform API definition
    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();
    let platform_spec = ApiDefinitionSpec {
        team: "platform-team".to_string(),
        domain: "platform.test.local".to_string(),
        listener: ListenerConfig {
            name: None,
            bind_address: "0.0.0.0".to_string(),
            port: 8082,
            protocol: "HTTP".to_string(),
            tls_config: None,
            http_filters: None,
        },
        tls_config: None,
        routes: vec![RouteSpec {
            match_type: "prefix".to_string(),
            match_value: "/platform".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!({
                "targets": [{
                    "name": "platform-backend",
                    "endpoint": "platform-backend.svc:8080",
                    "weight": 100
                }]
            }),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    let outcome = materializer.create_definition(platform_spec).await.unwrap();
    let platform_listener_id_str = outcome.generated_listener_id.clone().unwrap();
    let platform_listener_id =
        flowplane::domain::ListenerId::from_str_unchecked(&platform_listener_id_str);

    // Verify Platform API listener was created
    let platform_listener = listener_repo.get_by_id(&platform_listener_id).await.unwrap();
    assert_eq!(platform_listener.port, Some(8082));

    // Verify default-gateway-routes is unchanged (Platform API doesn't affect it)
    let after_create = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let after_create_config: RouteConfig =
        serde_json::from_str(&after_create.configuration).unwrap();
    assert_eq!(
        after_create_config.virtual_hosts.len(),
        initial_vhost_count,
        "Platform API should not affect default-gateway-routes"
    );

    // Delete the Platform API definition
    materializer.delete_definition(outcome.definition.id.as_str()).await.unwrap();

    // Verify Platform API listener was deleted
    assert!(
        listener_repo.get_by_id(&platform_listener_id).await.is_err(),
        "Platform API listener should be deleted"
    );

    // Verify default-gateway-routes is still unchanged
    let after_delete = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let after_delete_config: RouteConfig =
        serde_json::from_str(&after_delete.configuration).unwrap();

    assert_eq!(
        after_delete_config.virtual_hosts.len(),
        initial_vhost_count,
        "Native API routes should remain unchanged"
    );

    // Verify default-gateway-vhost still exists
    let default_vhost = after_delete_config
        .virtual_hosts
        .iter()
        .find(|vh| vh.name == "default-gateway-vhost")
        .expect("Default gateway vhost should still exist");
    assert!(!default_vhost.routes.is_empty(), "Default routes should still exist");
}

#[tokio::test]
async fn test_route_ordering_is_deterministic() {
    let ctx = create_test_context().await;
    let route_repo = RouteRepository::new(ctx.pool.clone());
    let materializer = PlatformApiMaterializer::new(ctx.state.clone()).unwrap();

    // Create multiple Platform API definitions
    let spec1 = ApiDefinitionSpec {
        team: "team-a".to_string(),
        domain: "a.test.local".to_string(),
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
            match_value: "/a".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!({"targets": [{"name": "a", "endpoint": "a:8080", "weight": 100}]}),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    let spec2 = ApiDefinitionSpec {
        team: "team-b".to_string(),
        domain: "b.test.local".to_string(),
        listener: ListenerConfig {
            name: None,
            bind_address: "0.0.0.0".to_string(),
            port: 8084,
            protocol: "HTTP".to_string(),
            tls_config: None,
            http_filters: None,
        },
        tls_config: None,
        routes: vec![RouteSpec {
            match_type: "prefix".to_string(),
            match_value: "/b".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: serde_json::json!({"targets": [{"name": "b", "endpoint": "b:8080", "weight": 100}]}),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    let _outcome1 = materializer.create_definition(spec1).await.unwrap();
    let _outcome2 = materializer.create_definition(spec2).await.unwrap();

    // Get route config
    let route1 = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let config1: RouteConfig = serde_json::from_str(&route1.configuration).unwrap();
    let vhost_names1: Vec<String> =
        config1.virtual_hosts.iter().map(|vh| vh.name.clone()).collect();

    // Trigger an xDS refresh (simulates update)
    ctx.state.refresh_routes_from_repository().await.unwrap();

    // Get route config again
    let route2 = route_repo.get_by_name("default-gateway-routes").await.unwrap();
    let config2: RouteConfig = serde_json::from_str(&route2.configuration).unwrap();
    let vhost_names2: Vec<String> =
        config2.virtual_hosts.iter().map(|vh| vh.name.clone()).collect();

    // Verify ordering is deterministic (should be alphabetically sorted)
    assert_eq!(
        vhost_names1, vhost_names2,
        "Virtual host ordering should be deterministic across xDS updates"
    );

    // Verify virtual hosts are sorted alphabetically
    let mut sorted_names = vhost_names1.clone();
    sorted_names.sort();
    assert_eq!(vhost_names1, sorted_names, "Virtual hosts should be sorted alphabetically");
}
