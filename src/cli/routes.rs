//! Route CLI commands
//!
//! Provides command-line interface for managing route configurations

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::FlowplaneClient;
use crate::api::handlers::PaginatedResponse;

#[derive(Subcommand)]
pub enum RouteCommands {
    /// Create a new route configuration
    #[command(
        long_about = "Create a new route by providing a JSON file with the route specification.\n\nThe JSON file should contain fields like name, path_prefix, cluster_name, and optional match conditions.",
        after_help = "EXAMPLES:\n    # Create a route from a JSON file\n    flowplane-cli route create --file route-spec.json\n\n    # Create and output as YAML\n    flowplane-cli route create --file route-spec.json --output yaml\n\n    # With authentication\n    flowplane-cli route create --file route-spec.json --token your-token"
    )]
    Create {
        /// Path to JSON file with route spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// List all route configurations
    #[command(
        long_about = "List all route configurations in the system with optional filtering and pagination.\n\nRoutes define path matching and routing rules for traffic to clusters.",
        after_help = "EXAMPLES:\n    # List all routes\n    flowplane-cli route list\n\n    # List with table output\n    flowplane-cli route list --output table\n\n    # Filter by cluster name\n    flowplane-cli route list --cluster backend-api\n\n    # Paginate results\n    flowplane-cli route list --limit 10 --offset 20"
    )]
    List {
        /// Filter by cluster name
        #[arg(long, value_name = "CLUSTER")]
        cluster: Option<String>,

        /// Maximum number of results
        #[arg(long, value_name = "NUMBER")]
        limit: Option<i32>,

        /// Offset for pagination
        #[arg(long, value_name = "NUMBER")]
        offset: Option<i32>,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Get details of a specific route by name
    #[command(
        long_about = "Retrieve detailed information about a specific route configuration by its name.\n\nShows path matching rules, cluster association, and metadata.",
        after_help = "EXAMPLES:\n    # Get route details in JSON format\n    flowplane-cli route get my-api-route\n\n    # Get route in YAML format\n    flowplane-cli route get my-api-route --output yaml\n\n    # With authentication\n    flowplane-cli route get my-api-route --token your-token --base-url https://api.example.com"
    )]
    Get {
        /// Route name
        #[arg(value_name = "NAME")]
        name: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Update an existing route configuration
    #[command(
        long_about = "Update an existing route configuration by providing a JSON file with the updated specification.\n\nYou can modify path matching, cluster association, and other route properties.",
        after_help = "EXAMPLES:\n    # Update a route from JSON file\n    flowplane-cli route update my-api-route --file updated-route.json\n\n    # Update and output as YAML\n    flowplane-cli route update my-api-route --file updated-route.json --output yaml\n\n    # With authentication\n    flowplane-cli route update my-api-route --file updated-route.json --token your-token"
    )]
    Update {
        /// Route name
        #[arg(value_name = "NAME")]
        name: String,

        /// Path to JSON file with update spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Delete a route configuration
    #[command(
        long_about = "Delete a route configuration by name.\n\nThis removes the route and stops traffic matching from being routed to the associated cluster.",
        after_help = "EXAMPLES:\n    # Delete a route (with confirmation)\n    flowplane-cli route delete my-api-route\n\n    # Delete without confirmation prompt\n    flowplane-cli route delete my-api-route --yes\n\n    # With authentication\n    flowplane-cli route delete my-api-route --token your-token"
    )]
    Delete {
        /// Route name
        #[arg(value_name = "NAME")]
        name: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

/// Route config response structure (matches API's RouteConfigResponse)
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteConfigResponse {
    pub name: String,
    pub team: String,
    pub path_prefix: String,
    pub cluster_targets: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_id: Option<String>,
    pub route_order: Option<i64>,
    pub config: serde_json::Value,
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

    let response: RouteConfigResponse = client.post_json("/api/v1/route-configs", &body).await?;

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
    let mut path = String::from("/api/v1/route-configs?");
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

    let response: PaginatedResponse<RouteConfigResponse> = client.get_json(&path).await?;

    if output == "table" {
        print_routes_table(&response.items);
    } else {
        print_output(&response.items, output)?;
    }

    Ok(())
}

async fn get_route(client: &FlowplaneClient, name: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/route-configs/{}", name);
    let response: RouteConfigResponse = client.get_json(&path).await?;

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

    let path = format!("/api/v1/route-configs/{}", name);
    let response: RouteConfigResponse = client.put_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn delete_route(client: &FlowplaneClient, name: &str, yes: bool) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete route config '{}'? (y/N)", name);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/route-configs/{}", name);
    client.delete_no_content(&path).await?;

    println!("Route config '{}' deleted successfully", name);
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

fn print_routes_table(routes: &[RouteConfigResponse]) {
    if routes.is_empty() {
        println!("No route configs found");
        return;
    }

    println!();
    println!("{:<30} {:<15} {:<25} {:<25}", "Name", "Team", "Path Prefix", "Cluster Targets");
    println!("{}", "-".repeat(100));

    for route in routes {
        println!(
            "{:<30} {:<15} {:<25} {:<25}",
            truncate(&route.name, 28),
            truncate(&route.team, 13),
            truncate(&route.path_prefix, 23),
            truncate(&route.cluster_targets, 23),
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
