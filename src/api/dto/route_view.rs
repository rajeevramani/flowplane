//! Route View DTOs for UI list display
//!
//! These DTOs provide a flattened view of route data for UI consumption.
//! All fields are derived at runtime from existing data without requiring
//! database schema changes. This supports the "Option A" prototype approach.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::domain::RouteMatchType;

/// Flattened route view for UI list display.
///
/// All fields are derived from existing data:
/// - Identity fields from normalized tables (routes, virtual_hosts, route_configs)
/// - Action fields extracted from route_configs.configuration JSON at runtime
/// - Related data from JOINs (mcp_tools, route_filters, route_metadata)
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "routeId": "550e8400-e29b-41d4-a716-446655440000",
    "routeName": "api-users",
    "virtualHostId": "660e8400-e29b-41d4-a716-446655440001",
    "virtualHostName": "default",
    "routeConfigId": "770e8400-e29b-41d4-a716-446655440002",
    "routeConfigName": "primary-routes",
    "pathPattern": "/api/users",
    "matchType": "prefix",
    "ruleOrder": 1,
    "domains": ["api.example.com", "*.example.com"],
    "upstreamCluster": "users-service",
    "fallbackCluster": null,
    "httpMethods": ["GET", "POST"],
    "timeoutSeconds": 30,
    "prefixRewrite": null,
    "mcpEnabled": true,
    "mcpToolName": "get-users",
    "filterCount": 2,
    "operationId": "getUsers",
    "summary": "Get all users",
    "createdAt": "2024-01-15T10:30:00Z",
    "updatedAt": "2024-01-15T10:30:00Z"
}))]
pub struct RouteListViewDto {
    // === Identity (from normalized tables) ===
    /// Unique ID of the route
    pub route_id: String,
    /// Name of the route
    pub route_name: String,
    /// ID of the parent virtual host
    pub virtual_host_id: String,
    /// Name of the parent virtual host
    pub virtual_host_name: String,
    /// ID of the parent route configuration
    pub route_config_id: String,
    /// Name of the parent route configuration
    pub route_config_name: String,

    // === From routes table (already denormalized) ===
    /// Path pattern for matching (e.g., "/api/users", "/v1/.*")
    pub path_pattern: String,
    /// Type of path matching (prefix, exact, regex, path_template)
    pub match_type: RouteMatchType,
    /// Order of the route within the virtual host (affects matching priority)
    pub rule_order: i32,

    // === From virtual_hosts table ===
    /// Domains the virtual host matches
    pub domains: Vec<String>,

    // === Derived from configuration JSON at runtime ===
    /// Primary upstream cluster for traffic routing
    pub upstream_cluster: Option<String>,
    /// Fallback cluster if primary is unavailable
    pub fallback_cluster: Option<String>,
    /// HTTP methods this route handles (empty = all methods)
    pub http_methods: Vec<String>,
    /// Request timeout in seconds
    pub timeout_seconds: Option<u64>,
    /// Path prefix rewrite rule
    pub prefix_rewrite: Option<String>,

    // === From related tables (JOINs) ===
    /// Whether MCP tool is enabled for this route
    pub mcp_enabled: bool,
    /// Name of the MCP tool (if enabled)
    pub mcp_tool_name: Option<String>,
    /// Number of filters attached to this route
    pub filter_count: i32,

    // === From route_metadata table ===
    /// OpenAPI operation ID (if imported from OpenAPI spec)
    pub operation_id: Option<String>,
    /// Summary description of the route
    pub summary: Option<String>,

    // === Timestamps ===
    /// When the route was created
    pub created_at: DateTime<Utc>,
    /// When the route was last updated
    pub updated_at: DateTime<Utc>,
}

/// Statistics summary for the route list page.
///
/// All values are computed on-the-fly from existing tables.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "totalRoutes": 150,
    "totalVirtualHosts": 12,
    "totalRouteConfigs": 5,
    "mcpEnabledCount": 45,
    "uniqueClusters": 8,
    "uniqueDomains": 15
}))]
pub struct RouteListStatsDto {
    /// Total number of routes
    pub total_routes: i64,
    /// Total number of virtual hosts
    pub total_virtual_hosts: i64,
    /// Total number of route configurations
    pub total_route_configs: i64,
    /// Number of routes with MCP enabled
    pub mcp_enabled_count: i64,
    /// Number of unique upstream clusters
    pub unique_clusters: i64,
    /// Number of unique domains
    pub unique_domains: i64,
}

/// Pagination metadata for list responses.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(example = json!({
    "page": 1,
    "pageSize": 20,
    "totalCount": 150,
    "totalPages": 8
}))]
pub struct PaginationDto {
    /// Current page number (1-indexed)
    pub page: i32,
    /// Number of items per page
    pub page_size: i32,
    /// Total number of items across all pages
    pub total_count: i64,
    /// Total number of pages
    pub total_pages: i32,
}

/// Paginated response for route list endpoint.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteListResponseDto {
    /// List of routes for the current page
    pub items: Vec<RouteListViewDto>,
    /// Aggregate statistics
    pub stats: RouteListStatsDto,
    /// Pagination information
    pub pagination: PaginationDto,
}

/// Query parameters for route list endpoint.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct RouteListQueryParams {
    /// Page number (1-indexed, default: 1)
    #[param(default = 1, minimum = 1)]
    pub page: Option<i32>,

    /// Number of items per page (default: 20, max: 100)
    #[param(default = 20, minimum = 1, maximum = 100)]
    pub page_size: Option<i32>,

    /// Search query (searches name, path, domain, cluster)
    #[param(example = "api")]
    pub search: Option<String>,

    /// Filter by MCP status ("enabled", "disabled", or null for all)
    #[param(example = "enabled")]
    pub mcp_filter: Option<String>,

    /// Filter by route config name
    #[param(example = "primary-routes")]
    pub route_config: Option<String>,

    /// Filter by virtual host name
    #[param(example = "default")]
    pub virtual_host: Option<String>,
}
