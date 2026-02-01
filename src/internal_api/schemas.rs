//! Aggregated Schema Operations for Internal API
//!
//! This module provides the unified aggregated schema operations layer that sits between
//! HTTP/MCP handlers and the repository layer. It handles:
//! - Request validation
//! - Team-based access control
//! - Error mapping
//! - Response formatting

use std::sync::Arc;
use tracing::{info, instrument};

use crate::internal_api::auth::{verify_team_access, InternalAuthContext};
use crate::internal_api::error::InternalError;
use crate::internal_api::types::{ListSchemasRequest, ListSchemasResponse};
use crate::storage::repositories::aggregated_schema::AggregatedSchemaData;
use crate::xds::XdsState;

/// Aggregated schema operations for the internal API layer
///
/// This struct provides all read operations for aggregated schemas with unified
/// validation and access control.
pub struct AggregatedSchemaOperations {
    xds_state: Arc<XdsState>,
}

impl AggregatedSchemaOperations {
    /// Create a new AggregatedSchemaOperations instance
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// List aggregated schemas with optional filters
    ///
    /// # Arguments
    /// * `req` - The list request with optional filters
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(ListSchemasResponse)` with filtered schemas
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, auth), fields(
        path = ?req.path,
        http_method = ?req.http_method,
        min_confidence = ?req.min_confidence,
        latest_only = ?req.latest_only
    ))]
    pub async fn list(
        &self,
        req: ListSchemasRequest,
        auth: &InternalAuthContext,
    ) -> Result<ListSchemasResponse, InternalError> {
        let repository = self.xds_state.aggregated_schema_repository.as_ref().ok_or_else(|| {
            InternalError::service_unavailable("Aggregated schema repository unavailable")
        })?;

        // Determine which team to query
        // - Admin users (empty allowed_teams) can see all teams, but we need a team filter
        //   for the repository query. For now, we'll require a team even for admins.
        // - Non-admin users query their first allowed team
        let team = if auth.is_admin {
            // For admin, if we don't have a specific team filter, we need to handle this differently
            // For simplicity, we'll require the caller to specify a team or we return an error
            return Err(InternalError::validation(
                "Admin users must specify a team filter for schema listing",
            ));
        } else {
            auth.allowed_teams
                .first()
                .ok_or_else(|| InternalError::forbidden("No team access for listing schemas"))?
        };

        // Call appropriate repository method based on filters
        let schemas = if req.latest_only.unwrap_or(false) {
            // Use list_latest_by_team to get only the latest version of each endpoint
            repository.list_latest_by_team(team).await.map_err(InternalError::from)?
        } else if req.path.is_some() || req.http_method.is_some() || req.min_confidence.is_some() {
            // Use filtered query
            repository
                .list_filtered(
                    team,
                    req.path.as_deref(),
                    req.http_method.as_deref(),
                    req.min_confidence,
                )
                .await
                .map_err(InternalError::from)?
        } else {
            // No filters, get all schemas for the team
            repository.list_by_team(team).await.map_err(InternalError::from)?
        };

        let count = schemas.len();

        info!(
            team = %team,
            count = count,
            latest_only = req.latest_only.unwrap_or(false),
            "Listed aggregated schemas"
        );

        Ok(ListSchemasResponse { schemas, count })
    }

    /// Get a single aggregated schema by ID
    ///
    /// # Arguments
    /// * `id` - The schema ID
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(AggregatedSchemaData)` if found and accessible
    /// * `Err(InternalError::NotFound)` if not found or not accessible
    #[instrument(skip(self, auth), fields(schema_id = %id))]
    pub async fn get(
        &self,
        id: i64,
        auth: &InternalAuthContext,
    ) -> Result<AggregatedSchemaData, InternalError> {
        let repository = self.xds_state.aggregated_schema_repository.as_ref().ok_or_else(|| {
            InternalError::service_unavailable("Aggregated schema repository unavailable")
        })?;

        // Get schema from repository
        let schema = repository.get_by_id(id).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not found") {
                InternalError::not_found("Aggregated schema", id.to_string())
            } else {
                InternalError::from(e)
            }
        })?;

        // Verify team access
        verify_team_access(schema, auth).await
    }

    /// Get version history for a specific endpoint
    ///
    /// # Arguments
    /// * `path` - The API path
    /// * `http_method` - The HTTP method
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(Vec<AggregatedSchemaData>)` with version history, newest first
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, auth), fields(path = %path, http_method = %http_method))]
    pub async fn get_version_history(
        &self,
        path: &str,
        http_method: &str,
        auth: &InternalAuthContext,
    ) -> Result<Vec<AggregatedSchemaData>, InternalError> {
        let repository = self.xds_state.aggregated_schema_repository.as_ref().ok_or_else(|| {
            InternalError::service_unavailable("Aggregated schema repository unavailable")
        })?;

        // Determine team
        let team = if auth.is_admin {
            return Err(InternalError::validation(
                "Admin users must specify a team filter for version history",
            ));
        } else {
            auth.allowed_teams.first().ok_or_else(|| {
                InternalError::forbidden("No team access for schema version history")
            })?
        };

        // Get version history
        let history = repository
            .get_version_history(team, path, http_method)
            .await
            .map_err(InternalError::from)?;

        info!(
            team = %team,
            path = %path,
            http_method = %http_method,
            version_count = history.len(),
            "Retrieved schema version history"
        );

        Ok(history)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::team::CreateTeamRequest;
    use crate::config::SimpleXdsConfig;
    use crate::storage::repositories::aggregated_schema::CreateAggregatedSchemaRequest;
    use crate::storage::repositories::{SqlxTeamRepository, TeamRepository};
    use crate::storage::{create_pool, DatabaseConfig, DbPool};

    fn create_test_config() -> DatabaseConfig {
        DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            auto_migrate: false,
            ..Default::default()
        }
    }

    struct TestSetup {
        state: Arc<XdsState>,
        pool: DbPool,
    }

    async fn setup_state() -> TestSetup {
        let pool = create_pool(&create_test_config()).await.expect("pool");

        // Run migrations to create aggregated_api_schemas table
        sqlx::migrate!("./migrations").run(&pool).await.expect("migrations");

        TestSetup {
            state: Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool.clone())),
            pool,
        }
    }

    async fn create_team(pool: &DbPool, name: &str) {
        let repo = SqlxTeamRepository::new(pool.clone());
        repo.create_team(CreateTeamRequest {
            name: name.to_string(),
            display_name: format!("Test {}", name),
            description: None,
            owner_user_id: None,
            settings: None,
        })
        .await
        .expect("create team");
    }

    async fn create_test_schema(
        state: &Arc<XdsState>,
        team: &str,
        path: &str,
        method: &str,
    ) -> AggregatedSchemaData {
        let repo =
            state.aggregated_schema_repository.as_ref().expect("aggregated schema repository");

        let request = CreateAggregatedSchemaRequest {
            team: team.to_string(),
            path: path.to_string(),
            http_method: method.to_string(),
            request_schema: Some(serde_json::json!({"type": "object"})),
            response_schemas: Some(serde_json::json!({"200": {"type": "object"}})),
            sample_count: 10,
            confidence_score: 0.95,
            breaking_changes: None,
            first_observed: chrono::Utc::now(),
            last_observed: chrono::Utc::now(),
            previous_version_id: None,
        };

        repo.create(request).await.expect("create schema")
    }

    #[tokio::test]
    async fn test_list_schemas_for_team() {
        let setup = setup_state().await;
        create_team(&setup.pool, "team-a").await;
        let ops = AggregatedSchemaOperations::new(setup.state.clone());

        // Create schemas for team-a
        create_test_schema(&setup.state, "team-a", "/users", "GET").await;
        create_test_schema(&setup.state, "team-a", "/products", "GET").await;

        // List as team-a
        let auth = InternalAuthContext::for_team("team-a");
        let req = ListSchemasRequest::default();
        let result = ops.list(req, &auth).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.count, 2);
        assert_eq!(response.schemas.len(), 2);
    }

    #[tokio::test]
    async fn test_list_schemas_filtered_by_path() {
        let setup = setup_state().await;
        create_team(&setup.pool, "team-a").await;
        let ops = AggregatedSchemaOperations::new(setup.state.clone());

        // Create schemas
        create_test_schema(&setup.state, "team-a", "/users", "GET").await;
        create_test_schema(&setup.state, "team-a", "/products", "GET").await;

        // Filter by path
        let auth = InternalAuthContext::for_team("team-a");
        let req = ListSchemasRequest { path: Some("users".to_string()), ..Default::default() };
        let result = ops.list(req, &auth).await.expect("list schemas");

        assert_eq!(result.count, 1);
        assert!(result.schemas[0].path.contains("users"));
    }

    #[tokio::test]
    async fn test_list_schemas_filtered_by_method() {
        let setup = setup_state().await;
        create_team(&setup.pool, "team-a").await;
        let ops = AggregatedSchemaOperations::new(setup.state.clone());

        // Create schemas with different methods
        create_test_schema(&setup.state, "team-a", "/users", "GET").await;
        create_test_schema(&setup.state, "team-a", "/users", "POST").await;

        // Filter by method
        let auth = InternalAuthContext::for_team("team-a");
        let req =
            ListSchemasRequest { http_method: Some("POST".to_string()), ..Default::default() };
        let result = ops.list(req, &auth).await.expect("list schemas");

        assert_eq!(result.count, 1);
        assert_eq!(result.schemas[0].http_method, "POST");
    }

    #[tokio::test]
    async fn test_list_schemas_latest_only() {
        let setup = setup_state().await;
        create_team(&setup.pool, "team-a").await;
        let ops = AggregatedSchemaOperations::new(setup.state.clone());

        // Create multiple versions of the same endpoint
        let schema_v1 = create_test_schema(&setup.state, "team-a", "/users", "GET").await;

        let repo = setup
            .state
            .aggregated_schema_repository
            .as_ref()
            .expect("aggregated schema repository");
        let request_v2 = CreateAggregatedSchemaRequest {
            team: "team-a".to_string(),
            path: "/users".to_string(),
            http_method: "GET".to_string(),
            request_schema: Some(serde_json::json!({"type": "object"})),
            response_schemas: Some(serde_json::json!({"200": {"type": "object"}})),
            sample_count: 20,
            confidence_score: 0.98,
            breaking_changes: None,
            first_observed: chrono::Utc::now(),
            last_observed: chrono::Utc::now(),
            previous_version_id: Some(schema_v1.id),
        };
        repo.create(request_v2).await.expect("create v2");

        // List with latest_only = true
        let auth = InternalAuthContext::for_team("team-a");
        let req = ListSchemasRequest { latest_only: Some(true), ..Default::default() };
        let result = ops.list(req, &auth).await.expect("list schemas");

        // Should only get version 2
        assert_eq!(result.count, 1);
        assert_eq!(result.schemas[0].version, 2);
    }

    #[tokio::test]
    async fn test_get_schema_by_id() {
        let setup = setup_state().await;
        create_team(&setup.pool, "team-a").await;
        let ops = AggregatedSchemaOperations::new(setup.state.clone());

        let created = create_test_schema(&setup.state, "team-a", "/users", "GET").await;

        // Get as team-a
        let auth = InternalAuthContext::for_team("team-a");
        let result = ops.get(created.id, &auth).await;

        assert!(result.is_ok());
        let schema = result.unwrap();
        assert_eq!(schema.id, created.id);
        assert_eq!(schema.team, "team-a");
    }

    #[tokio::test]
    async fn test_get_schema_cross_team_returns_not_found() {
        let setup = setup_state().await;
        create_team(&setup.pool, "team-a").await;
        create_team(&setup.pool, "team-b").await;
        let ops = AggregatedSchemaOperations::new(setup.state.clone());

        let created = create_test_schema(&setup.state, "team-a", "/users", "GET").await;

        // Try to access from team-b
        let auth = InternalAuthContext::for_team("team-b");
        let result = ops.get(created.id, &auth).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_get_schema_not_found() {
        let setup = setup_state().await;
        create_team(&setup.pool, "team-a").await;
        let ops = AggregatedSchemaOperations::new(setup.state);

        let auth = InternalAuthContext::for_team("team-a");
        let result = ops.get(99999, &auth).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_get_version_history() {
        let setup = setup_state().await;
        create_team(&setup.pool, "team-a").await;
        let ops = AggregatedSchemaOperations::new(setup.state.clone());

        // Create multiple versions
        let schema_v1 = create_test_schema(&setup.state, "team-a", "/users", "GET").await;

        let repo = setup
            .state
            .aggregated_schema_repository
            .as_ref()
            .expect("aggregated schema repository");
        for i in 2..=3 {
            let request = CreateAggregatedSchemaRequest {
                team: "team-a".to_string(),
                path: "/users".to_string(),
                http_method: "GET".to_string(),
                request_schema: Some(serde_json::json!({"type": "object"})),
                response_schemas: Some(serde_json::json!({"200": {"type": "object"}})),
                sample_count: i * 10,
                confidence_score: 0.95,
                breaking_changes: None,
                first_observed: chrono::Utc::now(),
                last_observed: chrono::Utc::now(),
                previous_version_id: if i == 2 { Some(schema_v1.id) } else { None },
            };
            repo.create(request).await.expect("create version");
        }

        // Get version history
        let auth = InternalAuthContext::for_team("team-a");
        let result = ops.get_version_history("/users", "GET", &auth).await.expect("get history");

        // Should have 3 versions
        assert_eq!(result.len(), 3);
        // Should be ordered newest first (version 3, 2, 1)
        assert_eq!(result[0].version, 3);
        assert_eq!(result[1].version, 2);
        assert_eq!(result[2].version, 1);
    }

    #[tokio::test]
    async fn test_list_admin_requires_team() {
        let setup = setup_state().await;
        let ops = AggregatedSchemaOperations::new(setup.state);

        // Admin context without team filter should fail
        let auth = InternalAuthContext::admin();
        let req = ListSchemasRequest::default();
        let result = ops.list(req, &auth).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::InvalidInput { .. }));
    }
}
