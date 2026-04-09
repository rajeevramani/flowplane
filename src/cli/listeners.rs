//! Listener CLI commands
//!
//! Provides command-line interface for managing listener configurations

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::FlowplaneClient;
use super::config_file;
use crate::api::handlers::PaginatedResponse;

#[derive(Subcommand)]
pub enum ListenerCommands {
    /// Create a new Envoy listener configuration
    #[command(
        long_about = "Create a new listener by providing a JSON file with the listener specification.\n\nListeners define how Envoy accepts incoming connections, including address, port, protocol, and filter chains.",
        after_help = "EXAMPLES:\n    # Create a listener from a JSON file\n    flowplane-cli listener create --file listener-spec.json\n\n    # Create and output as YAML\n    flowplane-cli listener create --file listener-spec.json --output yaml\n\n    # With authentication\n    flowplane-cli listener create --file listener-spec.json --token your-token"
    )]
    Create {
        /// Path to YAML or JSON file with resource spec
        #[arg(short, long, value_name = "FILE", help = config_file::FILE_ARG_HELP)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// List all listener configurations
    #[command(
        long_about = "List all listener configurations in the system with optional filtering and pagination.\n\nListeners define network listeners that accept incoming connections.",
        after_help = "EXAMPLES:\n    # List all listeners\n    flowplane-cli listener list\n\n    # List with table output\n    flowplane-cli listener list --output table\n\n    # Filter by protocol\n    flowplane-cli listener list --protocol http\n\n    # Paginate results\n    flowplane-cli listener list --limit 10 --offset 20"
    )]
    List {
        /// Filter by protocol
        #[arg(long, value_name = "PROTOCOL")]
        protocol: Option<String>,

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

    /// Get details of a specific listener by name
    #[command(
        long_about = "Retrieve detailed information about a specific listener configuration by its name.\n\nShows address, port, protocol, filter chains, and metadata.",
        after_help = "EXAMPLES:\n    # Get listener details in JSON format\n    flowplane-cli listener get http-listener\n\n    # Get listener in YAML format\n    flowplane-cli listener get http-listener --output yaml\n\n    # With authentication\n    flowplane-cli listener get http-listener --token your-token --base-url https://api.example.com"
    )]
    Get {
        /// Listener name
        #[arg(value_name = "NAME")]
        name: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Update an existing listener configuration
    #[command(
        long_about = "Update an existing listener configuration by providing a JSON file with the updated specification.\n\nYou can modify address, port, protocol, filter chains, and other listener properties.",
        after_help = "EXAMPLES:\n    # Update a listener from JSON file\n    flowplane-cli listener update http-listener --file updated-listener.json\n\n    # Update and output as YAML\n    flowplane-cli listener update http-listener --file updated-listener.json --output yaml\n\n    # With authentication\n    flowplane-cli listener update http-listener --file updated-listener.json --token your-token"
    )]
    Update {
        /// Listener name
        #[arg(value_name = "NAME")]
        name: String,

        /// Path to YAML or JSON file with resource spec
        #[arg(short, long, value_name = "FILE", help = config_file::FILE_ARG_HELP)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Delete a listener configuration
    #[command(
        long_about = "Delete a listener configuration by name.\n\nThis removes the listener and stops Envoy from accepting connections on the associated address and port.",
        after_help = "EXAMPLES:\n    # Delete a listener (with confirmation)\n    flowplane-cli listener delete http-listener\n\n    # Delete without confirmation prompt\n    flowplane-cli listener delete http-listener --yes\n\n    # With authentication\n    flowplane-cli listener delete http-listener --token your-token"
    )]
    Delete {
        /// Listener name
        #[arg(value_name = "NAME")]
        name: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Generate a template listener manifest
    Scaffold {
        /// Output format (yaml or json)
        #[arg(short, long, default_value = "yaml", value_parser = ["json", "yaml"])]
        output: String,
    },
}

/// Listener response structure matching API response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenerResponse {
    pub name: String,
    pub team: String,
    pub address: String,
    pub port: u16,
    pub protocol: String,
    #[serde(default)]
    pub version: Option<i64>,
    #[serde(default)]
    pub import_id: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

/// Handle listener commands
pub async fn handle_listener_command(
    command: ListenerCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        ListenerCommands::Create { file, output } => {
            create_listener(client, team, file, &output).await?
        }
        ListenerCommands::List { protocol, limit, offset, output } => {
            list_listeners(client, team, protocol, limit, offset, &output).await?
        }
        ListenerCommands::Get { name, output } => {
            get_listener(client, team, &name, &output).await?
        }
        ListenerCommands::Update { name, file, output } => {
            update_listener(client, team, &name, file, &output).await?
        }
        ListenerCommands::Delete { name, yes } => delete_listener(client, team, &name, yes).await?,
        ListenerCommands::Scaffold { output } => scaffold_listener(&output)?,
    }

    Ok(())
}

async fn create_listener(
    client: &FlowplaneClient,
    team: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let mut body = config_file::load_config_file(&file)?;
    config_file::strip_kind_field(&mut body);

    let path = format!("/api/v1/teams/{team}/listeners");
    let response: ListenerResponse = client.post_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn list_listeners(
    client: &FlowplaneClient,
    team: &str,
    protocol: Option<String>,
    limit: Option<i32>,
    offset: Option<i32>,
    output: &str,
) -> Result<()> {
    let mut path = format!("/api/v1/teams/{team}/listeners?");
    let mut params = Vec::new();

    if let Some(ref p) = protocol {
        params.push(format!("protocol={}", p));
    }
    if let Some(l) = limit {
        params.push(format!("limit={}", l));
    }
    if let Some(o) = offset {
        params.push(format!("offset={}", o));
    }

    path.push_str(&params.join("&"));

    let response: PaginatedResponse<ListenerResponse> = client.get_json(&path).await?;

    // Client-side filtering by protocol (API doesn't support this filter yet)
    let items: Vec<ListenerResponse> = if let Some(ref proto) = protocol {
        response.items.into_iter().filter(|l| l.protocol.eq_ignore_ascii_case(proto)).collect()
    } else {
        response.items
    };

    if output == "table" {
        print_listeners_table(&items);
    } else {
        print_output(&items, output)?;
    }

    Ok(())
}

async fn get_listener(
    client: &FlowplaneClient,
    team: &str,
    name: &str,
    output: &str,
) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/listeners/{name}");
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
    team: &str,
    name: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let mut body = config_file::load_config_file(&file)?;
    config_file::strip_kind_field(&mut body);

    let path = format!("/api/v1/teams/{team}/listeners/{name}");
    let response: ListenerResponse = client.put_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn delete_listener(
    client: &FlowplaneClient,
    team: &str,
    name: &str,
    yes: bool,
) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete listener '{}'? (y/N)", name);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/teams/{team}/listeners/{name}");
    client.delete_no_content(&path).await?;

    println!("Listener '{}' deleted successfully", name);
    Ok(())
}

fn scaffold_listener(output: &str) -> Result<()> {
    if output == "json" {
        let scaffold = serde_json::json!({
            "kind": "Listener",
            "name": "<your-listener-name>",
            "address": "0.0.0.0",
            "port": 10001,
            "dataplaneId": "<your-dataplane-id>",
            "filterChains": [
                {
                    "name": "default",
                    "filters": [
                        {
                            "name": "envoy.filters.network.http_connection_manager",
                            "type": "httpConnectionManager",
                            "routeConfigName": "<your-route-config-name>",
                            "httpFilters": [
                                { "filter": { "type": "router" } }
                            ]
                        }
                    ]
                }
            ]
        });
        let json =
            serde_json::to_string_pretty(&scaffold).context("Failed to serialize scaffold")?;
        println!("{json}");
    } else {
        println!("# Listener scaffold");
        println!("kind: Listener");
        println!("name: \"<your-listener-name>\"");
        println!("# Bind address (0.0.0.0 for all interfaces)");
        println!("address: \"0.0.0.0\"");
        println!("# Port to listen on (10000-10020 for Envoy)");
        println!("port: 10001");
        println!("# Dataplane ID to attach this listener to");
        println!("dataplaneId: \"<your-dataplane-id>\"");
        println!("# Filter chains (required — at least one with an HTTP connection manager)");
        println!("filterChains:");
        println!("  - name: default");
        println!("    filters:");
        println!("      - name: envoy.filters.network.http_connection_manager");
        println!("        type: httpConnectionManager");
        println!("        # Route config to bind to this listener");
        println!("        routeConfigName: \"<your-route-config-name>\"");
        println!("        httpFilters:");
        println!("          - filter:");
        println!("              type: router");
    }
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
    println!("{:<35} {:<18} {:<18} {:<8} {:<10}", "Name", "Team", "Address", "Port", "Protocol");
    println!("{}", "-".repeat(95));

    for listener in listeners {
        println!(
            "{:<35} {:<18} {:<18} {:<8} {:<10}",
            truncate(&listener.name, 33),
            truncate(&listener.team, 16),
            truncate(&listener.address, 16),
            listener.port,
            listener.protocol,
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
