//! Admin utility CLI commands
//!
//! Provides command-line interface for platform administration:
//! resource summaries, scope listings, and filter schema management.

use anyhow::Result;
use clap::Subcommand;

use super::client::FlowplaneClient;
use super::output::{print_output, print_table_header, truncate};

#[derive(Subcommand)]
pub enum AdminCommands {
    /// Show a summary of all resources across the platform
    #[command(
        long_about = "Display a summary of all resources across the platform.\n\n\
            Requires platform admin privileges (admin:all scope).",
        after_help = "EXAMPLES:\n    flowplane admin resources\n    flowplane admin resources -o json"
    )]
    Resources {
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "table"])]
        output: String,
    },

    /// List all registered authorization scopes
    #[command(
        long_about = "List all authorization scopes registered in the system.\n\n\
            Requires platform admin privileges (admin:all scope).",
        after_help = "EXAMPLES:\n    flowplane admin scopes\n    flowplane admin scopes -o json"
    )]
    Scopes {
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "table"])]
        output: String,
    },

    /// Reload filter schemas from disk
    #[command(
        name = "reload-filter-schemas",
        long_about = "Trigger a reload of all filter schemas from disk.\n\n\
            Useful after deploying new filter schema definitions.\n\
            Requires platform admin privileges.",
        after_help = "EXAMPLES:\n    flowplane admin reload-filter-schemas"
    )]
    ReloadFilterSchemas,
}

pub async fn handle_admin_command(command: AdminCommands, client: &FlowplaneClient) -> Result<()> {
    match command {
        AdminCommands::Resources { output } => {
            let path = "/api/v1/admin/resources/summary";
            let response: serde_json::Value = client.get_json(path).await?;

            if output == "table" {
                print_resources_table(&response);
            } else {
                print_output(&response, &output)?;
            }
        }
        AdminCommands::Scopes { output } => {
            let path = "/api/v1/admin/scopes";
            let response: serde_json::Value = client.get_json(path).await?;

            if output == "table" {
                print_scopes_table(&response);
            } else {
                print_output(&response, &output)?;
            }
        }
        AdminCommands::ReloadFilterSchemas => {
            let path = "/api/v1/admin/filter-schemas/reload";
            let _response: serde_json::Value =
                client.post_json(path, &serde_json::json!({})).await?;
            println!("Filter schemas reloaded successfully");
        }
    }

    Ok(())
}

fn print_resources_table(data: &serde_json::Value) {
    if let Some(obj) = data.as_object() {
        if obj.is_empty() {
            println!("No resource data available");
            return;
        }

        print_table_header(&[("Resource Type", 25), ("Count", 10)]);

        // Sort keys for stable output
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();

        for key in keys {
            let count = obj[key]
                .as_i64()
                .map(|n| n.to_string())
                .or_else(|| obj[key].as_u64().map(|n| n.to_string()))
                .unwrap_or_else(|| obj[key].to_string());
            println!("{:<25} {}", truncate(key, 23), count);
        }
        println!();
    } else if let Some(arr) = data.as_array() {
        if arr.is_empty() {
            println!("No resource data available");
            return;
        }

        print_table_header(&[("Resource Type", 25), ("Count", 10), ("Team", 20)]);

        for item in arr {
            let resource_type = item
                .get("resourceType")
                .or_else(|| item.get("resource_type"))
                .or_else(|| item.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let count = item.get("count").map(|v| v.to_string()).unwrap_or_else(|| "-".to_string());
            let team = item.get("team").and_then(|v| v.as_str()).unwrap_or("-");

            println!(
                "{:<25} {:<10} {}",
                truncate(resource_type, 23),
                truncate(&count, 8),
                truncate(team, 18),
            );
        }
        println!();
    } else {
        print_output(data, "json").ok();
    }
}

fn print_scopes_table(data: &serde_json::Value) {
    let items = match data.as_array() {
        Some(arr) => arr.clone(),
        None => vec![data.clone()],
    };

    if items.is_empty() {
        println!("No scopes found");
        return;
    }

    print_table_header(&[("Scope", 40), ("Resource", 20), ("Action", 15), ("Description", 40)]);

    for item in &items {
        let scope = item
            .get("scope")
            .or_else(|| item.get("key"))
            .or_else(|| item.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let resource = item.get("resource").and_then(|v| v.as_str()).unwrap_or("-");
        let action = item.get("action").and_then(|v| v.as_str()).unwrap_or("-");
        let description = item.get("description").and_then(|v| v.as_str()).unwrap_or("-");

        println!(
            "{:<40} {:<20} {:<15} {}",
            truncate(scope, 38),
            truncate(resource, 18),
            truncate(action, 13),
            truncate(description, 38),
        );
    }
    println!();
}
