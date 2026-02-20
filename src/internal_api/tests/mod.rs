//! Integration tests for internal API layer
//!
//! This module contains comprehensive integration tests covering:
//! - Route hierarchy operations (virtual hosts and routes)
//! - MCP tool execution
//! - Team isolation
//! - Cascade deletes
//! - Filter attachment operations

#[cfg(test)]
mod filter_attachment_integration;

#[cfg(test)]
mod mcp_tool_execution;

#[cfg(test)]
mod route_hierarchy_integration;
