//! MCP (Model Context Protocol) Server Implementation
//!
//! Provides stdio-based and HTTP-based MCP server for Flowplane control plane operations.

pub mod error;
pub mod gateway;
pub mod handler;
pub mod http;
pub mod protocol;
pub mod server;
pub mod tools;

pub use error::McpError;
pub use gateway::{GatewayExecutor, GatewayToolGenerator};
pub use handler::McpHandler;
pub use http::mcp_http_handler;
pub use protocol::*;
pub use server::McpStdioServer;
