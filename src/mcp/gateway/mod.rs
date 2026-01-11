//! Gateway API Tool Generation and Execution
//!
//! This module provides functionality to generate MCP tools from gateway routes
//! with metadata extracted from OpenAPI specifications or learned from traffic,
//! and execute HTTP requests through the Envoy gateway.

pub mod executor;
pub mod generator;

pub use executor::GatewayExecutor;
pub use generator::GatewayToolGenerator;
