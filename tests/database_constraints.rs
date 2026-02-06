// NOTE: This file requires PostgreSQL - disabled until Phase 4 of PostgreSQL migration
// To run these tests: cargo test --features postgres_tests
#![cfg(feature = "postgres_tests")]

//! Integration tests for database schema constraints and referential integrity
//!
//! Tests FK constraints, cascading deletes, and source enum validation

mod common;

use common::test_db::TestDatabase;
use flowplane::auth::team::CreateTeamRequest;
use flowplane::storage::repositories::team::{SqlxTeamRepository, TeamRepository};
use flowplane::storage::repositories::{CreateImportMetadataRequest, ImportMetadataRepository};
use flowplane::storage::repository::{
    ClusterRepository, CreateClusterRequest, CreateListenerRequest, CreateRouteConfigRequest,
    ListenerRepository, RouteConfigRepository,
};
use flowplane::storage::DbPool;

async fn create_test_pool() -> TestDatabase {
    TestDatabase::new("database_constraints").await
}

async fn create_test_team(pool: &DbPool, team_name: &str) {
    let team_repo = SqlxTeamRepository::new(pool.clone());
    let _ = team_repo
        .create_team(CreateTeamRequest {
            name: team_name.to_string(),
            display_name: format!("Test Team {}", team_name),
            description: Some("Test team for integration tests".to_string()),
            owner_user_id: None,
            org_id: None,
            settings: None,
        })
        .await;
}

#[tokio::test]
async fn test_source_enum_constraint_on_listeners() {
    let test_db = create_test_pool().await;
    let pool = &test_db.pool;

    // Try to insert with invalid source value (should fail)
    let result = sqlx::query(
        "INSERT INTO listeners (id, name, address, port, protocol, configuration, version, source, created_at, updated_at)
         VALUES ('test-id', 'test', '0.0.0.0', 8080, 'HTTP', '{}', 1, 'invalid_source', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"
    )
    .execute(pool)
    .await;

    assert!(result.is_err(), "Should fail with invalid source value");
}

#[tokio::test]
async fn test_source_enum_constraint_on_routes() {
    let test_db = create_test_pool().await;
    let pool = &test_db.pool;
    create_test_team(pool, "test").await;

    // Create a cluster first (required by FK constraint)
    let cluster_repo = ClusterRepository::new(pool.clone());
    cluster_repo
        .create(CreateClusterRequest {
            name: "test-cluster".to_string(),
            service_name: "test-service".to_string(),
            configuration: serde_json::json!({"type": "EDS"}),
            team: Some("test".into()),
            import_id: None,
        })
        .await
        .unwrap();

    // Try to insert route config with invalid source value (should fail)
    let result = sqlx::query(
        "INSERT INTO route_configs (id, name, path_prefix, cluster_name, configuration, version, source, created_at, updated_at)
         VALUES ('test-id', 'test', '/', 'test-cluster', '{}', 1, 'bad_source', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"
    )
    .execute(pool)
    .await;

    assert!(result.is_err(), "Should fail with invalid source value");
}

#[tokio::test]
async fn test_source_enum_constraint_on_clusters() {
    let test_db = create_test_pool().await;
    let pool = &test_db.pool;

    // Try to insert with invalid source value (should fail)
    let result = sqlx::query(
        "INSERT INTO clusters (id, name, service_name, configuration, version, source, created_at, updated_at)
         VALUES ('test-id', 'test', 'test-svc', '{}', 1, 'wrong_source', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"
    )
    .execute(pool)
    .await;

    assert!(result.is_err(), "Should fail with invalid source value");
}

#[tokio::test]
async fn test_valid_source_values_accepted() {
    let test_db = create_test_pool().await;
    let pool = &test_db.pool;
    create_test_team(pool, "test").await;

    let listener_repo = ListenerRepository::new(pool.clone());
    let cluster_repo = ClusterRepository::new(pool.clone());
    let route_config_repo = RouteConfigRepository::new(pool.clone());

    // Create resources with valid 'native_api' source (default)
    let listener = listener_repo
        .create(CreateListenerRequest {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(8080),
            protocol: Some("HTTP".to_string()),
            configuration: serde_json::json!({"name": "test"}),
            team: Some("test".into()),
            import_id: None,
            dataplane_id: None,
        })
        .await
        .unwrap();

    assert_eq!(listener.source, "native_api");

    let cluster = cluster_repo
        .create(CreateClusterRequest {
            name: "test-cluster".to_string(),
            service_name: "test-service".to_string(),
            configuration: serde_json::json!({"type": "EDS"}),
            team: Some("test".into()),
            import_id: None,
        })
        .await
        .unwrap();

    assert_eq!(cluster.source, "native_api");

    let route_config = route_config_repo
        .create(CreateRouteConfigRequest {
            name: "test-route".to_string(),
            path_prefix: "/".to_string(),
            cluster_name: "test-cluster".to_string(),
            configuration: serde_json::json!({"name": "test"}),
            team: Some("test".into()),
            import_id: None,
            route_order: None,
            headers: None,
        })
        .await
        .unwrap();

    assert_eq!(route_config.source, "native_api");
}

// ============================================================================
// Import ID Column Tests (Phase 7 - Listener/Import Linkage Fix)
// ============================================================================

#[tokio::test]
async fn test_listener_created_with_import_id_stores_in_column() {
    let test_db = create_test_pool().await;
    let pool = &test_db.pool;
    create_test_team(pool, "test").await;

    // Create import metadata first
    let import_repo = ImportMetadataRepository::new(pool.clone());
    let import = import_repo
        .create(CreateImportMetadataRequest {
            spec_name: "test-api".to_string(),
            spec_version: Some("1.0.0".to_string()),
            spec_checksum: None,
            team: "test".to_string(),
            source_content: None,
            listener_name: Some("imported-listener".to_string()),
        })
        .await
        .unwrap();

    // Create listener with import_id
    let listener_repo = ListenerRepository::new(pool.clone());
    let listener = listener_repo
        .create(CreateListenerRequest {
            name: "imported-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(8080),
            protocol: Some("HTTP".to_string()),
            configuration: serde_json::json!({"name": "imported-listener"}),
            team: Some("test".into()),
            import_id: Some(import.id.clone()),
            dataplane_id: None,
        })
        .await
        .unwrap();

    // Verify import_id is stored in the column, not just JSON
    assert_eq!(listener.import_id, Some(import.id.clone()));

    // Verify we can retrieve the listener and import_id is preserved
    let retrieved = listener_repo.get_by_name("imported-listener").await.unwrap();
    assert_eq!(retrieved.import_id, Some(import.id));
}

#[tokio::test]
async fn test_listener_without_import_id_has_none() {
    let test_db = create_test_pool().await;
    let pool = &test_db.pool;
    create_test_team(pool, "test").await;

    let listener_repo = ListenerRepository::new(pool.clone());
    let listener = listener_repo
        .create(CreateListenerRequest {
            name: "native-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(8081),
            protocol: Some("HTTP".to_string()),
            configuration: serde_json::json!({"name": "native-listener"}),
            team: Some("test".into()),
            import_id: None,
            dataplane_id: None,
        })
        .await
        .unwrap();

    assert_eq!(listener.import_id, None);
}

#[tokio::test]
async fn test_count_by_import_uses_column_not_json() {
    let test_db = create_test_pool().await;
    let pool = &test_db.pool;
    create_test_team(pool, "test").await;

    // Create two imports
    let import_repo = ImportMetadataRepository::new(pool.clone());
    let import1 = import_repo
        .create(CreateImportMetadataRequest {
            spec_name: "api-one".to_string(),
            spec_version: Some("1.0.0".to_string()),
            spec_checksum: None,
            team: "test".to_string(),
            source_content: None,
            listener_name: Some("import1-listener".to_string()),
        })
        .await
        .unwrap();

    let import2 = import_repo
        .create(CreateImportMetadataRequest {
            spec_name: "api-two".to_string(),
            spec_version: Some("2.0.0".to_string()),
            spec_checksum: None,
            team: "test".to_string(),
            source_content: None,
            listener_name: Some("import2-listener".to_string()),
        })
        .await
        .unwrap();

    let listener_repo = ListenerRepository::new(pool.clone());

    // Create 2 listeners for import1
    for i in 0..2 {
        listener_repo
            .create(CreateListenerRequest {
                name: format!("import1-listener-{}", i),
                address: "0.0.0.0".to_string(),
                port: Some(8080 + i),
                protocol: Some("HTTP".to_string()),
                configuration: serde_json::json!({"name": format!("listener-{}", i)}),
                team: Some("test".into()),
                import_id: Some(import1.id.clone()),
                dataplane_id: None,
            })
            .await
            .unwrap();
    }

    // Create 3 listeners for import2
    for i in 0..3 {
        listener_repo
            .create(CreateListenerRequest {
                name: format!("import2-listener-{}", i),
                address: "0.0.0.0".to_string(),
                port: Some(9080 + i),
                protocol: Some("HTTP".to_string()),
                configuration: serde_json::json!({"name": format!("listener-{}", i)}),
                team: Some("test".into()),
                import_id: Some(import2.id.clone()),
                dataplane_id: None,
            })
            .await
            .unwrap();
    }

    // Verify counts are correct
    let count1 = listener_repo.count_by_import(&import1.id).await.unwrap();
    let count2 = listener_repo.count_by_import(&import2.id).await.unwrap();

    assert_eq!(count1, 2, "Import 1 should have 2 listeners");
    assert_eq!(count2, 3, "Import 2 should have 3 listeners");
}

#[tokio::test]
async fn test_cascade_delete_removes_listeners_when_import_deleted() {
    let test_db = create_test_pool().await;
    let pool = &test_db.pool;
    create_test_team(pool, "test").await;

    // Create import metadata
    let import_repo = ImportMetadataRepository::new(pool.clone());
    let import = import_repo
        .create(CreateImportMetadataRequest {
            spec_name: "cascade-test-api".to_string(),
            spec_version: Some("1.0.0".to_string()),
            spec_checksum: None,
            team: "test".to_string(),
            source_content: None,
            listener_name: Some("cascade-listener".to_string()),
        })
        .await
        .unwrap();

    let listener_repo = ListenerRepository::new(pool.clone());

    // Create listeners linked to the import
    for i in 0..3 {
        listener_repo
            .create(CreateListenerRequest {
                name: format!("cascade-listener-{}", i),
                address: "0.0.0.0".to_string(),
                port: Some(8080 + i),
                protocol: Some("HTTP".to_string()),
                configuration: serde_json::json!({"name": format!("listener-{}", i)}),
                team: Some("test".into()),
                import_id: Some(import.id.clone()),
                dataplane_id: None,
            })
            .await
            .unwrap();
    }

    // Verify listeners exist
    let count_before = listener_repo.count_by_import(&import.id).await.unwrap();
    assert_eq!(count_before, 3, "Should have 3 listeners before delete");

    // Delete the import - CASCADE should remove linked listeners
    import_repo.delete(&import.id).await.unwrap();

    // Verify listeners are gone
    let count_after = listener_repo.count_by_import(&import.id).await.unwrap();
    assert_eq!(count_after, 0, "All listeners should be cascade deleted");

    // Double-check by trying to fetch individual listeners - should return error (not found)
    let listener0_result = listener_repo.get_by_name("cascade-listener-0").await;
    assert!(listener0_result.is_err(), "Listener should not exist after cascade delete");
}

#[tokio::test]
async fn test_cascade_delete_does_not_affect_unlinked_listeners() {
    let test_db = create_test_pool().await;
    let pool = &test_db.pool;
    create_test_team(pool, "test").await;

    // Create import metadata
    let import_repo = ImportMetadataRepository::new(pool.clone());
    let import = import_repo
        .create(CreateImportMetadataRequest {
            spec_name: "isolated-test-api".to_string(),
            spec_version: Some("1.0.0".to_string()),
            spec_checksum: None,
            team: "test".to_string(),
            source_content: None,
            listener_name: Some("linked-listener".to_string()),
        })
        .await
        .unwrap();

    let listener_repo = ListenerRepository::new(pool.clone());

    // Create a linked listener
    listener_repo
        .create(CreateListenerRequest {
            name: "linked-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(8080),
            protocol: Some("HTTP".to_string()),
            configuration: serde_json::json!({"name": "linked"}),
            team: Some("test".into()),
            import_id: Some(import.id.clone()),
            dataplane_id: None,
        })
        .await
        .unwrap();

    // Create an unlinked (native) listener
    listener_repo
        .create(CreateListenerRequest {
            name: "unlinked-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(8081),
            protocol: Some("HTTP".to_string()),
            configuration: serde_json::json!({"name": "unlinked"}),
            team: Some("test".into()),
            import_id: None,
            dataplane_id: None,
        })
        .await
        .unwrap();

    // Delete the import
    import_repo.delete(&import.id).await.unwrap();

    // Linked listener should be gone (returns error if not found)
    let linked_result = listener_repo.get_by_name("linked-listener").await;
    assert!(linked_result.is_err(), "Linked listener should be cascade deleted");

    // Unlinked listener should still exist (returns Ok if found)
    let unlinked_result = listener_repo.get_by_name("unlinked-listener").await;
    assert!(unlinked_result.is_ok(), "Unlinked listener should not be affected");
}
