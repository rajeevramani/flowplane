//! MCP (Model Context Protocol) Server Implementation
//!
//! Provides stdio-based and HTTP-based MCP server for Flowplane control plane operations.

pub mod api_handler;
pub mod cancellation;
pub mod connection;
pub mod connections_api;
pub mod error;
pub mod gateway;
pub mod handler;
pub mod http;
pub mod http_api;
pub mod logging;
pub mod notifications;
pub mod progress;
pub mod prompts;
pub mod protocol;
pub mod resources;
pub mod server;
pub mod session;
pub mod sse;
pub mod tools;

pub use api_handler::McpApiHandler;
pub use cancellation::{
    create_cancellation_manager, CancellationManager, SharedCancellationManager,
};
pub use connection::{
    create_connection_manager, ConnectionId, ConnectionManager, SharedConnectionManager,
};
pub use connections_api::list_connections_handler;
pub use error::McpError;
pub use gateway::{GatewayExecutor, GatewayToolGenerator};
pub use handler::McpHandler;
pub use http::mcp_http_handler;
pub use http_api::mcp_api_http_handler;
pub use logging::{create_mcp_logger, McpLogger, SetLogLevelParams, SharedMcpLogger};
pub use notifications::{
    LogLevel, LogNotification, NotificationMessage, ProgressNotification, ProgressToken,
};
pub use progress::{create_progress_tracker, ProgressTracker, SharedProgressTracker};
pub use protocol::*;
pub use resources::{list_resources, read_resource, ResourceType, ResourceUri};
pub use server::McpStdioServer;
pub use session::{
    create_session_manager, create_session_manager_with_ttl, McpSession, SessionId, SessionManager,
    SharedSessionManager,
};
pub use sse::{mcp_api_sse_handler, mcp_sse_handler};
