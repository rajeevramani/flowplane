// NOTE: Requires PostgreSQL - disabled until Phase 4
#![cfg(feature = "postgres_tests")]

//! Integration tests for the Stats Dashboard feature
//!
//! Tests cover:
//! - App management (enable/disable stats dashboard)
//! - Stats endpoints (overview, clusters)
//! - Authorization checks

use axum::http::{Method, StatusCode};
use serde_json::json;

#[allow(clippy::duplicate_mod)]
#[path = "common/mod.rs"]
mod common;

#[allow(clippy::duplicate_mod)]
#[path = "auth/support.rs"]
mod support;
use support::{read_json, send_request, setup_test_app};

// === App Management Tests ===

#[tokio::test]
async fn list_apps_requires_admin() {
    let app = setup_test_app().await;

    // Non-admin token
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;

    let response =
        send_request(&app, Method::GET, "/api/v1/admin/apps", Some(&regular_token.token), None)
            .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_apps_with_admin_token() {
    let app = setup_test_app().await;

    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    let response =
        send_request(&app, Method::GET, "/api/v1/admin/apps", Some(&admin_token.token), None).await;

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = read_json(response).await;
    assert!(body.get("apps").is_some());
    assert!(body.get("count").is_some());
}

#[tokio::test]
async fn enable_stats_dashboard() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Enable the stats dashboard
    let response = send_request(
        &app,
        Method::PUT,
        "/api/v1/admin/apps/stats_dashboard",
        Some(&admin_token.token),
        Some(json!({
            "enabled": true,
            "config": {
                "pollIntervalSeconds": 30
            }
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = read_json(response).await;
    assert_eq!(body["appId"], "stats_dashboard");
    assert_eq!(body["enabled"], true);
}

#[tokio::test]
async fn disable_stats_dashboard() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // First enable
    send_request(
        &app,
        Method::PUT,
        "/api/v1/admin/apps/stats_dashboard",
        Some(&admin_token.token),
        Some(json!({ "enabled": true })),
    )
    .await;

    // Then disable
    let response = send_request(
        &app,
        Method::PUT,
        "/api/v1/admin/apps/stats_dashboard",
        Some(&admin_token.token),
        Some(json!({ "enabled": false })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = read_json(response).await;
    assert_eq!(body["enabled"], false);
}

// === Stats Enabled Endpoint Tests ===

#[tokio::test]
async fn stats_enabled_returns_false_when_disabled() {
    let app = setup_test_app().await;
    let token = app.issue_token("user-token", &["stats:read"]).await;

    let response =
        send_request(&app, Method::GET, "/api/v1/stats/enabled", Some(&token.token), None).await;

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = read_json(response).await;
    assert_eq!(body["enabled"], false);
}

#[tokio::test]
async fn stats_enabled_returns_true_after_enabling() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;
    let user_token = app.issue_token("user-token", &["stats:read"]).await;

    // Enable stats dashboard
    send_request(
        &app,
        Method::PUT,
        "/api/v1/admin/apps/stats_dashboard",
        Some(&admin_token.token),
        Some(json!({ "enabled": true })),
    )
    .await;

    // Check enabled status
    let response =
        send_request(&app, Method::GET, "/api/v1/stats/enabled", Some(&user_token.token), None)
            .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = read_json(response).await;
    assert_eq!(body["enabled"], true);
}

// === Stats Overview Tests ===

#[tokio::test]
async fn stats_overview_forbidden_when_disabled() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create a team first
    support::create_team(&app, &admin_token.token, "test-team").await;

    let team_token = app.issue_token("team-token", &["team:test-team:stats:read"]).await;

    // Request stats overview without enabling stats dashboard
    let response = send_request(
        &app,
        Method::GET,
        "/api/v1/teams/test-team/stats/overview",
        Some(&team_token.token),
        None,
    )
    .await;

    // Should be forbidden because stats dashboard is not enabled
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn stats_overview_requires_team_scope() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Enable stats dashboard
    send_request(
        &app,
        Method::PUT,
        "/api/v1/admin/apps/stats_dashboard",
        Some(&admin_token.token),
        Some(json!({ "enabled": true })),
    )
    .await;

    // Create a team
    support::create_team(&app, &admin_token.token, "test-team").await;

    // Token for different team
    let wrong_team_token =
        app.issue_token("wrong-team-token", &["team:other-team:stats:read"]).await;

    let response = send_request(
        &app,
        Method::GET,
        "/api/v1/teams/test-team/stats/overview",
        Some(&wrong_team_token.token),
        None,
    )
    .await;

    // Should be forbidden due to team mismatch
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// === Stats Clusters Tests ===

#[tokio::test]
async fn stats_clusters_forbidden_when_disabled() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create a team first
    support::create_team(&app, &admin_token.token, "test-team").await;

    let team_token = app.issue_token("team-token", &["team:test-team:stats:read"]).await;

    // Request stats clusters without enabling stats dashboard
    let response = send_request(
        &app,
        Method::GET,
        "/api/v1/teams/test-team/stats/clusters",
        Some(&team_token.token),
        None,
    )
    .await;

    // Should be forbidden because stats dashboard is not enabled
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn stats_single_cluster_not_found_when_cluster_missing() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Enable stats dashboard
    send_request(
        &app,
        Method::PUT,
        "/api/v1/admin/apps/stats_dashboard",
        Some(&admin_token.token),
        Some(json!({ "enabled": true })),
    )
    .await;

    // Create a team
    support::create_team(&app, &admin_token.token, "test-team").await;

    let team_token = app.issue_token("team-token", &["team:test-team:stats:read"]).await;

    // Request specific cluster that doesn't exist
    // Note: This will fail to fetch stats from Envoy (which isn't running),
    // so we expect an error (5xx or 4xx depending on how the error is handled)
    let response = send_request(
        &app,
        Method::GET,
        "/api/v1/teams/test-team/stats/clusters/nonexistent-cluster",
        Some(&team_token.token),
        None,
    )
    .await;

    // Should be an error status (not 2xx)
    // Could be Internal (can't connect to Envoy), NotFound, or ServiceUnavailable
    assert!(
        !response.status().is_success(),
        "Expected error response when cluster doesn't exist, got {}",
        response.status()
    );
}
