//! Test utilities for API handler testing
//!
//! This module provides test helpers for unit and integration testing of API handlers:
//! - `TestApiStateBuilder` for creating test state with in-memory database
//! - Auth context helpers for testing authorization
//! - Test data builders for teams and users
//! - Request/response test helpers
//!
//! # Test Harness Hierarchy
//!
//! The codebase provides multiple test harness implementations for different test levels:
//!
//! | Harness | Location | Speed | Use Case |
//! |---------|----------|-------|----------|
//! | `TestApiStateBuilder` | This module | Fast | Unit tests - handler logic only |
//! | `TestApp` | `tests/auth/support.rs` | Medium | HTTP integration - API contracts |
//! | `TestServer` | `tests/cli_integration/support.rs` | Medium | CLI integration - command testing |
//! | `TestHarness` | `tests/e2e/common/harness.rs` | Slow | E2E - full stack with Envoy |
//!
//! ## When to Use Each Harness
//!
//! **Unit Tests (this module)**:
//! - Testing handler logic without HTTP layer
//! - Testing authorization checks
//! - Testing validation logic
//! - Fastest feedback loop
//!
//! ```ignore
//! use crate::api::test_utils::{create_test_state, admin_auth_context};
//! let state = create_test_state().await;
//! let auth = admin_auth_context();
//! // Call handler directly
//! ```
//!
//! **HTTP Integration Tests** (`tests/auth/support::TestApp`):
//! - Testing full HTTP request/response cycle
//! - Testing middleware behavior
//! - Testing API contracts
//!
//! ```ignore
//! let app = setup_test_app().await;
//! let response = send_request(&app, Method::GET, "/api/v1/clusters", token, None).await;
//! ```
//!
//! **CLI Integration Tests** (`tests/cli_integration/support::TestServer`):
//! - Testing CLI commands against running server
//! - Testing configuration file parsing
//! - Testing command-line argument handling
//!
//! **E2E Tests** (`tests/e2e/common/harness::TestHarness`):
//! - Testing complete system with Envoy
//! - Testing xDS protocol interactions
//! - Testing real network traffic routing

use std::sync::Arc;

use crate::api::routes::ApiState;
use crate::auth::models::{AuthContext, Grant, GrantType};
use crate::domain::{OrgId, TeamId, TokenId, UserId};
use crate::services::stats_cache::{StatsCache, StatsCacheConfig};
use crate::storage::test_helpers::{create_test_xds_state, TestDatabase};
use crate::storage::DbPool;
use crate::xds::XdsState;

/// Builder for creating test API state with configurable dependencies
#[derive(Default)]
pub struct TestApiStateBuilder {
    pool: Option<DbPool>,
}

impl TestApiStateBuilder {
    /// Create a new test state builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the database pool
    pub fn with_pool(mut self, pool: DbPool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Build the test API state with PostgreSQL test database
    ///
    /// If no pool is provided, creates a PostgreSQL test database via Testcontainers.
    /// Returns both the TestDatabase (which must be kept alive) and the ApiState.
    pub async fn build(self) -> (TestDatabase, ApiState) {
        let (test_db, xds_state) = match self.pool {
            Some(p) => {
                // When a pool is provided externally, create a dummy TestDatabase
                // The caller is responsible for keeping their own container alive
                let db = TestDatabase::new("api_state_builder").await;
                let state = Arc::new(XdsState::with_database(Default::default(), p));
                (db, state)
            }
            None => create_test_xds_state("api_state").await,
        };
        let stats_cache = Arc::new(StatsCache::new(StatsCacheConfig::default()));
        let mcp_connection_manager = crate::mcp::create_connection_manager();
        let mcp_session_manager = crate::mcp::create_session_manager();
        let certificate_rate_limiter = Arc::new(crate::api::rate_limit::RateLimiter::from_env());

        (
            test_db,
            ApiState {
                xds_state,
                filter_schema_registry: None,
                stats_cache,
                mcp_connection_manager,
                mcp_session_manager,
                certificate_rate_limiter,
                auth_config: Arc::new(crate::config::AuthConfig::default()),
                zitadel_admin: None,
                permission_cache: None,
            },
        )
    }
}

/// Create a PostgreSQL test database pool via Testcontainers.
///
/// Returns the TestDatabase (must be kept alive) and the DbPool.
pub async fn create_test_pool() -> (TestDatabase, DbPool) {
    let db = TestDatabase::new("api_test").await;
    let pool = db.pool.clone();
    (db, pool)
}

/// Create test API state with default configuration.
///
/// Returns the TestDatabase (must be kept alive) and the ApiState.
pub async fn create_test_state() -> (TestDatabase, ApiState) {
    TestApiStateBuilder::new().build().await
}

// === Auth Context Helpers ===

/// Create a resource grant for testing.
pub fn make_grant(resource: &str, action: &str, team_name: &str) -> Grant {
    Grant {
        grant_type: GrantType::Resource,
        team_id: format!("test-team-id-{}", team_name),
        team_name: team_name.to_string(),
        resource_type: Some(resource.to_string()),
        action: Some(action.to_string()),
        route_id: None,
        allowed_methods: vec![],
    }
}

/// Create an admin auth context for testing (governance-only).
///
/// `admin:all` is an org-level scope for platform governance. It does NOT grant
/// resource access (clusters, routes, etc.) — that requires resource grants.
pub fn admin_auth_context() -> AuthContext {
    AuthContext::new(TokenId::new(), "test-admin-token".to_string(), vec!["admin:all".to_string()])
}

/// Create an org admin auth context for testing.
///
/// Org admins have `org:{name}:admin` scope and can access all teams
/// within their organization via implicit team access.
pub fn org_admin_auth_context(org_name: &str) -> AuthContext {
    let org_id = OrgId::from_str_unchecked(crate::storage::test_helpers::TEST_ORG_ID);
    AuthContext::new(
        TokenId::new(),
        format!("{}-org-admin-token", org_name),
        vec![format!("org:{}:admin", org_name)],
    )
    .with_org(org_id, org_name.to_string())
}

/// Create a team member auth context for testing with full CRUD grants.
pub fn team_auth_context(team: &str) -> AuthContext {
    let grants: Vec<Grant> = ["clusters", "routes", "listeners", "filters"]
        .iter()
        .flat_map(|resource| {
            ["read", "create", "update", "delete"]
                .iter()
                .map(|action| make_grant(resource, action, team))
        })
        .collect();
    AuthContext::new(TokenId::new(), format!("{}-test-token", team), vec![])
        .with_grants(grants, None)
}

/// Create a read-only auth context for testing.
pub fn readonly_auth_context(team: &str) -> AuthContext {
    let grants: Vec<Grant> = ["clusters", "routes", "listeners", "filters"]
        .iter()
        .map(|resource| make_grant(resource, "read", team))
        .collect();
    AuthContext::new(TokenId::new(), format!("{}-readonly-token", team), vec![])
        .with_grants(grants, None)
}

/// Create a minimal auth context for testing (no scopes, no grants).
pub fn minimal_auth_context() -> AuthContext {
    AuthContext::new(TokenId::new(), "minimal-token".to_string(), vec![])
}

/// Create an auth context with `admin:all` org scope and full CRUD grants for a resource.
///
/// Used for testing handler logic where the test verifies behavior, not authorization.
///
/// Example: `resource_auth_context("tokens")` grants read/create/update/delete on tokens.
pub fn resource_auth_context(resource: &str) -> AuthContext {
    let team = "test-team";
    let grants: Vec<Grant> = ["read", "create", "update", "delete"]
        .iter()
        .map(|action| make_grant(resource, action, team))
        .collect();
    AuthContext::new(
        TokenId::new(),
        format!("{}-test-token", resource),
        vec!["admin:all".to_string()],
    )
    .with_grants(grants, None)
}

/// Create an auth context with a read-only grant for a specific resource.
///
/// Example: `readonly_resource_auth_context("tokens")` grants only tokens:read.
pub fn readonly_resource_auth_context(resource: &str) -> AuthContext {
    let grants = vec![make_grant(resource, "read", "test-team")];
    AuthContext::new(TokenId::new(), format!("{}-readonly-token", resource), vec![])
        .with_grants(grants, None)
}

// === Test Data Builders ===

/// Builder for creating test team data
pub struct TestTeamBuilder {
    name: String,
    display_name: String,
    description: Option<String>,
}

impl TestTeamBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            display_name: format!("Test Team {}", name),
            description: Some("Test team for unit tests".to_string()),
        }
    }

    pub fn with_display_name(mut self, display_name: &str) -> Self {
        self.display_name = display_name.to_string();
        self
    }

    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    /// Insert the team into the database, or return the existing team if it already exists.
    pub async fn insert(self, pool: &DbPool) -> TeamId {
        use crate::storage::repositories::SqlxTeamRepository;
        use crate::storage::repositories::TeamRepository;

        let repo = SqlxTeamRepository::new(pool.clone());

        // Try to get the existing team first (seed data may have created it)
        if let Ok(Some(team)) = repo.get_team_by_name(&self.name).await {
            return team.id;
        }

        // Team doesn't exist, create it
        use crate::auth::team::CreateTeamRequest;
        let team = repo
            .create_team(CreateTeamRequest {
                name: self.name.clone(),
                display_name: self.display_name,
                description: self.description,
                owner_user_id: None,
                org_id: OrgId::from_str_unchecked(crate::storage::test_helpers::TEST_ORG_ID),
                settings: None,
            })
            .await
            .expect("Failed to create test team");

        team.id
    }
}

/// Builder for creating test user data
pub struct TestUserBuilder {
    email: String,
    name: String,
    password: String,
    is_admin: bool,
}

impl TestUserBuilder {
    pub fn new(email: &str) -> Self {
        Self {
            email: email.to_string(),
            name: email.split('@').next().unwrap_or("testuser").to_string(),
            password: "TestPass123!".to_string(),
            is_admin: false,
        }
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn with_password(mut self, password: &str) -> Self {
        self.password = password.to_string();
        self
    }

    pub fn as_admin(mut self) -> Self {
        self.is_admin = true;
        self
    }

    /// Insert the user into the database
    pub async fn insert(self, pool: &DbPool) -> UserId {
        use crate::auth::user::{NewUser, UserStatus};
        use crate::storage::repositories::SqlxUserRepository;
        use crate::storage::repositories::UserRepository;

        let repo = SqlxUserRepository::new(pool.clone());
        let user_id = UserId::new();

        // Dummy hash — Zitadel handles authentication, password_hash column
        // kept until Task 2.5 drops the users table.
        let password_hash = "dummy-hash-zitadel-handles-auth".to_string();

        let new_user = NewUser {
            id: user_id.clone(),
            email: self.email,
            password_hash,
            name: self.name,
            status: UserStatus::Active,
            is_admin: self.is_admin,
        };

        repo.create_user(new_user).await.expect("Failed to create test user");

        user_id
    }
}

// === Handler Testing Helpers ===

/// Helper macro for testing handler responses
#[macro_export]
macro_rules! assert_status {
    ($response:expr, $expected:expr) => {
        assert_eq!($response.0, $expected, "Expected status {} but got {}", $expected, $response.0);
    };
}

/// Helper macro for testing JSON response body
#[macro_export]
macro_rules! assert_json_contains {
    ($response:expr, $field:expr, $value:expr) => {
        let json = &$response.1 .0;
        let field_value = json.get($field).expect(concat!("Missing field: ", $field));
        assert_eq!(field_value, $value);
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn test_create_test_pool() {
        let (_db, pool) = create_test_pool().await;
        // Pool should be usable
        let result: Result<(i32,), _> = sqlx::query_as("SELECT 1").fetch_one(&pool).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_test_state() {
        let (_db, state) = create_test_state().await;
        // State should have valid XDS state
        assert!(state.xds_state.cluster_repository.is_some());
    }

    #[tokio::test]
    async fn test_admin_auth_context() {
        let context = admin_auth_context();
        assert!(context.has_scope("admin:all"));
    }

    #[tokio::test]
    async fn test_team_auth_context() {
        let context = team_auth_context("my-team");
        assert!(context.has_grant("clusters", "create", "my-team"));
        assert!(context.has_grant("routes", "read", "my-team"));
        assert!(!context.has_scope("admin:all"));
    }

    #[tokio::test]
    async fn test_readonly_auth_context() {
        let context = readonly_auth_context("my-team");
        assert!(context.has_grant("clusters", "read", "my-team"));
        assert!(!context.has_grant("clusters", "create", "my-team"));
    }

    #[tokio::test]
    async fn test_team_builder() {
        let (_db, pool) = create_test_pool().await;

        let team_id =
            TestTeamBuilder::new("test-team").with_display_name("My Test Team").insert(&pool).await;

        assert!(!team_id.as_str().is_empty());
    }

    #[tokio::test]
    async fn test_user_builder() {
        let (_db, pool) = create_test_pool().await;

        let user_id =
            TestUserBuilder::new("testuser@test.com").with_name("Test User").insert(&pool).await;

        assert!(!user_id.as_str().is_empty());
    }

    #[test]
    fn test_assert_status_macro() {
        let response = (StatusCode::OK, ());
        assert_status!(response, StatusCode::OK);
    }

    #[tokio::test]
    async fn test_resource_auth_context() {
        let context = resource_auth_context("tokens");
        assert!(context.has_scope("admin:all"));
        assert!(context.has_grant("tokens", "read", "test-team"));
        assert!(context.has_grant("tokens", "create", "test-team"));
    }

    #[tokio::test]
    async fn test_readonly_resource_auth_context() {
        let context = readonly_resource_auth_context("tokens");
        assert!(context.has_grant("tokens", "read", "test-team"));
        assert!(!context.has_grant("tokens", "create", "test-team"));
    }

    #[tokio::test]
    async fn test_minimal_auth_context() {
        let context = minimal_auth_context();
        assert!(!context.has_scope("admin:all"));
        assert!(!context.has_any_grant("clusters", "read"));
    }
}
