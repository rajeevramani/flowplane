//! # Repository Pattern for Data Access
//!
//! This module has been refactored into focused repository modules under `repositories/`.
//! All types and implementations are re-exported here for backward compatibility.

pub use crate::storage::repositories::{
    // Audit Log repository
    AuditEvent,
    AuditLogRepository,
    // Cluster repository
    ClusterData,
    ClusterRepository,
    CreateClusterRequest,
    // Listener repository
    CreateListenerRequest,
    // Route Config repository
    CreateRouteConfigRequest,
    ListenerData,
    ListenerRepository,
    RouteConfigData,
    RouteConfigRepository,
    // Token repository
    SqlxTokenRepository,
    TokenRepository,
    UpdateClusterRequest,
    UpdateListenerRequest,
    UpdateRouteConfigRequest,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::TestDatabase;

    #[tokio::test]
    async fn test_cluster_crud_operations() {
        let _db = TestDatabase::new("repository_cluster_crud").await;
        let pool = _db.pool.clone();
        let repo = ClusterRepository::new(pool);

        // Create a test cluster
        let create_request = CreateClusterRequest {
            name: "test_cluster".to_string(),
            service_name: "test_service".to_string(),
            configuration: serde_json::json!({
                "type": "EDS",
                "endpoints": ["127.0.0.1:8080"]
            }),
            team: None,
            import_id: None,
        };

        let created = repo.create(create_request).await.unwrap();
        assert_eq!(created.name, "test_cluster");
        assert_eq!(created.service_name, "test_service");
        assert_eq!(created.version, 1);

        // Get by ID
        let fetched = repo.get_by_id(&created.id).await.unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.name, created.name);

        // Get by name
        let fetched_by_name = repo.get_by_name("test_cluster").await.unwrap();
        assert_eq!(fetched_by_name.id, created.id);

        // Update cluster
        let update_request = UpdateClusterRequest {
            service_name: Some("updated_service".to_string()),
            configuration: Some(serde_json::json!({
                "type": "EDS",
                "endpoints": ["127.0.0.1:9090"]
            })),
            team: None,
        };

        let updated = repo.update(&created.id, update_request).await.unwrap();
        assert_eq!(updated.service_name, "updated_service");
        assert_eq!(updated.version, 2);

        // List clusters (includes seed clusters from TestDatabase)
        let clusters = repo.list(None, None).await.unwrap();
        assert!(clusters.iter().any(|c| c.id == created.id));

        // Check existence
        assert!(repo.exists_by_name("test_cluster").await.unwrap());
        assert!(!repo.exists_by_name("nonexistent").await.unwrap());

        // Get count (includes seed clusters)
        let count_before_delete = repo.count().await.unwrap();

        // Delete cluster
        repo.delete(&created.id).await.unwrap();

        // Verify deletion
        assert!(repo.get_by_id(&created.id).await.is_err());
        let count_after_delete = repo.count().await.unwrap();
        assert_eq!(count_after_delete, count_before_delete - 1);
    }

    #[tokio::test]
    async fn test_cluster_not_found() {
        let _db = TestDatabase::new("repository_cluster_not_found").await;
        let pool = _db.pool.clone();
        let repo = ClusterRepository::new(pool);

        let result =
            repo.get_by_id(&crate::domain::ClusterId::from_str_unchecked("nonexistent-id")).await;
        assert!(result.is_err());

        let result = repo.get_by_name("nonexistent-name").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_route_config_crud_operations() {
        let _db = TestDatabase::new("repository_route_config_crud").await;
        let pool = _db.pool.clone();
        let repo = RouteConfigRepository::new(pool.clone());

        let create_request = CreateRouteConfigRequest {
            name: "test_route_config".to_string(),
            path_prefix: "/api".to_string(),
            cluster_name: "cluster-a".to_string(),
            configuration: serde_json::json!({
                "name": "test_route_config",
                "virtualHosts": [
                    {
                        "name": "default",
                        "domains": ["*"],
                        "routes": [
                            {
                                "name": "api",
                                "match": {
                                    "path": { "Prefix": "/api" }
                                },
                                "action": {
                                    "Cluster": {
                                        "name": "cluster-a"
                                    }
                                }
                            }
                        ]
                    }
                ]
            }),
            team: None,
            import_id: None,
            route_order: None,
            headers: None,
        };

        let created = repo.create(create_request).await.unwrap();
        assert_eq!(created.name, "test_route_config");
        assert_eq!(created.version, 1);

        let fetched = repo.get_by_id(&created.id).await.unwrap();
        assert_eq!(fetched.id, created.id);

        let fetched_by_name = repo.get_by_name("test_route_config").await.unwrap();
        assert_eq!(fetched_by_name.id, created.id);

        let update_request = UpdateRouteConfigRequest {
            path_prefix: Some("/api/v2".to_string()),
            cluster_name: Some("cluster-b".to_string()),
            configuration: Some(serde_json::json!({
                "name": "test_route_config",
                "virtualHosts": [
                    {
                        "name": "default",
                        "domains": ["*"],
                        "routes": [
                            {
                                "name": "api",
                                "match": {
                                    "path": { "Prefix": "/api/v2" }
                                },
                                "action": {
                                    "WeightedClusters": {
                                        "clusters": [
                                            {"name": "cluster-b", "weight": 70},
                                            {"name": "cluster-c", "weight": 30}
                                        ]
                                    }
                                }
                            }
                        ]
                    }
                ]
            })),
            team: None,
        };

        let updated = repo.update(&created.id, update_request).await.unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(updated.path_prefix, "/api/v2");
        assert_eq!(updated.cluster_name, "cluster-b");

        let listed = repo.list(None, None).await.unwrap();
        assert!(listed.iter().any(|rc| rc.id == created.id));

        repo.delete(&created.id).await.unwrap();
        assert!(repo.get_by_id(&created.id).await.is_err());
    }

    #[tokio::test]
    async fn test_listener_crud_operations() {
        let _db = TestDatabase::new("repository_listener_crud").await;
        let pool = _db.pool.clone();
        let repo = ListenerRepository::new(pool);

        let create_request = CreateListenerRequest {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(8080),
            protocol: Some("HTTP".to_string()),
            configuration: serde_json::json!({
                "name": "test-listener",
                "address": "0.0.0.0",
                "port": 8080
            }),
            team: None,
            import_id: None,
            dataplane_id: None,
        };

        let created = repo.create(create_request).await.unwrap();
        assert_eq!(created.name, "test-listener");
        assert_eq!(created.port, Some(8080));
        assert_eq!(created.protocol, "HTTP");
        assert_eq!(created.version, 1);

        let fetched = repo.get_by_id(&created.id).await.unwrap();
        assert_eq!(fetched.id, created.id);

        let fetched_by_name = repo.get_by_name("test-listener").await.unwrap();
        assert_eq!(fetched_by_name.id, created.id);

        let update_request = UpdateListenerRequest {
            address: Some("127.0.0.1".to_string()),
            port: Some(Some(9090)),
            protocol: Some("TCP".to_string()),
            configuration: Some(serde_json::json!({
                "name": "test-listener",
                "address": "127.0.0.1",
                "port": 9090
            })),
            team: None,
            dataplane_id: None,
        };

        let updated = repo.update(&created.id, update_request).await.unwrap();
        assert_eq!(updated.address, "127.0.0.1");
        assert_eq!(updated.port, Some(9090));
        assert_eq!(updated.protocol, "TCP");
        assert_eq!(updated.version, 2);

        let listeners = repo.list(None, None).await.unwrap();
        assert!(listeners.iter().any(|l| l.id == created.id));

        assert!(repo.exists_by_name("test-listener").await.unwrap());
        assert!(!repo.exists_by_name("missing").await.unwrap());

        let count_before = repo.count().await.unwrap();
        repo.delete(&created.id).await.unwrap();
        assert!(repo.get_by_id(&created.id).await.is_err());
        assert_eq!(repo.count().await.unwrap(), count_before - 1);
    }
}
