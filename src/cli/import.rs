//! Import CLI commands
//!
//! Provides CLI for importing API definitions (OpenAPI specs) into the gateway.
//! Wraps POST /api/v1/teams/{team}/openapi/import.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use super::client::FlowplaneClient;

/// Paginated response wrapper for dataplane listing
#[derive(Debug, Deserialize)]
struct PaginatedDataplanes {
    items: Vec<DataplaneItem>,
}

/// Minimal dataplane info needed for ID resolution
#[derive(Debug, Deserialize)]
struct DataplaneItem {
    id: String,
}

#[derive(Subcommand)]
pub enum ImportCommands {
    /// Import routes from an OpenAPI specification file
    Openapi {
        /// Path to the OpenAPI spec file (YAML or JSON)
        file: PathBuf,
        /// Name for the imported service (derived from spec title if omitted)
        #[arg(long)]
        name: Option<String>,
        /// Port for the listener (default: 10000)
        #[arg(long)]
        port: Option<u16>,
    },

    /// List all imports for the team
    List {
        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Get details of a specific import
    Get {
        /// Import ID
        #[arg(value_name = "ID")]
        id: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml"])]
        output: String,
    },

    /// Delete an import and its associated resources
    Delete {
        /// Import ID
        #[arg(value_name = "ID")]
        id: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

/// Response from the import API (matches backend ImportResponse)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportResponse {
    import_id: String,
    spec_name: String,
    spec_version: Option<String>,
    routes_created: usize,
    clusters_created: usize,
    clusters_reused: usize,
    listener_name: Option<String>,
}

/// Response from list imports endpoint
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListImportsResponse {
    imports: Vec<ImportSummary>,
}

/// Summary of a single import
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportSummary {
    id: String,
    spec_name: String,
    spec_version: Option<String>,
    team: String,
    listener_name: Option<String>,
    imported_at: String,
    updated_at: String,
}

/// Detailed import response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportDetailsResponse {
    id: String,
    spec_name: String,
    spec_version: Option<String>,
    spec_checksum: Option<String>,
    team: String,
    listener_name: Option<String>,
    imported_at: String,
    updated_at: String,
    route_count: usize,
    cluster_count: usize,
    listener_count: usize,
}

pub async fn handle_import_command(
    client: &FlowplaneClient,
    team: &str,
    command: ImportCommands,
) -> Result<()> {
    match command {
        ImportCommands::Openapi { file, name, port } => {
            handle_openapi_import(client, team, &file, name.as_deref(), port).await
        }
        ImportCommands::List { output } => list_imports(client, team, &output).await,
        ImportCommands::Get { id, output } => get_import(client, &id, &output).await,
        ImportCommands::Delete { id, yes } => delete_import(client, &id, yes).await,
    }
}

async fn handle_openapi_import(
    client: &FlowplaneClient,
    team: &str,
    file: &PathBuf,
    name: Option<&str>,
    port: Option<u16>,
) -> Result<()> {
    let content =
        std::fs::read(file).with_context(|| format!("Failed to read file '{}'", file.display()))?;

    if content.is_empty() {
        anyhow::bail!("File '{}' is empty", file.display());
    }

    // Determine content type from extension
    let content_type = match file.extension().and_then(|e| e.to_str()) {
        Some("json") => "application/json",
        Some("yaml" | "yml") => "application/yaml",
        _ => "application/yaml", // default to YAML
    };

    // Resolve the default dataplane for the team (required when listener_mode=new)
    let dataplanes: PaginatedDataplanes = client
        .get_json(&format!("/api/v1/teams/{team}/dataplanes?limit=1"))
        .await
        .context("Failed to list dataplanes. Create one with `flowplane dataplane create`")?;

    let dataplane_id = dataplanes.items.first().map(|dp| dp.id.clone()).ok_or_else(|| {
        anyhow::anyhow!(
            "No dataplane found for team '{team}'. Create one with `flowplane dataplane create`"
        )
    })?;

    // Build query string for listener_mode=new
    let mut query_parts =
        vec![("listener_mode", "new".to_string()), ("dataplane_id", dataplane_id)];

    if let Some(n) = name {
        query_parts.push(("new_listener_name", format!("{n}-listener")));
    }

    if let Some(p) = port {
        query_parts.push(("new_listener_port", p.to_string()));
    }

    let query_string: String =
        query_parts.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");

    let path = format!("/api/v1/teams/{team}/openapi/import?{query_string}");

    let response = client
        .post(&path)
        .header("Content-Type", content_type)
        .body(content)
        .send()
        .await
        .context("Failed to send import request")?;

    let status = response.status();

    if !status.is_success() {
        let error_text =
            response.text().await.unwrap_or_else(|_| "<unable to read error>".to_string());
        anyhow::bail!("Import failed with status {}: {}", status, error_text);
    }

    let body = response.text().await.context("Failed to read response body")?;
    let result: ImportResponse =
        serde_json::from_str(&body).context("Failed to parse import response")?;

    println!("Imported '{}'", result.spec_name);
    if let Some(version) = &result.spec_version {
        println!("  Version:          {version}");
    }
    println!("  Import ID:        {}", result.import_id);
    println!("  Routes created:   {}", result.routes_created);
    println!("  Clusters created: {}", result.clusters_created);
    println!("  Clusters reused:  {}", result.clusters_reused);
    if let Some(listener) = &result.listener_name {
        println!("  Listener:         {listener}");
    }

    Ok(())
}

async fn list_imports(client: &FlowplaneClient, team: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/openapi/imports");
    let response: ListImportsResponse = client.get_json(&path).await?;

    if output == "table" {
        print_imports_table(&response.imports);
    } else {
        print_output(&response.imports, output)?;
    }

    Ok(())
}

async fn get_import(client: &FlowplaneClient, id: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/openapi/imports/{id}");
    let response: ImportDetailsResponse = client.get_json(&path).await?;

    print_output(&response, output)?;

    Ok(())
}

async fn delete_import(client: &FlowplaneClient, id: &str, yes: bool) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete import '{id}'? This will also delete associated routes, clusters, and listeners. (y/N)");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/openapi/imports/{id}");
    client.delete_no_content(&path).await?;

    println!("Import '{id}' deleted successfully");
    Ok(())
}

fn print_imports_table(imports: &[ImportSummary]) {
    if imports.is_empty() {
        println!("No imports found");
        return;
    }

    println!();
    println!("{:<38} {:<25} {:<15} {:<25}", "ID", "Name", "Team", "Imported At");
    println!("{}", "-".repeat(103));

    for imp in imports {
        println!(
            "{:<38} {:<25} {:<15} {:<25}",
            truncate(&imp.id, 36),
            truncate(&imp.spec_name, 23),
            truncate(&imp.team, 13),
            truncate(&imp.imported_at, 23),
        );
    }
    println!();
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

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_import_commands_parse_openapi() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            command: ImportCommands,
        }

        let cli = TestCli::try_parse_from(["test", "openapi", "petstore.yaml"]);
        assert!(cli.is_ok());
    }

    #[test]
    fn test_import_commands_parse_openapi_with_options() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            command: ImportCommands,
        }

        let cli = TestCli::try_parse_from([
            "test",
            "openapi",
            "petstore.yaml",
            "--name",
            "petstore",
            "--port",
            "10002",
        ]);
        assert!(cli.is_ok());
    }

    #[test]
    fn test_import_response_deserialization() {
        let json = r#"{
            "importId": "abc-123",
            "specName": "Petstore API",
            "specVersion": "1.0.0",
            "routesCreated": 5,
            "clustersCreated": 2,
            "clustersReused": 1,
            "listenerName": "petstore-listener"
        }"#;

        let response: ImportResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.import_id, "abc-123");
        assert_eq!(response.spec_name, "Petstore API");
        assert_eq!(response.spec_version, Some("1.0.0".to_string()));
        assert_eq!(response.routes_created, 5);
        assert_eq!(response.clusters_created, 2);
        assert_eq!(response.clusters_reused, 1);
        assert_eq!(response.listener_name, Some("petstore-listener".to_string()));
    }

    #[test]
    fn test_import_response_deserialization_minimal() {
        let json = r#"{
            "importId": "abc-123",
            "specName": "My API",
            "specVersion": null,
            "routesCreated": 0,
            "clustersCreated": 0,
            "clustersReused": 0,
            "listenerName": null
        }"#;

        let response: ImportResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.spec_name, "My API");
        assert!(response.spec_version.is_none());
        assert!(response.listener_name.is_none());
    }
}
