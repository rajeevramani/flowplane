//! Route CLI commands
//!
//! Provides command-line interface for managing route configurations

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::FlowplaneClient;

#[derive(Subcommand)]
pub enum RouteCommands {
    /// Create a new route
    Create {
        /// Path to JSON file with route spec
        #[arg(short, long)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// List all routes
    List {
        /// Filter by cluster name
        #[arg(long)]
        cluster: Option<String>,

        /// Maximum number of results
        #[arg(long)]
        limit: Option<i32>,

        /// Offset for pagination
        #[arg(long)]
        offset: Option<i32>,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table")]
        output: String,
    },

    /// Get a specific route by name
    Get {
        /// Route name
        name: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// Update a route
    Update {
        /// Route name
        name: String,

        /// Path to JSON file with update spec
        #[arg(short, long)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// Delete a route
    Delete {
        /// Route name
        name: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

/// Route response structure
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteResponse {
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Handle route commands
pub async fn handle_route_command(command: RouteCommands, client: &FlowplaneClient) -> Result<()> {
    match command {
        RouteCommands::Create { file, output } => create_route(client, file, &output).await?,
        RouteCommands::List { cluster, limit, offset, output } => {
            list_routes(client, cluster, limit, offset, &output).await?
        }
        RouteCommands::Get { name, output } => get_route(client, &name, &output).await?,
        RouteCommands::Update { name, file, output } => {
            update_route(client, &name, file, &output).await?
        }
        RouteCommands::Delete { name, yes } => delete_route(client, &name, yes).await?,
    }

    Ok(())
}

async fn create_route(client: &FlowplaneClient, file: PathBuf, output: &str) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse JSON from file")?;

    let response: RouteResponse = client.post_json("/api/v1/routes", &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn list_routes(
    client: &FlowplaneClient,
    cluster: Option<String>,
    limit: Option<i32>,
    offset: Option<i32>,
    output: &str,
) -> Result<()> {
    let mut path = String::from("/api/v1/routes?");
    let mut params = Vec::new();

    if let Some(c) = cluster {
        params.push(format!("cluster={}", c));
    }
    if let Some(l) = limit {
        params.push(format!("limit={}", l));
    }
    if let Some(o) = offset {
        params.push(format!("offset={}", o));
    }

    path.push_str(&params.join("&"));

    let response: Vec<RouteResponse> = client.get_json(&path).await?;

    if output == "table" {
        print_routes_table(&response);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn get_route(client: &FlowplaneClient, name: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/routes/{}", name);
    let response: RouteResponse = client.get_json(&path).await?;

    if output == "table" {
        print_routes_table(&[response]);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn update_route(
    client: &FlowplaneClient,
    name: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse JSON from file")?;

    let path = format!("/api/v1/routes/{}", name);
    let response: RouteResponse = client.put_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn delete_route(client: &FlowplaneClient, name: &str, yes: bool) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete route '{}'? (y/N)", name);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/routes/{}", name);
    let _: serde_json::Value = client.delete_json(&path).await?;

    println!("Route '{}' deleted successfully", name);
    Ok(())
}

fn print_output<T: Serialize>(data: &T, format: &str) -> Result<()> {
    match format {
        "json" => {
            let json = serde_json::to_string_pretty(data).context("Failed to serialize to JSON")?;
            println!("{}", json);
        }
        "yaml" => {
            let yaml = serde_yaml::to_string(data).context("Failed to serialize to YAML")?;
            println!("{}", yaml);
        }
        _ => {
            anyhow::bail!("Unsupported output format: {}. Use 'json' or 'yaml'.", format);
        }
    }
    Ok(())
}

fn print_routes_table(routes: &[RouteResponse]) {
    if routes.is_empty() {
        println!("No routes found");
        return;
    }

    println!();
    println!("{:<30} {:<25} {:<25} {:<10}", "Name", "Path Prefix", "Cluster", "Version");
    println!("{}", "-".repeat(95));

    for route in routes {
        println!(
            "{:<30} {:<25} {:<25} {:<10}",
            truncate(&route.name, 28),
            truncate(&route.path_prefix, 23),
            truncate(&route.cluster_name, 23),
            route.version
        );
    }
    println!();
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}
