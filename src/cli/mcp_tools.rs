//! MCP tool management CLI commands
//!
//! Provides command-line interface for listing MCP tools and enabling/disabling
//! MCP exposure on routes. These commands manage the MCP tool registry, not the
//! MCP protocol client.

use anyhow::Result;
use clap::Subcommand;

use super::client::FlowplaneClient;
use super::output::{print_output, print_table_header, truncate};

#[derive(Subcommand)]
pub enum McpToolsCommands {
    /// List registered MCP tools for the current team
    #[command(
        name = "tools",
        long_about = "List all MCP tools registered for the current team.\n\n\
            Shows tool names, descriptions, and enabled status.",
        after_help = "EXAMPLES:\n    flowplane mcp tools\n    flowplane mcp tools -o json"
    )]
    Tools {
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "table"])]
        output: String,
    },

    /// Enable MCP exposure on a route
    #[command(
        long_about = "Enable MCP tool generation for a specific route.\n\n\
            Once enabled, the route's operations are exposed as callable MCP tools.",
        after_help = "EXAMPLES:\n    flowplane mcp enable my-route-id"
    )]
    Enable {
        /// Route ID to enable MCP on
        route_id: String,
    },

    /// Disable MCP exposure on a route
    #[command(
        long_about = "Disable MCP tool generation for a specific route.\n\n\
            The route continues to function normally but is no longer exposed as MCP tools.",
        after_help = "EXAMPLES:\n    flowplane mcp disable my-route-id"
    )]
    Disable {
        /// Route ID to disable MCP on
        route_id: String,
    },
}

pub async fn handle_mcp_tools_command(
    command: McpToolsCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        McpToolsCommands::Tools { output } => {
            let path = format!("/api/v1/teams/{team}/mcp/tools");
            let response: serde_json::Value = client.get_json(&path).await?;

            if output == "table" {
                print_mcp_tools_table(&response);
            } else {
                print_output(&response, &output)?;
            }
        }
        McpToolsCommands::Enable { route_id } => {
            let path = format!("/api/v1/teams/{team}/routes/{route_id}/mcp/enable");
            let response: serde_json::Value =
                client.post_json(&path, &serde_json::json!({})).await?;
            println!("MCP enabled for route '{route_id}'");
            let json = serde_json::to_string_pretty(&response)?;
            println!("{json}");
        }
        McpToolsCommands::Disable { route_id } => {
            let path = format!("/api/v1/teams/{team}/routes/{route_id}/mcp/disable");
            let response: serde_json::Value =
                client.post_json(&path, &serde_json::json!({})).await?;
            println!("MCP disabled for route '{route_id}'");
            let json = serde_json::to_string_pretty(&response)?;
            println!("{json}");
        }
    }

    Ok(())
}

fn print_mcp_tools_table(data: &serde_json::Value) {
    let items = match data.as_array() {
        Some(arr) => arr.clone(),
        None => vec![data.clone()],
    };

    if items.is_empty() {
        println!("No MCP tools found");
        return;
    }

    print_table_header(&[("Name", 30), ("Description", 45), ("Enabled", 8), ("Route", 25)]);

    for item in &items {
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
        let description = item.get("description").and_then(|v| v.as_str()).unwrap_or("-");
        let enabled = item
            .get("enabled")
            .and_then(|v| v.as_bool())
            .map(|b| if b { "yes" } else { "no" })
            .unwrap_or("-");
        let route = item
            .get("routeName")
            .or_else(|| item.get("route_name"))
            .or_else(|| item.get("route"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");

        println!(
            "{:<30} {:<45} {:<8} {}",
            truncate(name, 28),
            truncate(description, 43),
            enabled,
            truncate(route, 23),
        );
    }
    println!();
}
