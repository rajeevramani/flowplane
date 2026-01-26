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
