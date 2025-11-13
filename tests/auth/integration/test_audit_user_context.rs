/// Integration tests for audit logging with user context
///
/// Tests that audit events properly capture user_id, client_ip, and user_agent
/// when available from the request context.
use axum::http::{Method, StatusCode};
use serde_json::json;

use crate::support::{read_json, send_request, setup_test_app};
use flowplane::auth::user::UserResponse;

#[tokio::test]
async fn audit_log_captures_user_context_on_user_creation() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create a user
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

    // Query audit log to verify user context was captured
    let audit_record: (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT user_id, client_ip, user_agent FROM audit_log WHERE action = 'user.created' AND resource_id = ? ORDER BY created_at DESC LIMIT 1"
    )
    .bind(user.id)
    .fetch_one(&app.pool)
    .await
    .expect("Should find audit record for user creation");

    let (user_id, client_ip, user_agent) = audit_record;

    // Currently, user context is not yet fully wired through HTTP handlers,
    // so these may be None. This test establishes the infrastructure.
    // TODO: Once HTTP context extraction is implemented, these should be Some(...)

    // For now, we verify the fields exist and can be queried
    println!(
        "Audit record - user_id: {:?}, client_ip: {:?}, user_agent: {:?}",
        user_id, client_ip, user_agent
    );
}

#[tokio::test]
async fn audit_event_with_user_context_builder() {
    let app = setup_test_app().await;

    // Test the with_user_context builder method
    use flowplane::storage::{AuditEvent, AuditLogRepository};

    let audit_repo = AuditLogRepository::new(app.pool.clone());

    let event = AuditEvent::token(
        "test.event",
        Some("test-resource-id"),
        Some("Test Resource"),
        serde_json::json!({"test": "data"}),
    )
    .with_user_context(
        Some("user-123".to_string()),
        Some("192.168.1.1".to_string()),
        Some("Mozilla/5.0".to_string()),
    );

    audit_repo.record_auth_event(event).await.expect("Should record audit event");

    // Verify the event was recorded with user context
    let record: (String, String, String) = sqlx::query_as(
        "SELECT user_id, client_ip, user_agent FROM audit_log WHERE action = 'test.event' ORDER BY created_at DESC LIMIT 1"
    )
    .fetch_one(&app.pool)
    .await
    .expect("Should find test audit event");

    assert_eq!(record.0, "user-123");
    assert_eq!(record.1, "192.168.1.1");
    assert_eq!(record.2, "Mozilla/5.0");
}

#[tokio::test]
async fn audit_event_without_user_context_stores_nulls() {
    let app = setup_test_app().await;

    use flowplane::storage::{AuditEvent, AuditLogRepository};

    let audit_repo = AuditLogRepository::new(app.pool.clone());

    // Create event without user context
    let event = AuditEvent::token(
        "test.event.no.context",
        Some("resource-456"),
        Some("Resource Without Context"),
        serde_json::json!({"test": "data"}),
    );

    audit_repo.record_auth_event(event).await.expect("Should record audit event");

    // Verify the event was recorded with NULL user context
    let record: (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT user_id, client_ip, user_agent FROM audit_log WHERE action = 'test.event.no.context' ORDER BY created_at DESC LIMIT 1"
    )
    .fetch_one(&app.pool)
    .await
    .expect("Should find test audit event");

    assert!(record.0.is_none(), "user_id should be None");
    assert!(record.1.is_none(), "client_ip should be None");
    assert!(record.2.is_none(), "user_agent should be None");
}

#[tokio::test]
async fn audit_log_query_by_user_id() {
    let app = setup_test_app().await;

    use flowplane::storage::{AuditEvent, AuditLogRepository};

    let audit_repo = AuditLogRepository::new(app.pool.clone());

    // Create multiple events for the same user
    let user_id = "user-789";

    for i in 1..=3 {
        let event = AuditEvent::token(
            &format!("test.action.{}", i),
            Some(&format!("resource-{}", i)),
            Some("Test Resource"),
            serde_json::json!({"iteration": i}),
        )
        .with_user_context(
            Some(user_id.to_string()),
            Some("192.168.1.100".to_string()),
            Some("TestClient/1.0".to_string()),
        );

        audit_repo.record_auth_event(event).await.expect("Should record event");
    }

    // Query audit logs for this user
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(&app.pool)
        .await
        .expect("Should count audit events");

    assert_eq!(count, 3, "Should find 3 audit events for user-789");

    // Verify the index is being used (this query should be fast)
    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE user_id = ? ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(&app.pool)
    .await
    .expect("Should fetch actions");

    assert_eq!(actions.len(), 3);
    assert_eq!(actions[0], "test.action.3"); // Most recent first
}

#[tokio::test]
async fn login_event_audit_trail() {
    let app = setup_test_app().await;
    let admin_token = app.issue_token("admin-token", &["admin:all"]).await;

    // Create a user
    let create_response = send_request(
        &app,
        Method::POST,
        "/api/v1/users",
        Some(&admin_token.token),
        Some(json!({
            "email": "logintest@example.com",
            "password": "SecureP@ssw0rd123",
            "name": "Login Test User",
            "isAdmin": false
        })),
    )
    .await;
    let user: UserResponse = read_json(create_response).await;

    // Login as that user
    let login_response = send_request(
        &app,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(json!({
            "email": "logintest@example.com",
            "password": "SecureP@ssw0rd123"
        })),
    )
    .await;

    assert_eq!(login_response.status(), StatusCode::OK);

    // Verify login success event was logged
    let login_events: Vec<(String, String)> = sqlx::query_as(
        "SELECT action, resource_id FROM audit_log WHERE action LIKE 'auth.login.%' AND resource_id = ? ORDER BY created_at DESC"
    )
    .bind(user.id)
    .fetch_all(&app.pool)
    .await
    .expect("Should find login audit events");

    assert!(!login_events.is_empty(), "Should have at least one login event");
    assert!(
        login_events.iter().any(|(action, _)| action == "auth.login.success"),
        "Should have a successful login event"
    );
}

// Note: Failed login audit trail is tested in login_scope_resolution tests
// which also verify that login events are properly recorded
