//! Filter injection module for dynamically injecting filters into xDS resources.
//!
//! This module provides reusable abstractions for injecting filters into:
//! - Routes (JSON-based per-route configuration)
//! - Listeners (protobuf-based HCM filter chain)
//!
//! # Architecture
//!
//! The `FilterInjector` struct coordinates filter injection across different
//! resource types. It handles:
//!
//! - Loading filter configurations from the database
//! - Merging compatible filters (e.g., JWT authentication)
//! - Converting filter configs to Envoy protobuf format
//! - Auto-creating dependent resources (e.g., JWKS clusters)
//!
//! # Example
//!
//! ```rust,ignore
//! use flowplane::xds::filters::injection::FilterInjector;
//!
//! let injector = FilterInjector::new(
//!     &filter_repo,
//!     &cluster_repo,
//!     Some(&route_repo),
//! );
//!
//! // Inject filters into a route
//! let modified = injector.inject_into_route(&mut route_data).await?;
//!
//! // Inject filters into a listener
//! let (modified, new_clusters) = injector
//!     .inject_into_listener(&mut built_resource, &listener_id, xds_state)
//!     .await?;
//! ```

mod learning_session;
mod listener;
mod merger;
mod route;

pub use learning_session::{inject_access_logs, inject_ext_proc};
pub use listener::inject_listener_filters;
pub use merger::JwtConfigMerger;
pub use route::{
    inject_route_config_filters, inject_route_filters_hierarchical, HierarchicalFilterContext,
};

use crate::storage::{ClusterRepository, FilterRepository, RouteRepository};

/// Coordinates filter injection across different resource types.
///
/// Provides a unified interface for injecting filters into routes and listeners,
/// handling filter loading, merging, and conversion to Envoy protobuf format.
pub struct FilterInjector<'a> {
    pub filter_repo: &'a FilterRepository,
    pub cluster_repo: &'a ClusterRepository,
    pub route_repo: Option<&'a RouteRepository>,
}

impl<'a> FilterInjector<'a> {
    /// Create a new FilterInjector with the required repositories.
    pub fn new(
        filter_repo: &'a FilterRepository,
        cluster_repo: &'a ClusterRepository,
        route_repo: Option<&'a RouteRepository>,
    ) -> Self {
        Self { filter_repo, cluster_repo, route_repo }
    }
}
