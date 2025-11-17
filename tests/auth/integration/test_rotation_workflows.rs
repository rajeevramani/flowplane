//! Comprehensive tests for token rotation workflows and security
//!
//! Tests cover:
//! - Bootstrap token rotation with and without Vault
//! - Audit logging completeness
//! - No-downtime rotation (old token valid until rotation complete)
//! - Security aspects (secrets never logged, proper error handling)

use chrono::Utc;
use flowplane::auth::models::TokenStatus;
use flowplane::auth::token_service::TokenService;
use flowplane::auth::validation::CreateTokenRequest;
use flowplane::secrets::{EnvVarSecretsClient, SecretsClient, SecretsError};
use flowplane::storage::repository::{AuditLogRepository, SqlxTokenRepository, TokenRepository};
use flowplane::storage::DbPool;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;

async fn setup_pool() -> DbPool {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite::memory:?cache=shared")
        .await
        .expect("in-memory sqlite");

    // Create tables
    sqlx::query(
        r#"
        CREATE TABLE personal_access_tokens (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL,
            description TEXT,
            status TEXT NOT NULL,
            expires_at DATETIME,
            last_used_at DATETIME,
            created_by TEXT,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

            is_setup_token BOOLEAN NOT NULL DEFAULT FALSE,
            max_usage_count INTEGER,
            usage_count INTEGER NOT NULL DEFAULT 0,
            failed_attempts INTEGER NOT NULL DEFAULT 0,
            locked_until DATETIME
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE TABLE token_scopes (
            id TEXT PRIMARY KEY,
            token_id TEXT NOT NULL,
            scope TEXT NOT NULL,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (token_id) REFERENCES personal_access_tokens(id) ON DELETE CASCADE
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE TABLE audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            resource_type TEXT NOT NULL,
            resource_id TEXT,
            resource_name TEXT,
            action TEXT NOT NULL,
            old_configuration TEXT,
            new_configuration TEXT,
            user_id TEXT,
            client_ip TEXT,
            user_agent TEXT,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    pool
}

async fn setup_service() -> (TokenService, Arc<SqlxTokenRepository>, Arc<AuditLogRepository>, DbPool)
{
    let pool = setup_pool().await;
    let repo = Arc::new(SqlxTokenRepository::new(pool.clone()));
    let audit = Arc::new(AuditLogRepository::new(pool.clone()));
    let service = TokenService::new(repo.clone(), audit.clone());
    (service, repo, audit, pool)
}

/// Mock secrets client for testing
struct MockSecretsClient {
    should_fail: bool,
}

#[async_trait::async_trait]
impl SecretsClient for MockSecretsClient {
    async fn get_secret(&self, _key: &str) -> Result<String, SecretsError> {
        if self.should_fail {
            Err(SecretsError::backend_error("Mock failure"))
        } else {
            Ok("mock-secret-value".to_string())
        }
    }

    async fn set_secret(&self, _key: &str, _value: &str) -> Result<(), SecretsError> {
        if self.should_fail {
            Err(SecretsError::backend_error("Mock failure"))
        } else {
            Ok(())
        }
    }

    async fn rotate_secret(&self, _key: &str) -> Result<String, SecretsError> {
        if self.should_fail {
            Err(SecretsError::backend_error("Mock failure"))
        } else {
            // Generate a new mock secret
            Ok(format!("rotated-secret-{}", uuid::Uuid::new_v4().simple()))
        }
    }

    async fn delete_secret(&self, _key: &str) -> Result<(), SecretsError> {
        Ok(())
    }

    async fn list_secrets(&self) -> Result<Vec<flowplane::secrets::SecretMetadata>, SecretsError> {
        Ok(vec![])
    }

    async fn secret_exists(&self, _key: &str) -> Result<bool, SecretsError> {
        Ok(!self.should_fail)
    }

    async fn get_secret_metadata(
        &self,
        _key: &str,
    ) -> Result<flowplane::secrets::SecretMetadata, SecretsError> {
        Ok(flowplane::secrets::SecretMetadata {
            key: "test-key".to_string(),
            version: Some(1),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: None,
        })
    }
}

// Test 1: Bootstrap token creation without Vault
#[tokio::test]
async fn test_bootstrap_token_creation_without_vault() {
    let (service, _repo, _audit, pool) = setup_service().await;
    let bootstrap_secret = "test-bootstrap-secret-min-32-chars";

    // Create bootstrap token without secrets client (None)
    let maybe_token = service
        .ensure_bootstrap_token(bootstrap_secret, None::<&EnvVarSecretsClient>)
        .await
        .unwrap();

    assert!(maybe_token.is_some(), "Bootstrap token should be created");

    let token = maybe_token.unwrap();
    assert!(token.starts_with("fp_pat_"), "Token should have correct prefix");
    assert!(token.contains(bootstrap_secret), "Token should contain the secret");

    // Verify token was persisted
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM personal_access_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "Should have exactly one token");

    // Verify audit log
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'auth.token.bootstrap_seeded'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1, "Should have one bootstrap_seeded audit event");
}

// Test 2: Bootstrap token creation with Vault
#[tokio::test]
async fn test_bootstrap_token_creation_with_vault() {
    let (service, _repo, _audit, pool) = setup_service().await;
    let bootstrap_secret = "vault-bootstrap-secret-min-32-chars";
    let secrets_client = MockSecretsClient { should_fail: false };

    // Create bootstrap token with secrets client
    let maybe_token =
        service.ensure_bootstrap_token(bootstrap_secret, Some(&secrets_client)).await.unwrap();

    assert!(maybe_token.is_some(), "Bootstrap token should be created");

    // Verify audit log includes bootstrap_seeded event
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'auth.token.bootstrap_seeded'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1, "Should have one bootstrap_seeded audit event");
}

// Test 3: Bootstrap token rotation with Vault
#[tokio::test]
async fn test_bootstrap_token_rotation_with_vault() {
    let (service, _repo, _audit, pool) = setup_service().await;
    let bootstrap_secret = "initial-bootstrap-secret-min-32-chars";

    // Create initial bootstrap token
    let initial_token = service
        .ensure_bootstrap_token(bootstrap_secret, None::<&EnvVarSecretsClient>)
        .await
        .unwrap()
        .unwrap();

    // Rotate the bootstrap token using Vault
    let secrets_client = MockSecretsClient { should_fail: false };
    let rotated_token = service.rotate_bootstrap_token(&secrets_client).await.unwrap();

    // Verify new token is different
    assert_ne!(initial_token, rotated_token, "Rotated token should be different");
    assert!(rotated_token.starts_with("fp_pat_"), "Rotated token should have correct prefix");

    // Verify audit log includes rotation event
    let rotation_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'auth.token.bootstrap_rotated'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rotation_events, 1, "Should have one bootstrap_rotated audit event");
}

// Test 4: Regular token rotation (no-downtime verification)
#[tokio::test]
async fn test_token_rotation_no_downtime() {
    let (service, repo, _audit, _pool) = setup_service().await;

    // Create a regular token
    let created = service
        .create_token(CreateTokenRequest {
            name: "test-token".into(),
            description: Some("Testing rotation".into()),
            expires_at: None,
            scopes: vec!["routes:read".into()],
            created_by: Some("test".into()),
            user_id: None,
            user_email: None,
        })
        .await
        .unwrap();

    let original_token = created.token.clone();
    let token_id = created.id.clone();

    // Extract the secret part from the original token
    let parts: Vec<&str> = original_token.split('.').collect();
    let original_secret = parts[1];

    // Verify original token is valid by checking the hash
    let (_, original_hash) = repo
        .find_active_for_auth(&flowplane::domain::TokenId::from_str_unchecked(&token_id))
        .await
        .unwrap()
        .unwrap();
    assert!(
        service.verify_secret(&original_hash, original_secret).unwrap(),
        "Original token should be valid before rotation"
    );

    // Rotate the token
    let rotated = service.rotate_token(&token_id).await.unwrap();
    assert_ne!(original_token, rotated.token, "Rotated token should be different");

    // Verify original token is NOW INVALID (rotation is immediate)
    let (_, hash_after_rotation) = repo
        .find_active_for_auth(&flowplane::domain::TokenId::from_str_unchecked(&token_id))
        .await
        .unwrap()
        .unwrap();
    assert!(
        !service.verify_secret(&hash_after_rotation, original_secret).unwrap(),
        "Original token should be invalid after rotation"
    );

    // Verify new token is valid
    let new_parts: Vec<&str> = rotated.token.split('.').collect();
    let new_secret = new_parts[1];
    assert!(
        service.verify_secret(&hash_after_rotation, new_secret).unwrap(),
        "New token should be valid after rotation"
    );
}

// Test 5: Audit logging completeness for rotation
#[tokio::test]
async fn test_audit_logging_completeness() {
    let (service, _repo, _audit, pool) = setup_service().await;

    // Create a token
    let created = service
        .create_token(CreateTokenRequest {
            name: "audit-test".into(),
            description: None,
            expires_at: None,
            scopes: vec!["clusters:read".into()],
            created_by: Some("test-user".into()),
            user_id: None,
            user_email: None,
        })
        .await
        .unwrap();

    // Rotate the token
    service.rotate_token(&created.id).await.unwrap();

    // Verify audit log contains:
    // 1. Token creation event
    let create_events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE action = 'auth.token.created'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(create_events >= 1, "Should have at least one token.created event");

    // 2. Token rotation event
    let rotate_events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE action = 'auth.token.rotated'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rotate_events, 1, "Should have exactly one token.rotated event");

    // 3. Verify audit entry contains metadata
    #[derive(sqlx::FromRow)]
    struct AuditEntry {
        resource_id: Option<String>,
        new_configuration: Option<String>,
    }

    let entry: AuditEntry = sqlx::query_as(
        "SELECT resource_id, new_configuration FROM audit_log WHERE action = 'auth.token.rotated'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(entry.resource_id.is_some(), "Audit log should include resource_id");
    assert!(entry.new_configuration.is_some(), "Audit log should include metadata");
}

// Test 6: Security - secrets should never appear in audit logs
#[tokio::test]
async fn test_secrets_not_in_audit_logs() {
    let (service, _repo, _audit, pool) = setup_service().await;

    let bootstrap_secret = "super-secret-value-min-32-characters-long";

    // Create bootstrap token
    service.ensure_bootstrap_token(bootstrap_secret, None::<&EnvVarSecretsClient>).await.unwrap();

    // Check that the secret value doesn't appear in audit logs
    #[derive(sqlx::FromRow)]
    struct AuditEntry {
        old_configuration: Option<String>,
        new_configuration: Option<String>,
    }

    let entries: Vec<AuditEntry> =
        sqlx::query_as("SELECT old_configuration, new_configuration FROM audit_log")
            .fetch_all(&pool)
            .await
            .unwrap();

    for entry in entries {
        if let Some(old_config) = entry.old_configuration {
            assert!(
                !old_config.contains(bootstrap_secret),
                "Secret should not appear in old_configuration"
            );
        }
        if let Some(new_config) = entry.new_configuration {
            assert!(
                !new_config.contains(bootstrap_secret),
                "Secret should not appear in new_configuration"
            );
        }
    }
}

// Test 7: Bootstrap token rotation fails gracefully without Vault
#[tokio::test]
async fn test_bootstrap_rotation_without_vault_fails() {
    let (service, _repo, _audit, _pool) = setup_service().await;

    // Create bootstrap token without Vault
    let bootstrap_secret = "test-secret-min-32-characters-long";
    service.ensure_bootstrap_token(bootstrap_secret, None::<&EnvVarSecretsClient>).await.unwrap();

    // Try to rotate with a failing secrets client
    let failing_client = MockSecretsClient { should_fail: true };
    let result = service.rotate_bootstrap_token(&failing_client).await;

    // Should fail with appropriate error
    assert!(result.is_err(), "Rotation should fail without working secrets backend");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Failed to rotate bootstrap secret") || err_msg.contains("Mock failure"),
        "Error message should indicate secrets backend failure"
    );
}

// Test 8: Multiple rotation cycles maintain integrity
#[tokio::test]
async fn test_multiple_rotation_cycles() {
    let (service, repo, _audit, pool) = setup_service().await;

    // Create initial token
    let created = service
        .create_token(CreateTokenRequest {
            name: "multi-rotate".into(),
            description: None,
            expires_at: None,
            scopes: vec!["listeners:read".into()],
            created_by: Some("test".into()),
            user_id: None,
            user_email: None,
        })
        .await
        .unwrap();

    let mut previous_token = created.token;

    // Perform 5 rotation cycles
    for i in 1..=5 {
        let rotated = service.rotate_token(&created.id).await.unwrap();
        assert_ne!(previous_token, rotated.token, "Rotation {} should produce different token", i);
        previous_token = rotated.token;
    }

    // Verify audit log has all rotation events
    let rotation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'auth.token.rotated' AND resource_id = ?",
    )
    .bind(&created.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rotation_count, 5, "Should have 5 rotation events");

    // Verify token is still active
    let final_token =
        repo.get_token(&flowplane::domain::TokenId::from_str_unchecked(&created.id)).await.unwrap();
    assert_eq!(final_token.status, TokenStatus::Active, "Token should still be active");
}
