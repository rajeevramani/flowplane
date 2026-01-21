//! Internal API Types
//!
//! Shared request/response types for the internal API layer.
//! These types provide a unified interface for both REST and MCP entry points.

use crate::storage::ClusterData;
use crate::xds::ClusterSpec;

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
