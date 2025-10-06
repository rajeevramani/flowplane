//! Listener CLI commands
//!
//! Provides command-line interface for managing listener configurations

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::FlowplaneClient;

#[derive(Subcommand)]
pub enum ListenerCommands {
    /// Create a new listener
    Create {
        /// Path to JSON file with listener spec
        #[arg(short, long)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// List all listeners
    List {
        /// Filter by protocol
        #[arg(long)]
        protocol: Option<String>,

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

    /// Get a specific listener by name
    Get {
        /// Listener name
        name: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// Update a listener
    Update {
        /// Listener name
        name: String,

        /// Path to JSON file with update spec
        #[arg(short, long)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// Delete a listener
    Delete {
        /// Listener name
        name: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

/// Listener response structure
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenerResponse {
    pub name: String,
    pub address: String,
    pub port: Option<u16>,
    pub protocol: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Handle listener commands
pub async fn handle_listener_command(
    command: ListenerCommands,
    client: &FlowplaneClient,
) -> Result<()> {
    match command {
        ListenerCommands::Create { file, output } => create_listener(client, file, &output).await?,
        ListenerCommands::List { protocol, limit, offset, output } => {
            list_listeners(client, protocol, limit, offset, &output).await?
        }
        ListenerCommands::Get { name, output } => get_listener(client, &name, &output).await?,
        ListenerCommands::Update { name, file, output } => {
            update_listener(client, &name, file, &output).await?
        }
        ListenerCommands::Delete { name, yes } => delete_listener(client, &name, yes).await?,
    }

    Ok(())
}

async fn create_listener(client: &FlowplaneClient, file: PathBuf, output: &str) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value = serde_json::from_str(&contents)
        .context("Failed to parse JSON from file")?;

    let response: ListenerResponse = client.post_json("/api/v1/listeners", &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn list_listeners(
    client: &FlowplaneClient,
    protocol: Option<String>,
    limit: Option<i32>,
    offset: Option<i32>,
    output: &str,
) -> Result<()> {
    let mut path = String::from("/api/v1/listeners?");
    let mut params = Vec::new();

    if let Some(p) = protocol {
        params.push(format!("protocol={}", p));
    }
    if let Some(l) = limit {
        params.push(format!("limit={}", l));
    }
    if let Some(o) = offset {
        params.push(format!("offset={}", o));
    }

    path.push_str(&params.join("&"));

    let response: Vec<ListenerResponse> = client.get_json(&path).await?;

    if output == "table" {
        print_listeners_table(&response);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn get_listener(client: &FlowplaneClient, name: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/listeners/{}", name);
    let response: ListenerResponse = client.get_json(&path).await?;

    if output == "table" {
        print_listeners_table(&[response]);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn update_listener(
    client: &FlowplaneClient,
    name: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value = serde_json::from_str(&contents)
        .context("Failed to parse JSON from file")?;

    let path = format!("/api/v1/listeners/{}", name);
    let response: ListenerResponse = client.put_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn delete_listener(client: &FlowplaneClient, name: &str, yes: bool) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete listener '{}'? (y/N)", name);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/listeners/{}", name);
    let _: serde_json::Value = client.delete_json(&path).await?;

    println!("Listener '{}' deleted successfully", name);
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

fn print_listeners_table(listeners: &[ListenerResponse]) {
    if listeners.is_empty() {
        println!("No listeners found");
        return;
    }

    println!();
    println!(
        "{:<30} {:<20} {:<10} {:<12} {:<10}",
        "Name", "Address", "Port", "Protocol", "Version"
    );
    println!("{}", "-".repeat(90));

    for listener in listeners {
        println!(
            "{:<30} {:<20} {:<10} {:<12} {:<10}",
            truncate(&listener.name, 28),
            truncate(&listener.address, 18),
            listener.port.map_or("-".to_string(), |p| p.to_string()),
            listener.protocol,
            listener.version
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
