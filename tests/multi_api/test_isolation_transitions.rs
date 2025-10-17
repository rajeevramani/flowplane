//! Isolation mode transition tests for Platform API
//!
//! These tests validate that isolation boundaries are maintained throughout
//! the API definition lifecycle. Since isolation mode is immutable after creation,
//! these tests verify that:
//! 1. Isolation mode cannot be changed after creation
//! 2. Route updates respect the original isolation mode
//! 3. Deletion and recreation with different mode works correctly

use flowplane::{
    domain::api_definition::{ApiDefinitionSpec, RouteConfig as RouteSpec},
    storage::repository::ListenerRepository,
};
use serde_json::json;

use super::support::setup_multi_api_app;

/// Scenario: Isolated Mode Preserves Boundaries During Route Updates
///
/// Test that when an API definition with listener_isolation=true has its routes
/// updated, it continues to use the isolated listener and doesn't leak into shared.
#[tokio::test]
async fn isolated_mode_preserves_boundaries_on_route_update() {
    let app = setup_multi_api_app().await;
    let listener_repo = ListenerRepository::new(app.pool.clone());

    // Create API definition with listener_isolation=true
    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "isolated-update-test.example.com".to_string(),
        listener_isolation: true,
        isolation_listener: Some(flowplane::domain::ListenerConfig {
            name: Some("isolated-update-listener".to_string()),
            bind_address: "127.0.0.1".to_string(),
            port: 20000,
            protocol: "HTTP".to_string(),
            tls_config: None,
            http_filters: None,
        }),
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
            upstream_targets: json!({
                "targets": [{
                    "name": "backend-v1",
                    "endpoint": "backend-v1.svc:8080",
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
        .expect("create API definition with isolation");

    let def_id = outcome.definition.id.clone();

    // Verify isolated listener was created
    let listeners_before = listener_repo.list(Some(100), None).await.expect("list listeners");
    let isolated_listener = listeners_before
        .iter()
        .find(|l| l.name == "isolated-update-listener")
        .expect("isolated listener should exist");
    assert_eq!(isolated_listener.port, Some(20000));

    // Update routes (add a second route)
    let updated_routes = vec![
        RouteSpec {
            match_type: "prefix".to_string(),
            match_value: "/api/v1".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: json!({
                "targets": [{
                    "name": "backend-v1",
                    "endpoint": "backend-v1.svc:8080",
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
            match_value: "/api/v2".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: json!({
                "targets": [{
                    "name": "backend-v2",
                    "endpoint": "backend-v2.svc:8080",
                    "weight": 100
                }]
            }),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(1),
        },
    ];

    app.materializer
        .update_definition(def_id.as_str(), updated_routes)
        .await
        .expect("update routes");

    // Verify isolated listener still exists and hasn't changed
    let listeners_after = listener_repo.list(Some(100), None).await.expect("list listeners");
    let isolated_listener_after = listeners_after
        .iter()
        .find(|l| l.name == "isolated-update-listener")
        .expect("isolated listener should still exist");
    assert_eq!(isolated_listener_after.id, isolated_listener.id);
    assert_eq!(isolated_listener_after.port, Some(20000));
    assert_eq!(isolated_listener_after.address, "127.0.0.1");

    // Verify routes are still associated with the isolated listener
    // (by checking that shared listeners don't have this domain)
    let shared_listener = listeners_after
        .iter()
        .find(|l| l.name == "default-gateway-listener")
        .expect("shared listener should exist");

    // Parse shared listener config to verify it doesn't contain our domain
    let shared_config: serde_json::Value =
        serde_json::from_str(&shared_listener.configuration).expect("parse listener config");
    let config_str = shared_config.to_string();
    assert!(
        !config_str.contains("isolated-update-test.example.com"),
        "Shared listener should not contain isolated API domain"
    );
}

/// Scenario: Shared Mode Preserves Boundaries During Route Updates
///
/// Test that when an API definition with listener_isolation=false has its routes
/// updated, it continues to use shared listeners and doesn't create isolated ones.
#[tokio::test]
async fn shared_mode_preserves_boundaries_on_route_update() {
    let app = setup_multi_api_app().await;
    let listener_repo = ListenerRepository::new(app.pool.clone());

    // Count listeners before creating any API definitions
    let listeners_before = listener_repo.list(Some(100), None).await.expect("list listeners");
    let initial_listener_count = listeners_before.len();

    // Create API definition with listener_isolation=false (shared mode)
    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "shared-update-test.example.com".to_string(),
        listener_isolation: false,
        isolation_listener: None,
        target_listeners: None, // Uses default-gateway-listener
        tls_config: None,
        routes: vec![RouteSpec {
            match_type: "prefix".to_string(),
            match_value: "/api/v1".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: json!({
                "targets": [{
                    "name": "backend-v1",
                    "endpoint": "backend-v1.svc:8080",
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
        .expect("create API definition in shared mode");

    let def_id = outcome.definition.id.clone();

    // Verify no new isolated listener was created
    let listeners_after_create = listener_repo.list(Some(100), None).await.expect("list listeners");
    assert_eq!(
        listeners_after_create.len(),
        initial_listener_count,
        "No new listeners should be created in shared mode"
    );

    // Verify API definition was created in shared mode (domain goes into route config, not listener)
    let api_def_repo = flowplane::storage::ApiDefinitionRepository::new(app.pool.clone());
    let api_def =
        api_def_repo.get_definition(&outcome.definition.id).await.expect("get API definition");
    assert!(!api_def.listener_isolation, "Should be in shared mode");
    assert!(api_def.target_listeners.is_none(), "Should use default listeners");

    // Update routes (add a second route)
    let updated_routes = vec![
        RouteSpec {
            match_type: "prefix".to_string(),
            match_value: "/api/v1".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: json!({
                "targets": [{
                    "name": "backend-v1",
                    "endpoint": "backend-v1.svc:8080",
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
            match_value: "/api/v2".to_string(),
            case_sensitive: true,
            headers: None,
            rewrite_prefix: None,
            rewrite_regex: None,
            rewrite_substitution: None,
            upstream_targets: json!({
                "targets": [{
                    "name": "backend-v2",
                    "endpoint": "backend-v2.svc:8080",
                    "weight": 100
                }]
            }),
            timeout_seconds: Some(30),
            override_config: None,
            deployment_note: None,
            route_order: Some(1),
        },
    ];

    app.materializer
        .update_definition(def_id.as_str(), updated_routes)
        .await
        .expect("update routes");

    // Verify still no new isolated listeners created
    let listeners_after_update = listener_repo.list(Some(100), None).await.expect("list listeners");
    assert_eq!(
        listeners_after_update.len(),
        initial_listener_count,
        "No new listeners should be created after route update in shared mode"
    );

    // Verify API definition is still in shared mode after route update
    let api_def_after =
        api_def_repo.get_definition(&def_id).await.expect("get API definition after update");
    assert!(!api_def_after.listener_isolation, "Should still be in shared mode");
    assert_eq!(
        api_def.listener_isolation, api_def_after.listener_isolation,
        "Isolation mode should be unchanged after route update"
    );

    // Verify routes were actually updated
    let routes_after = api_def_repo.list_routes(&def_id).await.expect("list routes after update");
    assert_eq!(routes_after.len(), 2, "Should have 2 routes after update");
}

/// Scenario: Delete and Recreate with Different Isolation Mode
///
/// Test that we can delete an API definition and recreate it with a different
/// isolation mode. This validates that isolation mode transitions are possible
/// via delete+recreate workflow.
#[tokio::test]
async fn delete_and_recreate_with_different_isolation_mode() {
    let app = setup_multi_api_app().await;
    let listener_repo = ListenerRepository::new(app.pool.clone());

    // Phase 1: Create with isolation=false (shared mode)
    let spec_shared = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "transition-test.example.com".to_string(),
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

    let outcome1 = app
        .materializer
        .create_definition(spec_shared)
        .await
        .expect("create shared API definition");

    // Verify no isolated listener created
    let listeners_after_shared = listener_repo.list(Some(100), None).await.expect("list listeners");
    assert!(
        !listeners_after_shared.iter().any(|l| l.name.contains("transition-test")),
        "No isolated listener should exist for shared mode"
    );

    // Delete the API definition
    app.materializer
        .delete_definition(outcome1.definition.id.as_str())
        .await
        .expect("delete shared API definition");

    // Phase 2: Recreate with isolation=true (isolated mode)
    let spec_isolated = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "transition-test.example.com".to_string(), // Same domain
        listener_isolation: true,                          // Different isolation mode
        isolation_listener: Some(flowplane::domain::ListenerConfig {
            name: Some("transition-test-listener".to_string()),
            bind_address: "127.0.0.1".to_string(),
            port: 21000,
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

    let outcome2 = app
        .materializer
        .create_definition(spec_isolated)
        .await
        .expect("recreate with isolated mode");

    // Verify isolated listener was created
    let listeners_after_isolated =
        listener_repo.list(Some(100), None).await.expect("list listeners");
    let isolated_listener = listeners_after_isolated
        .iter()
        .find(|l| l.name == "transition-test-listener")
        .expect("isolated listener should exist after recreation");
    assert_eq!(isolated_listener.port, Some(21000));
    assert_eq!(isolated_listener.address, "127.0.0.1");

    // Cleanup
    app.materializer
        .delete_definition(outcome2.definition.id.as_str())
        .await
        .expect("cleanup isolated API definition");

    // Verify cleanup
    let listeners_final = listener_repo.list(Some(100), None).await.expect("list listeners");
    assert!(
        !listeners_final.iter().any(|l| l.name == "transition-test-listener"),
        "Isolated listener should be deleted"
    );
}

/// Scenario: Isolation Mode Field Is Immutable
///
/// Test that the isolation mode field in an API definition cannot be changed
/// after creation. The current implementation only supports updating routes,
/// not the isolation mode itself.
#[tokio::test]
async fn isolation_mode_field_is_immutable() {
    let app = setup_multi_api_app().await;
    let api_def_repo = flowplane::storage::ApiDefinitionRepository::new(app.pool.clone());

    // Create API definition with isolation=true
    let spec = ApiDefinitionSpec {
        team: "test-team".to_string(),
        domain: "immutable-test.example.com".to_string(),
        listener_isolation: true,
        isolation_listener: Some(flowplane::domain::ListenerConfig {
            name: Some("immutable-test-listener".to_string()),
            bind_address: "127.0.0.1".to_string(),
            port: 22000,
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

    let outcome = app.materializer.create_definition(spec).await.expect("create API definition");

    // Verify initial isolation mode
    let def_before =
        api_def_repo.get_definition(&outcome.definition.id).await.expect("get definition");
    assert!(def_before.listener_isolation, "Should be in isolated mode");

    // Update routes (this is all that update_definition supports)
    let updated_routes = vec![RouteSpec {
        match_type: "prefix".to_string(),
        match_value: "/api/v2".to_string(),
        case_sensitive: true,
        headers: None,
        rewrite_prefix: None,
        rewrite_regex: None,
        rewrite_substitution: None,
        upstream_targets: json!({
            "targets": [{
                "name": "backend-v2",
                "endpoint": "backend-v2.svc:8080",
                "weight": 100
            }]
        }),
        timeout_seconds: Some(30),
        override_config: None,
        deployment_note: None,
        route_order: Some(0),
    }];

    app.materializer
        .update_definition(outcome.definition.id.as_str(), updated_routes)
        .await
        .expect("update routes");

    // Verify isolation mode is unchanged
    let def_after = api_def_repo
        .get_definition(&outcome.definition.id)
        .await
        .expect("get definition after update");
    assert!(
        def_after.listener_isolation,
        "Isolation mode should remain unchanged after route update"
    );
    assert_eq!(
        def_before.listener_isolation, def_after.listener_isolation,
        "Isolation mode must be immutable"
    );
}
