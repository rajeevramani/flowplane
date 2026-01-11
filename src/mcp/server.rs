//! MCP Stdio Server
//!
//! Implements the stdio transport for MCP: reads JSON-RPC messages from stdin
//! and writes responses to stdout.

use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, info, warn};

use crate::mcp::handler::McpHandler;
use crate::mcp::protocol::{error_codes, JsonRpcError, JsonRpcResponse};

pub struct McpStdioServer {
    handler: McpHandler,
}

impl McpStdioServer {
    pub fn new(db_pool: Arc<SqlitePool>, team: String) -> Self {
        Self { handler: McpHandler::new(db_pool, team) }
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
    use crate::config::DatabaseConfig;
    use crate::storage::create_pool;

    async fn create_test_server() -> McpStdioServer {
        let config = DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            max_connections: 5,
            min_connections: 1,
            connect_timeout_seconds: 5,
            idle_timeout_seconds: 0,
            auto_migrate: false,
        };
        let pool = create_pool(&config).await.expect("Failed to create pool");
        McpStdioServer::new(Arc::new(pool), "test-team".to_string())
    }

    #[tokio::test]
    async fn test_server_creation() {
        let _server = create_test_server().await;
    }
}
