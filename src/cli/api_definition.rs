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
    /// Create a new API definition from a JSON specification file
    #[command(
        long_about = "Create a new API definition by providing a JSON file with the specification.\n\nThe JSON file should contain fields like name, team, domain, and specification details.",
        after_help = "EXAMPLES:\n    # Create an API definition from a JSON file\n    flowplane api create --file api-spec.json\n\n    # Create and output as YAML\n    flowplane api create --file api-spec.json --output yaml\n\n    # With authentication\n    flowplane api create --file api-spec.json --token your-token"
    )]
    Create {
        /// Path to JSON file with API definition spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// List all API definitions with optional filtering
    #[command(
        long_about = "List all API definitions in the system. Supports filtering by team, domain, and pagination.",
        after_help = "EXAMPLES:\n    # List all API definitions\n    flowplane api list\n\n    # List with table output\n    flowplane api list --output table\n\n    # Filter by team\n    flowplane api list --team platform\n\n    # Filter by team and domain\n    flowplane api list --team platform --domain users\n\n    # Paginate results\n    flowplane api list --limit 10 --offset 20"
    )]
    List {
        /// Filter by team name
        #[arg(long, value_name = "TEAM")]
        team: Option<String>,

        /// Filter by domain
        #[arg(long, value_name = "DOMAIN")]
        domain: Option<String>,

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

    /// Get a specific API definition by its ID
    #[command(
        long_about = "Retrieve detailed information about a specific API definition using its unique ID.",
        after_help = "EXAMPLES:\n    # Get an API definition by ID\n    flowplane api get abc123\n\n    # Get with YAML output\n    flowplane api get abc123 --output yaml\n\n    # Get with table output\n    flowplane api get abc123 --output table"
    )]
    Get {
        /// API definition ID
        #[arg(value_name = "ID")]
        id: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Get Envoy bootstrap configuration for an API definition
    #[command(
        long_about = "Generate and retrieve the Envoy bootstrap configuration for a specific API definition.\n\nThe bootstrap config includes static resources like listeners, routes, and clusters that Envoy needs to start.",
        after_help = "EXAMPLES:\n    # Get bootstrap config in YAML format\n    flowplane api bootstrap abc123\n\n    # Get bootstrap config in JSON\n    flowplane api bootstrap abc123 --format json\n\n    # Scope to team listeners only\n    flowplane api bootstrap abc123 --scope team\n\n    # Use allowlist scope\n    flowplane api bootstrap abc123 --scope allowlist --allowlist listener1,listener2\n\n    # Include default listeners in team scope\n    flowplane api bootstrap abc123 --scope team --include-default"
    )]
    Bootstrap {
        /// API definition ID
        #[arg(value_name = "ID")]
        id: String,

        /// Output format (yaml or json)
        #[arg(short, long, default_value = "yaml", value_parser = ["yaml", "json"])]
        format: String,

        /// Scoping mode (all, team, or allowlist)
        #[arg(long, default_value = "all", value_parser = ["all", "team", "allowlist"])]
        scope: String,

        /// Listener allowlist (comma-separated) when scope=allowlist
        #[arg(long, value_name = "LISTENERS")]
        allowlist: Option<String>,

        /// Include default listeners in team scope
        #[arg(long)]
        include_default: bool,
    },

    /// Import an API definition from an OpenAPI 3.0 specification
    #[command(
        long_about = "Import an API definition by parsing an OpenAPI 3.0 specification file.\n\nSupports both JSON and YAML formats. Can include x-flowplane-* extensions for filter configurations.",
        after_help = "EXAMPLES:\n    # Import from OpenAPI YAML file\n    flowplane api import-openapi --file openapi.yaml\n\n    # Import from OpenAPI JSON file\n    flowplane api import-openapi --file openapi.json\n\n    # Import with YAML output\n    flowplane api import-openapi --file openapi.yaml --output yaml\n\n    # Import with authentication\n    flowplane api import-openapi --file openapi.yaml --token your-token"
    )]
    ImportOpenapi {
        /// Path to OpenAPI spec file (JSON or YAML)
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Team name for the API definition
        #[arg(short, long)]
        team: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Validate x-flowplane-filters syntax in an OpenAPI spec before importing
    #[command(
        long_about = "Validate the syntax of x-flowplane-filters extension in an OpenAPI spec file.\n\nThis is useful to check filter configurations before actually importing the API definition.\nNo authentication required - validation is performed locally.",
        after_help = "EXAMPLES:\n    # Validate filters in OpenAPI file\n    flowplane api validate-filters --file openapi-with-filters.yaml\n\n    # Validate with JSON output\n    flowplane api validate-filters --file openapi.yaml --output json\n\n    # No authentication required (local validation)\n    flowplane api validate-filters --file myapi.yaml"
    )]
    ValidateFilters {
        /// Path to OpenAPI spec file (JSON or YAML)
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Show deployed filter configurations from an API definition
    #[command(
        long_about = "Retrieve and display the HTTP filter configurations that are currently deployed for a specific API definition.",
        after_help = "EXAMPLES:\n    # Show filters for an API definition\n    flowplane api show-filters abc123\n\n    # Show filters as JSON\n    flowplane api show-filters abc123 --output json\n\n    # Show filters as table\n    flowplane api show-filters abc123 --output table"
    )]
    ShowFilters {
        /// API definition ID
        #[arg(value_name = "ID")]
        id: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "yaml", value_parser = ["json", "yaml", "table"])]
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
    // Handle commands that don't require authentication
    if let ApiCommands::ValidateFilters { file, output } = &command {
        return validate_filters(file.clone(), output).await;
    }

    // Resolve configuration from multiple sources for commands that need API access
    let token = resolve_token(token, token_file)?;
    let base_url = resolve_base_url(base_url);
    let timeout = resolve_timeout(timeout);

    let config = ClientConfig { base_url, token, timeout, verbose };

    let client = FlowplaneClient::new(config)?;

    match command {
        ApiCommands::Create { file, output } => {
            create_api_definition(&client, file, &output).await?
        }
        ApiCommands::List { team, domain, limit, offset, output } => {
            list_api_definitions(&client, team, domain, limit, offset, &output).await?
        }
        ApiCommands::Get { id, output } => get_api_definition(&client, &id, &output).await?,
        ApiCommands::Bootstrap { id, format, scope, allowlist, include_default } => {
            get_bootstrap_config(&client, &id, &format, &scope, allowlist, include_default).await?
        }
        ApiCommands::ImportOpenapi { file, team, output } => {
            import_openapi(&client, file, &team, &output).await?
        }
        ApiCommands::ValidateFilters { .. } => unreachable!("Handled above"),
        ApiCommands::ShowFilters { id, output } => show_filters(&client, &id, &output).await?,
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

    let body: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse JSON from file")?;

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
    let mut path =
        format!("/api/v1/api-definitions/{}/bootstrap?format={}&scope={}", id, format, scope);

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

async fn import_openapi(
    client: &FlowplaneClient,
    file: PathBuf,
    team: &str,
    output: &str,
) -> Result<()> {
    let contents =
        std::fs::read(&file).with_context(|| format!("Failed to read file: {}", file.display()))?;

    let response = client
        .post("/api/v1/api-definitions/from-openapi")
        .query(&[("team", team)])
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
    let parsed: CreateApiDefinitionResponse =
        serde_json::from_str(&body).context("Failed to parse response as JSON")?;

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
    println!("{:<40} {:<15} {:<30} {:<10} {:<8}", "ID", "Team", "Domain", "Isolation", "Version");
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

/// Validate x-flowplane-filters in an OpenAPI spec before import
async fn validate_filters(file: PathBuf, output: &str) -> Result<()> {
    use crate::openapi::parse_global_filters;

    // Read file contents
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    // Parse OpenAPI document
    let openapi: openapiv3::OpenAPI = if file.extension().and_then(|s| s.to_str()) == Some("json") {
        serde_json::from_str(&contents).context("Failed to parse OpenAPI JSON document")?
    } else {
        serde_yaml::from_str(&contents).context("Failed to parse OpenAPI YAML document")?
    };

    // Extract and validate global filters
    match parse_global_filters(&openapi) {
        Ok(filters) => {
            if filters.is_empty() {
                println!("✅ No x-flowplane-filters found in OpenAPI spec");
                println!(
                    "\nTo add filters, include the x-flowplane-filters extension at the top level:"
                );
                println!("\nExample:");
                println!("  x-flowplane-filters:");
                println!("    - filter:");
                println!("        type: cors");
                println!("        policy:");
                println!("          allow_origin:");
                println!("            - type: exact");
                println!("              value: \"https://example.com\"");
            } else {
                println!("✅ Successfully validated {} filter(s) in OpenAPI spec", filters.len());
                println!();

                if output == "table" {
                    print_filters_table(&filters);
                } else if output == "json" {
                    let json = serde_json::to_string_pretty(&filters)
                        .context("Failed to serialize filters to JSON")?;
                    println!("{}", json);
                } else {
                    let yaml = serde_yaml::to_string(&filters)
                        .context("Failed to serialize filters to YAML")?;
                    println!("{}", yaml);
                }
            }
            Ok(())
        }
        Err(err) => {
            println!("❌ Filter validation failed:");
            println!("\n{}", err);
            println!("\nCommon filter types:");
            println!("  - cors");
            println!("  - header_mutation");
            println!("  - local_rate_limit");
            println!("  - custom_response");
            println!("\nSee examples/openapi-with-x-flowplane-filters.yaml for usage examples");
            anyhow::bail!("Filter validation failed")
        }
    }
}

/// Show filter configurations from an imported API definition
async fn show_filters(client: &FlowplaneClient, id: &str, output: &str) -> Result<()> {
    // Fetch bootstrap config which contains materialized filter configurations
    let path = format!("/api/v1/api-definitions/{}/bootstrap?scope=all&format=json", id);
    let bootstrap: serde_json::Value =
        client.get_json(&path).await.context("Failed to fetch bootstrap configuration")?;

    // Extract filter configurations from Envoy bootstrap config
    let filters = extract_filters_from_bootstrap(&bootstrap)?;

    if filters.is_empty() {
        println!("No HTTP filters configured for API definition '{}'", id);
        return Ok(());
    }

    println!("HTTP Filters for API definition '{}':", id);
    println!();

    if output == "json" {
        let json = serde_json::to_string_pretty(&filters)
            .context("Failed to serialize filters to JSON")?;
        println!("{}", json);
    } else if output == "table" {
        print_bootstrap_filters_table(&filters);
    } else {
        let yaml =
            serde_yaml::to_string(&filters).context("Failed to serialize filters to YAML")?;
        println!("{}", yaml);
    }

    Ok(())
}

/// Extract HTTP filter names from Envoy bootstrap configuration
fn extract_filters_from_bootstrap(bootstrap: &serde_json::Value) -> Result<Vec<serde_json::Value>> {
    let mut filters = Vec::new();

    // Navigate through Envoy bootstrap structure to find HTTP filters
    // Bootstrap -> static_resources -> listeners -> filter_chains -> filters -> typed_config -> http_filters
    if let Some(static_resources) = bootstrap.get("static_resources") {
        if let Some(listeners) = static_resources.get("listeners").and_then(|v| v.as_array()) {
            for listener in listeners {
                if let Some(filter_chains) =
                    listener.get("filter_chains").and_then(|v| v.as_array())
                {
                    for chain in filter_chains {
                        if let Some(chain_filters) = chain.get("filters").and_then(|v| v.as_array())
                        {
                            for filter in chain_filters {
                                if let Some(typed_config) = filter.get("typed_config") {
                                    if let Some(http_filters) =
                                        typed_config.get("http_filters").and_then(|v| v.as_array())
                                    {
                                        for http_filter in http_filters {
                                            filters.push(http_filter.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(filters)
}

/// Print filters in a table format for validation command
fn print_filters_table(filters: &[crate::xds::filters::http::HttpFilterConfigEntry]) {
    println!("{:<5} {:<30} Configuration", "No.", "Filter Type");
    println!("{}", "-".repeat(80));

    for (i, filter) in filters.iter().enumerate() {
        let filter_json = serde_json::to_value(filter).unwrap_or(serde_json::Value::Null);
        let filter_type = filter_json
            .get("filter")
            .and_then(|f| f.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("unknown");

        let config_preview = serde_json::to_string(&filter).unwrap_or_else(|_| "{}".to_string());
        let truncated = truncate(&config_preview, 45);

        println!("{:<5} {:<30} {}", i + 1, filter_type, truncated);
    }
    println!();
}

/// Print bootstrap filters in a table format for show-filters command
fn print_bootstrap_filters_table(filters: &[serde_json::Value]) {
    println!("{:<5} {:<30} Type URL", "No.", "Filter Name");
    println!("{}", "-".repeat(80));

    for (i, filter) in filters.iter().enumerate() {
        let name = filter.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
        let type_url = filter
            .get("typed_config")
            .and_then(|tc| tc.get("@type"))
            .and_then(|t| t.as_str())
            .unwrap_or("unknown");

        // Extract just the filter type from the type URL
        let filter_type = type_url.rsplit('.').next().unwrap_or(type_url);

        println!("{:<5} {:<30} {}", i + 1, name, filter_type);
    }
    println!();
}
