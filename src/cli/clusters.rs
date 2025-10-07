//! Cluster CLI commands
//!
//! Provides command-line interface for managing cluster configurations

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::FlowplaneClient;

#[derive(Subcommand)]
pub enum ClusterCommands {
    /// Create a new Envoy cluster configuration
    #[command(
        long_about = "Create a new cluster configuration that defines upstream service endpoints.\n\nClusters specify how to connect to backend services, including endpoints, load balancing, and health checking.",
        after_help = "EXAMPLES:\n    # Create a cluster from JSON file\n    flowplane-cli cluster create --file cluster-spec.json\n\n    # Create with YAML output\n    flowplane-cli cluster create --file cluster.json --output yaml\n\n    # With authentication\n    flowplane-cli cluster create --file cluster.json --token your-token"
    )]
    Create {
        /// Path to JSON file with cluster spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// List all cluster configurations
    #[command(
        long_about = "List all cluster configurations in the system with optional filtering and pagination.",
        after_help = "EXAMPLES:\n    # List all clusters\n    flowplane-cli cluster list\n\n    # List with table output\n    flowplane-cli cluster list --output table\n\n    # Filter by service name\n    flowplane-cli cluster list --service backend-api\n\n    # Paginate results\n    flowplane-cli cluster list --limit 10 --offset 20"
    )]
    List {
        /// Filter by service name
        #[arg(long, value_name = "SERVICE")]
        service: Option<String>,

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

    /// Get details of a specific cluster by name
    #[command(
        long_about = "Retrieve detailed information about a specific cluster configuration.",
        after_help = "EXAMPLES:\n    # Get cluster details\n    flowplane-cli cluster get my-backend-cluster\n\n    # Get with YAML output\n    flowplane-cli cluster get my-cluster --output yaml\n\n    # Get with table output\n    flowplane-cli cluster get my-cluster --output table"
    )]
    Get {
        /// Cluster name
        #[arg(value_name = "NAME")]
        name: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Update an existing cluster configuration
    #[command(
        long_about = "Update an existing cluster configuration with new settings.\n\nProvide a JSON file with the updated configuration fields.",
        after_help = "EXAMPLES:\n    # Update a cluster\n    flowplane-cli cluster update my-cluster --file updated-cluster.json\n\n    # Update with YAML output\n    flowplane-cli cluster update my-cluster --file update.json --output yaml"
    )]
    Update {
        /// Cluster name
        #[arg(value_name = "NAME")]
        name: String,

        /// Path to JSON file with update spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Delete a cluster configuration
    #[command(
        long_about = "Delete a specific cluster configuration from the system.\n\nWARNING: This will remove the cluster and may affect routing if it's in use.",
        after_help = "EXAMPLES:\n    # Delete a cluster (with confirmation prompt)\n    flowplane-cli cluster delete my-cluster\n\n    # Delete without confirmation\n    flowplane-cli cluster delete my-cluster --yes"
    )]
    Delete {
        /// Cluster name
        #[arg(value_name = "NAME")]
        name: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

/// Cluster response structure
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterResponse {
    pub name: String,
    pub service_name: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Handle cluster commands
pub async fn handle_cluster_command(command: ClusterCommands, client: &FlowplaneClient) -> Result<()> {
    match command {
        ClusterCommands::Create { file, output } => create_cluster(client, file, &output).await?,
        ClusterCommands::List { service, limit, offset, output } => {
            list_clusters(client, service, limit, offset, &output).await?
        }
        ClusterCommands::Get { name, output } => get_cluster(client, &name, &output).await?,
        ClusterCommands::Update { name, file, output } => {
            update_cluster(client, &name, file, &output).await?
        }
        ClusterCommands::Delete { name, yes } => delete_cluster(client, &name, yes).await?,
    }

    Ok(())
}

async fn create_cluster(client: &FlowplaneClient, file: PathBuf, output: &str) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value = serde_json::from_str(&contents)
        .context("Failed to parse JSON from file")?;

    let response: ClusterResponse = client.post_json("/api/v1/clusters", &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn list_clusters(
    client: &FlowplaneClient,
    service: Option<String>,
    limit: Option<i32>,
    offset: Option<i32>,
    output: &str,
) -> Result<()> {
    let mut path = String::from("/api/v1/clusters?");
    let mut params = Vec::new();

    if let Some(s) = service {
        params.push(format!("service={}", s));
    }
    if let Some(l) = limit {
        params.push(format!("limit={}", l));
    }
    if let Some(o) = offset {
        params.push(format!("offset={}", o));
    }

    path.push_str(&params.join("&"));

    let response: Vec<ClusterResponse> = client.get_json(&path).await?;

    if output == "table" {
        print_clusters_table(&response);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn get_cluster(client: &FlowplaneClient, name: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/clusters/{}", name);
    let response: ClusterResponse = client.get_json(&path).await?;

    if output == "table" {
        print_clusters_table(&[response]);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn update_cluster(
    client: &FlowplaneClient,
    name: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value = serde_json::from_str(&contents)
        .context("Failed to parse JSON from file")?;

    let path = format!("/api/v1/clusters/{}", name);
    let response: ClusterResponse = client.put_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn delete_cluster(client: &FlowplaneClient, name: &str, yes: bool) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete cluster '{}'? (y/N)", name);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/clusters/{}", name);
    let _: serde_json::Value = client.delete_json(&path).await?;

    println!("Cluster '{}' deleted successfully", name);
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

fn print_clusters_table(clusters: &[ClusterResponse]) {
    if clusters.is_empty() {
        println!("No clusters found");
        return;
    }

    println!();
    println!(
        "{:<30} {:<30} {:<10}",
        "Name", "Service", "Version"
    );
    println!("{}", "-".repeat(75));

    for cluster in clusters {
        println!(
            "{:<30} {:<30} {:<10}",
            truncate(&cluster.name, 28),
            truncate(&cluster.service_name, 28),
            cluster.version
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
