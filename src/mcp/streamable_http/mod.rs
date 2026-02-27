//! MCP Streamable HTTP Transport (MCP 2025-11-25)
//!
//! Unified HTTP transport supporting POST, GET, DELETE per MCP 2025-11-25 spec.
//!
//! # Endpoint
//! - `/api/v1/mcp` - Unified endpoint for CP and Gateway API tools (POST, GET, DELETE)
//!
//! # Methods
//! - POST: Send JSON-RPC request (JSON or SSE response based on Accept header)
//! - GET: Open SSE stream for server notifications (requires existing session)
//! - DELETE: Terminate session
//!
//! # Headers (MCP 2025-11-25)
//! - `MCP-Protocol-Version`: Optional - supported versions: 2025-11-25, 2025-03-26
//! - `MCP-Session-Id`: Required after initialize - UUID v4 format
//! - `Accept`: `application/json` or `text/event-stream` for response mode

mod delete_handler;
mod get_handler;
mod post_handler;

pub use delete_handler::delete_handler;
pub use get_handler::get_handler;
pub use post_handler::post_handler;
