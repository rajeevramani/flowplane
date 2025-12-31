//! Validation helpers for filter API handlers

use crate::{
    api::{error::ApiError, routes::ApiState},
    domain::{
        filter_schema::FilterSchemaRegistry, FilterConfig, FilterType, SharedFilterSchemaRegistry,
    },
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

/// Filter type names that require backend clusters
const CLUSTER_REQUIRING_FILTER_TYPES: &[&str] = &["oauth2", "jwt_auth", "ext_authz"];

/// Check if a filter type requires a backend cluster
pub fn filter_type_requires_cluster(filter_type: &str) -> bool {
    CLUSTER_REQUIRING_FILTER_TYPES.contains(&filter_type)
}

/// Check if a filter type is valid (either built-in or in schema registry)
fn is_valid_filter_type(filter_type: &str, registry: &FilterSchemaRegistry) -> bool {
    // Check if it's a built-in type
    if filter_type.parse::<FilterType>().is_ok() {
        return true;
    }
    // Check if it's in the schema registry (custom types)
    registry.get(filter_type).is_some()
}

/// Check if a built-in filter type is fully implemented
fn is_builtin_filter_implemented(filter_type: &str) -> bool {
    if let Ok(ft) = filter_type.parse::<FilterType>() {
        return ft.is_fully_implemented();
    }
    false
}

/// Validate create filter request
pub async fn validate_create_filter_request(
    payload: &CreateFilterRequest,
    schema_registry: Option<&SharedFilterSchemaRegistry>,
) -> Result<(), ApiError> {
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

    // Check if this is a known filter type (built-in or from schema registry)
    let is_valid = if let Some(registry) = schema_registry {
        let reg = registry.read().await;
        is_valid_filter_type(&payload.filter_type, &reg)
    } else {
        // Fallback to built-in only if no registry provided
        let reg = FilterSchemaRegistry::with_builtin_schemas();
        is_valid_filter_type(&payload.filter_type, &reg)
    };

    if !is_valid {
        return Err(ApiError::validation(format!(
            "Unknown filter type '{}'. Available built-in types: header_mutation, jwt_auth, local_rate_limit, custom_response, mcp, cors, compressor, ext_authz, rbac, oauth2. Custom types must have a schema in filter-schemas/custom/",
            payload.filter_type
        )));
    }

    // For built-in types, check if fully implemented
    if payload.filter_type.parse::<FilterType>().is_ok()
        && !is_builtin_filter_implemented(&payload.filter_type)
    {
        return Err(ApiError::validation(format!(
            "Filter type '{}' is not yet fully supported",
            payload.filter_type
        )));
    }

    // Validate cluster_config if provided
    if let Some(ref cluster_config) = payload.cluster_config {
        // Validate cluster config
        cluster_config.validate().map_err(ApiError::validation)?;

        // Warn if cluster_config is provided for a filter type that doesn't need it
        if !filter_type_requires_cluster(&payload.filter_type) {
            tracing::warn!(
                filter_type = ?payload.filter_type,
                "cluster_config provided for filter type that doesn't require a cluster"
            );
        }
    }

    // Validate filter type matches config
    tracing::debug!(
        filter_type = ?payload.filter_type,
        config_type = ?std::mem::discriminant(&payload.config),
        "Validating filter type matches config"
    );

    // For custom filters, the config type is "custom" and filter_type comes from the CustomFilterConfig
    if payload.config.is_custom() {
        // Verify the filter_type in the request matches the one in the config
        let config_filter_type = payload.config.filter_type_str();
        if payload.filter_type != config_filter_type {
            return Err(ApiError::validation(format!(
                "Filter type '{}' in request does not match config type '{}'",
                payload.filter_type, config_filter_type
            )));
        }
        return Ok(());
    }

    // For built-in types, validate filter type matches config variant
    match (payload.filter_type.as_str(), &payload.config) {
        ("header_mutation", FilterConfig::HeaderMutation(_)) => Ok(()),
        ("jwt_auth", FilterConfig::JwtAuth(_)) => Ok(()),
        ("local_rate_limit", FilterConfig::LocalRateLimit(_)) => Ok(()),
        ("custom_response", FilterConfig::CustomResponse(_)) => Ok(()),
        ("mcp", FilterConfig::Mcp(_)) => Ok(()),
        ("cors", FilterConfig::Cors(_)) => Ok(()),
        ("compressor", FilterConfig::Compressor(_)) => Ok(()),
        ("ext_authz", FilterConfig::ExtAuthz(_)) => Ok(()),
        ("rbac", FilterConfig::Rbac(_)) => Ok(()),
        ("oauth2", FilterConfig::OAuth2(_)) => Ok(()),
        _ => {
            tracing::warn!(
                filter_type = ?payload.filter_type,
                config = ?payload.config,
                "Filter type and configuration do not match"
            );
            Err(ApiError::validation(format!(
                "Filter type '{}' does not match configuration type",
                payload.filter_type
            )))
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

/// Convert FilterData to FilterResponse with attachment count
pub fn filter_response_from_data_with_count(
    data: FilterData,
    attachment_count: Option<i64>,
) -> Result<FilterResponse, ApiError> {
    let config = parse_filter_config(&data)?;
    Ok(FilterResponse::from_data_with_count(data, config, attachment_count))
}
