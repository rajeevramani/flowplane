//! Domain layer
//!
//! This module contains pure domain entities and business logic
//! with zero infrastructure dependencies. Domain types represent
//! the core concepts of the API gateway configuration system.
//!
//! ## Design Principles
//!
//! - **Zero Infrastructure Dependencies**: Domain types do not depend on
//!   HTTP frameworks, databases, or external systems
//! - **Business Logic Encapsulation**: Domain entities contain their own
//!   validation and transformation logic
//! - **Testability**: All domain logic can be tested without mocks or
//!   external systems
//! - **Reusability**: Domain types can be used across different API layers
//!   (Native, Gateway, Platform)
//!
//! ## Module Organization
//!
//! - `id`: Type-safe domain identifiers with NewType pattern
//! - `api_definition`: API definition specification and configuration
//! - `route`: Route configuration and matching logic
//! - `listener`: Listener configuration and network bindings
//! - `cluster`: Cluster (upstream) configuration and policies

pub mod api_definition;
pub mod cluster;
pub mod id;
pub mod listener;
pub mod route;

// Re-export main types from each module
pub use api_definition::{
    ApiDefinitionSpec, AppendRouteOutcome, CreateDefinitionOutcome, ListenerConfig, RouteConfig,
};
pub use cluster::{
    CircuitBreaker, ClusterSpec, ClusterValidationError, Endpoint, EndpointAddress, HealthCheck,
    HealthCheckProtocol, HealthStatus, LoadBalancingPolicy, OutlierDetection, UpstreamTlsConfig,
};
pub use id::{
    ApiDefinitionId, ApiRouteId, ClusterId, ListenerId, RouteId, TeamId, TokenId, UserId,
};
pub use listener::{
    BindAddress, IsolationMode, ListenerSpec, ListenerValidationError, Protocol,
    TlsConfig as ListenerTlsConfig, TlsVersion as ListenerTlsVersion,
};
pub use route::{
    HeaderMatch, HeaderMatcher, PathMatchStrategy, PathRewrite, QueryParameterMatch,
    QueryParameterMatcher, RetryCondition, RetryPolicy, RouteAction, RouteMatch, RouteTarget,
    WeightedCluster,
};
