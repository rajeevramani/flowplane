//! E2E Test Infrastructure v2
//!
//! This module provides a robust test harness for end-to-end testing with:
//! - Hard timeouts (30s max per operation) - no more hanging tests
//! - wiremock-based external service mocks (Auth0, httpbin, ext_authz)
//! - TestHarness orchestrator for clean setup/teardown
//! - Typed API client with proper error handling
//! - Shared infrastructure mode for faster test execution
//! - DRY resource setup builder for filter tests
//! - Type-safe filter configuration builders

pub mod api_client;
pub mod control_plane;
pub mod envoy;
pub mod filter_configs;
pub mod harness;
pub mod mocks;
pub mod ports;
pub mod resource_setup;
pub mod shared_infra;
pub mod stats;
pub mod timeout;

pub use api_client::*;
pub use control_plane::ControlPlaneHandle;
pub use envoy::EnvoyHandle;
pub use harness::{TestHarness, TestHarnessConfig};
pub use mocks::MockServices;
pub use ports::PortAllocator;
pub use resource_setup::{
    ClusterConfig, FilterConfig, ListenerConfig, ResourceSetup, RouteConfig, TestResources,
};
pub use shared_infra::{SharedInfrastructure, SHARED_LISTENER_PORT};
pub use timeout::{with_timeout, TestTimeout};
