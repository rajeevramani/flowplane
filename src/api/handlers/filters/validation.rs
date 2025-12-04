//! Validation helpers for filter API handlers

use crate::{
    api::{error::ApiError, routes::ApiState},
    domain::FilterConfig,
    errors::Error,
    storage::{FilterData, FilterRepository},
};

use super::types::{CreateFilterRequest, FilterResponse, UpdateFilterRequest};

/// Require filter repository to be configured
pub fn require_filter_repository(state: &ApiState) -> Result<FilterRepository, ApiError> {
    state
        .xds_state
        .filter_repository
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::internal("Filter repository not configured"))
}

/// Validate create filter request
pub fn validate_create_filter_request(payload: &CreateFilterRequest) -> Result<(), ApiError> {
    // Validate name is not empty
    if payload.name.trim().is_empty() {
        return Err(ApiError::validation("Filter name cannot be empty"));
    }

    // Validate name length
    if payload.name.len() > 255 {
        return Err(ApiError::validation("Filter name must be 255 characters or less"));
    }

    // Validate team is not empty
    if payload.team.trim().is_empty() {
        return Err(ApiError::validation("Team name cannot be empty"));
    }

    // Validate filter type is fully implemented
    if !payload.filter_type.is_fully_implemented() {
        return Err(ApiError::validation(format!(
            "Filter type '{}' is not yet supported. Available types: header_mutation, jwt_auth, local_rate_limit",
            payload.filter_type
        )));
    }

    // Validate filter type matches config
    tracing::debug!(
        filter_type = ?payload.filter_type,
        config_type = ?std::mem::discriminant(&payload.config),
        "Validating filter type matches config"
    );
    match (&payload.filter_type, &payload.config) {
        (crate::domain::FilterType::HeaderMutation, FilterConfig::HeaderMutation(_)) => Ok(()),
        (crate::domain::FilterType::JwtAuth, FilterConfig::JwtAuth(_)) => Ok(()),
        (crate::domain::FilterType::LocalRateLimit, FilterConfig::LocalRateLimit(_)) => Ok(()),
        _ => {
            tracing::warn!(
                filter_type = ?payload.filter_type,
                config = ?payload.config,
                "Filter type and configuration do not match"
            );
            Err(ApiError::validation("Filter type and configuration do not match"))
        }
    }
}

/// Validate update filter request
pub fn validate_update_filter_request(payload: &UpdateFilterRequest) -> Result<(), ApiError> {
    // Validate name if provided
    if let Some(ref name) = payload.name {
        if name.trim().is_empty() {
            return Err(ApiError::validation("Filter name cannot be empty"));
        }
        if name.len() > 255 {
            return Err(ApiError::validation("Filter name must be 255 characters or less"));
        }
    }

    Ok(())
}

/// Parse filter configuration from stored JSON
pub fn parse_filter_config(data: &FilterData) -> Result<FilterConfig, ApiError> {
    serde_json::from_str(&data.configuration).map_err(|err| {
        ApiError::from(Error::internal(format!(
            "Failed to parse stored filter configuration: {}",
            err
        )))
    })
}

/// Convert FilterData to FilterResponse
pub fn filter_response_from_data(data: FilterData) -> Result<FilterResponse, ApiError> {
    let config = parse_filter_config(&data)?;
    Ok(FilterResponse::from_data(data, config))
}

/// Verify that a filter belongs to one of the user's teams
pub async fn verify_filter_access(
    filter: FilterData,
    team_scopes: &[String],
) -> Result<FilterData, ApiError> {
    // Admin:all or resource-level scopes (empty team_scopes) can access everything
    if team_scopes.is_empty() {
        return Ok(filter);
    }

    // Check if filter belongs to one of user's teams
    if team_scopes.contains(&filter.team) {
        Ok(filter)
    } else {
        // Record cross-team access attempt for security monitoring
        if let Some(from_team) = team_scopes.first() {
            crate::observability::metrics::record_cross_team_access_attempt(
                from_team,
                &filter.team,
                "filters",
            )
            .await;
        }

        // Return 404 to avoid leaking existence of other teams' resources
        Err(ApiError::NotFound(format!("Filter with id '{}' not found", filter.id)))
    }
}
