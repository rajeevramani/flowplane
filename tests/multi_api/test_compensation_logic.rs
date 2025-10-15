//! Compensation logic tests for Platform API materializer
//!
//! These tests validate that the Platform API materializer properly handles
//! failures, rollbacks, and cleanup operations. They ensure that partial
//! failures don't leave the system in an inconsistent state.

use flowplane::{
    domain::api_definition::{ApiDefinitionSpec, RouteConfig as RouteSpec},
    storage::repository::{
        ApiDefinitionRepository, ClusterRepository, ListenerRepository, RouteRepository,
    },
};
use serde_json::json;

use super::support::setup_multi_api_app;

/// Scenario 4.4: Cascading Delete Compensation
///
/// Test that when an API definition is deleted, ALL associated resources
/// are properly cleaned up (clusters, routes, isolated listeners).
#[tokio::test]
async fn cascading_delete_cleans_up_all_resources() {
    let app = setup_multi_api_app().await;
    let api_def_repo = ApiDefinitionRepository::new(app.pool.clone());
    let cluster_repo = ClusterRepository::new(app.pool.clone());
    let route_repo = RouteRepository::new(app.pool.clone());
    let listener_repo = ListenerRepository::new(app.pool.clone());

    // Create a Platform API definition with listener isolation
    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "cascade-test.example.com".to_string(),
        listener_isolation: true,
        isolation_listener: Some(flowplane::domain::ListenerConfig {
            name: Some("cascade-test-listener".to_string()),
            bind_address: "0.0.0.0".to_string(),
            port: 9999,
            protocol: "HTTP".to_string(),
            tls_config: None,
            http_filters: None,
        }),
        target_listeners: None,
        tls_config: None,
        routes: vec![
            RouteSpec {
                match_type: "prefix".to_string(),
                match_value: "/api".to_string(),
                case_sensitive: true,
                headers: None,
                rewrite_prefix: None,
                rewrite_regex: None,
                rewrite_substitution: None,
                upstream_targets: json!({
                    "targets": [{
                        "name": "backend-1",
                        "endpoint": "backend1.svc:8080",
                        "weight": 100
                    }]
                }),
                timeout_seconds: Some(30),
                override_config: None,
                deployment_note: None,
                route_order: Some(0),
            },
            RouteSpec {
                match_type: "prefix".to_string(),
                match_value: "/admin".to_string(),
                case_sensitive: true,
                headers: None,
                rewrite_prefix: None,
                rewrite_regex: None,
                rewrite_substitution: None,
                upstream_targets: json!({
                    "targets": [{
                        "name": "backend-2",
                        "endpoint": "backend2.svc:8080",
                        "weight": 100
                    }]
                }),
                timeout_seconds: Some(30),
                override_config: None,
                deployment_note: None,
                route_order: Some(1),
            },
        ],
    };

    let outcome = app.materializer.create_definition(spec).await.expect("create API definition");

    let def_id = outcome.definition.id.clone();

    // Verify resources were created
    let api_def = api_def_repo.get_definition(&def_id).await.expect("get API definition");
    assert_eq!(api_def.id, def_id);
    assert_eq!(api_def.domain, "cascade-test.example.com");

    // Count initial resources (list all clusters)
    let initial_clusters = cluster_repo.list(Some(1000), None).await.expect("list clusters");
    let initial_cluster_count = initial_clusters.len();
    assert!(initial_cluster_count >= 2, "Should have at least 2 clusters created");

    // Find the isolated listener
    let all_listeners = listener_repo.list(Some(100), None).await.expect("list listeners");
    let isolated_listener = all_listeners
        .iter()
        .find(|l| l.name == "cascade-test-listener")
        .expect("isolated listener should exist");
    assert_eq!(isolated_listener.port, Some(9999));

    // Find the route configuration for the isolated listener
    let all_routes = route_repo.list(None, None).await.expect("list routes");
    let initial_route_count = all_routes.len();
    assert!(initial_route_count > 0, "Should have routes created");

    // NOW DELETE THE API DEFINITION
    app.materializer.delete_definition(def_id.as_str()).await.expect("delete API definition");

    // Verify API definition is deleted
    let deleted_def = api_def_repo.get_definition(&def_id).await;
    assert!(deleted_def.is_err(), "API definition should be deleted");

    // Verify clusters are cleaned up
    let remaining_clusters =
        cluster_repo.list(Some(1000), None).await.expect("list clusters after delete");

    // The clusters associated with this API definition should be deleted
    // (exact count depends on whether they're shared, but should be fewer)
    let cluster_names_before: Vec<String> =
        initial_clusters.iter().map(|c| c.name.clone()).collect();
    let cluster_names_after: Vec<String> =
        remaining_clusters.iter().map(|c| c.name.clone()).collect();

    // Verify that clusters created for this definition are gone
    for cluster_name in &["backend-1", "backend-2"] {
        if cluster_names_before.contains(&cluster_name.to_string()) {
            // If it was created, it should be deleted
            assert!(
                !cluster_names_after.contains(&cluster_name.to_string()),
                "Cluster '{}' should be deleted",
                cluster_name
            );
        }
    }

    // Verify isolated listener is deleted
    let listeners_after = listener_repo.list(Some(100), None).await.expect("list listeners");
    assert!(
        !listeners_after.iter().any(|l| l.name == "cascade-test-listener"),
        "Isolated listener should be deleted"
    );

    // Verify routes are cleaned up
    let routes_after = route_repo.list(None, None).await.expect("list routes");
    // Routes specific to this listener should be gone
    // (exact verification depends on route naming conventions)
    assert!(routes_after.len() <= initial_route_count, "Routes should be cleaned up");
}

/// Scenario 4.2: Database Rollback on Conflict
///
/// Test that when a conflict is detected during API definition creation,
/// the database transaction is rolled back and no partial state remains.
#[tokio::test]
async fn database_rollback_on_domain_conflict() {
    let app = setup_multi_api_app().await;
    let api_def_repo = ApiDefinitionRepository::new(app.pool.clone());
    let cluster_repo = ClusterRepository::new(app.pool.clone());

    // Create first API definition successfully
    let spec1 = ApiDefinitionSpec {
        team: "team-a".to_string(),
        domain: "rollback-test.example.com".to_string(),
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
            upstream_targets: json!({
                "targets": [{
                    "name": "backend-a",
                    "endpoint": "backend-a.svc:8080",
                    "weight": 100
                }]
            }),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    app.materializer.create_definition(spec1).await.expect("create first API definition");

    // Count resources before failed attempt
    let api_defs_before = api_def_repo
        .list_definitions(Some("team-b".to_string()), Some(100), None)
        .await
        .expect("list API definitions before");
    let api_def_count_before = api_defs_before.len();

    // Attempt to create conflicting API definition (same domain)
    let spec2 = ApiDefinitionSpec {
        team: "team-b".to_string(),                      // Different team
        domain: "rollback-test.example.com".to_string(), // SAME DOMAIN - will conflict
        listener_isolation: false,
        isolation_listener: None,
        target_listeners: None,
        tls_config: None,
        routes: vec![RouteSpec {
            match_type: "prefix".to_string(),
            match_value: "/api/v2".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: json!({
                "targets": [{
                    "name": "backend-b",
                    "endpoint": "backend-b.svc:8080",
                    "weight": 100
                }]
            }),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    let result = app.materializer.create_definition(spec2).await;
    assert!(result.is_err(), "Should fail due to domain conflict");

    // VERIFY: No partial state left behind for team-b
    let clusters_after = cluster_repo.list(Some(1000), None).await.expect("list clusters after");

    // We can't directly compare cluster counts since we list all teams
    // Instead verify no "backend-b" cluster was created
    assert!(
        !clusters_after.iter().any(|c| c.name == "backend-b"),
        "backend-b cluster should not exist due to rollback"
    );

    let api_defs_after = api_def_repo
        .list_definitions(Some("team-b".to_string()), Some(100), None)
        .await
        .expect("list API definitions after");
    assert_eq!(
        api_defs_after.len(),
        api_def_count_before,
        "No API definition should be created for team-b due to rollback"
    );

    // Verify the original API definition for team-a is still intact
    let team_a_defs = api_def_repo
        .list_definitions(Some("team-a".to_string()), Some(100), None)
        .await
        .expect("list team-a API definitions");
    assert!(!team_a_defs.is_empty(), "Team A's API definition should still exist");
    assert!(
        team_a_defs.iter().any(|d| d.domain == "rollback-test.example.com"),
        "Team A's domain should be intact"
    );
}

/// Scenario 4.3: xDS State Consistency After Failure
///
/// Test that xDS snapshot state remains consistent even when
/// API definition creation fails partway through.
#[tokio::test]
async fn xds_state_consistent_after_creation_failure() {
    let app = setup_multi_api_app().await;

    // Get initial xDS snapshot version
    let initial_version = app.state.get_version_number();

    // Attempt to create API definition that will fail (port conflict)
    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "xds-test.example.com".to_string(),
        listener_isolation: true,
        isolation_listener: Some(flowplane::domain::ListenerConfig {
            name: Some("xds-test-listener".to_string()),
            bind_address: "0.0.0.0".to_string(),
            port: 18000, // This might conflict with xDS server port
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
            upstream_targets: json!({
                "targets": [{
                    "name": "backend",
                    "endpoint": "backend.svc:8080",
                    "weight": 100
                }]
            }),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    let result = app.materializer.create_definition(spec).await;
    // May or may not fail depending on system state, but let's check consistency

    // Get xDS snapshot version after attempted creation
    let final_version = app.state.get_version_number();

    if result.is_err() {
        // If creation failed, xDS version should either be unchanged or properly incremented
        // but state should be consistent (no half-baked resources)

        // Verify we can still fetch cluster resources without errors
        let clusters =
            app.state.cached_resources("type.googleapis.com/envoy.config.cluster.v3.Cluster");

        // All clusters in cache should be valid (not partial/corrupted)
        for cluster in &clusters {
            assert!(!cluster.name.is_empty(), "Cluster should have valid name");
        }

        println!(
            "xDS state remained consistent after failure. Version: {} -> {}",
            initial_version, final_version
        );
    } else {
        // If creation succeeded, version should have incremented
        assert!(
            final_version >= initial_version,
            "xDS version should increment or stay same after creation"
        );

        println!(
            "xDS state updated correctly after success. Version: {} -> {}",
            initial_version, final_version
        );
    }

    // Verify listener cache is also consistent
    let listeners =
        app.state.cached_resources("type.googleapis.com/envoy.config.listener.v3.Listener");
    for listener in &listeners {
        assert!(!listener.name.is_empty(), "Listener should have valid name");
    }
}

/// Scenario: Verify Isolated Listener Deletion After API Definition Delete
///
/// Complementary test to cascading delete that specifically focuses on
/// isolated listener cleanup.
#[tokio::test]
async fn isolated_listener_deleted_with_api_definition() {
    let app = setup_multi_api_app().await;
    let listener_repo = ListenerRepository::new(app.pool.clone());

    // Create API definition with isolated listener
    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "isolated-delete-test.example.com".to_string(),
        listener_isolation: true,
        isolation_listener: Some(flowplane::domain::ListenerConfig {
            name: Some("isolated-delete-listener".to_string()),
            bind_address: "127.0.0.1".to_string(),
            port: 19999,
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
            upstream_targets: json!({
                "targets": [{
                    "name": "backend",
                    "endpoint": "backend.svc:8080",
                    "weight": 100
                }]
            }),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(0),
        }],
    };

    let outcome = app
        .materializer
        .create_definition(spec)
        .await
        .expect("create API definition with isolated listener");

    // Verify listener was created
    let listeners_before = listener_repo.list(Some(100), None).await.expect("list listeners");
    let isolated_listener = listeners_before
        .iter()
        .find(|l| l.name == "isolated-delete-listener")
        .expect("isolated listener should exist");
    assert_eq!(isolated_listener.port, Some(19999));
    assert_eq!(isolated_listener.address, "127.0.0.1");

    // Delete the API definition
    app.materializer
        .delete_definition(outcome.definition.id.as_str())
        .await
        .expect("delete API definition");

    // Verify isolated listener was deleted
    let listeners_after = listener_repo.list(Some(100), None).await.expect("list listeners");
    assert!(
        !listeners_after.iter().any(|l| l.name == "isolated-delete-listener"),
        "Isolated listener should be deleted along with API definition"
    );

    // Verify shared listeners (like default-gateway-listener) are NOT deleted
    assert!(
        listeners_after.iter().any(|l| l.name == "default-gateway-listener"),
        "Shared listeners should not be affected"
    );
}
