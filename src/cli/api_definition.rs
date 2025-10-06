//! API Definition CLI commands
//!
//! Provides command-line interface for managing API definitions, including
//! creating definitions, importing from OpenAPI specs, and retrieving bootstrap configs.

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::{ClientConfig, FlowplaneClient};
use super::config::{resolve_base_url, resolve_timeout, resolve_token};

#[derive(Subcommand)]
pub enum ApiCommands {
    /// Create a new API definition
    Create {
        /// Path to JSON file with API definition spec
        #[arg(short, long)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// List all API definitions
    List {
        /// Filter by team name
        #[arg(long)]
        team: Option<String>,

        /// Filter by domain
        #[arg(long)]
        domain: Option<String>,

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

    /// Get a specific API definition by ID
    Get {
        /// API definition ID
        id: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// Get bootstrap configuration for an API definition
    Bootstrap {
        /// API definition ID
        id: String,

        /// Output format (yaml or json)
        #[arg(short, long, default_value = "yaml")]
        format: String,

        /// Scoping mode (all, team, or allowlist)
        #[arg(long, default_value = "all")]
        scope: String,

        /// Listener allowlist (comma-separated) when scope=allowlist
        #[arg(long)]
        allowlist: Option<String>,

        /// Include default listeners in team scope
        #[arg(long)]
        include_default: bool,
    },

    /// Import API definition from OpenAPI spec
    ImportOpenapi {
        /// Path to OpenAPI spec file (JSON or YAML)
        #[arg(short, long)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json")]
        output: String,
    },
}

/// API definition summary response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiDefinitionSummary {
    pub id: String,
    pub team: String,
    pub domain: String,
    pub listener_isolation: bool,
    pub bootstrap_uri: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Create API definition response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateApiDefinitionResponse {
    pub id: String,
    pub bootstrap_uri: String,
    pub routes: Vec<String>,
}

/// Handle API definition commands
pub async fn handle_api_command(
    command: ApiCommands,
    token: Option<String>,
    token_file: Option<PathBuf>,
    base_url: Option<String>,
    timeout: Option<u64>,
    verbose: bool,
) -> Result<()> {
    // Resolve configuration from multiple sources
    let token = resolve_token(token, token_file)?;
    let base_url = resolve_base_url(base_url);
    let timeout = resolve_timeout(timeout);

    let config = ClientConfig {
        base_url,
        token,
        timeout,
        verbose,
    };

    let client = FlowplaneClient::new(config)?;

    match command {
        ApiCommands::Create { file, output } => create_api_definition(&client, file, &output).await?,
        ApiCommands::List { team, domain, limit, offset, output } => {
            list_api_definitions(&client, team, domain, limit, offset, &output).await?
        }
        ApiCommands::Get { id, output } => get_api_definition(&client, &id, &output).await?,
        ApiCommands::Bootstrap { id, format, scope, allowlist, include_default } => {
            get_bootstrap_config(&client, &id, &format, &scope, allowlist, include_default).await?
        }
        ApiCommands::ImportOpenapi { file, output } => import_openapi(&client, file, &output).await?,
    }

    Ok(())
}

async fn create_api_definition(
    client: &FlowplaneClient,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value = serde_json::from_str(&contents)
        .context("Failed to parse JSON from file")?;

    let response: CreateApiDefinitionResponse =
        client.post_json("/api/v1/api-definitions", &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn list_api_definitions(
    client: &FlowplaneClient,
    team: Option<String>,
    domain: Option<String>,
    limit: Option<i32>,
    offset: Option<i32>,
    output: &str,
) -> Result<()> {
    let mut path = String::from("/api/v1/api-definitions?");
    let mut params = Vec::new();

    if let Some(t) = team {
        params.push(format!("team={}", t));
    }
    if let Some(d) = domain {
        params.push(format!("domain={}", d));
    }
    if let Some(l) = limit {
        params.push(format!("limit={}", l));
    }
    if let Some(o) = offset {
        params.push(format!("offset={}", o));
    }

    path.push_str(&params.join("&"));

    let response: Vec<ApiDefinitionSummary> = client.get_json(&path).await?;

    if output == "table" {
        print_definitions_table(&response);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn get_api_definition(client: &FlowplaneClient, id: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/api-definitions/{}", id);
    let response: ApiDefinitionSummary = client.get_json(&path).await?;

    if output == "table" {
        print_definitions_table(&[response]);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn get_bootstrap_config(
    client: &FlowplaneClient,
    id: &str,
    format: &str,
    scope: &str,
    allowlist: Option<String>,
    include_default: bool,
) -> Result<()> {
    let mut path = format!("/api/v1/api-definitions/{}/bootstrap?format={}&scope={}", id, format, scope);

    if include_default {
        path.push_str("&includeDefault=true");
    }

    if let Some(list) = allowlist {
        path.push_str(&format!("&allowlist={}", list));
    }

    let response = client.get(&path).send().await.context("Failed to get bootstrap config")?;

    let status = response.status();
    if !status.is_success() {
        let error = response.text().await.unwrap_or_else(|_| "<unable to read error>".to_string());
        anyhow::bail!("HTTP request failed with status {}: {}", status, error);
    }

    let body = response.text().await.context("Failed to read response body")?;
    println!("{}", body);

    Ok(())
}

async fn import_openapi(client: &FlowplaneClient, file: PathBuf, output: &str) -> Result<()> {
    let contents = std::fs::read(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let response = client
        .post("/api/v1/api-definitions/from-openapi")
        .header("Content-Type", "application/octet-stream")
        .body(contents)
        .send()
        .await
        .context("Failed to import OpenAPI spec")?;

    let status = response.status();
    if !status.is_success() {
        let error = response.text().await.unwrap_or_else(|_| "<unable to read error>".to_string());
        anyhow::bail!("HTTP request failed with status {}: {}", status, error);
    }

    let body = response.text().await.context("Failed to read response body")?;
    let parsed: CreateApiDefinitionResponse = serde_json::from_str(&body)
        .context("Failed to parse response as JSON")?;

    print_output(&parsed, output)?;
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

fn print_definitions_table(definitions: &[ApiDefinitionSummary]) {
    if definitions.is_empty() {
        println!("No API definitions found");
        return;
    }

    println!();
    println!(
        "{:<40} {:<15} {:<30} {:<10} {:<8}",
        "ID", "Team", "Domain", "Isolation", "Version"
    );
    println!("{}", "-".repeat(110));

    for def in definitions {
        println!(
            "{:<40} {:<15} {:<30} {:<10} {:<8}",
            truncate(&def.id, 38),
            truncate(&def.team, 13),
            truncate(&def.domain, 28),
            def.listener_isolation,
            def.version
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
