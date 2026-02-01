//! Internal API Types
//!
//! Shared request/response types for the internal API layer.
//! These types provide a unified interface for both REST and MCP entry points.

use crate::storage::{ClusterData, FilterData, ListenerData, RouteConfigData};
use crate::xds::listener::ListenerConfig;
use crate::xds::ClusterSpec;
use serde_json::Value;

/// Common result wrapper for operations that modify data
#[derive(Debug)]
pub struct OperationResult<T> {
    /// The operation result data
    pub data: T,
    /// Optional success message
    pub message: Option<String>,
}

impl<T> OperationResult<T> {
    /// Create a new operation result with data and message
    pub fn with_message(data: T, message: impl Into<String>) -> Self {
        Self { data, message: Some(message.into()) }
    }

    /// Create a new operation result with just data
    pub fn new(data: T) -> Self {
        Self { data, message: None }
    }
}

/// Request to create a new cluster
#[derive(Debug, Clone)]
pub struct CreateClusterRequest {
    /// Cluster name (unique identifier)
    pub name: String,
    /// Service name (defaults to cluster name if not provided)
    pub service_name: String,
    /// Team that owns this cluster
    pub team: Option<String>,
    /// Cluster configuration
    pub config: ClusterSpec,
}

/// Request to list clusters with pagination
#[derive(Debug, Clone, Default)]
pub struct ListClustersRequest {
    /// Maximum number of clusters to return
    pub limit: Option<i32>,
    /// Offset for pagination
    pub offset: Option<i32>,
    /// Include default/global resources in the list
    pub include_defaults: bool,
}

/// Response for listing clusters
#[derive(Debug)]
pub struct ListClustersResponse {
    /// List of clusters
    pub clusters: Vec<ClusterData>,
    /// Total count of clusters matching the query
    pub count: usize,
    /// Applied limit
    pub limit: Option<i32>,
    /// Applied offset
    pub offset: Option<i32>,
}

/// Request to update an existing cluster
#[derive(Debug, Clone)]
pub struct UpdateClusterRequest {
    /// New service name (optional)
    pub service_name: Option<String>,
    /// New cluster configuration
    pub config: ClusterSpec,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_result_with_message() {
        let result = OperationResult::with_message("data", "success");
        assert_eq!(result.data, "data");
        assert_eq!(result.message, Some("success".to_string()));
    }

    #[test]
    fn test_operation_result_new() {
        let result = OperationResult::new(42);
        assert_eq!(result.data, 42);
        assert!(result.message.is_none());
    }

    #[test]
    fn test_list_clusters_request_defaults() {
        let req = ListClustersRequest::default();
        assert!(req.limit.is_none());
        assert!(req.offset.is_none());
        assert!(!req.include_defaults);
    }
}

// =============================================================================
// Listener Types
// =============================================================================

/// Request to create a new listener
#[derive(Debug, Clone)]
pub struct CreateListenerRequest {
    /// Listener name (unique identifier)
    pub name: String,
    /// IP address to bind (e.g., "0.0.0.0")
    pub address: String,
    /// Port number
    pub port: u16,
    /// Protocol type (HTTP, HTTPS, TCP)
    pub protocol: Option<String>,
    /// Team that owns this listener
    pub team: Option<String>,
    /// Listener configuration (filter chains, etc.)
    pub config: ListenerConfig,
    /// The dataplane ID this listener belongs to (required)
    pub dataplane_id: String,
}

/// Request to list listeners with pagination
#[derive(Debug, Clone, Default)]
pub struct ListListenersRequest {
    /// Maximum number of listeners to return
    pub limit: Option<i32>,
    /// Offset for pagination
    pub offset: Option<i32>,
    /// Include default/global resources in the list
    pub include_defaults: bool,
}

/// Response for listing listeners
#[derive(Debug)]
pub struct ListListenersResponse {
    /// List of listeners
    pub listeners: Vec<ListenerData>,
    /// Total count of listeners matching the query
    pub count: usize,
    /// Applied limit
    pub limit: Option<i32>,
    /// Applied offset
    pub offset: Option<i32>,
}

/// Request to update an existing listener
#[derive(Debug, Clone)]
pub struct UpdateListenerRequest {
    /// New address (optional)
    pub address: Option<String>,
    /// New port (optional)
    pub port: Option<u16>,
    /// New protocol (optional)
    pub protocol: Option<String>,
    /// New listener configuration
    pub config: ListenerConfig,
    /// New dataplane ID (optional) - if provided, listener will be assigned to this dataplane
    pub dataplane_id: Option<String>,
}

// =============================================================================
// Route Config Types
// =============================================================================

/// Request to create a new route config
#[derive(Debug, Clone)]
pub struct CreateRouteConfigRequest {
    /// Route config name (unique identifier)
    pub name: String,
    /// Team that owns this route config
    pub team: Option<String>,
    /// Route configuration (virtual hosts, routes, etc.)
    pub config: Value,
}

/// Request to list route configs with pagination
#[derive(Debug, Clone, Default)]
pub struct ListRouteConfigsRequest {
    /// Maximum number of route configs to return
    pub limit: Option<i32>,
    /// Offset for pagination
    pub offset: Option<i32>,
    /// Include default/global resources in the list
    pub include_defaults: bool,
}

/// Response for listing route configs
#[derive(Debug)]
pub struct ListRouteConfigsResponse {
    /// List of route configs
    pub routes: Vec<RouteConfigData>,
    /// Total count of route configs matching the query
    pub count: usize,
    /// Applied limit
    pub limit: Option<i32>,
    /// Applied offset
    pub offset: Option<i32>,
}

/// Request to update an existing route config
#[derive(Debug, Clone)]
pub struct UpdateRouteConfigRequest {
    /// New route configuration (full replacement)
    pub config: Value,
}

// =============================================================================
// Filter Types
// =============================================================================

/// Request to create a new filter
#[derive(Debug, Clone)]
pub struct CreateFilterRequest {
    /// Filter name (unique identifier)
    pub name: String,
    /// Filter type (jwt_auth, cors, rate_limit, etc.)
    pub filter_type: String,
    /// Human-readable description
    pub description: Option<String>,
    /// Team that owns this filter
    pub team: Option<String>,
    /// Filter-specific configuration
    pub config: crate::domain::FilterConfig,
}

/// Request to list filters with pagination
#[derive(Debug, Clone, Default)]
pub struct ListFiltersRequest {
    /// Maximum number of filters to return
    pub limit: Option<i32>,
    /// Offset for pagination
    pub offset: Option<i32>,
    /// Filter by type
    pub filter_type: Option<String>,
    /// Include default/global resources in the list
    pub include_defaults: bool,
}

/// Response for listing filters
#[derive(Debug)]
pub struct ListFiltersResponse {
    /// List of filters
    pub filters: Vec<FilterData>,
    /// Total count of filters matching the query
    pub count: usize,
    /// Applied limit
    pub limit: Option<i32>,
    /// Applied offset
    pub offset: Option<i32>,
}

/// Request to update an existing filter
#[derive(Debug, Clone)]
pub struct UpdateFilterRequest {
    /// New filter name (optional, for renaming)
    pub name: Option<String>,
    /// New description (optional)
    pub description: Option<String>,
    /// New filter configuration (optional)
    pub config: Option<crate::domain::FilterConfig>,
}

/// Filter with installation information
#[derive(Debug)]
pub struct FilterWithInstallations {
    /// The filter data
    pub filter: FilterData,
    /// Listeners where this filter is installed
    pub listener_installations: Vec<FilterInstallation>,
    /// Route configs where this filter is attached
    pub route_config_installations: Vec<FilterInstallation>,
}

/// Installation record for a filter
#[derive(Debug, Clone)]
pub struct FilterInstallation {
    /// ID of the resource (listener or route config)
    pub resource_id: String,
    /// Name of the resource
    pub resource_name: String,
    /// Order/priority of the filter on this resource
    pub order: i64,
}

// =============================================================================
// Virtual Host Types
// =============================================================================

/// Request to create a new virtual host
#[derive(Debug, Clone)]
pub struct CreateVirtualHostRequest {
    /// Route config name this virtual host belongs to
    pub route_config: String,
    /// Virtual host name (unique within route config)
    pub name: String,
    /// Domain patterns this virtual host matches
    pub domains: Vec<String>,
    /// Rule order for deterministic matching (default: 0)
    pub rule_order: Option<i32>,
}

/// Request to list virtual hosts with pagination
#[derive(Debug, Clone, Default)]
pub struct ListVirtualHostsRequest {
    /// Filter by route config name (optional)
    pub route_config: Option<String>,
    /// Maximum number of virtual hosts to return
    pub limit: Option<i32>,
    /// Offset for pagination
    pub offset: Option<i32>,
}

/// Response for listing virtual hosts
#[derive(Debug)]
pub struct ListVirtualHostsResponse {
    /// List of virtual hosts
    pub virtual_hosts: Vec<crate::storage::VirtualHostData>,
    /// Total count of virtual hosts matching the query
    pub count: usize,
    /// Applied limit
    pub limit: Option<i32>,
    /// Applied offset
    pub offset: Option<i32>,
}

/// Request to update an existing virtual host
#[derive(Debug, Clone)]
pub struct UpdateVirtualHostRequest {
    /// New domain patterns (optional)
    pub domains: Option<Vec<String>>,
    /// New rule order (optional)
    pub rule_order: Option<i32>,
}

// =============================================================================
// Route Types (individual routes within virtual hosts)
// =============================================================================

/// Request to create a new route
#[derive(Debug, Clone)]
pub struct CreateRouteRequest {
    /// Route config name that contains the virtual host
    pub route_config: String,
    /// Virtual host name that contains this route
    pub virtual_host: String,
    /// Route name (unique within the virtual host)
    pub name: String,
    /// Path pattern (e.g., "/api", "/users/{id}")
    pub path_pattern: String,
    /// Match type (prefix, exact, regex, template)
    pub match_type: String,
    /// Rule order (lower values = higher priority)
    pub rule_order: Option<i32>,
    /// Route action (forward, redirect, weighted)
    pub action: serde_json::Value,
}

/// Request to list routes with pagination
#[derive(Debug, Clone, Default)]
pub struct ListRoutesRequest {
    /// Filter by route config name (optional)
    pub route_config: Option<String>,
    /// Filter by virtual host name within the route config (optional)
    pub virtual_host: Option<String>,
    /// Maximum number of routes to return
    pub limit: Option<i32>,
    /// Offset for pagination
    pub offset: Option<i32>,
}

/// Response for listing routes
#[derive(Debug)]
pub struct ListRoutesResponse {
    /// List of routes
    pub routes: Vec<crate::storage::RouteData>,
    /// Total count of routes matching the query
    pub count: usize,
    /// Applied limit
    pub limit: Option<i32>,
    /// Applied offset
    pub offset: Option<i32>,
}

/// Request to update an existing route
#[derive(Debug, Clone)]
pub struct UpdateRouteRequest {
    /// New path pattern (optional)
    pub path_pattern: Option<String>,
    /// New match type (optional)
    pub match_type: Option<String>,
    /// New rule order (optional)
    pub rule_order: Option<i32>,
    /// New action (optional)
    pub action: Option<serde_json::Value>,
}

// =============================================================================
// Learning Session Types
// =============================================================================

/// Request to create a new learning session
#[derive(Debug, Clone)]
pub struct CreateLearningSessionInternalRequest {
    /// Team that owns this learning session (optional, defaults to auth team)
    pub team: Option<String>,
    /// Route pattern to match for learning
    pub route_pattern: String,
    /// Cluster name to filter by (optional)
    pub cluster_name: Option<String>,
    /// HTTP methods to filter by (optional)
    pub http_methods: Option<Vec<String>>,
    /// Number of samples to collect before completing
    pub target_sample_count: i64,
    /// Whether to automatically start the session after creation
    pub auto_start: Option<bool>,
}

/// Request to list learning sessions with pagination
#[derive(Debug, Clone, Default)]
pub struct ListLearningSessionsRequest {
    /// Filter by status (optional)
    pub status: Option<String>,
    /// Maximum number of sessions to return
    pub limit: Option<i64>,
    /// Offset for pagination
    pub offset: Option<i64>,
}

// =============================================================================
// Aggregated Schema Types
// =============================================================================

/// Request to list aggregated schemas with optional filters
#[derive(Debug, Clone, Default)]
pub struct ListSchemasRequest {
    /// Filter by path pattern (supports LIKE matching)
    pub path: Option<String>,
    /// Filter by HTTP method
    pub http_method: Option<String>,
    /// Filter by minimum confidence score
    pub min_confidence: Option<f64>,
    /// Return only latest versions of each endpoint
    pub latest_only: Option<bool>,
}

/// Response for listing aggregated schemas
#[derive(Debug)]
pub struct ListSchemasResponse {
    /// List of schemas
    pub schemas: Vec<crate::storage::repositories::aggregated_schema::AggregatedSchemaData>,
    /// Total count of schemas matching the query
    pub count: usize,
}

// =============================================================================
// Dataplane Types
// =============================================================================

/// Request to list dataplanes with pagination
#[derive(Debug, Clone, Default)]
pub struct ListDataplanesInternalRequest {
    /// Maximum number of dataplanes to return
    pub limit: Option<i32>,
    /// Offset for pagination
    pub offset: Option<i32>,
}

/// Request to create a new dataplane
#[derive(Debug, Clone)]
pub struct CreateDataplaneInternalRequest {
    /// Team that owns this dataplane
    pub team: String,
    /// Dataplane name (unique within team)
    pub name: String,
    /// Optional gateway host URL for MCP tool execution
    pub gateway_host: Option<String>,
    /// Optional human-readable description
    pub description: Option<String>,
}

/// Request to update an existing dataplane
#[derive(Debug, Clone)]
pub struct UpdateDataplaneInternalRequest {
    /// New gateway host (optional)
    pub gateway_host: Option<String>,
    /// New description (optional)
    pub description: Option<String>,
}

// =============================================================================
// OpenAPI Import Types
// =============================================================================

/// Request to list OpenAPI imports with pagination
#[derive(Debug, Clone, Default)]
pub struct ListOpenApiImportsRequest {
    /// Maximum number of imports to return
    pub limit: Option<i32>,
    /// Offset for pagination
    pub offset: Option<i32>,
}
