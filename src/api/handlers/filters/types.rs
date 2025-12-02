//! Request and response types for filter API handlers

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::{AttachmentPoint, FilterConfig, FilterType};
use crate::storage::FilterData;

/// Query parameters for listing filters
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListFiltersQuery {
    #[serde(default)]
    pub limit: Option<i32>,
    #[serde(default)]
    pub offset: Option<i32>,
}

/// Request body for creating a filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateFilterRequest {
    pub name: String,
    pub filter_type: FilterType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub config: FilterConfig,
    pub team: String,
}

/// Request body for updating a filter
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFilterRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<FilterConfig>,
}

/// Response body for filter operations
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FilterResponse {
    pub id: String,
    pub name: String,
    pub filter_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub config: FilterConfig,
    pub version: i64,
    pub source: String,
    pub team: String,
    pub created_at: String,
    pub updated_at: String,
    /// Valid attachment points for this filter type
    pub allowed_attachment_points: Vec<AttachmentPoint>,
}

impl FilterResponse {
    /// Convert from FilterData and parsed config
    pub fn from_data(data: FilterData, config: FilterConfig) -> Self {
        // Parse filter type to get allowed attachment points
        let allowed_attachment_points = data
            .filter_type
            .parse::<FilterType>()
            .map(|ft| ft.allowed_attachment_points())
            .unwrap_or_default();

        Self {
            id: data.id.to_string(),
            name: data.name,
            filter_type: data.filter_type,
            description: data.description,
            config,
            version: data.version,
            source: data.source,
            team: data.team,
            created_at: data.created_at.to_rfc3339(),
            updated_at: data.updated_at.to_rfc3339(),
            allowed_attachment_points,
        }
    }
}

/// Request body for attaching a filter to a route
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttachFilterRequest {
    pub filter_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
}

/// Response for listing route filters
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteFiltersResponse {
    pub route_id: String,
    pub filters: Vec<FilterResponse>,
}

/// Response for listing listener filters
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerFiltersResponse {
    pub listener_id: String,
    pub filters: Vec<FilterResponse>,
}
