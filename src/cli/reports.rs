//! Reports CLI commands
//!
//! Provides command-line interface for viewing operational reports.

use anyhow::Result;
use clap::Subcommand;

use super::client::FlowplaneClient;
use super::output::{print_output, truncate};

#[derive(Subcommand)]
pub enum ReportsCommands {
    /// Show route flow reports
    #[command(
        long_about = "Show route flow reports displaying how traffic flows through listeners,\nroute configs, and clusters.",
        after_help = "EXAMPLES:\n    flowplane reports route-flows\n    flowplane reports route-flows -o json"
    )]
    RouteFlows {
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },
}

pub async fn handle_reports_command(
    command: ReportsCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        ReportsCommands::RouteFlows { output } => {
            let path = format!("/api/v1/teams/{team}/reports/route-flows");
            let response: serde_json::Value = client.get_json(&path).await?;

            if output == "table" {
                print_route_flows_table(&response);
            } else {
                print_output(&response, &output)?;
            }
        }
    }

    Ok(())
}

fn print_route_flows_table(data: &serde_json::Value) {
    if let Some(arr) = data.as_array() {
        if arr.is_empty() {
            println!("No route flows found");
            return;
        }

        println!();
        println!("{:<25} {:<25} {:<25} {:>10}", "Listener", "Route Config", "Cluster", "Routes");
        println!("{}", "-".repeat(89));

        for item in arr {
            let listener = item
                .get("listener")
                .or_else(|| item.get("listenerName"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let route_config = item
                .get("routeConfig")
                .or_else(|| item.get("routeConfigName"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let cluster = item
                .get("cluster")
                .or_else(|| item.get("clusterName"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let routes = item
                .get("routeCount")
                .or_else(|| item.get("routes"))
                .map(format_value)
                .unwrap_or_else(|| "-".to_string());

            println!(
                "{:<25} {:<25} {:<25} {:>10}",
                truncate(listener, 23),
                truncate(route_config, 23),
                truncate(cluster, 23),
                routes,
            );
        }
        println!();
    } else if let Some(obj) = data.as_object() {
        if let Some(items) = obj.get("items").and_then(|v| v.as_array()) {
            let as_val = serde_json::Value::Array(items.clone());
            print_route_flows_table(&as_val);
        } else {
            println!();
            println!("Route Flows");
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

fn format_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "-".to_string(),
        _ => v.to_string(),
    }
}
