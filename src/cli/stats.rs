//! Stats CLI commands
//!
//! Provides command-line interface for viewing system and cluster statistics.

use anyhow::Result;
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use super::client::FlowplaneClient;
use super::output::{print_output, truncate};

#[derive(Subcommand)]
pub enum StatsCommands {
    /// Show system overview statistics
    #[command(
        long_about = "Show high-level statistics for the current team including cluster count,\nlistener count, route count, and learning session metrics.",
        after_help = "EXAMPLES:\n    flowplane stats overview\n    flowplane stats overview -o json"
    )]
    Overview {
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Show statistics for all clusters
    #[command(
        long_about = "Show statistics for all clusters in the current team.",
        after_help = "EXAMPLES:\n    flowplane stats clusters\n    flowplane stats clusters -o json"
    )]
    Clusters {
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Show statistics for a specific cluster
    #[command(
        long_about = "Show detailed statistics for a specific cluster by name.",
        after_help = "EXAMPLES:\n    flowplane stats cluster my-cluster\n    flowplane stats cluster my-cluster -o json"
    )]
    Cluster {
        /// Cluster name
        #[arg(value_name = "NAME")]
        name: String,

        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },
}

/// Stats overview response from the API
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsOverviewResponse {
    #[serde(flatten)]
    pub data: serde_json::Value,
}

/// Cluster stats response from the API
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterStatsResponse {
    #[serde(flatten)]
    pub data: serde_json::Value,
}

pub async fn handle_stats_command(
    command: StatsCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        StatsCommands::Overview { output } => {
            let path = format!("/api/v1/teams/{team}/stats/overview");
            let response: serde_json::Value = client.get_json(&path).await?;

            if output == "table" {
                print_stats_overview_table(&response);
            } else {
                print_output(&response, &output)?;
            }
        }
        StatsCommands::Clusters { output } => {
            let path = format!("/api/v1/teams/{team}/stats/clusters");
            let response: serde_json::Value = client.get_json(&path).await?;

            if output == "table" {
                print_cluster_stats_table(&response);
            } else {
                print_output(&response, &output)?;
            }
        }
        StatsCommands::Cluster { name, output } => {
            let path = format!("/api/v1/teams/{team}/stats/clusters/{name}");
            let response: serde_json::Value = client.get_json(&path).await?;

            if output == "table" {
                print_single_cluster_stats(&name, &response);
            } else {
                print_output(&response, &output)?;
            }
        }
    }

    Ok(())
}

fn print_stats_overview_table(data: &serde_json::Value) {
    println!();
    println!("System Overview");
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

fn print_cluster_stats_table(data: &serde_json::Value) {
    if let Some(arr) = data.as_array() {
        if arr.is_empty() {
            println!("No cluster statistics found");
            return;
        }

        println!();
        println!("{:<35} {:>12} {:>12} {:>12}", "Cluster", "Requests", "Errors", "Latency");
        println!("{}", "-".repeat(75));

        for item in arr {
            let name = item
                .get("name")
                .or_else(|| item.get("clusterName"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let requests =
                item.get("totalRequests").or_else(|| item.get("requests")).map(format_value);
            let errors = item.get("totalErrors").or_else(|| item.get("errors")).map(format_value);
            let latency = item.get("avgLatency").or_else(|| item.get("latency")).map(format_value);

            println!(
                "{:<35} {:>12} {:>12} {:>12}",
                truncate(name, 33),
                requests.as_deref().unwrap_or("-"),
                errors.as_deref().unwrap_or("-"),
                latency.as_deref().unwrap_or("-"),
            );
        }
        println!();
    } else if let Some(obj) = data.as_object() {
        // Single object with cluster stats
        println!();
        println!("Cluster Statistics");
        println!("{}", "-".repeat(40));
        for (key, value) in obj {
            let display_key = key.replace('_', " ");
            println!("  {:<25} {}", display_key, format_value(value));
        }
        println!();
    } else {
        println!("{data}");
    }
}

fn print_single_cluster_stats(name: &str, data: &serde_json::Value) {
    println!();
    println!("Cluster: {name}");
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
