//! Schema CLI commands
//!
//! Provides command-line interface for listing, inspecting, and exporting
//! aggregated API schemas discovered by learning sessions.

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use super::client::FlowplaneClient;
use super::output::{print_output, truncate};

#[derive(Subcommand)]
pub enum SchemaCommands {
    /// List discovered API schemas
    #[command(
        long_about = "List aggregated API schemas discovered by learning sessions.\n\nShows path, HTTP method, confidence score, sample count, and version for each schema.",
        after_help = "EXAMPLES:\n    # List all schemas\n    flowplane schema list\n\n    # Filter by confidence\n    flowplane schema list --min-confidence 0.7\n\n    # Filter by HTTP method\n    flowplane schema list --method GET\n\n    # JSON output for scripting\n    flowplane schema list -o json"
    )]
    List {
        /// Filter by learning session (name or UUID)
        #[arg(long)]
        session: Option<String>,

        /// Minimum confidence score (0.0 - 1.0)
        #[arg(long)]
        min_confidence: Option<f64>,

        /// Filter by path pattern
        #[arg(long)]
        path: Option<String>,

        /// Filter by HTTP method
        #[arg(long)]
        method: Option<String>,

        /// Show only latest version of each schema
        #[arg(long)]
        latest_only: bool,

        /// Maximum number of results
        #[arg(long)]
        limit: Option<i32>,

        /// Offset for pagination
        #[arg(long)]
        offset: Option<i32>,

        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Get schema details
    #[command(
        long_about = "Show full details of an aggregated schema including request/response schemas,\nheaders, confidence, sample count, version, and breaking changes.",
        after_help = "EXAMPLES:\n    # View schema details\n    flowplane schema get 1\n\n    # Table summary\n    flowplane schema get 1 -o table"
    )]
    Get {
        /// Schema ID
        #[arg(value_name = "ID")]
        id: i64,

        /// Output format
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Compare two schema versions
    #[command(
        long_about = "Compare two aggregated schema versions to see differences.",
        after_help = "EXAMPLES:\n    # Compare two specific schemas\n    flowplane schema compare 5 --with 3\n\n    # Diff output\n    flowplane schema compare 5 --with 3 -o diff"
    )]
    Compare {
        /// Schema ID to compare
        #[arg(value_name = "ID")]
        id: i64,

        /// Schema ID to compare against
        #[arg(long, value_name = "ID")]
        with: i64,

        /// Output format
        #[arg(short, long, default_value = "json", value_parser = ["json", "diff"])]
        output: String,
    },

    /// Export schemas as OpenAPI 3.1 spec
    #[command(
        long_about = "Export aggregated schemas as an OpenAPI 3.1 specification.\n\nOutput goes to stdout by default (pipeable). Use -o to write to a file.\nFile format is auto-detected from extension (.yaml/.yml = YAML, .json = JSON).\nStdout defaults to YAML.",
        after_help = "EXAMPLES:\n    # Export all schemas as YAML to stdout\n    flowplane schema export --all\n\n    # Export specific schemas to a file\n    flowplane schema export --id 1,2,3 -o api.yaml\n\n    # Export high-confidence schemas as JSON\n    flowplane schema export --all --min-confidence 0.7 -o api.json\n\n    # Pipe to another tool\n    flowplane schema export --all | yq '.paths'"
    )]
    Export {
        /// Export schemas from a specific session (name or UUID)
        #[arg(long)]
        session: Option<String>,

        /// Schema IDs to export (comma-separated)
        #[arg(long, value_delimiter = ',')]
        id: Option<Vec<i64>>,

        /// Export all latest schemas
        #[arg(long)]
        all: bool,

        /// Minimum confidence filter (used with --all)
        #[arg(long)]
        min_confidence: Option<f64>,

        /// API title in the OpenAPI spec
        #[arg(long, default_value = "Learned API")]
        title: String,

        /// API version in the OpenAPI spec
        #[arg(long, default_value = "1.0.0")]
        version: String,

        /// API description
        #[arg(long)]
        description: Option<String>,

        /// Output file (auto-detects format from extension; stdout if omitted)
        #[arg(short, long)]
        output: Option<String>,
    },
}

/// Aggregated schema response from the API
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregatedSchemaResponse {
    pub id: i64,
    pub team: String,
    pub path: String,
    pub http_method: String,
    pub version: i64,
    pub previous_version_id: Option<i64>,
    pub request_schema: Option<serde_json::Value>,
    pub response_schemas: Option<serde_json::Value>,
    pub sample_count: i64,
    pub confidence_score: f64,
    pub breaking_changes: Option<Vec<serde_json::Value>>,
    pub first_observed: String,
    pub last_observed: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub snapshot_number: Option<i64>,
}

/// Export request body
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportRequest {
    schema_ids: Vec<i64>,
    title: String,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

pub async fn handle_schema_command(
    command: SchemaCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        SchemaCommands::List {
            session,
            min_confidence,
            path,
            method,
            latest_only,
            limit,
            offset,
            output,
        } => {
            let schemas = list_schemas(
                client,
                team,
                min_confidence,
                path,
                method,
                latest_only,
                limit,
                offset,
                session,
            )
            .await?;
            if output == "table" {
                print_schemas_table(&schemas);
            } else {
                print_output(&schemas, &output)?;
            }
        }
        SchemaCommands::Get { id, output } => {
            let schema = get_schema(client, team, id).await?;
            if output == "table" {
                print_schema_detail(&schema);
            } else {
                print_output(&schema, &output)?;
            }
        }
        SchemaCommands::Compare { id, with: with_version, output } => {
            compare_schemas(client, team, id, with_version, &output).await?;
        }
        SchemaCommands::Export {
            session,
            id,
            all,
            min_confidence,
            title,
            version,
            description,
            output,
        } => {
            export_schemas(
                client,
                team,
                id,
                all,
                min_confidence,
                title,
                version,
                description,
                output,
                session,
            )
            .await?;
        }
    }

    Ok(())
}

/// List schemas — reused by both `schema list` and `learn export`
#[allow(clippy::too_many_arguments)]
pub async fn list_schemas(
    client: &FlowplaneClient,
    team: &str,
    min_confidence: Option<f64>,
    path: Option<String>,
    method: Option<String>,
    latest_only: bool,
    limit: Option<i32>,
    offset: Option<i32>,
    session: Option<String>,
) -> Result<Vec<AggregatedSchemaResponse>> {
    let mut query_path = format!("/api/v1/teams/{team}/aggregated-schemas?");
    let mut params = Vec::new();

    if let Some(c) = min_confidence {
        params.push(format!("minConfidence={c}"));
    }
    if let Some(p) = &path {
        params.push(format!("path={p}"));
    }
    if let Some(m) = &method {
        params.push(format!("httpMethod={m}"));
    }
    if latest_only {
        params.push("latestOnly=true".to_string());
    }
    if let Some(l) = limit {
        params.push(format!("limit={l}"));
    }
    if let Some(o) = offset {
        params.push(format!("offset={o}"));
    }
    if let Some(s) = &session {
        params.push(format!("sessionId={s}"));
    }

    query_path.push_str(&params.join("&"));

    client.get_json(&query_path).await
}

/// Export schemas as OpenAPI — reused by both `schema export` and `learn export`
#[allow(clippy::too_many_arguments)]
pub async fn export_schemas(
    client: &FlowplaneClient,
    team: &str,
    ids: Option<Vec<i64>>,
    all: bool,
    min_confidence: Option<f64>,
    title: String,
    version: String,
    description: Option<String>,
    output: Option<String>,
    session: Option<String>,
) -> Result<()> {
    // Resolve schema IDs
    let schema_ids = if let Some(ids) = ids {
        if ids.is_empty() {
            anyhow::bail!("No schema IDs provided. Use --id 1,2,3 or --all");
        }
        ids
    } else if all {
        let has_session = session.is_some();
        let schemas = list_schemas(
            client,
            team,
            min_confidence,
            None,
            None,
            true,
            None,
            None,
            session,
        )
        .await?;
        if schemas.is_empty() {
            // When exporting from a specific session with no schemas (no traffic captured),
            // emit a minimal empty OpenAPI spec instead of erroring.
            if has_session {
                let empty_spec = serde_json::json!({
                    "openapi": "3.1.0",
                    "info": { "title": title, "version": version },
                    "paths": {}
                });
                match output {
                    Some(ref file_path) => {
                        let content = if file_path.ends_with(".json") {
                            serde_json::to_string_pretty(&empty_spec)
                                .context("Failed to serialize to JSON")?
                        } else {
                            serde_yaml::to_string(&empty_spec)
                                .context("Failed to serialize to YAML")?
                        };
                        std::fs::write(file_path, &content)
                            .with_context(|| format!("Failed to write to {file_path}"))?;
                        eprintln!("Exported empty OpenAPI spec to {file_path} (no schemas found for session)");
                    }
                    None => {
                        let yaml = serde_yaml::to_string(&empty_spec)
                            .context("Failed to serialize to YAML")?;
                        print!("{yaml}");
                    }
                }
                return Ok(());
            }
            anyhow::bail!("No schemas found. Run a learning session first.");
        }
        schemas.iter().map(|s| s.id).collect()
    } else {
        // Show available schemas and error
        let schemas =
            list_schemas(client, team, None, None, None, true, Some(10), None, None).await?;
        if schemas.is_empty() {
            anyhow::bail!("No schemas found. Run a learning session first.");
        }
        eprintln!("Available schemas:");
        for s in &schemas {
            eprintln!(
                "  [{:3}] {:6} {:40} conf={:.2}  samples={}",
                s.id, s.http_method, s.path, s.confidence_score, s.sample_count
            );
        }
        anyhow::bail!("Specify schemas with --id 1,2,3 or use --all to export everything");
    };

    let body = ExportRequest { schema_ids, title, version, description };

    let export_path = format!("/api/v1/teams/{team}/aggregated-schemas/export");
    let spec: serde_json::Value = client.post_json(&export_path, &body).await?;

    // Determine output format and destination
    match output {
        Some(ref file_path) => {
            let content = if file_path.ends_with(".json") {
                serde_json::to_string_pretty(&spec).context("Failed to serialize to JSON")?
            } else {
                // Default to YAML for .yaml, .yml, or any other extension
                serde_yaml::to_string(&spec).context("Failed to serialize to YAML")?
            };
            std::fs::write(file_path, &content)
                .with_context(|| format!("Failed to write to {file_path}"))?;
            eprintln!("Exported OpenAPI spec to {file_path}");
        }
        None => {
            // Stdout — default to YAML (more readable for humans)
            let yaml = serde_yaml::to_string(&spec).context("Failed to serialize to YAML")?;
            print!("{yaml}");
        }
    }

    Ok(())
}

async fn compare_schemas(
    client: &FlowplaneClient,
    team: &str,
    id: i64,
    with_version: i64,
    output: &str,
) -> Result<()> {
    let path =
        format!("/api/v1/teams/{team}/aggregated-schemas/{id}/compare?withVersion={with_version}");

    let response: serde_json::Value = client.get_json(&path).await?;

    match output {
        "diff" => {
            // Print as human-readable diff
            let yaml = serde_yaml::to_string(&response).context("Failed to serialize to YAML")?;
            println!("{yaml}");
        }
        _ => {
            let json =
                serde_json::to_string_pretty(&response).context("Failed to serialize to JSON")?;
            println!("{json}");
        }
    }

    Ok(())
}

async fn get_schema(
    client: &FlowplaneClient,
    team: &str,
    id: i64,
) -> Result<AggregatedSchemaResponse> {
    let path = format!("/api/v1/teams/{team}/aggregated-schemas/{id}");
    client.get_json(&path).await
}

fn print_schemas_table(schemas: &[AggregatedSchemaResponse]) {
    if schemas.is_empty() {
        println!("No schemas found. Run a learning session to discover API schemas.");
        return;
    }

    println!();
    println!(
        "{:>5}  {:<7} {:<45} {:>10} {:>8} {:>7}",
        "ID", "Method", "Path", "Confidence", "Samples", "Version"
    );
    println!("{}", "-".repeat(88));

    for s in schemas {
        println!(
            "{:>5}  {:<7} {:<45} {:>9.2}% {:>8} {:>7}",
            s.id,
            s.http_method,
            truncate(&s.path, 43),
            s.confidence_score * 100.0,
            s.sample_count,
            s.version,
        );
    }

    let total_samples: i64 = schemas.iter().map(|s| s.sample_count).sum();
    let avg_confidence: f64 =
        schemas.iter().map(|s| s.confidence_score).sum::<f64>() / schemas.len() as f64;
    println!("{}", "-".repeat(88));
    println!(
        "{:>5}  {:<7} {:<45} {:>9.2}% {:>8}",
        schemas.len(),
        "total",
        "",
        avg_confidence * 100.0,
        total_samples,
    );
    println!();
}

fn print_schema_detail(s: &AggregatedSchemaResponse) {
    println!();
    println!("Schema #{}", s.id);
    println!("{}", "-".repeat(50));
    println!("  Path:            {}", s.path);
    println!("  Method:          {}", s.http_method);
    println!("  Confidence:      {:.1}%", s.confidence_score * 100.0);
    println!("  Samples:         {}", s.sample_count);
    println!("  Version:         {}", s.version);
    println!("  First observed:  {}", s.first_observed);
    println!("  Last observed:   {}", s.last_observed);

    if let Some(ref changes) = s.breaking_changes {
        if !changes.is_empty() {
            println!("  Breaking changes: {}", changes.len());
        }
    }

    if let Some(ref req) = s.request_schema {
        println!();
        println!("  Request Schema:");
        if let Some(props) = req.get("properties").and_then(|p| p.as_object()) {
            for (name, prop) in props {
                let type_str = prop.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");
                let format_str = prop.get("format").and_then(|f| f.as_str());
                let required = req
                    .get("required")
                    .and_then(|r| r.as_array())
                    .map(|arr| arr.iter().any(|v| v.as_str() == Some(name)))
                    .unwrap_or(false);
                let req_marker = if required { "*" } else { " " };
                match format_str {
                    Some(fmt) => println!("    {req_marker} {name}: {type_str} ({fmt})"),
                    None => println!("    {req_marker} {name}: {type_str}"),
                }
            }
            println!("    (* = required)");
        }
    }

    if let Some(ref resp) = s.response_schemas {
        if let Some(obj) = resp.as_object() {
            for (code, schema) in obj {
                println!();
                println!("  Response {code}:");
                if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                    for (name, prop) in props {
                        let type_str =
                            prop.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");
                        let format_str = prop.get("format").and_then(|f| f.as_str());
                        match format_str {
                            Some(fmt) => println!("      {name}: {type_str} ({fmt})"),
                            None => println!("      {name}: {type_str}"),
                        }
                    }
                } else if let Some(type_str) = schema.get("type").and_then(|t| t.as_str()) {
                    println!("      type: {type_str}");
                }
            }
        }
    }
    println!();
}
