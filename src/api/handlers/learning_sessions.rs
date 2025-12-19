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
    api::{error::ApiError, routes::ApiState},
    auth::authorization::{extract_team_scopes, require_resource_access},
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
    "routePattern": "^/api/v2/payments/.*",
    "clusterName": "payments-api-prod",
    "httpMethods": ["POST", "PUT"],
    "targetSampleCount": 1000,
    "maxDurationSeconds": 7200,
    "triggeredBy": "deploy-pipeline-v2.3.4",
    "deploymentVersion": "v2.3.4"
}))]
pub struct CreateLearningSessionBody {
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

/// Verify that a learning session belongs to one of the user's teams.
/// Returns the session if authorized, otherwise returns NotFound error.
async fn verify_session_access(
    session: crate::storage::repositories::LearningSessionData,
    team_scopes: &[String],
) -> Result<crate::storage::repositories::LearningSessionData, ApiError> {
    // Admin:all or resource-level scopes (empty team_scopes) can access everything
    if team_scopes.is_empty() {
        return Ok(session);
    }

    // Check if session belongs to one of user's teams
    if team_scopes.contains(&session.team) {
        Ok(session)
    } else {
        // Record cross-team access attempt for security monitoring
        if let Some(from_team) = team_scopes.first() {
            crate::observability::metrics::record_cross_team_access_attempt(
                from_team,
                &session.team,
                "learning_sessions",
            )
            .await;
        }

        // Return 404 to avoid leaking existence of other teams' resources
        Err(ApiError::NotFound(format!("Learning session with ID '{}' not found", session.id)))
    }
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
    // Authorization: require learning-sessions:write scope
    require_resource_access(&context, "learning-sessions", "write", None)?;

    // Extract team from auth context (team-scoped users create for their team)
    let team = extract_team_scopes(&context).into_iter().next().ok_or_else(|| {
        ApiError::BadRequest("Team scope required for learning sessions".to_string())
    })?;

    // Validate payload
    use validator::Validate;
    payload.validate().map_err(|err| ApiError::from(Error::from(err)))?;

    // Validate regex pattern
    if let Err(e) = regex::Regex::new(&payload.route_pattern) {
        return Err(ApiError::BadRequest(format!("Invalid route pattern regex: {}", e)));
    }

    // Get repository
    let repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    let session_repo = LearningSessionRepository::new(repo.pool().clone());

    // Create session
    let create_request = CreateLearningSessionRequest {
        team: team.clone(),
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
        tracing::error!(error = %e, team = %team, "Failed to create learning session");
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

    // Extract team from auth context
    let team = extract_team_scopes(&context).into_iter().next().ok_or_else(|| {
        ApiError::BadRequest("Team scope required for learning sessions".to_string())
    })?;

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

    // List sessions
    let sessions = session_repo
        .list_by_team(&team, status_filter, query.limit, query.offset)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to list learning sessions");
            ApiError::Internal(format!("Failed to list learning sessions: {}", e))
        })?;

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

    // Extract team from auth context
    let team_scopes = extract_team_scopes(&context);
    let team = team_scopes.first().ok_or_else(|| {
        ApiError::BadRequest("Team scope required for learning sessions".to_string())
    })?;

    // Get repository
    let repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    let session_repo = LearningSessionRepository::new(repo.pool().clone());

    // Get session
    let session = session_repo.get_by_id_and_team(&id, team).await.map_err(|e| {
        tracing::error!(error = %e, session_id = %id, team = %team, "Failed to get learning session");
        match e {
            Error::NotFound { .. } => ApiError::NotFound(format!("Learning session with ID '{}' not found", id)),
            _ => ApiError::Internal(format!("Failed to get learning session: {}", e)),
        }
    })?;

    // Verify access
    let authorized_session = verify_session_access(session, &team_scopes).await?;

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
    // Authorization: require learning-sessions:write scope
    require_resource_access(&context, "learning-sessions", "write", None)?;

    // Extract team from auth context
    let team_scopes = extract_team_scopes(&context);
    let team = team_scopes.first().ok_or_else(|| {
        ApiError::BadRequest("Team scope required for learning sessions".to_string())
    })?;

    // Get repository
    let repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Repository not configured".to_string()))?;

    let session_repo = LearningSessionRepository::new(repo.pool().clone());

    // First, get the session to update its status to cancelled
    let session = session_repo.get_by_id_and_team(&id, team).await.map_err(|e| {
        tracing::error!(error = %e, session_id = %id, team = %team, "Failed to get learning session for cancellation");
        match e {
            Error::NotFound { .. } => ApiError::NotFound(format!("Learning session with ID '{}' not found", id)),
            _ => ApiError::Internal(format!("Failed to get learning session: {}", e)),
        }
    })?;

    // Verify access
    verify_session_access(session.clone(), &team_scopes).await?;

    // Use the learning session service to properly handle cancellation
    // This ensures Access Log Service is unregistered
    if let Some(learning_service) = state.xds_state.get_learning_session_service() {
        // If session is active, we need to unregister from Access Log Service
        // The fail_session method handles this
        learning_service.fail_session(&id, "Cancelled by user".to_string()).await.map_err(|e| {
            tracing::error!(error = %e, session_id = %id, team = %team, "Failed to cancel learning session via service");
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
            tracing::error!(error = %e, session_id = %id, team = %team, "Failed to cancel learning session");
            ApiError::Internal(format!("Failed to cancel learning session: {}", e))
        })?;
    }

    Ok(StatusCode::NO_CONTENT)
}
