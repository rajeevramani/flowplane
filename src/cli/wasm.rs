//! WASM custom filter management CLI commands
//!
//! Provides command-line interface for managing custom WASM filters:
//! list, get, create, update, delete, and download binary.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;

use super::client::FlowplaneClient;
use super::output::{print_output, print_table_header, truncate};

#[derive(Subcommand)]
pub enum WasmCommands {
    /// List custom WASM filters
    #[command(
        long_about = "List all custom WASM filters for the current team.",
        after_help = "EXAMPLES:\n    flowplane wasm list\n    flowplane wasm list -o json"
    )]
    List {
        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "table"])]
        output: String,
    },

    /// Get details of a custom WASM filter
    #[command(
        long_about = "Retrieve details of a specific custom WASM filter by ID.",
        after_help = "EXAMPLES:\n    flowplane wasm get abc-123\n    flowplane wasm get abc-123 -o yaml"
    )]
    Get {
        /// Filter ID
        id: String,

        /// Output format
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml"])]
        output: String,
    },

    /// Upload a new custom WASM filter
    #[command(
        long_about = "Create a new custom WASM filter by uploading a definition file.\n\n\
            The file should be a JSON document describing the filter metadata\n\
            and optionally embedding the WASM binary.",
        after_help = "EXAMPLES:\n    flowplane wasm create -f filter.json\n    flowplane wasm create -f filter.json -o json"
    )]
    Create {
        /// Path to the filter definition file (JSON)
        #[arg(short, long)]
        file: PathBuf,

        /// Output format
        #[arg(short, long, default_value = "json", value_parser = ["json"])]
        output: String,
    },

    /// Update an existing custom WASM filter
    #[command(
        long_about = "Update a custom WASM filter by ID using a definition file.",
        after_help = "EXAMPLES:\n    flowplane wasm update abc-123 -f filter.json\n    flowplane wasm update abc-123 -f filter.json -o json"
    )]
    Update {
        /// Filter ID to update
        id: String,

        /// Path to the updated filter definition file (JSON)
        #[arg(short, long)]
        file: PathBuf,

        /// Output format
        #[arg(short, long, default_value = "json", value_parser = ["json"])]
        output: String,
    },

    /// Delete a custom WASM filter
    #[command(
        long_about = "Delete a custom WASM filter by ID.\n\nRequires confirmation unless --yes is provided.",
        after_help = "EXAMPLES:\n    flowplane wasm delete abc-123\n    flowplane wasm delete abc-123 --yes"
    )]
    Delete {
        /// Filter ID to delete
        id: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Download a WASM filter binary
    #[command(
        long_about = "Download the compiled WASM binary for a custom filter.",
        after_help = "EXAMPLES:\n    flowplane wasm download abc-123 -o filter.wasm"
    )]
    Download {
        /// Filter ID to download
        id: String,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
    },
}

pub async fn handle_wasm_command(
    command: WasmCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        WasmCommands::List { output } => {
            let path = format!("/api/v1/teams/{team}/custom-filters");
            let response: serde_json::Value = client.get_json(&path).await?;

            if output == "table" {
                print_wasm_table(&response);
            } else {
                print_output(&response, &output)?;
            }
        }
        WasmCommands::Get { id, output } => {
            let path = format!("/api/v1/teams/{team}/custom-filters/{id}");
            let response: serde_json::Value = client.get_json(&path).await?;
            print_output(&response, &output)?;
        }
        WasmCommands::Create { file, output } => {
            let content = tokio::fs::read_to_string(&file).await.with_context(|| {
                format!("Failed to read filter definition from {}", file.display())
            })?;
            let body: serde_json::Value =
                serde_json::from_str(&content).context("Invalid JSON in filter definition file")?;

            let path = format!("/api/v1/teams/{team}/custom-filters");
            let response: serde_json::Value = client.post_json(&path, &body).await?;
            print_output(&response, &output)?;
        }
        WasmCommands::Update { id, file, output } => {
            let content = tokio::fs::read_to_string(&file).await.with_context(|| {
                format!("Failed to read filter definition from {}", file.display())
            })?;
            let body: serde_json::Value =
                serde_json::from_str(&content).context("Invalid JSON in filter definition file")?;

            let path = format!("/api/v1/teams/{team}/custom-filters/{id}");
            let response: serde_json::Value = client.patch_json(&path, &body).await?;
            print_output(&response, &output)?;
        }
        WasmCommands::Delete { id, yes } => {
            if !yes {
                if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    anyhow::bail!(
                        "Cannot prompt for confirmation: stdin is not a terminal. Use --yes to skip confirmation."
                    );
                }
                println!("Are you sure you want to delete WASM filter '{id}'? (y/N)");
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted");
                    return Ok(());
                }
            }

            let path = format!("/api/v1/teams/{team}/custom-filters/{id}");
            client.delete_no_content(&path).await?;
            println!("WASM filter '{id}' deleted successfully");
        }
        WasmCommands::Download { id, output } => {
            let path = format!("/api/v1/teams/{team}/custom-filters/{id}/download");
            client.download_binary(&path, &output).await?;
            println!("Downloaded WASM binary to {}", output.display());
        }
    }

    Ok(())
}

fn print_wasm_table(data: &serde_json::Value) {
    let items = match data.as_array() {
        Some(arr) => arr.clone(),
        None => vec![data.clone()],
    };

    if items.is_empty() {
        println!("No custom WASM filters found");
        return;
    }

    print_table_header(&[
        ("ID", 38),
        ("Name", 25),
        ("Version", 10),
        ("Status", 12),
        ("Created", 25),
    ]);

    for item in &items {
        let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
        let version = item.get("version").map(|v| v.to_string()).unwrap_or_else(|| "-".to_string());
        let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("-");
        let created = item
            .get("createdAt")
            .or_else(|| item.get("created_at"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");

        println!(
            "{:<38} {:<25} {:<10} {:<12} {}",
            truncate(id, 36),
            truncate(name, 23),
            truncate(&version, 8),
            truncate(status, 10),
            truncate(created, 23),
        );
    }
    println!();
}
