//! Learning Session HTTP handlers
//!
//! This module provides CRUD operations for learning session lifecycle management
//! through the REST API, with validation and team-scoped authorization.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::ToSchema;
use validator::Validate;

use crate::{
    api::{
        error::ApiError,
        handlers::team_access::{
            get_effective_team_ids, require_resource_access_resolved, team_repo_from_state,
            verify_team_access,
        },
        routes::ApiState,
    },
    auth::authorization::{extract_team_scopes, has_admin_bypass, require_resource_access},
    auth::models::AuthContext,
    errors::Error,
    storage::repositories::{
        CreateLearningSessionRequest, LearningSessionRepository, LearningSessionStatus,
        UpdateLearningSessionRequest,
    },
};

// === DTOs ===

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "team": "engineering",
    "routePattern": "^/api/v2/payments/.*",
    "clusterName": "payments-api-prod",
    "httpMethods": ["POST", "PUT"],
    "targetSampleCount": 1000,
    "maxDurationSeconds": 7200,
    "triggeredBy": "deploy-pipeline-v2.3.4",
    "deploymentVersion": "v2.3.4"
}))]
pub struct CreateLearningSessionBody {
    /// Team identifier for the learning session
    #[validate(length(min = 1, max = 100))]
    #[schema(example = "engineering")]
    pub team: String,

    /// Route pattern (regex) to match for learning
    #[validate(length(min = 1, max = 500))]
    #[schema(example = "^/api/v2/payments/.*")]
    pub route_pattern: String,

    /// Optional cluster name to filter
    #[serde(default)]
    #[schema(example = "payments-api-prod")]
    pub cluster_name: Option<String>,

    /// Optional HTTP methods to filter (e.g., ["GET", "POST"])
    #[serde(default)]
    #[schema(example = json!(["POST", "PUT"]))]
    pub http_methods: Option<Vec<String>>,

    /// Target number of samples to collect
    #[validate(range(min = 1, max = 100000))]
    #[schema(example = 1000, minimum = 1, maximum = 100000)]
    pub target_sample_count: i64,

    /// Maximum duration in seconds (optional)
    #[serde(default)]
    #[schema(example = 7200)]
    pub max_duration_seconds: Option<i64>,

    /// Who/what triggered this session
    #[serde(default)]
    #[schema(example = "deploy-pipeline-v2.3.4")]
    pub triggered_by: Option<String>,

    /// Deployment version being learned
    #[serde(default)]
    #[schema(example = "v2.3.4")]
    pub deployment_version: Option<String>,

    /// Optional configuration snapshot (JSON)
    #[serde(default)]
    pub configuration_snapshot: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LearningSessionResponse {
    pub id: String,
    pub team: String,
    pub route_pattern: String,
    pub cluster_name: Option<String>,
    pub http_methods: Option<Vec<String>>,
    pub status: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub ends_at: Option<String>,
    pub completed_at: Option<String>,
    pub target_sample_count: i64,
    pub current_sample_count: i64,
    pub progress_percentage: f64,
    pub triggered_by: Option<String>,
    pub deployment_version: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ListLearningSessionsQuery {
    #[serde(default)]
    pub status: Option<String>,

    #[serde(default)]
    pub limit: Option<i32>,

    #[serde(default)]
    pub offset: Option<i32>,
}

// === Helper Functions ===

fn session_response_from_data(
    data: crate::storage::repositories::LearningSessionData,
) -> LearningSessionResponse {
    let progress_percentage = if data.target_sample_count > 0 {
        (data.current_sample_count as f64 / data.target_sample_count as f64) * 100.0
    } else {
        0.0
    };

    LearningSessionResponse {
        id: data.id,
        team: data.team,
        route_pattern: data.route_pattern,
        cluster_name: data.cluster_name,
        http_methods: data.http_methods,
        status: data.status.to_string(),
        created_at: data.created_at.to_rfc3339(),
        started_at: data.started_at.map(|t| t.to_rfc3339()),
        ends_at: data.ends_at.map(|t| t.to_rfc3339()),
        completed_at: data.completed_at.map(|t| t.to_rfc3339()),
        target_sample_count: data.target_sample_count,
        current_sample_count: data.current_sample_count,
        progress_percentage,
        triggered_by: data.triggered_by,
        deployment_version: data.deployment_version,
        error_message: data.error_message,
    }
}

/// Extract a single team from auth context for team-scoped operations.
/// Returns BadRequest error if no team scope is available.
fn require_single_team_scope(context: &AuthContext) -> Result<String, ApiError> {
    extract_team_scopes(context).into_iter().next().ok_or_else(|| {
        ApiError::BadRequest("Team scope required for learning sessions".to_string())
    })
}

/// Valid HTTP methods for learning session filtering
const VALID_HTTP_METHODS: &[&str] =
    &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE", "CONNECT"];

/// Validate that all HTTP methods in the list are valid HTTP verbs.
/// Returns BadRequest error if any method is invalid.
fn validate_http_methods(methods: &Option<Vec<String>>) -> Result<(), ApiError> {
    if let Some(methods) = methods {
        for method in methods {
            let upper = method.to_uppercase();
            if !VALID_HTTP_METHODS.contains(&upper.as_str()) {
                return Err(ApiError::BadRequest(format!(
                    "Invalid HTTP method '{}'. Valid methods are: {}",
                    method,
                    VALID_HTTP_METHODS.join(", ")
                )));
            }
        }
    }
    Ok(())
}

// === Handler Implementations ===

#[utoipa::path(
    post,
    path = "/api/v1/learning-sessions",
    request_body = CreateLearningSessionBody,
    responses(
        (status = 201, description = "Learning session created", body = LearningSessionResponse),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 503, description = "Repository unavailable")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state, payload), fields(route_pattern = %payload.route_pattern, user_id = ?context.user_id))]
pub async fn create_learning_session_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Json(payload): Json<CreateLearningSessionBody>,
) -> Result<(StatusCode, Json<LearningSessionResponse>), ApiError> {
    // Authorization: require learning-sessions:write scope for the SPECIFIED team
    require_resource_access_resolved(
        &state,
        &context,
        "learning-sessions",
        "write",
        Some(&payload.team),
        context.org_id.as_ref(),
    )
    .await?;

    // Validate payload
    use validator::Validate;
    payload.validate().map_err(|err| ApiError::from(Error::from(err)))?;

    // Validate regex pattern
    if let Err(e) = regex::Regex::new(&payload.route_pattern) {
        return Err(ApiError::BadRequest(format!("Invalid route pattern regex: {}", e)));
    }

    // Validate HTTP methods
    validate_http_methods(&payload.http_methods)?;

    // Get repository
    let repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    let session_repo = LearningSessionRepository::new(repo.pool().clone());

    // Create session using team from request body
    let create_request = CreateLearningSessionRequest {
        team: payload.team.clone(),
        route_pattern: payload.route_pattern,
        cluster_name: payload.cluster_name,
        http_methods: payload.http_methods,
        target_sample_count: payload.target_sample_count,
        max_duration_seconds: payload.max_duration_seconds,
        triggered_by: payload.triggered_by,
        deployment_version: payload.deployment_version,
        configuration_snapshot: payload.configuration_snapshot,
    };

    let created = session_repo.create(create_request).await.map_err(|e| {
        tracing::error!(error = %e, team = %payload.team, "Failed to create learning session");
        ApiError::Internal(format!("Failed to create learning session: {}", e))
    })?;

    // Automatically activate the session if learning session service is available
    let activated = if let Some(learning_service) = state.xds_state.get_learning_session_service() {
        match learning_service.activate_session(&created.id).await {
            Ok(session) => session,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    session_id = %created.id,
                    "Failed to activate learning session, leaving in pending state"
                );
                created // Return the created session in pending state
            }
        }
    } else {
        created // No service available, return pending session
    };

    let response = session_response_from_data(activated);

    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/learning-sessions",
    params(
        ("status" = Option<String>, Query, description = "Filter by status"),
        ("limit" = Option<i32>, Query, description = "Limit results"),
        ("offset" = Option<i32>, Query, description = "Offset for pagination")
    ),
    responses(
        (status = 200, description = "List of learning sessions", body = Vec<LearningSessionResponse>),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 503, description = "Repository unavailable")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state), fields(user_id = ?context.user_id, status = ?query.status))]
pub async fn list_learning_sessions_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Query(query): Query<ListLearningSessionsQuery>,
) -> Result<Json<Vec<LearningSessionResponse>>, ApiError> {
    // Authorization: require learning-sessions:read scope
    require_resource_access(&context, "learning-sessions", "read", None)?;

    // Get repository
    let repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    let session_repo = LearningSessionRepository::new(repo.pool().clone());

    // Parse status filter
    let status_filter = query
        .status
        .as_ref()
        .map(|s| s.parse::<LearningSessionStatus>())
        .transpose()
        .map_err(|e| ApiError::BadRequest(format!("Invalid status filter: {}", e)))?;

    // List sessions based on authorization
    // Admin users (with admin:all scope) can see all sessions across all teams
    // Regular users can only see sessions for their team
    let sessions = if has_admin_bypass(&context) {
        tracing::info!(user_id = ?context.user_id, "Admin listing all learning sessions");
        session_repo.list_all(status_filter, query.limit, query.offset).await.map_err(|e| {
            tracing::error!(error = %e, "Failed to list all learning sessions");
            ApiError::Internal(format!("Failed to list learning sessions: {}", e))
        })?
    } else {
        // Extract team from auth context for non-admin users and resolve to UUID
        let team_name = require_single_team_scope(&context)?;
        use crate::storage::repositories::TeamRepository as _;
        let team_repo = crate::api::handlers::team_access::team_repo_from_state(&state)?;
        let team_ids = team_repo
            .resolve_team_ids(context.org_id.as_ref(), &[team_name])
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to resolve team ID: {}", e)))?;
        let team = team_ids
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::NotFound("Team not found".to_string()))?;
        session_repo.list_by_team(&team, status_filter, query.limit, query.offset).await.map_err(
            |e| {
                tracing::error!(error = %e, team = %team, "Failed to list learning sessions");
                ApiError::Internal(format!("Failed to list learning sessions: {}", e))
            },
        )?
    };

    let responses: Vec<LearningSessionResponse> =
        sessions.into_iter().map(session_response_from_data).collect();

    Ok(Json(responses))
}

#[utoipa::path(
    get,
    path = "/api/v1/learning-sessions/{id}",
    params(
        ("id" = String, Path, description = "Learning session ID")
    ),
    responses(
        (status = 200, description = "Learning session details", body = LearningSessionResponse),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Learning session not found"),
        (status = 503, description = "Repository unavailable")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state), fields(session_id = %id, user_id = ?context.user_id))]
pub async fn get_learning_session_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<LearningSessionResponse>, ApiError> {
    // Authorization: require learning-sessions:read scope
    require_resource_access(&context, "learning-sessions", "read", None)?;

    // Get effective team scopes for access verification
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;

    // Get repository
    let repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    let session_repo = LearningSessionRepository::new(repo.pool().clone());

    // Get session by ID (without team filter)
    let session = session_repo.get_by_id(&id).await.map_err(|e| {
        tracing::error!(error = %e, session_id = %id, "Failed to get learning session");
        match e {
            Error::NotFound { .. } => {
                ApiError::NotFound(format!("Learning session with ID '{}' not found", id))
            }
            _ => ApiError::Internal(format!("Failed to get learning session: {}", e)),
        }
    })?;

    // Verify user has access to this session's team
    let authorized_session = verify_team_access(session, &team_scopes).await?;

    let response = session_response_from_data(authorized_session);

    Ok(Json(response))
}

#[utoipa::path(
    delete,
    path = "/api/v1/learning-sessions/{id}",
    params(
        ("id" = String, Path, description = "Learning session ID")
    ),
    responses(
        (status = 204, description = "Learning session cancelled"),
        (status = 400, description = "Invalid state transition - session already completed, cancelled, or failed"),
        (status = 403, description = "Forbidden - insufficient permissions"),
        (status = 404, description = "Learning session not found"),
        (status = 503, description = "Repository unavailable")
    ),
    tag = "API Discovery"
)]
#[instrument(skip(state), fields(session_id = %id, user_id = ?context.user_id))]
pub async fn delete_learning_session_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Authorization: require learning-sessions:delete scope
    // Note: This uses the dedicated delete scope (not write) to follow principle of least privilege.
    // Users who should only create/modify sessions need write scope, but cannot delete unless
    // explicitly granted the delete scope.
    require_resource_access(&context, "learning-sessions", "delete", None)?;

    // Get effective team scopes for access verification
    let team_repo = team_repo_from_state(&state)?;
    let team_scopes = get_effective_team_ids(&context, team_repo, context.org_id.as_ref()).await?;

    // Get repository
    let repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    let session_repo = LearningSessionRepository::new(repo.pool().clone());

    // First, get the session by ID (without team filter)
    let session = session_repo.get_by_id(&id).await.map_err(|e| {
        tracing::error!(error = %e, session_id = %id, "Failed to get learning session for cancellation");
        match e {
            Error::NotFound { .. } => ApiError::NotFound(format!("Learning session with ID '{}' not found", id)),
            _ => ApiError::Internal(format!("Failed to get learning session: {}", e)),
        }
    })?;

    // Verify access
    verify_team_access(session.clone(), &team_scopes).await?;

    // Validate session state - only allow cancellation of pending/active/completing sessions
    // Terminal states (completed, cancelled, failed) cannot be cancelled
    match session.status {
        LearningSessionStatus::Pending
        | LearningSessionStatus::Active
        | LearningSessionStatus::Completing => {
            // Valid states for cancellation
        }
        LearningSessionStatus::Completed => {
            tracing::warn!(
                session_id = %id,
                status = %session.status,
                "Attempted to cancel already completed session"
            );
            return Err(ApiError::BadRequest(
                "Cannot cancel a completed learning session".to_string(),
            ));
        }
        LearningSessionStatus::Cancelled => {
            tracing::warn!(
                session_id = %id,
                status = %session.status,
                "Attempted to cancel already cancelled session"
            );
            return Err(ApiError::BadRequest("Learning session is already cancelled".to_string()));
        }
        LearningSessionStatus::Failed => {
            tracing::warn!(
                session_id = %id,
                status = %session.status,
                "Attempted to cancel already failed session"
            );
            return Err(ApiError::BadRequest(
                "Cannot cancel a failed learning session".to_string(),
            ));
        }
    }

    // Use the learning session service to properly handle cancellation
    // This ensures Access Log Service is unregistered
    if let Some(learning_service) = state.xds_state.get_learning_session_service() {
        // If session is active, we need to unregister from Access Log Service
        // The fail_session method handles this
        learning_service.fail_session(&id, "Cancelled by user".to_string()).await.map_err(|e| {
            tracing::error!(error = %e, session_id = %id, team = %session.team, "Failed to cancel learning session via service");
            ApiError::Internal(format!("Failed to cancel learning session: {}", e))
        })?;
    } else {
        // Fallback to direct repository update if service not available
        let update_request = UpdateLearningSessionRequest {
            status: Some(LearningSessionStatus::Cancelled),
            started_at: None,
            ends_at: None,
            completed_at: Some(chrono::Utc::now()),
            current_sample_count: None,
            error_message: Some("Cancelled by user".to_string()),
        };

        session_repo.update(&id, update_request).await.map_err(|e| {
            tracing::error!(error = %e, session_id = %id, team = %session.team, "Failed to cancel learning session");
            ApiError::Internal(format!("Failed to cancel learning session: {}", e))
        })?;
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests for session_response_from_data helper function
    mod response_conversion {
        use super::*;
        use crate::storage::repositories::{LearningSessionData, LearningSessionStatus};

        fn sample_session_data() -> LearningSessionData {
            LearningSessionData {
                id: "test-session-123".to_string(),
                team: "test-team".to_string(),
                route_pattern: "^/api/v1/.*".to_string(),
                cluster_name: Some("api-cluster".to_string()),
                http_methods: Some(vec!["GET".to_string(), "POST".to_string()]),
                status: LearningSessionStatus::Active,
                created_at: chrono::Utc::now(),
                started_at: Some(chrono::Utc::now()),
                ends_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                completed_at: None,
                target_sample_count: 100,
                current_sample_count: 50,
                triggered_by: Some("deploy-pipeline".to_string()),
                deployment_version: Some("v1.0.0".to_string()),
                configuration_snapshot: None,
                error_message: None,
                updated_at: chrono::Utc::now(),
            }
        }

        #[test]
        fn test_session_response_progress_percentage() {
            let data = sample_session_data();
            let response = session_response_from_data(data);

            assert_eq!(response.progress_percentage, 50.0);
            assert_eq!(response.current_sample_count, 50);
            assert_eq!(response.target_sample_count, 100);
        }

        #[test]
        fn test_session_response_zero_target() {
            let mut data = sample_session_data();
            data.target_sample_count = 0;
            data.current_sample_count = 10;

            let response = session_response_from_data(data);

            // Should return 0% when target is 0 (avoid division by zero)
            assert_eq!(response.progress_percentage, 0.0);
        }

        #[test]
        fn test_session_response_over_target() {
            let mut data = sample_session_data();
            data.target_sample_count = 100;
            data.current_sample_count = 150;

            let response = session_response_from_data(data);

            // Progress can exceed 100%
            assert_eq!(response.progress_percentage, 150.0);
        }

        #[test]
        fn test_session_response_status_serialization() {
            let data = sample_session_data();
            let response = session_response_from_data(data);

            assert_eq!(response.status, "active");
        }

        #[test]
        fn test_session_response_all_statuses() {
            for (status, expected_str) in [
                (LearningSessionStatus::Pending, "pending"),
                (LearningSessionStatus::Active, "active"),
                (LearningSessionStatus::Completing, "completing"),
                (LearningSessionStatus::Completed, "completed"),
                (LearningSessionStatus::Failed, "failed"),
                (LearningSessionStatus::Cancelled, "cancelled"),
            ] {
                let mut data = sample_session_data();
                data.status = status;
                let response = session_response_from_data(data);
                assert_eq!(response.status, expected_str);
            }
        }

        #[test]
        fn test_session_response_optional_fields() {
            let mut data = sample_session_data();
            data.cluster_name = None;
            data.http_methods = None;
            data.started_at = None;
            data.ends_at = None;
            data.completed_at = None;
            data.triggered_by = None;
            data.deployment_version = None;
            data.error_message = None;

            let response = session_response_from_data(data);

            assert!(response.cluster_name.is_none());
            assert!(response.http_methods.is_none());
            assert!(response.started_at.is_none());
            assert!(response.ends_at.is_none());
            assert!(response.completed_at.is_none());
            assert!(response.triggered_by.is_none());
            assert!(response.deployment_version.is_none());
            assert!(response.error_message.is_none());
        }

        #[test]
        fn test_session_response_with_error_message() {
            let mut data = sample_session_data();
            data.status = LearningSessionStatus::Failed;
            data.error_message = Some("Connection timeout".to_string());

            let response = session_response_from_data(data);

            assert_eq!(response.status, "failed");
            assert_eq!(response.error_message, Some("Connection timeout".to_string()));
        }
    }

    /// Tests for session access verification
    mod access_verification {
        use super::*;
        use crate::storage::repositories::{LearningSessionData, LearningSessionStatus};

        fn sample_session(team: &str) -> LearningSessionData {
            LearningSessionData {
                id: "test-session".to_string(),
                team: team.to_string(),
                route_pattern: "^/api/.*".to_string(),
                cluster_name: None,
                http_methods: None,
                status: LearningSessionStatus::Active,
                created_at: chrono::Utc::now(),
                started_at: None,
                ends_at: None,
                completed_at: None,
                target_sample_count: 100,
                current_sample_count: 0,
                triggered_by: None,
                deployment_version: None,
                configuration_snapshot: None,
                error_message: None,
                updated_at: chrono::Utc::now(),
            }
        }

        #[tokio::test]
        async fn test_verify_access_same_team() {
            let session = sample_session("team-a");
            let team_scopes = vec!["team-a".to_string()];

            let result = verify_team_access(session.clone(), &team_scopes).await;

            assert!(result.is_ok());
            assert_eq!(result.unwrap().team, "team-a");
        }

        #[tokio::test]
        async fn test_verify_access_different_team() {
            let session = sample_session("team-a");
            let team_scopes = vec!["team-b".to_string()];

            let result = verify_team_access(session, &team_scopes).await;

            assert!(result.is_err());
            match result {
                Err(ApiError::NotFound(_)) => {} // Expected - return 404 to avoid leaking info
                _ => panic!("Expected NotFound error for cross-team access"),
            }
        }

        #[tokio::test]
        async fn test_verify_access_admin_empty_scopes() {
            // Admin users have empty team_scopes and can access everything
            let session = sample_session("any-team");
            let team_scopes: Vec<String> = vec![];

            let result = verify_team_access(session.clone(), &team_scopes).await;

            assert!(result.is_ok());
            assert_eq!(result.unwrap().team, "any-team");
        }

        #[tokio::test]
        async fn test_verify_access_multiple_teams() {
            let session = sample_session("team-b");
            let team_scopes =
                vec!["team-a".to_string(), "team-b".to_string(), "team-c".to_string()];

            let result = verify_team_access(session.clone(), &team_scopes).await;

            assert!(result.is_ok());
        }
    }

    /// Tests for request validation
    mod validation {
        use super::*;

        #[test]
        fn test_valid_route_pattern_regex() {
            let pattern = "^/api/v1/users/.*";
            let result = regex::Regex::new(pattern);
            assert!(result.is_ok());
        }

        #[test]
        fn test_invalid_route_pattern_regex() {
            let pattern = "[invalid(regex";
            let result = regex::Regex::new(pattern);
            assert!(result.is_err());
        }

        #[test]
        fn test_create_body_validation_valid() {
            let body = CreateLearningSessionBody {
                team: "engineering".to_string(),
                route_pattern: "^/api/.*".to_string(),
                cluster_name: None,
                http_methods: Some(vec!["GET".to_string(), "POST".to_string()]),
                target_sample_count: 1000,
                max_duration_seconds: Some(3600),
                triggered_by: None,
                deployment_version: None,
                configuration_snapshot: None,
            };

            use validator::Validate;
            let result = body.validate();
            assert!(result.is_ok());
        }

        #[test]
        fn test_create_body_validation_sample_count_too_low() {
            let body = CreateLearningSessionBody {
                team: "engineering".to_string(),
                route_pattern: "^/api/.*".to_string(),
                cluster_name: None,
                http_methods: None,
                target_sample_count: 0, // Below minimum of 1
                max_duration_seconds: None,
                triggered_by: None,
                deployment_version: None,
                configuration_snapshot: None,
            };

            use validator::Validate;
            let result = body.validate();
            assert!(result.is_err());
        }

        #[test]
        fn test_create_body_validation_sample_count_too_high() {
            let body = CreateLearningSessionBody {
                team: "engineering".to_string(),
                route_pattern: "^/api/.*".to_string(),
                cluster_name: None,
                http_methods: None,
                target_sample_count: 100001, // Above maximum of 100000
                max_duration_seconds: None,
                triggered_by: None,
                deployment_version: None,
                configuration_snapshot: None,
            };

            use validator::Validate;
            let result = body.validate();
            assert!(result.is_err());
        }

        #[test]
        fn test_create_body_validation_empty_route_pattern() {
            let body = CreateLearningSessionBody {
                team: "engineering".to_string(),
                route_pattern: "".to_string(), // Empty pattern
                cluster_name: None,
                http_methods: None,
                target_sample_count: 100,
                max_duration_seconds: None,
                triggered_by: None,
                deployment_version: None,
                configuration_snapshot: None,
            };

            use validator::Validate;
            let result = body.validate();
            assert!(result.is_err());
        }

        #[test]
        fn test_create_body_validation_boundary_values() {
            // Minimum valid sample count
            let body_min = CreateLearningSessionBody {
                team: "engineering".to_string(),
                route_pattern: "^/".to_string(),
                cluster_name: None,
                http_methods: None,
                target_sample_count: 1,
                max_duration_seconds: None,
                triggered_by: None,
                deployment_version: None,
                configuration_snapshot: None,
            };

            use validator::Validate;
            assert!(body_min.validate().is_ok());

            // Maximum valid sample count
            let body_max = CreateLearningSessionBody {
                team: "engineering".to_string(),
                route_pattern: "^/".to_string(),
                cluster_name: None,
                http_methods: None,
                target_sample_count: 100000,
                max_duration_seconds: None,
                triggered_by: None,
                deployment_version: None,
                configuration_snapshot: None,
            };

            assert!(body_max.validate().is_ok());
        }

        #[test]
        fn test_create_body_validation_empty_team() {
            let body = CreateLearningSessionBody {
                team: "".to_string(), // Empty team
                route_pattern: "^/api/.*".to_string(),
                cluster_name: None,
                http_methods: None,
                target_sample_count: 100,
                max_duration_seconds: None,
                triggered_by: None,
                deployment_version: None,
                configuration_snapshot: None,
            };

            use validator::Validate;
            let result = body.validate();
            assert!(result.is_err());
        }

        #[test]
        fn test_validate_http_methods_valid() {
            let methods = Some(vec!["GET".to_string(), "POST".to_string(), "PUT".to_string()]);
            let result = validate_http_methods(&methods);
            assert!(result.is_ok());
        }

        #[test]
        fn test_validate_http_methods_lowercase_valid() {
            let methods = Some(vec!["get".to_string(), "post".to_string()]);
            let result = validate_http_methods(&methods);
            assert!(result.is_ok());
        }

        #[test]
        fn test_validate_http_methods_mixed_case_valid() {
            let methods = Some(vec!["Get".to_string(), "Post".to_string()]);
            let result = validate_http_methods(&methods);
            assert!(result.is_ok());
        }

        #[test]
        fn test_validate_http_methods_all_valid() {
            let methods = Some(vec![
                "GET".to_string(),
                "POST".to_string(),
                "PUT".to_string(),
                "DELETE".to_string(),
                "PATCH".to_string(),
                "HEAD".to_string(),
                "OPTIONS".to_string(),
                "TRACE".to_string(),
                "CONNECT".to_string(),
            ]);
            let result = validate_http_methods(&methods);
            assert!(result.is_ok());
        }

        #[test]
        fn test_validate_http_methods_invalid() {
            let methods = Some(vec!["GET".to_string(), "INVALID".to_string()]);
            let result = validate_http_methods(&methods);
            assert!(result.is_err());
            match result {
                Err(ApiError::BadRequest(msg)) => {
                    assert!(msg.contains("INVALID"));
                    assert!(msg.contains("Valid methods are"));
                }
                _ => panic!("Expected BadRequest error"),
            }
        }

        #[test]
        fn test_validate_http_methods_none() {
            let methods: Option<Vec<String>> = None;
            let result = validate_http_methods(&methods);
            assert!(result.is_ok());
        }

        #[test]
        fn test_validate_http_methods_empty() {
            let methods = Some(vec![]);
            let result = validate_http_methods(&methods);
            assert!(result.is_ok());
        }
    }

    /// Tests for query parameter parsing
    mod query_parsing {
        use super::*;
        use crate::storage::repositories::LearningSessionStatus;

        #[test]
        fn test_status_filter_parsing_valid() {
            for status_str in
                ["pending", "active", "completing", "completed", "failed", "cancelled"]
            {
                let result = status_str.parse::<LearningSessionStatus>();
                assert!(result.is_ok(), "Failed to parse status: {}", status_str);
            }
        }

        #[test]
        fn test_status_filter_parsing_invalid() {
            let result = "invalid_status".parse::<LearningSessionStatus>();
            assert!(result.is_err());
        }

        #[test]
        fn test_list_query_default_values() {
            let query: ListLearningSessionsQuery = serde_json::from_str("{}").unwrap();
            assert!(query.status.is_none());
            assert!(query.limit.is_none());
            assert!(query.offset.is_none());
        }

        #[test]
        fn test_list_query_with_values() {
            let query: ListLearningSessionsQuery =
                serde_json::from_str(r#"{"status": "active", "limit": 50, "offset": 10}"#).unwrap();
            assert_eq!(query.status, Some("active".to_string()));
            assert_eq!(query.limit, Some(50));
            assert_eq!(query.offset, Some(10));
        }
    }
}
