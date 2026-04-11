//! Route views CLI commands
//!
//! Provides command-line interface for viewing aggregated route information.

use anyhow::Result;
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use super::client::FlowplaneClient;
use super::output::{print_output, truncate};

#[derive(Subcommand)]
pub enum RouteViewsCommands {
    /// List all route views
    #[command(
        long_about = "List aggregated route views showing how routes map across listeners and clusters.",
        after_help = "EXAMPLES:\n    flowplane route-views list\n    flowplane route-views list -o json"
    )]
    List {
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Show route view statistics
    #[command(
        long_about = "Show summary statistics for route views including totals and breakdowns.",
        after_help = "EXAMPLES:\n    flowplane route-views stats\n    flowplane route-views stats -o json"
    )]
    Stats {
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },
}

/// Route view entry from the API
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteViewResponse {
    #[serde(flatten)]
    pub data: serde_json::Value,
}

pub async fn handle_route_views_command(
    command: RouteViewsCommands,
    client: &FlowplaneClient,
    _team: &str,
) -> Result<()> {
    match command {
        RouteViewsCommands::List { output } => {
            let path = "/api/v1/route-views".to_string();
            let response: serde_json::Value = client.get_json(&path).await?;

            if output == "table" {
                print_route_views_table(&response);
            } else {
                print_output(&response, &output)?;
            }
        }
        RouteViewsCommands::Stats { output } => {
            let path = "/api/v1/route-views/stats".to_string();
            let response: serde_json::Value = client.get_json(&path).await?;

            if output == "table" {
                print_route_views_stats(&response);
            } else {
                print_output(&response, &output)?;
            }
        }
    }

    Ok(())
}

fn print_route_views_table(data: &serde_json::Value) {
    if let Some(arr) = data.as_array() {
        if arr.is_empty() {
            println!("No route views found");
            return;
        }

        println!();
        println!("{:<30} {:<10} {:<30} {:<20}", "Route", "Method", "Cluster", "Listener");
        println!("{}", "-".repeat(94));

        for item in arr {
            let route = item
                .get("path")
                .or_else(|| item.get("route"))
                .or_else(|| item.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let method = item
                .get("method")
                .or_else(|| item.get("httpMethod"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let cluster = item
                .get("cluster")
                .or_else(|| item.get("clusterName"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let listener = item
                .get("listener")
                .or_else(|| item.get("listenerName"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");

            println!(
                "{:<30} {:<10} {:<30} {:<20}",
                truncate(route, 28),
                truncate(method, 8),
                truncate(cluster, 28),
                truncate(listener, 18),
            );
        }
        println!();
    } else if let Some(obj) = data.as_object() {
        // Could be a wrapper object with items field
        if let Some(items) = obj.get("items").and_then(|v| v.as_array()) {
            let as_val = serde_json::Value::Array(items.clone());
            print_route_views_table(&as_val);
        } else {
            println!();
            println!("Route Views");
            println!("{}", "-".repeat(40));
            for (key, value) in obj {
                println!("  {:<25} {}", key, format_value(value));
            }
            println!();
        }
    } else {
        println!("{data}");
    }
}

fn print_route_views_stats(data: &serde_json::Value) {
    println!();
    println!("Route View Statistics");
    println!("{}", "-".repeat(40));

    if let Some(obj) = data.as_object() {
        for (key, value) in obj {
            let display_key = key.replace('_', " ");
            println!("  {:<25} {}", display_key, format_value(value));
        }
    } else {
        println!("{data}");
    }
    println!();
}

fn format_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "-".to_string(),
        _ => v.to_string(),
    }
}
