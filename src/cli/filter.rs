//! Filter CLI commands
//!
//! Provides command-line interface for managing HTTP filters: CRUD operations
//! and listener-level attach/detach.

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::FlowplaneClient;
use crate::api::handlers::PaginatedResponse;

#[derive(Subcommand)]
pub enum FilterCommands {
    /// Create a new HTTP filter from a JSON spec file
    Create {
        /// Path to JSON file with filter spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// List all filters
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

    /// Get details of a specific filter by name
    Get {
        /// Filter name
        #[arg(value_name = "NAME")]
        name: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Delete a filter by name
    Delete {
        /// Filter name
        #[arg(value_name = "NAME")]
        name: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Attach a filter to a listener
    Attach {
        /// Filter name to attach
        #[arg(value_name = "FILTER_NAME")]
        filter_name: String,

        /// Listener name to attach the filter to
        #[arg(long)]
        listener: String,

        /// Execution order (lower numbers execute first)
        #[arg(long)]
        order: Option<i64>,
    },

    /// Detach a filter from a listener
    Detach {
        /// Filter name to detach
        #[arg(value_name = "FILTER_NAME")]
        filter_name: String,

        /// Listener name to detach the filter from
        #[arg(long)]
        listener: String,
    },
}

/// Filter response structure matching the API
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterResponse {
    pub id: String,
    pub name: String,
    pub filter_type: String,
    #[serde(default)]
    pub description: Option<String>,
    pub version: i64,
    pub source: String,
    pub team: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub attachment_count: Option<i64>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub allowed_attachment_points: Option<Vec<String>>,
}

/// Request body for attaching a filter to a listener
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AttachFilterRequest {
    filter_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    order: Option<i64>,
}

/// Handle filter commands
pub async fn handle_filter_command(
    command: FilterCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        FilterCommands::Create { file, output } => {
            create_filter(client, team, file, &output).await?
        }
        FilterCommands::List { limit, offset, output } => {
            list_filters(client, team, limit, offset, &output).await?
        }
        FilterCommands::Get { name, output } => get_filter(client, team, &name, &output).await?,
        FilterCommands::Delete { name, yes } => delete_filter(client, team, &name, yes).await?,
        FilterCommands::Attach { filter_name, listener, order } => {
            attach_filter(client, team, &filter_name, &listener, order).await?
        }
        FilterCommands::Detach { filter_name, listener } => {
            detach_filter(client, team, &filter_name, &listener).await?
        }
    }

    Ok(())
}

/// Find a filter by name, returning its ID. Searches via the list endpoint.
async fn resolve_filter_id(client: &FlowplaneClient, team: &str, name: &str) -> Result<String> {
    let path = format!("/api/v1/teams/{team}/filters?limit=1000");
    let response: PaginatedResponse<FilterResponse> = client.get_json(&path).await?;

    response
        .items
        .into_iter()
        .find(|f| f.name == name)
        .map(|f| f.id)
        .ok_or_else(|| anyhow::anyhow!("Filter '{}' not found", name))
}

async fn create_filter(
    client: &FlowplaneClient,
    team: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse JSON from file")?;

    let path = format!("/api/v1/teams/{team}/filters");
    let response: FilterResponse = client.post_json(&path, &body).await?;

    if output == "table" {
        print_filters_table(&[response]);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn list_filters(
    client: &FlowplaneClient,
    team: &str,
    limit: Option<i32>,
    offset: Option<i32>,
    output: &str,
) -> Result<()> {
    let mut path = format!("/api/v1/teams/{team}/filters?");
    let mut params = Vec::new();

    if let Some(l) = limit {
        params.push(format!("limit={l}"));
    }
    if let Some(o) = offset {
        params.push(format!("offset={o}"));
    }

    path.push_str(&params.join("&"));

    let response: PaginatedResponse<FilterResponse> = client.get_json(&path).await?;

    if output == "table" {
        print_filters_table(&response.items);
    } else {
        print_output(&response.items, output)?;
    }

    Ok(())
}

async fn get_filter(client: &FlowplaneClient, team: &str, name: &str, output: &str) -> Result<()> {
    let id = resolve_filter_id(client, team, name).await?;
    let path = format!("/api/v1/teams/{team}/filters/{id}");
    let response: FilterResponse = client.get_json(&path).await?;

    if output == "table" {
        print_filters_table(&[response]);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn delete_filter(client: &FlowplaneClient, team: &str, name: &str, yes: bool) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete filter '{name}'? (y/N)");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let id = resolve_filter_id(client, team, name).await?;
    let path = format!("/api/v1/teams/{team}/filters/{id}");
    client.delete_no_content(&path).await?;

    println!("Filter '{name}' deleted successfully");
    Ok(())
}

async fn attach_filter(
    client: &FlowplaneClient,
    team: &str,
    filter_name: &str,
    listener_name: &str,
    order: Option<i64>,
) -> Result<()> {
    let filter_id = resolve_filter_id(client, team, filter_name).await?;

    let body = AttachFilterRequest { filter_id, order };
    let path = format!("/api/v1/teams/{team}/listeners/{listener_name}/filters");
    let response =
        client.post(&path).json(&body).send().await.context("Failed to attach filter")?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text =
            response.text().await.unwrap_or_else(|_| "<unable to read error>".to_string());
        anyhow::bail!("Attach failed with status {status}: {error_text}");
    }

    println!("Filter '{filter_name}' attached to listener '{listener_name}'");
    Ok(())
}

async fn detach_filter(
    client: &FlowplaneClient,
    team: &str,
    filter_name: &str,
    listener_name: &str,
) -> Result<()> {
    let filter_id = resolve_filter_id(client, team, filter_name).await?;

    let path = format!("/api/v1/teams/{team}/listeners/{listener_name}/filters/{filter_id}");
    client.delete_no_content(&path).await?;

    println!("Filter '{filter_name}' detached from listener '{listener_name}'");
    Ok(())
}

fn print_output<T: Serialize>(data: &T, format: &str) -> Result<()> {
    match format {
        "json" => {
            let json = serde_json::to_string_pretty(data).context("Failed to serialize to JSON")?;
            println!("{json}");
        }
        "yaml" => {
            let yaml = serde_yaml::to_string(data).context("Failed to serialize to YAML")?;
            println!("{yaml}");
        }
        _ => {
            anyhow::bail!("Unsupported output format: {}. Use 'json' or 'yaml'.", format);
        }
    }
    Ok(())
}

fn print_filters_table(filters: &[FilterResponse]) {
    if filters.is_empty() {
        println!("No filters found");
        return;
    }

    println!();
    println!("{:<30} {:<20} {:<15} {:<10} {:<10}", "Name", "Type", "Team", "Version", "Attached");
    println!("{}", "-".repeat(90));

    for filter in filters {
        let attached =
            filter.attachment_count.map(|c| c.to_string()).unwrap_or_else(|| "-".to_string());
        println!(
            "{:<30} {:<20} {:<15} {:<10} {:<10}",
            truncate(&filter.name, 28),
            truncate(&filter.filter_type, 18),
            truncate(&filter.team, 13),
            filter.version,
            attached,
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
