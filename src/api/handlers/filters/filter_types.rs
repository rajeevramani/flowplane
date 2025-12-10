//! Filter types API handlers
//!
//! This module provides endpoints for listing available filter types
//! and their schemas, supporting the dynamic filter framework.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::ToSchema;

use crate::{
    api::{error::ApiError, routes::ApiState},
    auth::authorization::require_resource_access,
    auth::models::AuthContext,
    domain::{
        filter_schema::{FormLayout, SchemaSource},
        AttachmentPoint, PerRouteBehavior,
    },
};

/// Information about a filter type available in the system.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilterTypeInfo {
    /// Unique filter type name (e.g., "header_mutation")
    pub name: String,

    /// Human-readable display name (e.g., "Header Mutation")
    pub display_name: String,

    /// Description of what this filter does
    pub description: String,

    /// Schema version
    pub version: String,

    /// Envoy HTTP filter name
    pub envoy_filter_name: String,

    /// Valid attachment points for this filter
    pub attachment_points: Vec<AttachmentPoint>,

    /// Whether this filter requires listener-level configuration
    pub requires_listener_config: bool,

    /// How this filter handles per-route configuration
    pub per_route_behavior: String,

    /// Whether this filter type is fully implemented
    pub is_implemented: bool,

    /// Source of this filter definition (built_in or custom)
    pub source: String,

    /// JSON Schema for configuration validation
    pub config_schema: serde_json::Value,

    /// UI hints for form generation (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_hints: Option<FilterTypeUiHints>,
}

/// UI hints for filter form generation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilterTypeUiHints {
    /// Form layout style
    pub form_layout: String,

    /// Form sections for grouped fields
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<FilterTypeFormSection>,

    /// Custom form component name (if using a custom form)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_form_component: Option<String>,
}

/// A section in a form layout.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilterTypeFormSection {
    /// Section name/title
    pub name: String,

    /// Field names included in this section
    pub fields: Vec<String>,

    /// Whether the section is collapsible
    pub collapsible: bool,

    /// Whether the section is collapsed by default
    pub collapsed_by_default: bool,
}

/// Response for listing all filter types.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilterTypesResponse {
    /// List of available filter types
    pub filter_types: Vec<FilterTypeInfo>,

    /// Total count of filter types
    pub total: usize,

    /// Count of implemented filter types
    pub implemented_count: usize,
}

/// List all available filter types with their schemas.
///
/// Returns information about all filter types registered in the system,
/// including built-in and custom (dynamically loaded) filters.
#[utoipa::path(
    get,
    path = "/api/v1/filter-types",
    responses(
        (status = 200, description = "List of available filter types", body = FilterTypesResponse),
        (status = 401, description = "Unauthorized"),
        (status = 503, description = "Schema registry not available"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context), fields(user_id = ?context.user_id))]
pub async fn list_filter_types_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
) -> Result<Json<FilterTypesResponse>, ApiError> {
    require_resource_access(&context, "filters", "read", None)?;

    let registry = state
        .filter_schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Filter schema registry not available"))?;

    let registry = registry.read().await;
    let schemas = registry.list_all();

    let filter_types: Vec<FilterTypeInfo> = schemas
        .into_iter()
        .map(|schema| {
            let ui_hints = schema.ui_hints.as_ref().map(|h| FilterTypeUiHints {
                form_layout: match h.form_layout {
                    FormLayout::Flat => "flat".to_string(),
                    FormLayout::Sections => "sections".to_string(),
                    FormLayout::Tabs => "tabs".to_string(),
                },
                sections: h
                    .sections
                    .iter()
                    .map(|s| FilterTypeFormSection {
                        name: s.name.clone(),
                        fields: s.fields.clone(),
                        collapsible: s.collapsible,
                        collapsed_by_default: s.collapsed_by_default,
                    })
                    .collect(),
                custom_form_component: h.custom_form_component.clone(),
            });

            FilterTypeInfo {
                name: schema.name.clone(),
                display_name: schema.display_name.clone(),
                description: schema.description.clone(),
                version: schema.version.clone(),
                envoy_filter_name: schema.envoy.http_filter_name.clone(),
                attachment_points: schema.capabilities.attachment_points.clone(),
                requires_listener_config: schema.capabilities.requires_listener_config,
                per_route_behavior: match schema.capabilities.per_route_behavior {
                    PerRouteBehavior::FullConfig => "full_config".to_string(),
                    PerRouteBehavior::ReferenceOnly => "reference_only".to_string(),
                    PerRouteBehavior::DisableOnly => "disable_only".to_string(),
                    PerRouteBehavior::NotSupported => "not_supported".to_string(),
                },
                is_implemented: schema.is_implemented,
                source: match schema.source {
                    SchemaSource::BuiltIn => "built_in".to_string(),
                    SchemaSource::Custom => "custom".to_string(),
                },
                config_schema: schema.config_schema.clone(),
                ui_hints,
            }
        })
        .collect();

    let total = filter_types.len();
    let implemented_count = filter_types.iter().filter(|ft| ft.is_implemented).count();

    Ok(Json(FilterTypesResponse { filter_types, total, implemented_count }))
}

/// Get information about a specific filter type.
#[utoipa::path(
    get,
    path = "/api/v1/filter-types/{filter_type}",
    params(
        ("filter_type" = String, Path, description = "Filter type name"),
    ),
    responses(
        (status = 200, description = "Filter type information", body = FilterTypeInfo),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Filter type not found"),
        (status = 503, description = "Schema registry not available"),
    ),
    tag = "filters"
)]
#[instrument(skip(state, context), fields(filter_type = %filter_type, user_id = ?context.user_id))]
pub async fn get_filter_type_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(filter_type): Path<String>,
) -> Result<Json<FilterTypeInfo>, ApiError> {
    require_resource_access(&context, "filters", "read", None)?;

    let registry = state
        .filter_schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Filter schema registry not available"))?;

    let registry = registry.read().await;
    let schema = registry
        .get(&filter_type)
        .ok_or_else(|| ApiError::NotFound(format!("Filter type '{}' not found", filter_type)))?;

    let ui_hints = schema.ui_hints.as_ref().map(|h| FilterTypeUiHints {
        form_layout: match h.form_layout {
            FormLayout::Flat => "flat".to_string(),
            FormLayout::Sections => "sections".to_string(),
            FormLayout::Tabs => "tabs".to_string(),
        },
        sections: h
            .sections
            .iter()
            .map(|s| FilterTypeFormSection {
                name: s.name.clone(),
                fields: s.fields.clone(),
                collapsible: s.collapsible,
                collapsed_by_default: s.collapsed_by_default,
            })
            .collect(),
        custom_form_component: h.custom_form_component.clone(),
    });

    let filter_info = FilterTypeInfo {
        name: schema.name.clone(),
        display_name: schema.display_name.clone(),
        description: schema.description.clone(),
        version: schema.version.clone(),
        envoy_filter_name: schema.envoy.http_filter_name.clone(),
        attachment_points: schema.capabilities.attachment_points.clone(),
        requires_listener_config: schema.capabilities.requires_listener_config,
        per_route_behavior: match schema.capabilities.per_route_behavior {
            PerRouteBehavior::FullConfig => "full_config".to_string(),
            PerRouteBehavior::ReferenceOnly => "reference_only".to_string(),
            PerRouteBehavior::DisableOnly => "disable_only".to_string(),
            PerRouteBehavior::NotSupported => "not_supported".to_string(),
        },
        is_implemented: schema.is_implemented,
        source: match schema.source {
            SchemaSource::BuiltIn => "built_in".to_string(),
            SchemaSource::Custom => "custom".to_string(),
        },
        config_schema: schema.config_schema.clone(),
        ui_hints,
    };

    Ok(Json(filter_info))
}

/// Reload filter schemas from the schema directory.
///
/// This endpoint allows hot-reloading of custom filter schemas without
/// restarting the control plane. It's an admin-only operation.
#[utoipa::path(
    post,
    path = "/api/v1/admin/filter-schemas/reload",
    responses(
        (status = 204, description = "Schemas reloaded successfully"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - admin access required"),
        (status = 500, description = "Failed to reload schemas"),
        (status = 503, description = "Schema registry not available"),
    ),
    tag = "admin"
)]
#[instrument(skip(state, context), fields(user_id = ?context.user_id))]
pub async fn reload_filter_schemas_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
) -> Result<StatusCode, ApiError> {
    // Require admin access for schema reload
    require_resource_access(&context, "admin", "write", None)?;

    let registry = state
        .filter_schema_registry
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Filter schema registry not available"))?;

    let mut registry = registry.write().await;
    registry
        .reload()
        .map_err(|e| ApiError::internal(format!("Failed to reload filter schemas: {}", e)))?;

    tracing::info!("Filter schemas reloaded successfully");

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_type_info_serialization() {
        let info = FilterTypeInfo {
            name: "header_mutation".to_string(),
            display_name: "Header Mutation".to_string(),
            description: "Add, modify, or remove HTTP headers".to_string(),
            version: "1.0".to_string(),
            envoy_filter_name: "envoy.filters.http.header_mutation".to_string(),
            attachment_points: vec![AttachmentPoint::Route],
            requires_listener_config: false,
            per_route_behavior: "full_config".to_string(),
            is_implemented: true,
            source: "built_in".to_string(),
            config_schema: serde_json::json!({"type": "object"}),
            ui_hints: None,
        };

        let json = serde_json::to_string(&info).expect("serialize");
        assert!(json.contains("displayName"), "field name should be camelCase");
        assert!(
            json.contains("\"name\":\"header_mutation\""),
            "value should be preserved as snake_case"
        );

        let parsed: FilterTypeInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.name, "header_mutation");
        assert_eq!(parsed.display_name, "Header Mutation");
    }

    #[test]
    fn test_filter_types_response_serialization() {
        let response = FilterTypesResponse { filter_types: vec![], total: 0, implemented_count: 0 };

        let json = serde_json::to_string(&response).expect("serialize");
        assert!(json.contains("filterTypes")); // camelCase
        assert!(json.contains("implementedCount")); // camelCase
    }
}
