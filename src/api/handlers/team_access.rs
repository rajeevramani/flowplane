//! Team Access Verification
//!
//! This module provides unified team-based access control for all resources.
//! It eliminates duplication across handlers by providing a single, generic
//! implementation of team access verification.

use crate::api::error::ApiError;
use crate::auth::authorization::{extract_team_scopes, has_admin_bypass};
use crate::auth::models::AuthContext;

/// Trait for resources that belong to a team.
///
/// Implement this trait for any resource type that needs team-based access control.
/// The trait provides the necessary information to verify access and generate
/// appropriate error messages.
pub trait TeamOwned {
    /// Returns the team that owns this resource.
    ///
    /// - `Some(team)` - Resource belongs to a specific team
    /// - `None` - Resource is global (accessible to all teams)
    fn team(&self) -> Option<&str>;

    /// Returns the resource identifier (typically name or id).
    ///
    /// Used in error messages: "Resource with name '{name}' not found"
    fn resource_name(&self) -> &str;

    /// Returns the resource type name for metrics and error messages.
    ///
    /// Examples: "Cluster", "Listener", "Learning session"
    fn resource_type() -> &'static str;

    /// Returns the resource type identifier for metrics tracking.
    ///
    /// Examples: "clusters", "listeners", "learning_sessions"
    fn resource_type_metric() -> &'static str;

    /// Returns the identifier label for error messages.
    ///
    /// Default is "name", override for resources that use "ID" etc.
    fn identifier_label() -> &'static str {
        "name"
    }
}

/// Get effective team scopes from auth context.
///
/// This is the single source of truth for extracting team scopes with admin bypass.
/// Admin users (with admin:all scope) get an empty vec, allowing access to all resources.
///
/// # Arguments
/// * `context` - The authentication context from the request
///
/// # Returns
/// * Empty `Vec` for admin users (bypass all team checks)
/// * Team scopes for regular users
pub fn get_effective_team_scopes(context: &AuthContext) -> Vec<String> {
    if has_admin_bypass(context) {
        Vec::new()
    } else {
        extract_team_scopes(context)
    }
}

/// Verify that a resource belongs to one of the user's teams or is global.
///
/// This is the unified access verification function that replaces all the
/// duplicate `verify_*_access` functions across handlers.
///
/// # Behavior
/// - Returns `Ok(resource)` if:
///   - `team_scopes` is empty (admin bypass)
///   - Resource has no team (global resource)
///   - Resource's team is in `team_scopes`
/// - Returns `Err(NotFound)` if resource's team is not in `team_scopes`
/// - Records cross-team access attempts for security monitoring
///
/// # Arguments
/// * `resource` - The resource to verify access for
/// * `team_scopes` - The user's team scopes (empty for admin bypass)
///
/// # Returns
/// * `Ok(resource)` if access is allowed
/// * `Err(ApiError::NotFound)` if access is denied (to avoid leaking existence)
pub async fn verify_team_access<T: TeamOwned>(
    resource: T,
    team_scopes: &[String],
) -> Result<T, ApiError> {
    // Admin:all or resource-level scopes (empty team_scopes) can access everything
    if team_scopes.is_empty() {
        return Ok(resource);
    }

    // Get the resource's team
    match resource.team() {
        // Global resource (team = NULL) - accessible to all
        None => Ok(resource),

        // Team-owned resource - verify membership
        Some(resource_team) => {
            if team_scopes.iter().any(|scope| scope == resource_team) {
                Ok(resource)
            } else {
                // Record cross-team access attempt for security monitoring
                if let Some(from_team) = team_scopes.first() {
                    crate::observability::metrics::record_cross_team_access_attempt(
                        from_team,
                        resource_team,
                        T::resource_type_metric(),
                    )
                    .await;
                }

                // Return 404 to avoid leaking existence of other teams' resources
                Err(ApiError::NotFound(format!(
                    "{} with {} '{}' not found",
                    T::resource_type(),
                    T::identifier_label(),
                    resource.resource_name()
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test resource for unit tests
    #[derive(Debug, Clone)]
    struct TestResource {
        name: String,
        team: Option<String>,
    }

    impl TeamOwned for TestResource {
        fn team(&self) -> Option<&str> {
            self.team.as_deref()
        }

        fn resource_name(&self) -> &str {
            &self.name
        }

        fn resource_type() -> &'static str {
            "Test resource"
        }

        fn resource_type_metric() -> &'static str {
            "test_resources"
        }
    }

    /// Test resource with non-optional team (like SecretData)
    #[derive(Debug, Clone)]
    struct TestSecretResource {
        name: String,
        team: String,
    }

    impl TeamOwned for TestSecretResource {
        fn team(&self) -> Option<&str> {
            Some(&self.team)
        }

        fn resource_name(&self) -> &str {
            &self.name
        }

        fn resource_type() -> &'static str {
            "Secret"
        }

        fn resource_type_metric() -> &'static str {
            "secrets"
        }
    }

    /// Test resource with ID instead of name (like LearningSessionData)
    #[derive(Debug, Clone)]
    struct TestSessionResource {
        id: String,
        team: String,
    }

    impl TeamOwned for TestSessionResource {
        fn team(&self) -> Option<&str> {
            Some(&self.team)
        }

        fn resource_name(&self) -> &str {
            &self.id
        }

        fn resource_type() -> &'static str {
            "Learning session"
        }

        fn resource_type_metric() -> &'static str {
            "learning_sessions"
        }

        fn identifier_label() -> &'static str {
            "ID"
        }
    }

    #[tokio::test]
    async fn test_admin_bypass_allows_access() {
        let resource =
            TestResource { name: "test-cluster".to_string(), team: Some("other-team".to_string()) };

        // Empty team_scopes = admin bypass
        let result = verify_team_access(resource, &[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_global_resource_accessible_to_all() {
        let resource = TestResource {
            name: "global-cluster".to_string(),
            team: None, // Global resource
        };

        let team_scopes = vec!["team-a".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_same_team_allows_access() {
        let resource =
            TestResource { name: "team-cluster".to_string(), team: Some("team-a".to_string()) };

        let team_scopes = vec!["team-a".to_string(), "team-b".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_different_team_denies_access() {
        let resource =
            TestResource { name: "secret-cluster".to_string(), team: Some("team-x".to_string()) };

        let team_scopes = vec!["team-a".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_error_message_format() {
        let resource =
            TestResource { name: "my-cluster".to_string(), team: Some("team-x".to_string()) };

        let team_scopes = vec!["team-a".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;

        let err = result.unwrap_err();
        if let ApiError::NotFound(msg) = err {
            assert_eq!(msg, "Test resource with name 'my-cluster' not found");
        } else {
            panic!("Expected NotFound error");
        }
    }

    #[tokio::test]
    async fn test_secret_resource_with_required_team() {
        let resource =
            TestSecretResource { name: "my-secret".to_string(), team: "team-a".to_string() };

        let team_scopes = vec!["team-a".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_session_resource_with_id_label() {
        let resource =
            TestSessionResource { id: "session-123".to_string(), team: "team-x".to_string() };

        let team_scopes = vec!["team-a".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;

        let err = result.unwrap_err();
        if let ApiError::NotFound(msg) = err {
            assert_eq!(msg, "Learning session with ID 'session-123' not found");
        } else {
            panic!("Expected NotFound error");
        }
    }

    #[tokio::test]
    async fn test_multiple_team_scopes() {
        let resource = TestResource {
            name: "multi-team-cluster".to_string(),
            team: Some("team-c".to_string()),
        };

        // User has access to multiple teams, including team-c
        let team_scopes = vec!["team-a".to_string(), "team-b".to_string(), "team-c".to_string()];
        let result = verify_team_access(resource, &team_scopes).await;
        assert!(result.is_ok());
    }
}
