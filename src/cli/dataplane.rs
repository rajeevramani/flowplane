//! Dataplane CLI commands
//!
//! Provides command-line interface for managing dataplane configurations

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::FlowplaneClient;
use super::output::{print_output, print_table_header, truncate};
use crate::api::handlers::PaginatedResponse;

#[derive(Subcommand)]
pub enum DataplaneCommands {
    /// List all dataplanes
    List {
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

    /// Get details of a specific dataplane by name
    Get {
        /// Dataplane name
        #[arg(value_name = "NAME")]
        name: String,

        /// Output format (json or yaml)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml"])]
        output: String,
    },

    /// Create a new dataplane
    Create {
        /// Path to JSON file with dataplane spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json or yaml)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml"])]
        output: String,
    },

    /// Update an existing dataplane
    Update {
        /// Dataplane name
        #[arg(value_name = "NAME")]
        name: String,

        /// Path to JSON file with update spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json or yaml)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml"])]
        output: String,
    },

    /// Delete a dataplane
    Delete {
        /// Dataplane name
        #[arg(value_name = "NAME")]
        name: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Get the Envoy bootstrap configuration for a dataplane
    Config {
        /// Dataplane name
        #[arg(value_name = "NAME")]
        name: String,

        /// Output format (json or yaml)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml"])]
        output: String,
    },
}

/// Dataplane response structure matching API response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataplaneResponse {
    pub id: String,
    pub team: String,
    pub name: String,
    #[serde(default)]
    pub gateway_host: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub certificate_serial: Option<String>,
    #[serde(default)]
    pub certificate_expires_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Handle dataplane commands
pub async fn handle_dataplane_command(
    command: DataplaneCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        DataplaneCommands::List { limit, offset, output } => {
            list_dataplanes(client, team, limit, offset, &output).await?
        }
        DataplaneCommands::Get { name, output } => {
            get_dataplane(client, team, &name, &output).await?
        }
        DataplaneCommands::Create { file, output } => {
            create_dataplane(client, team, file, &output).await?
        }
        DataplaneCommands::Update { name, file, output } => {
            update_dataplane(client, team, &name, file, &output).await?
        }
        DataplaneCommands::Delete { name, yes } => {
            delete_dataplane(client, team, &name, yes).await?
        }
        DataplaneCommands::Config { name, output } => {
            get_dataplane_config(client, team, &name, &output).await?
        }
    }

    Ok(())
}

async fn list_dataplanes(
    client: &FlowplaneClient,
    team: &str,
    limit: Option<i32>,
    offset: Option<i32>,
    output: &str,
) -> Result<()> {
    let mut path = format!("/api/v1/teams/{team}/dataplanes?");
    let mut params = Vec::new();

    if let Some(l) = limit {
        params.push(format!("limit={}", l));
    }
    if let Some(o) = offset {
        params.push(format!("offset={}", o));
    }

    path.push_str(&params.join("&"));

    let response: PaginatedResponse<DataplaneResponse> = client.get_json(&path).await?;

    if output == "table" {
        print_dataplanes_table(&response.items);
    } else {
        print_output(&response.items, output)?;
    }

    Ok(())
}

async fn get_dataplane(
    client: &FlowplaneClient,
    team: &str,
    name: &str,
    output: &str,
) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/dataplanes/{name}");
    let response: DataplaneResponse = client.get_json(&path).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn create_dataplane(
    client: &FlowplaneClient,
    team: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse JSON from file")?;

    let path = format!("/api/v1/teams/{team}/dataplanes");
    let response: DataplaneResponse = client.post_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn update_dataplane(
    client: &FlowplaneClient,
    team: &str,
    name: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse JSON from file")?;

    let path = format!("/api/v1/teams/{team}/dataplanes/{name}");
    let response: DataplaneResponse = client.patch_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn delete_dataplane(
    client: &FlowplaneClient,
    team: &str,
    name: &str,
    yes: bool,
) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete dataplane '{}'? (y/N)", name);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/teams/{team}/dataplanes/{name}");
    client.delete_no_content(&path).await?;

    println!("Dataplane '{}' deleted successfully", name);
    Ok(())
}

async fn get_dataplane_config(
    client: &FlowplaneClient,
    team: &str,
    name: &str,
    output: &str,
) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/dataplanes/{name}/envoy-config?format={output}");
    let response: serde_json::Value = client.get_json(&path).await?;

    print_output(&response, output)?;
    Ok(())
}

fn print_dataplanes_table(dataplanes: &[DataplaneResponse]) {
    if dataplanes.is_empty() {
        println!("No dataplanes found");
        return;
    }

    print_table_header(&[("Name", 30), ("Team", 20), ("Gateway Host", 25), ("Description", 30)]);

    for dp in dataplanes {
        println!(
            "{:<30} {:<20} {:<25} {:<30}",
            truncate(&dp.name, 28),
            truncate(&dp.team, 18),
            truncate(dp.gateway_host.as_deref().unwrap_or("-"), 23),
            truncate(dp.description.as_deref().unwrap_or("-"), 28),
        );
    }
    println!();
}
