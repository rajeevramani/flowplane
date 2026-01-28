//! MCP CLI Commands

use crate::config::DatabaseConfig;
use crate::mcp::McpStdioServer;
use crate::storage::create_pool;
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum McpCommands {
    /// Start MCP server via stdio
    Serve {
        /// Team to serve tools for
        #[arg(long)]
        team: String,

        /// Database URL (optional, defaults to env)
        #[arg(long)]
        database_url: Option<String>,
    },
}

pub async fn handle_mcp_command(command: McpCommands, db: &DatabaseConfig) -> anyhow::Result<()> {
    match command {
        McpCommands::Serve { team, database_url } => handle_mcp_serve(team, database_url, db).await,
    }
}

async fn handle_mcp_serve(
    team: String,
    database_url: Option<String>,
    db_config: &DatabaseConfig,
) -> anyhow::Result<()> {
    let mut config = db_config.clone();
    if let Some(url) = database_url {
        config.url = url;
    }
    // Disable auto-migrate for MCP server (migrations should be run separately)
    config.auto_migrate = false;

    let pool = create_pool(&config).await?;
    let mut server = McpStdioServer::new(Arc::new(pool), team);

    server.run().await?;
    Ok(())
}
