use axum::http::{Method, StatusCode};
use serde_json::json;

use crate::support::{create_team, read_json, send_request, setup_test_app};
use flowplane::api::handlers::ListUsersResponse;
use flowplane::auth::user::{UserResponse, UserTeamMembership, UserWithTeamsResponse};

#[tokio::test]
async fn create_user_requires_admin() {
    let app = setup_test_app().await;

    // Non-admin token
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&regular_token.token),
        Some(json!({
            "email": "newuser@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "New User",
            "isAdmin": false
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_user_with_admin_token() {
    let app = setup_test_app().await;

    // Admin token
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "testuser@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Test User",
            "isAdmin": false
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CREATED);
    let user: UserResponse = read_json(response).await;
    assert_eq!(user.email, "testuser@example.com");
    assert_eq!(user.name, "Test User");
    assert!(!user.is_admin);
}

#[tokio::test]
async fn create_user_validates_email() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "invalid-email",
            "password": "SecureP@ssw0rd123",
            "name": "Test User",
            "isAdmin": false
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_user_validates_password() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    let response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "test@example.com",
            "password": "weak",
            "name": "Test User",
            "isAdmin": false
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_duplicate_user_returns_conflict() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create first user
    let response1 = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "duplicate@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "First User",
            "isAdmin": false
        })),
    )
    .await;
    assert_eq!(response1.status(), StatusCode::CREATED);

    // Try to create duplicate
    let response2 = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "duplicate@example.com",
            "password": "AnotherP@ssw0rd456",
            "name": "Second User",
            "isAdmin": false
        })),
    )
    .await;

    assert_eq!(response2.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_user_by_id() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create user
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "getuser@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Get User",
            "isAdmin": false
        })),
    )
    .await;
    let created: UserResponse = read_json(create_response).await;

    // Get user by ID
    let get_url = format!("/api/v1/users/{}", created.id);
    let get_response =
        send_request(&app, Method::GET, &get_url, Some(&admin_token.token), None).await;

    assert_eq!(get_response.status(), StatusCode::OK);
    let user: UserWithTeamsResponse = read_json(get_response).await;
    assert_eq!(user.user.id, created.id);
    assert_eq!(user.user.email, "getuser@example.com");
    assert!(user.teams.is_empty());
}

#[tokio::test]
async fn get_user_requires_admin() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;

    // Create user as admin
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "user@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "User",
            "isAdmin": false
        })),
    )
    .await;
    let created: UserResponse = read_json(create_response).await;

    // Try to get as non-admin
    let get_url = format!("/api/v1/users/{}", created.id);
    let get_response =
        send_request(&app, Method::GET, &get_url, Some(&regular_token.token), None).await;

    assert_eq!(get_response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn get_nonexistent_user_returns_404() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    let response = send_request(
        &app,
        Method::GET,
        "/api/v1/users/nonexistent-id",
        Some(&admin_token.token),
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_users_with_pagination() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create multiple users
    for i in 1..=3 {
        send_request(
            &app,
            Method::POST,
            "/api/v1/users",
            Some(&admin_token.token),
            Some(json!({
                "email": format!("user{}@example.com", i),
                "password": "SecureP@ssw0rd123",
                "name": format!("User {}", i),
                "isAdmin": false
            })),
        )
        .await;
    }

    // List users
    let response = send_request(
        &app,
        Method::GET,
        "/api/v1/users?limit=10&offset=0",
        Some(&admin_token.token),
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let list: ListUsersResponse = read_json(response).await;
    assert!(list.users.len() >= 3);
    assert!(list.total >= 3);
    assert_eq!(list.limit, 10);
    assert_eq!(list.offset, 0);
}

#[tokio::test]
async fn list_users_requires_admin() {
    let app = setup_test_app().await;
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;

    let response = send_request(
        &app,
        Method::GET,
        "/api/v1/users?limit=10&offset=0",
        Some(&regular_token.token),
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn update_user() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create user
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "update@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Original Name",
            "isAdmin": false
        })),
    )
    .await;
    let created: UserResponse = read_json(create_response).await;

    // Update user
    let update_url = format!("/api/v1/users/{}", created.id);
    let update_response = send_request(
        &app,
        Method::PUT,
        &update_url,
        Some(&admin_token.token),
        Some(json!({
            "name": "Updated Name",
            "status": "active"
        })),
    )
    .await;

    assert_eq!(update_response.status(), StatusCode::OK);
    let updated: UserResponse = read_json(update_response).await;
    assert_eq!(updated.name, "Updated Name");
    assert_eq!(updated.email, created.email); // Unchanged
}

#[tokio::test]
async fn update_user_requires_admin() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;

    // Create user as admin
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "user@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "User",
            "isAdmin": false
        })),
    )
    .await;
    let created: UserResponse = read_json(create_response).await;

    // Try to update as non-admin
    let update_url = format!("/api/v1/users/{}", created.id);
    let response = send_request(
        &app,
        Method::PUT,
        &update_url,
        Some(&regular_token.token),
        Some(json!({"name": "Hacked Name"})),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_user() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create user
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "delete@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Delete User",
            "isAdmin": false
        })),
    )
    .await;
    let created: UserResponse = read_json(create_response).await;

    // Delete user
    let delete_url = format!("/api/v1/users/{}", created.id);
    let delete_response =
        send_request(&app, Method::DELETE, &delete_url, Some(&admin_token.token), None).await;

    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    // Verify user is deleted
    let get_response =
        send_request(&app, Method::GET, &delete_url, Some(&admin_token.token), None).await;
    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_user_requires_admin() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;

    // Create user as admin
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "user@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "User",
            "isAdmin": false
        })),
    )
    .await;
    let created: UserResponse = read_json(create_response).await;

    // Try to delete as non-admin
    let delete_url = format!("/api/v1/users/{}", created.id);
    let response =
        send_request(&app, Method::DELETE, &delete_url, Some(&regular_token.token), None).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn add_team_membership() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create team first
    create_team(&app, &admin_token.token, "engineering").await;

    // Create user
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "teammember@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Team Member",
            "isAdmin": false
        })),
    )
    .await;
    let created: UserResponse = read_json(create_response).await;

    // Add team membership
    let teams_url = format!("/api/v1/users/{}/teams", created.id);
    let response = send_request(
        &app,
        Method::POST,
        &teams_url,
        Some(&admin_token.token),
        Some(json!({
            "userId": created.id,
            "team": "engineering",
            "scopes": ["team:engineering:clusters:read", "team:engineering:routes:write"]
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CREATED);
    let membership: UserTeamMembership = read_json(response).await;
    assert_eq!(membership.user_id, created.id);
    // Team is stored as UUID after FK migration (resolved from name "engineering")
    assert!(uuid::Uuid::parse_str(&membership.team).is_ok(), "team should be a UUID");
    assert_eq!(membership.scopes.len(), 2);
}

#[tokio::test]
async fn add_team_membership_requires_admin() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;

    // Create user as admin
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "user@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "User",
            "isAdmin": false
        })),
    )
    .await;
    let created: UserResponse = read_json(create_response).await;

    // Try to add team membership as non-admin
    let teams_url = format!("/api/v1/users/{}/teams", created.id);
    let response = send_request(
        &app,
        Method::POST,
        &teams_url,
        Some(&regular_token.token),
        Some(json!({
            "userId": created.id,
            "team": "engineering",
            "scopes": ["team:engineering:clusters:read"]
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_user_teams() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create teams first
    create_team(&app, &admin_token.token, "engineering").await;
    create_team(&app, &admin_token.token, "platform").await;
    create_team(&app, &admin_token.token, "security").await;

    // Create user
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "multiteam@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Multi Team User",
            "isAdmin": false
        })),
    )
    .await;
    let created: UserResponse = read_json(create_response).await;

    // Add multiple team memberships
    let teams_url = format!("/api/v1/users/{}/teams", created.id);
    for team in ["engineering", "platform", "security"] {
        send_request(
            &app,
            Method::POST,
            &teams_url,
            Some(&admin_token.token),
            Some(json!({
                "userId": created.id,
                "team": team,
                "scopes": [format!("team:{}:clusters:read", team)]
            })),
        )
        .await;
    }

    // List user teams
    let list_response =
        send_request(&app, Method::GET, &teams_url, Some(&admin_token.token), None).await;

    assert_eq!(list_response.status(), StatusCode::OK);
    let teams: Vec<UserTeamMembership> = read_json(list_response).await;
    assert_eq!(teams.len(), 3);
}

#[tokio::test]
async fn remove_team_membership() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create team first
    create_team(&app, &admin_token.token, "engineering").await;

    // Create user
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "removeteam@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Remove Team User",
            "isAdmin": false
        })),
    )
    .await;
    let created: UserResponse = read_json(create_response).await;

    // Add team membership
    let teams_url = format!("/api/v1/users/{}/teams", created.id);
    send_request(
        &app,
        Method::POST,
        &teams_url,
        Some(&admin_token.token),
        Some(json!({
            "userId": created.id,
            "team": "engineering",
            "scopes": ["team:engineering:clusters:read"]
        })),
    )
    .await;

    // Remove team membership
    let remove_url = format!("/api/v1/users/{}/teams/engineering", created.id);
    let remove_response =
        send_request(&app, Method::DELETE, &remove_url, Some(&admin_token.token), None).await;

    assert_eq!(remove_response.status(), StatusCode::NO_CONTENT);

    // Verify membership is removed
    let list_response =
        send_request(&app, Method::GET, &teams_url, Some(&admin_token.token), None).await;
    let teams: Vec<UserTeamMembership> = read_json(list_response).await;
    assert_eq!(teams.len(), 0);
}

#[tokio::test]
async fn remove_team_membership_requires_admin() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;
    let regular_token = app.issue_token("regular-user", &["clusters:read"]).await;

    // Create user and add team membership as admin
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "user@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "User",
            "isAdmin": false
        })),
    )
    .await;
    let created: UserResponse = read_json(create_response).await;

    let teams_url = format!("/api/v1/users/{}/teams", created.id);
    send_request(
        &app,
        Method::POST,
        &teams_url,
        Some(&admin_token.token),
        Some(json!({
            "userId": created.id,
            "team": "engineering",
            "scopes": ["team:engineering:clusters:read"]
        })),
    )
    .await;

    // Try to remove as non-admin
    let remove_url = format!("/api/v1/users/{}/teams/engineering", created.id);
    let response =
        send_request(&app, Method::DELETE, &remove_url, Some(&regular_token.token), None).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn user_lifecycle_integration() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create team first
    create_team(&app, &admin_token.token, "platform").await;

    // 1. Create user
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "lifecycle@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Lifecycle User",
            "isAdmin": false
        })),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let user: UserResponse = read_json(create_response).await;

    // 2. Get user
    let get_url = format!("/api/v1/users/{}", user.id);
    let get_response =
        send_request(&app, Method::GET, &get_url, Some(&admin_token.token), None).await;
    assert_eq!(get_response.status(), StatusCode::OK);

    // 3. Update user
    let update_response = send_request(
        &app,
        Method::PUT,
        &get_url,
        Some(&admin_token.token),
        Some(json!({"name": "Updated Lifecycle User"})),
    )
    .await;
    assert_eq!(update_response.status(), StatusCode::OK);

    // 4. Add team membership
    let teams_url = format!("/api/v1/users/{}/teams", user.id);
    let add_team_response = send_request(
        &app,
        Method::POST,
        &teams_url,
        Some(&admin_token.token),
        Some(json!({
            "userId": user.id,
            "team": "platform",
            "scopes": ["team:platform:clusters:write"]
        })),
    )
    .await;
    assert_eq!(add_team_response.status(), StatusCode::CREATED);

    // 5. List user teams
    let list_teams_response =
        send_request(&app, Method::GET, &teams_url, Some(&admin_token.token), None).await;
    assert_eq!(list_teams_response.status(), StatusCode::OK);
    let teams: Vec<UserTeamMembership> = read_json(list_teams_response).await;
    assert_eq!(teams.len(), 1);

    // 6. Remove team membership
    let remove_url = format!("/api/v1/users/{}/teams/platform", user.id);
    let remove_response =
        send_request(&app, Method::DELETE, &remove_url, Some(&admin_token.token), None).await;
    assert_eq!(remove_response.status(), StatusCode::NO_CONTENT);

    // 7. Delete user
    let delete_response =
        send_request(&app, Method::DELETE, &get_url, Some(&admin_token.token), None).await;
    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    // 8. Verify user is deleted
    let final_get = send_request(&app, Method::GET, &get_url, Some(&admin_token.token), None).await;
    assert_eq!(final_get.status(), StatusCode::NOT_FOUND);
}
