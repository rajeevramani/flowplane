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
pub mod logging;
pub mod message_buffer;
pub mod notifications;
pub mod progress;
pub mod prompts;
pub mod protocol;
pub mod resources;
pub mod response_builders;
pub mod security;
pub mod server;
pub mod session;
pub mod streamable_http;
pub mod tool_registry;
pub mod tools;
pub mod transport_common;

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
pub use server::McpStdioServer;
pub use session::{
    create_session_manager, create_session_manager_with_ttl, McpSession, SessionId, SessionManager,
    SharedSessionManager,
};
pub use streamable_http::{
    delete_handler_api, delete_handler_cp, get_handler_api, get_handler_cp, post_handler_api,
    post_handler_cp, McpScope,
};
pub use tool_registry::{
    check_scope_grants_authorization, get_tool_authorization, ToolAuthorization,
};
pub use transport_common::{
    check_method_authorization, determine_response_mode, error_response_json, extract_mcp_headers,
    extract_team, get_db_pool, validate_protocol_version, McpHeaders, ResponseMode, ScopeConfig,
    API_SCOPES, CP_SCOPES,
};
