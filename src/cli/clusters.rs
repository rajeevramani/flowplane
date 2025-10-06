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
    /// Create a new cluster
    Create {
        /// Path to JSON file with cluster spec
        #[arg(short, long)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// List all clusters
    List {
        /// Filter by service name
        #[arg(long)]
        service: Option<String>,

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

    /// Get a specific cluster by name
    Get {
        /// Cluster name
        name: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// Update a cluster
    Update {
        /// Cluster name
        name: String,

        /// Path to JSON file with update spec
        #[arg(short, long)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// Delete a cluster
    Delete {
        /// Cluster name
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
