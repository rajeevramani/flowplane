//! Integration tests for database-level team filtering in repositories
//!
//! These tests verify that team-scoped RBAC filtering works correctly at the database
//! level, ensuring proper data isolation between teams.

use flowplane::storage::{
    ClusterRepository, CreateClusterRequest, CreateListenerRequest,
    CreateRouteRepositoryRequest, DbPool, ListenerRepository, RouteRepository,
};
use sqlx::sqlite::SqlitePoolOptions;

/// Set up an in-memory SQLite database with migrations applied
async fn setup_pool() -> DbPool {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("in-memory sqlite");

    // Run migrations to create tables with team columns
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrations should apply");

    pool
}

#[tokio::test]
async fn cluster_repository_filters_by_team() {
    let pool = setup_pool().await;
    let repo = ClusterRepository::new(pool.clone());

    // Create clusters for different teams
    let team_a_cluster = CreateClusterRequest {
        name: "team-a-cluster".to_string(),
        service_name: "team-a-service".to_string(),
        configuration: serde_json::json!({
            "endpoints": [{"Address": {"host": "127.0.0.1", "port": 8080}}],
            "connect_timeout_seconds": 5
        }),
        team: Some("team-a".to_string()),
    };

    let team_b_cluster = CreateClusterRequest {
        name: "team-b-cluster".to_string(),
        service_name: "team-b-service".to_string(),
        configuration: serde_json::json!({
            "endpoints": [{"Address": {"host": "127.0.0.1", "port": 8081}}],
            "connect_timeout_seconds": 5
        }),
        team: Some("team-b".to_string()),
    };

    let global_cluster = CreateClusterRequest {
        name: "global-cluster".to_string(),
        service_name: "global-service".to_string(),
        configuration: serde_json::json!({
            "endpoints": [{"Address": {"host": "127.0.0.1", "port": 8082}}],
            "connect_timeout_seconds": 5
        }),
        team: None, // Global cluster with NULL team
    };

    repo.create(team_a_cluster).await.unwrap();
    repo.create(team_b_cluster).await.unwrap();
    repo.create(global_cluster).await.unwrap();

    // Test: Team A should see only their cluster + global cluster
    let team_a_results = repo
        .list_by_teams(&["team-a".to_string()], None, None)
        .await
        .unwrap();
    assert_eq!(team_a_results.len(), 2);
    let names: Vec<&str> = team_a_results.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"team-a-cluster"));
    assert!(names.contains(&"global-cluster"));
    assert!(!names.contains(&"team-b-cluster"));

    // Test: Team B should see only their cluster + global cluster
    let team_b_results = repo
        .list_by_teams(&["team-b".to_string()], None, None)
        .await
        .unwrap();
    assert_eq!(team_b_results.len(), 2);
    let names: Vec<&str> = team_b_results.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"team-b-cluster"));
    assert!(names.contains(&"global-cluster"));
    assert!(!names.contains(&"team-a-cluster"));

    // Test: Empty teams list (admin:all) should see all clusters
    let admin_results = repo.list_by_teams(&[], None, None).await.unwrap();
    assert_eq!(admin_results.len(), 3);
    let names: Vec<&str> = admin_results.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"team-a-cluster"));
    assert!(names.contains(&"team-b-cluster"));
    assert!(names.contains(&"global-cluster"));

    // Test: Multiple teams should see resources from all specified teams + global
    let multi_team_results = repo
        .list_by_teams(&["team-a".to_string(), "team-b".to_string()], None, None)
        .await
        .unwrap();
    assert_eq!(multi_team_results.len(), 3);
}

#[tokio::test]
async fn route_repository_filters_by_team() {
    let pool = setup_pool().await;
    let route_repo = RouteRepository::new(pool.clone());
    let cluster_repo = ClusterRepository::new(pool.clone());

    // Create clusters first (foreign key dependency)
    cluster_repo
        .create(CreateClusterRequest {
            name: "team-a-cluster".to_string(),
            service_name: "team-a-service".to_string(),
            configuration: serde_json::json!({
                "endpoints": [{"Address": {"host": "127.0.0.1", "port": 8080}}],
                "connect_timeout_seconds": 5
            }),
            team: Some("team-a".to_string()),
        })
        .await
        .unwrap();

    cluster_repo
        .create(CreateClusterRequest {
            name: "team-b-cluster".to_string(),
            service_name: "team-b-service".to_string(),
            configuration: serde_json::json!({
                "endpoints": [{"Address": {"host": "127.0.0.1", "port": 8081}}],
                "connect_timeout_seconds": 5
            }),
            team: Some("team-b".to_string()),
        })
        .await
        .unwrap();

    cluster_repo
        .create(CreateClusterRequest {
            name: "global-cluster".to_string(),
            service_name: "global-service".to_string(),
            configuration: serde_json::json!({
                "endpoints": [{"Address": {"host": "127.0.0.1", "port": 8082}}],
                "connect_timeout_seconds": 5
            }),
            team: None,
        })
        .await
        .unwrap();

    // Create routes for different teams
    let team_a_route = CreateRouteRepositoryRequest {
        name: "team-a-routes".to_string(),
        path_prefix: "/team-a".to_string(),
        cluster_name: "team-a-cluster".to_string(),
        configuration: serde_json::json!({
            "name": "team-a-routes",
            "virtual_hosts": []
        }),
        team: Some("team-a".to_string()),
    };

    let team_b_route = CreateRouteRepositoryRequest {
        name: "team-b-routes".to_string(),
        path_prefix: "/team-b".to_string(),
        cluster_name: "team-b-cluster".to_string(),
        configuration: serde_json::json!({
            "name": "team-b-routes",
            "virtual_hosts": []
        }),
        team: Some("team-b".to_string()),
    };

    let global_route = CreateRouteRepositoryRequest {
        name: "global-routes".to_string(),
        path_prefix: "/global".to_string(),
        cluster_name: "global-cluster".to_string(),
        configuration: serde_json::json!({
            "name": "global-routes",
            "virtual_hosts": []
        }),
        team: None,
    };

    route_repo.create(team_a_route).await.unwrap();
    route_repo.create(team_b_route).await.unwrap();
    route_repo.create(global_route).await.unwrap();

    // Test: Team A should see only their route + global route
    let team_a_results = route_repo
        .list_by_teams(&["team-a".to_string()], None, None)
        .await
        .unwrap();
    assert_eq!(team_a_results.len(), 2);
    let names: Vec<&str> = team_a_results.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"team-a-routes"));
    assert!(names.contains(&"global-routes"));
    assert!(!names.contains(&"team-b-routes"));

    // Test: Team B should see only their route + global route
    let team_b_results = route_repo
        .list_by_teams(&["team-b".to_string()], None, None)
        .await
        .unwrap();
    assert_eq!(team_b_results.len(), 2);
    let names: Vec<&str> = team_b_results.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"team-b-routes"));
    assert!(names.contains(&"global-routes"));
    assert!(!names.contains(&"team-a-routes"));

    // Test: Empty teams list (admin:all) should see all routes
    let admin_results = route_repo.list_by_teams(&[], None, None).await.unwrap();
    assert_eq!(admin_results.len(), 3);

    // Test: Multiple teams should see resources from all specified teams + global
    let multi_team_results = route_repo
        .list_by_teams(&["team-a".to_string(), "team-b".to_string()], None, None)
        .await
        .unwrap();
    assert_eq!(multi_team_results.len(), 3);
}

#[tokio::test]
async fn listener_repository_filters_by_team() {
    let pool = setup_pool().await;
    let repo = ListenerRepository::new(pool.clone());

    // Create listeners for different teams
    let team_a_listener = CreateListenerRequest {
        name: "team-a-listener".to_string(),
        address: "0.0.0.0".to_string(),
        port: Some(8080),
        protocol: Some("HTTP".to_string()),
        configuration: serde_json::json!({
            "name": "team-a-listener",
            "address": "0.0.0.0",
            "port": 8080,
            "filter_chains": []
        }),
        team: Some("team-a".to_string()),
    };

    let team_b_listener = CreateListenerRequest {
        name: "team-b-listener".to_string(),
        address: "0.0.0.0".to_string(),
        port: Some(8081),
        protocol: Some("HTTP".to_string()),
        configuration: serde_json::json!({
            "name": "team-b-listener",
            "address": "0.0.0.0",
            "port": 8081,
            "filter_chains": []
        }),
        team: Some("team-b".to_string()),
    };

    let global_listener = CreateListenerRequest {
        name: "global-listener".to_string(),
        address: "0.0.0.0".to_string(),
        port: Some(8082),
        protocol: Some("HTTP".to_string()),
        configuration: serde_json::json!({
            "name": "global-listener",
            "address": "0.0.0.0",
            "port": 8082,
            "filter_chains": []
        }),
        team: None,
    };

    repo.create(team_a_listener).await.unwrap();
    repo.create(team_b_listener).await.unwrap();
    repo.create(global_listener).await.unwrap();

    // Test: Team A should see only their listener + global listener
    let team_a_results = repo
        .list_by_teams(&["team-a".to_string()], None, None)
        .await
        .unwrap();
    assert_eq!(team_a_results.len(), 2);
    let names: Vec<&str> = team_a_results.iter().map(|l| l.name.as_str()).collect();
    assert!(names.contains(&"team-a-listener"));
    assert!(names.contains(&"global-listener"));
    assert!(!names.contains(&"team-b-listener"));

    // Test: Team B should see only their listener + global listener
    let team_b_results = repo
        .list_by_teams(&["team-b".to_string()], None, None)
        .await
        .unwrap();
    assert_eq!(team_b_results.len(), 2);
    let names: Vec<&str> = team_b_results.iter().map(|l| l.name.as_str()).collect();
    assert!(names.contains(&"team-b-listener"));
    assert!(names.contains(&"global-listener"));
    assert!(!names.contains(&"team-a-listener"));

    // Test: Empty teams list (admin:all) should see all listeners
    let admin_results = repo.list_by_teams(&[], None, None).await.unwrap();
    assert_eq!(admin_results.len(), 3);

    // Test: Multiple teams should see resources from all specified teams + global
    let multi_team_results = repo
        .list_by_teams(&["team-a".to_string(), "team-b".to_string()], None, None)
        .await
        .unwrap();
    assert_eq!(multi_team_results.len(), 3);
}

#[tokio::test]
async fn team_filtering_respects_pagination() {
    let pool = setup_pool().await;
    let repo = ClusterRepository::new(pool.clone());

    // Create 5 clusters for team-a
    for i in 0..5 {
        let cluster = CreateClusterRequest {
            name: format!("team-a-cluster-{}", i),
            service_name: format!("service-{}", i),
            configuration: serde_json::json!({
                "endpoints": [{"Address": {"host": "127.0.0.1", "port": 8080 + i}}],
                "connect_timeout_seconds": 5
            }),
            team: Some("team-a".to_string()),
        };
        repo.create(cluster).await.unwrap();
    }

    // Test pagination with limit
    let page1 = repo
        .list_by_teams(&["team-a".to_string()], Some(2), Some(0))
        .await
        .unwrap();
    assert_eq!(page1.len(), 2);

    let page2 = repo
        .list_by_teams(&["team-a".to_string()], Some(2), Some(2))
        .await
        .unwrap();
    assert_eq!(page2.len(), 2);

    // Verify we got different clusters
    assert_ne!(page1[0].id, page2[0].id);
}

#[tokio::test]
async fn team_filtering_handles_special_characters_in_team_names() {
    let pool = setup_pool().await;
    let repo = ClusterRepository::new(pool.clone());

    // Create cluster with team name containing special characters
    let cluster = CreateClusterRequest {
        name: "special-team-cluster".to_string(),
        service_name: "special-service".to_string(),
        configuration: serde_json::json!({
            "endpoints": [{"Address": {"host": "127.0.0.1", "port": 8080}}],
            "connect_timeout_seconds": 5
        }),
        team: Some("team-with-dashes_and_underscores".to_string()),
    };

    repo.create(cluster).await.unwrap();

    // Test: Special characters in team name should work correctly
    let results = repo
        .list_by_teams(&["team-with-dashes_and_underscores".to_string()], None, None)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "special-team-cluster");
    assert_eq!(results[0].team.as_deref(), Some("team-with-dashes_and_underscores"));
}
