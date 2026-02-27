//! MCP (Model Context Protocol) Server Implementation
//!
//! Provides HTTP-based MCP server for Flowplane control plane and gateway operations.

pub mod cancellation;
pub mod connection;
pub mod connections_api;
pub mod error;
pub mod gateway;
pub mod handler;
pub mod logging;
pub mod message_buffer;
pub mod notifications;
pub mod progress;
pub mod prompts;
pub mod protocol;
pub mod resources;
pub mod response_builders;
pub mod security;
// pub mod server; // Removed — MCP stdio server replaced by HTTP endpoint
pub mod session;
pub mod streamable_http;
pub mod tool_registry;
pub mod tools;
pub mod transport_common;

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
pub use logging::{create_mcp_logger, McpLogger, SetLogLevelParams, SharedMcpLogger};
pub use message_buffer::MessageBuffer;
pub use notifications::{
    LogLevel, LogNotification, NotificationMessage, ProgressNotification, ProgressToken,
};
pub use progress::{create_progress_tracker, ProgressTracker, SharedProgressTracker};
pub use protocol::*;
pub use resources::{list_resources, read_resource, ResourceType, ResourceUri};
pub use security::{
    check_team_ownership, generate_secure_connection_id, generate_secure_session_id,
    get_default_origin_allowlist, load_origin_allowlist_from_env, validate_origin_header,
    validate_session_id_format,
};
// McpStdioServer removed — MCP is served via HTTP at /api/v1/mcp
pub use session::{
    create_session_manager, create_session_manager_with_ttl, McpSession, SessionId, SessionManager,
    SharedSessionManager,
};
pub use streamable_http::{delete_handler, get_handler, post_handler};
pub use tool_registry::{get_tool_authorization, ToolAuthorization};
pub use transport_common::{
    determine_response_mode, error_response_json, extract_mcp_headers, extract_team, get_db_pool,
    validate_protocol_version, McpHeaders, ResponseMode,
};
