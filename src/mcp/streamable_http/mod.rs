//! MCP Streamable HTTP Transport (MCP 2025-11-25)
//!
//! Unified HTTP transport supporting POST, GET, DELETE per MCP 2025-11-25 spec.
//! Replaces separate HTTP and SSE endpoints with a single endpoint.
//!
//! # Endpoints
//! - `/api/v1/mcp/cp` - Control Plane tools (POST, GET, DELETE)
//! - `/api/v1/mcp/api` - API/Gateway tools (POST, GET, DELETE)
//!
//! # Methods
//! - POST: Send JSON-RPC request (JSON or SSE response based on Accept header)
//! - GET: Open SSE stream for server notifications (requires existing session)
//! - DELETE: Terminate session
//!
//! # Headers (MCP 2025-11-25)
//! - `MCP-Protocol-Version`: Required - must be "2025-11-25"
//! - `MCP-Session-Id`: Required after initialize - UUID v4 format
//! - `Accept`: `application/json` or `text/event-stream` for response mode

mod delete_handler;
mod get_handler;
mod post_handler;

pub use delete_handler::{delete_handler_api, delete_handler_cp};
pub use get_handler::{get_handler_api, get_handler_cp};
pub use post_handler::{post_handler_api, post_handler_cp};

/// MCP scope type for distinguishing Control Plane vs API endpoints
///
/// Used by generic handlers to determine authorization requirements
/// and handler routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpScope {
    /// Control Plane tools - infrastructure management (clusters, routes, filters)
    ControlPlane,

    /// Gateway API tools - runtime API operations
    GatewayApi,
}

impl McpScope {
    /// Get the scope configuration for authorization
    pub fn scope_config(&self) -> &'static crate::mcp::transport_common::ScopeConfig {
        match self {
            McpScope::ControlPlane => &crate::mcp::transport_common::CP_SCOPES,
            McpScope::GatewayApi => &crate::mcp::transport_common::API_SCOPES,
        }
    }

    /// Get the endpoint path for this scope
    pub fn endpoint_path(&self) -> &'static str {
        match self {
            McpScope::ControlPlane => "/api/v1/mcp/cp",
            McpScope::GatewayApi => "/api/v1/mcp/api",
        }
    }
}
