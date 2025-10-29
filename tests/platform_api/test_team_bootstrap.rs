use super::support::{read_json, send_request, setup_platform_api_app};
use axum::body::to_bytes;
use axum::http::{Method, StatusCode};

#[tokio::test]
async fn team_bootstrap_returns_yaml_by_default() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("team-bootstrap", &["api-definitions:read"]).await;

    // Request team-scoped bootstrap (no API definition needed!)
    let path = "/api/v1/teams/payments/bootstrap";
    let resp = send_request(&app, Method::GET, path, Some(&token.token), None).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Should be YAML format
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("admin:"), "should have admin config");
    assert!(text.contains("node:"), "should have node config");
    assert!(text.contains("team: payments"), "should have team metadata");
    assert!(text.contains("dynamic_resources:"), "should have dynamic_resources");
}

#[tokio::test]
async fn team_bootstrap_returns_json_when_requested() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("team-bootstrap-json", &["api-definitions:read"]).await;

    // Request JSON format
    let path = "/api/v1/teams/platform/bootstrap?format=json";
    let resp = send_request(&app, Method::GET, path, Some(&token.token), None).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify it's valid JSON with expected structure
    let bootstrap: serde_json::Value = read_json(resp).await;
    assert!(bootstrap.get("admin").is_some(), "should have admin config");
    assert!(bootstrap.get("node").is_some(), "should have node config");
    assert!(bootstrap.get("dynamic_resources").is_some(), "should have dynamic_resources");
    assert!(
        bootstrap.get("static_resources").is_some(),
        "should have static_resources with xds_cluster"
    );

    // Verify node metadata contains team
    let node = bootstrap.get("node").unwrap();
    let metadata = node.get("metadata").unwrap();
    assert_eq!(
        metadata.get("team").unwrap().as_str().unwrap(),
        "platform",
        "team metadata should match requested team"
    );
    assert_eq!(
        metadata.get("include_default").unwrap().as_bool().unwrap(),
        false,
        "include_default should be false by default"
    );
}

#[tokio::test]
async fn team_bootstrap_respects_include_default_parameter() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("team-bootstrap-defaults", &["api-definitions:read"]).await;

    // Request with include_default=true
    let path = "/api/v1/teams/engineering/bootstrap?format=json&include_default=true";
    let resp = send_request(&app, Method::GET, path, Some(&token.token), None).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let bootstrap: serde_json::Value = read_json(resp).await;
    let node = bootstrap.get("node").unwrap();
    let metadata = node.get("metadata").unwrap();

    assert_eq!(
        metadata.get("include_default").unwrap().as_bool().unwrap(),
        true,
        "include_default should be true when specified"
    );
}

#[tokio::test]
async fn team_bootstrap_generates_unique_node_ids() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("team-bootstrap-node-id", &["api-definitions:read"]).await;

    let path = "/api/v1/teams/testing/bootstrap?format=json";

    // Make two requests
    let resp1 = send_request(&app, Method::GET, path, Some(&token.token), None).await;
    let bootstrap1: serde_json::Value = read_json(resp1).await;
    let node_id1 = bootstrap1["node"]["id"].as_str().unwrap();

    let resp2 = send_request(&app, Method::GET, path, Some(&token.token), None).await;
    let bootstrap2: serde_json::Value = read_json(resp2).await;
    let node_id2 = bootstrap2["node"]["id"].as_str().unwrap();

    // Node IDs should be different (they include UUIDs)
    assert_ne!(node_id1, node_id2, "each bootstrap should get unique node ID");

    // But both should start with the team prefix
    assert!(node_id1.starts_with("team=testing/"), "node ID should start with team=testing/");
    assert!(node_id2.starts_with("team=testing/"), "node ID should start with team=testing/");
}

#[tokio::test]
async fn team_bootstrap_includes_xds_cluster_configuration() {
    let app = setup_platform_api_app().await;
    let token = app.issue_token("team-bootstrap-xds", &["api-definitions:read"]).await;

    let path = "/api/v1/teams/infrastructure/bootstrap?format=json";
    let resp = send_request(&app, Method::GET, path, Some(&token.token), None).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let bootstrap: serde_json::Value = read_json(resp).await;

    // Verify static resources include xds_cluster
    let static_resources = bootstrap.get("static_resources").unwrap();
    let clusters = static_resources.get("clusters").unwrap().as_array().unwrap();

    assert!(!clusters.is_empty(), "should have at least xds_cluster in static resources");

    let xds_cluster = &clusters[0];
    assert_eq!(
        xds_cluster.get("name").unwrap().as_str().unwrap(),
        "xds_cluster",
        "first cluster should be xds_cluster"
    );
    assert_eq!(
        xds_cluster.get("type").unwrap().as_str().unwrap(),
        "LOGICAL_DNS",
        "xds_cluster should use LOGICAL_DNS"
    );

    // Verify ADS configuration points to xds_cluster
    let dynamic_resources = bootstrap.get("dynamic_resources").unwrap();
    let ads_config = dynamic_resources.get("ads_config").unwrap();
    let grpc_services = ads_config.get("grpc_services").unwrap().as_array().unwrap();
    let envoy_grpc = &grpc_services[0].get("envoy_grpc").unwrap();

    assert_eq!(
        envoy_grpc.get("cluster_name").unwrap().as_str().unwrap(),
        "xds_cluster",
        "ADS should reference xds_cluster"
    );
}

// TODO: Add team isolation tests once team-scoped token creation is available
// These tests would verify that tokens scoped to team A cannot access team B's bootstrap
