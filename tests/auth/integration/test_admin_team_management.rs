use axum::http::{Method, StatusCode};
use serde_json::json;

use crate::support::{read_json, send_request, setup_test_app};
use flowplane::api::handlers::PaginatedResponse;
use flowplane::auth::team::Team;

#[tokio::test]
async fn create_team_requires_admin() {
    let app = setup_test_app().await;

    // Non-admin token
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&regular_token.token),
        Some(json!({
            "name": "engineering",
            "displayName": "Engineering Team",
            "description": "Main engineering team"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_team_with_admin_token() {
    let app = setup_test_app().await;

    // Admin token
    let admin_token = app.issue_admin_token("admin-token").await;

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "platform",
            "displayName": "Platform Team",
            "description": "Infrastructure and platform team"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CREATED);
    let team: Team = read_json(response).await;
    assert_eq!(team.name, "platform");
    assert_eq!(team.display_name, "Platform Team");
    assert_eq!(team.description, Some("Infrastructure and platform team".to_string()));
}

#[tokio::test]
async fn create_team_validates_name_format() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    // Invalid name with uppercase letters
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "InvalidName",
            "displayName": "Invalid Team"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Invalid name with spaces
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "invalid name",
            "displayName": "Invalid Team"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Valid name
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "valid-team-name",
            "displayName": "Valid Team"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn create_team_validates_required_fields() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    // Missing name
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "displayName": "Test Team"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Missing display_name
    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "test-team"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn create_duplicate_team_returns_conflict() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    // Create first team
    let response1 = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "duplicate-team",
            "displayName": "First Team"
        })),
    )
    .await;
    assert_eq!(response1.status(), StatusCode::CREATED);

    // Try to create duplicate
    let response2 = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "duplicate-team",
            "displayName": "Second Team"
        })),
    )
    .await;

    assert_eq!(response2.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn get_team_by_id() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    // Create team
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "devops",
            "displayName": "DevOps Team",
            "description": "DevOps and infrastructure"
        })),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let created_team: Team = read_json(create_response).await;

    // Get team by ID
    let get_response = send_request(
        &app,
        Method::GET,
        &format!("/api/v1/admin/teams/{}", created_team.id),
        Some(&admin_token.token),
        None,
    )
    .await;

    assert_eq!(get_response.status(), StatusCode::OK);
    let team: Team = read_json(get_response).await;
    assert_eq!(team.id, created_team.id);
    assert_eq!(team.name, "devops");
    assert_eq!(team.display_name, "DevOps Team");
}

#[tokio::test]
async fn get_team_requires_admin() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    // Create team
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "security",
            "displayName": "Security Team"
        })),
    )
    .await;
    let created_team: Team = read_json(create_response).await;

    // Try to get with non-admin token
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;
    let get_response = send_request(
        &app,
        Method::GET,
        &format!("/api/v1/admin/teams/{}", created_team.id),
        Some(&regular_token.token),
        None,
    )
    .await;

    assert_eq!(get_response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn get_nonexistent_team_returns_not_found() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    let response = send_request(
        &app,
        Method::GET,
        "/api/v1/admin/teams/nonexistent-team-id",
        Some(&admin_token.token),
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_teams_with_pagination() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    // Create multiple teams
    for i in 1..=5 {
        let response = send_request(
            &app,
            Method::POST,
            "/api/v1/admin/teams",
            Some(&admin_token.token),
            Some(json!({
                "name": format!("team{}", i),
                "displayName": format!("Team {}", i)
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    // List teams
    let response = send_request(
        &app,
        Method::GET,
        "/api/v1/admin/teams?limit=10&offset=0",
        Some(&admin_token.token),
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let list_response: PaginatedResponse<Team> = read_json(response).await;
    assert!(list_response.items.len() >= 5);
    assert_eq!(list_response.limit, 10);
    assert_eq!(list_response.offset, 0);
}

#[tokio::test]
async fn list_teams_requires_admin() {
    let app = setup_test_app().await;
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;

    let response =
        send_request(&app, Method::GET, "/api/v1/admin/teams", Some(&regular_token.token), None)
            .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn update_team() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    // Create team
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "mobile",
            "displayName": "Mobile Team",
            "description": "iOS and Android development"
        })),
    )
    .await;
    let created_team: Team = read_json(create_response).await;

    // Update team
    let update_response = send_request(
        &app,
        Method::PUT,
        &format!("/api/v1/admin/teams/{}", created_team.id),
        Some(&admin_token.token),
        Some(json!({
            "displayName": "Mobile Engineering Team",
            "description": "Mobile application development (iOS, Android, React Native)"
        })),
    )
    .await;

    assert_eq!(update_response.status(), StatusCode::OK);
    let updated_team: Team = read_json(update_response).await;
    assert_eq!(updated_team.id, created_team.id);
    assert_eq!(updated_team.name, "mobile"); // Name is immutable
    assert_eq!(updated_team.display_name, "Mobile Engineering Team");
    assert_eq!(
        updated_team.description,
        Some("Mobile application development (iOS, Android, React Native)".to_string())
    );
}

#[tokio::test]
async fn update_team_requires_admin() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    // Create team
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "data",
            "displayName": "Data Team"
        })),
    )
    .await;
    let created_team: Team = read_json(create_response).await;

    // Try to update with non-admin token
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;
    let update_response = send_request(
        &app,
        Method::PUT,
        &format!("/api/v1/admin/teams/{}", created_team.id),
        Some(&regular_token.token),
        Some(json!({
            "displayName": "Data Engineering Team"
        })),
    )
    .await;

    assert_eq!(update_response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn update_nonexistent_team_returns_not_found() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    let response = send_request(
        &app,
        Method::PUT,
        "/api/v1/admin/teams/nonexistent-team-id",
        Some(&admin_token.token),
        Some(json!({
            "displayName": "Updated Team"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_team() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    // Create team
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "temp-team",
            "displayName": "Temporary Team"
        })),
    )
    .await;
    let created_team: Team = read_json(create_response).await;

    // Delete team
    let delete_response = send_request(
        &app,
        Method::DELETE,
        &format!("/api/v1/admin/teams/{}", created_team.id),
        Some(&admin_token.token),
        None,
    )
    .await;

    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    // Verify team is deleted
    let get_response = send_request(
        &app,
        Method::GET,
        &format!("/api/v1/admin/teams/{}", created_team.id),
        Some(&admin_token.token),
        None,
    )
    .await;

    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_team_requires_admin() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    // Create team
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/admin/teams",
        Some(&admin_token.token),
        Some(json!({
            "name": "protected-team",
            "displayName": "Protected Team"
        })),
    )
    .await;
    let created_team: Team = read_json(create_response).await;

    // Try to delete with non-admin token
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;
    let delete_response = send_request(
        &app,
        Method::DELETE,
        &format!("/api/v1/admin/teams/{}", created_team.id),
        Some(&regular_token.token),
        None,
    )
    .await;

    assert_eq!(delete_response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_nonexistent_team_returns_not_found() {
    let app = setup_test_app().await;
    let admin_token = app.issue_admin_token("admin-token").await;

    let response = send_request(
        &app,
        Method::DELETE,
        "/api/v1/admin/teams/nonexistent-team-id",
        Some(&admin_token.token),
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
