//! Request and response types for hierarchical route filter attachment handlers

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    domain::RouteMatchType,
    storage::{FilterData, RouteData, VirtualHostData},
};

// === Request Types ===

/// Request to attach a filter to a virtual host or route rule
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttachFilterRequest {
    /// ID of the filter to attach
    pub filter_id: String,
    /// Optional order for the filter (defaults to next available order)
    pub order: Option<i32>,
}

// === Response Types ===

/// Response for a virtual host with counts
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VirtualHostResponse {
    /// Unique ID of the virtual host
    pub id: String,
    /// Name of the virtual host
    pub name: String,
    /// Domains the virtual host matches
    pub domains: Vec<String>,
    /// Order of the virtual host within the route config
    pub rule_order: i32,
    /// Number of routes in this virtual host
    pub route_count: i64,
    /// Number of filters attached to this virtual host
    pub filter_count: i64,
}

impl VirtualHostResponse {
    /// Create a VirtualHostResponse from data with counts
    pub fn from_data_with_counts(
        data: VirtualHostData,
        route_count: i64,
        filter_count: i64,
    ) -> Self {
        Self {
            id: data.id.to_string(),
            name: data.name,
            domains: data.domains,
            rule_order: data.rule_order,
            route_count,
            filter_count,
        }
    }
}

/// Response for listing virtual hosts
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListVirtualHostsResponse {
    /// Name of the route configuration
    pub route_config_name: String,
    /// List of virtual hosts
    pub virtual_hosts: Vec<VirtualHostResponse>,
}

/// Response for a route rule with filter count
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteRuleResponse {
    /// Unique ID of the route rule
    pub id: String,
    /// Name of the route rule
    pub name: String,
    /// Path pattern for matching
    pub path_pattern: String,
    /// Type of path matching
    pub match_type: RouteMatchType,
    /// Order of the rule within the virtual host
    pub rule_order: i32,
    /// Number of filters attached to this route
    pub filter_count: i64,
}

impl RouteRuleResponse {
    /// Create a RouteRuleResponse from data with filter count
    pub fn from_data_with_count(data: RouteData, filter_count: i64) -> Self {
        Self {
            id: data.id.to_string(),
            name: data.name,
            path_pattern: data.path_pattern,
            match_type: data.match_type,
            rule_order: data.rule_order,
            filter_count,
        }
    }
}

/// Response for listing route rules
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListRouteRulesResponse {
    /// Name of the route configuration
    pub route_config_name: String,
    /// Name of the virtual host
    pub virtual_host_name: String,
    /// List of route rules - uses "routes" for Envoy terminology alignment
    #[serde(rename = "routes")]
    pub route_rules: Vec<RouteRuleResponse>,
}

/// Response for a filter attachment
#[derive(Debug, Serialize, ToSchema)]
pub struct FilterResponse {
    /// Unique ID of the filter
    pub id: String,
    /// Name of the filter
    pub name: String,
    /// Type of the filter
    pub filter_type: String,
    /// Description of the filter
    pub description: Option<String>,
    /// Filter version
    pub version: i64,
    /// When the filter was created
    pub created_at: DateTime<Utc>,
}

impl From<FilterData> for FilterResponse {
    fn from(data: FilterData) -> Self {
        Self {
            id: data.id.to_string(),
            name: data.name,
            filter_type: data.filter_type,
            description: data.description,
            version: data.version,
            created_at: data.created_at,
        }
    }
}

/// Response for listing virtual host filters
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VirtualHostFiltersResponse {
    /// Name of the route configuration
    pub route_config_name: String,
    /// Name of the virtual host
    pub virtual_host_name: String,
    /// List of attached filters
    pub filters: Vec<FilterResponse>,
}

/// Response for listing route rule filters
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteRuleFiltersResponse {
    /// Name of the route configuration
    pub route_config_name: String,
    /// Name of the virtual host
    pub virtual_host_name: String,
    /// Name of the route
    pub route_name: String,
    /// List of attached filters
    pub filters: Vec<FilterResponse>,
}
