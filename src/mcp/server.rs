//! MCP Stdio Server
//!
//! Implements the stdio transport for MCP: reads JSON-RPC messages from stdin
//! and writes responses to stdout.

use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, info, warn};

use crate::mcp::handler::McpHandler;
use crate::mcp::protocol::{error_codes, JsonRpcError, JsonRpcResponse};
use crate::storage::DbPool;

pub struct McpStdioServer {
    handler: McpHandler,
}

impl McpStdioServer {
    /// Create a new MCP stdio server
    ///
    /// # Arguments
    /// * `db_pool` - Database connection pool
    /// * `team` - Team context for multi-tenancy
    ///
    /// Note: CLI access grants admin:all scope since the user has direct machine access
    pub fn new(db_pool: Arc<DbPool>, team: String) -> Self {
        // CLI access grants full permissions - user has direct machine access
        let scopes = vec!["admin:all".to_string()];
        Self { handler: McpHandler::new(db_pool, team, scopes) }
    }

    /// Run the stdio server
    ///
    /// Reads JSON-RPC messages from stdin (line-delimited), processes them through
    /// the handler, and writes responses to stdout. Exits cleanly on EOF.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        info!("Starting MCP stdio server");

        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }

            debug!(line = %line, "Received input line");

            // Parse request
            let request = match serde_json::from_str(&line) {
                Ok(req) => req,
                Err(e) => {
                    warn!(error = %e, line = %line, "Failed to parse JSON-RPC request");

                    let error_response = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: None,
                        result: None,
                        error: Some(JsonRpcError {
                            code: error_codes::PARSE_ERROR,
                            message: format!("Parse error: {}", e),
                            data: None,
                        }),
                    };

                    self.write_response(&mut stdout, &error_response).await?;
                    continue;
                }
            };

            // Handle request
            let response = self.handler.handle_request(request).await;

            // Write response
            self.write_response(&mut stdout, &response).await?;
        }

        info!("MCP stdio server shutting down (EOF received)");
        Ok(())
    }

    async fn write_response(
        &self,
        stdout: &mut tokio::io::Stdout,
        response: &JsonRpcResponse,
    ) -> anyhow::Result<()> {
        let json = serde_json::to_string(response)?;
        debug!(response = %json, "Writing response");

        stdout.write_all(json.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::TestDatabase;

    async fn create_test_server() -> (TestDatabase, McpStdioServer) {
        let test_db = TestDatabase::new("mcp_server").await;
        let pool = test_db.pool.clone();
        let server = McpStdioServer::new(Arc::new(pool), "test-team".to_string());
        (test_db, server)
    }

    #[tokio::test]
    async fn test_server_creation() {
        let (_db, _server) = create_test_server().await;
    }
}
