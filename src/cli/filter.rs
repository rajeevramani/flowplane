//! Filter CLI commands
//!
//! Provides command-line interface for managing HTTP filters: CRUD operations
//! and listener-level attach/detach.

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

use super::client::FlowplaneClient;
use super::config_file;
use super::output::{print_output, truncate};
use crate::api::handlers::PaginatedResponse;

#[derive(Subcommand)]
pub enum FilterCommands {
    /// Create a new HTTP filter from a JSON spec file
    Create {
        /// Path to YAML or JSON file with resource spec
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

    /// Update an existing filter from a JSON spec file
    Update {
        /// Filter name
        #[arg(value_name = "NAME")]
        name: String,

        /// Path to YAML or JSON file with resource spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// List available filter types
    Types {
        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Show details and schema for a specific filter type
    Type {
        /// Filter type name (e.g., header_mutation)
        #[arg(value_name = "NAME")]
        name: String,

        /// Output format (json or yaml)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml"])]
        output: String,
    },

    /// Generate a template manifest for a filter type
    Scaffold {
        /// Filter type name (e.g., header_mutation)
        #[arg(value_name = "TYPE")]
        filter_type: String,

        /// Output format (yaml or json)
        #[arg(short, long, default_value = "yaml", value_parser = ["json", "yaml"])]
        output: String,
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

/// Response for listing filter types (matches API FilterTypesResponse)
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterTypesResponse {
    pub filter_types: Vec<FilterTypeInfo>,
    pub total: usize,
    pub implemented_count: usize,
}

/// Info about a single filter type (matches API FilterTypeInfo)
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterTypeInfo {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub version: String,
    pub is_implemented: bool,
    pub source: String,
    #[serde(default)]
    pub config_schema: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
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
        FilterCommands::Update { name, file, output } => {
            update_filter(client, team, &name, file, &output).await?
        }
        FilterCommands::Types { output } => list_filter_types(client, &output).await?,
        FilterCommands::Type { name, output } => get_filter_type(client, &name, &output).await?,
        FilterCommands::Scaffold { filter_type, output } => {
            scaffold_filter(client, &filter_type, &output).await?
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
    let mut body = config_file::load_config_file(&file)?;
    config_file::strip_kind_field(&mut body);

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

async fn update_filter(
    client: &FlowplaneClient,
    team: &str,
    name: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let id = resolve_filter_id(client, team, name).await?;

    let mut body = config_file::load_config_file(&file)?;
    config_file::strip_kind_field(&mut body);

    let path = format!("/api/v1/teams/{team}/filters/{id}");
    let response: FilterResponse = client.patch_json(&path, &body).await?;

    if output == "table" {
        print_filters_table(&[response]);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn list_filter_types(client: &FlowplaneClient, output: &str) -> Result<()> {
    let path = "/api/v1/filter-types";
    let response: FilterTypesResponse = client.get_json(path).await?;

    if output == "table" {
        print_filter_types_table(&response.filter_types);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn get_filter_type(client: &FlowplaneClient, name: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/filter-types/{name}");
    let response: FilterTypeInfo = client.get_json(&path).await?;

    print_output(&response, output)?;

    Ok(())
}

async fn scaffold_filter(client: &FlowplaneClient, filter_type: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/filter-types/{filter_type}");
    let type_info: FilterTypeInfo = client
        .get_json(&path)
        .await
        .with_context(|| format!("Failed to fetch filter type '{filter_type}'"))?;

    let mut stdout = std::io::stdout();
    scaffold_filter_to_writer(&mut stdout, &type_info, filter_type, output)
}

/// Write filter scaffold to a writer. Shared by CLI and unit tests.
fn scaffold_filter_to_writer(
    writer: &mut impl Write,
    type_info: &FilterTypeInfo,
    filter_type: &str,
    output: &str,
) -> Result<()> {
    let config_fields = build_config_from_schema(&type_info.config_schema);

    if output == "json" {
        let mut scaffold = serde_json::Map::new();
        scaffold.insert("kind".to_string(), serde_json::Value::String("Filter".to_string()));
        scaffold.insert(
            "name".to_string(),
            serde_json::Value::String("<your-filter-name>".to_string()),
        );
        scaffold
            .insert("filterType".to_string(), serde_json::Value::String(filter_type.to_string()));
        scaffold.insert(
            "description".to_string(),
            serde_json::Value::String(type_info.description.clone()),
        );

        let mut config = serde_json::Map::new();
        config.insert("type".to_string(), serde_json::Value::String(filter_type.to_string()));
        config.insert("config".to_string(), config_fields);
        scaffold.insert("config".to_string(), serde_json::Value::Object(config));

        let json = serde_json::to_string_pretty(&serde_json::Value::Object(scaffold))
            .context("Failed to serialize scaffold to JSON")?;
        writeln!(writer, "{json}").context("Failed to write scaffold JSON")?;
    } else {
        writeln!(writer, "# Scaffold for filter type: {filter_type}")?;
        writeln!(writer, "# {}", type_info.description)?;
        writeln!(writer, "kind: Filter")?;
        writeln!(writer, "name: \"<your-filter-name>\"")?;
        writeln!(writer, "filterType: \"{}\"", filter_type)?;
        writeln!(writer, "description: \"{}\"", type_info.description)?;
        writeln!(writer, "config:")?;
        writeln!(writer, "  type: \"{}\"", filter_type)?;
        writeln!(writer, "  config:")?;
        write_yaml_from_schema(writer, &type_info.config_schema, 4)?;
    }

    Ok(())
}

/// Build a JSON Value from the config_schema properties with default/placeholder values.
/// Recurses into nested objects to populate required sub-fields.
fn build_config_from_schema(config_schema: &Option<serde_json::Value>) -> serde_json::Value {
    match config_schema {
        Some(s) => build_value_from_schema(s),
        None => serde_json::Value::Object(serde_json::Map::new()),
    }
}

/// Recursively build a JSON value from a schema node, populating defaults and
/// required nested fields.
fn build_value_from_schema(schema: &serde_json::Value) -> serde_json::Value {
    let type_str = schema.get("type").and_then(|t| t.as_str()).unwrap_or("string");

    // If there's a default, use it
    if let Some(default) = schema.get("default") {
        return default.clone();
    }

    // If there's an enum, use the first value as the placeholder
    if let Some(enum_vals) = schema.get("enum").and_then(|e| e.as_array()) {
        if let Some(first) = enum_vals.first() {
            return first.clone();
        }
    }

    match type_str {
        "object" => {
            let properties = match schema.get("properties").and_then(|p| p.as_object()) {
                Some(p) => p,
                None => {
                    // additionalProperties object (e.g., providers map) — generate
                    // a single example entry
                    if schema.get("additionalProperties").is_some() {
                        let mut map = serde_json::Map::new();
                        let inner = schema
                            .get("additionalProperties")
                            .map(build_value_from_schema)
                            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                        map.insert("<your-key>".to_string(), inner);
                        return serde_json::Value::Object(map);
                    }
                    return serde_json::Value::Object(serde_json::Map::new());
                }
            };

            let required: Vec<&str> = schema
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            let mut result = serde_json::Map::new();
            for (key, prop) in properties {
                // Include field if it's required, has a default, or has required sub-fields
                let is_required = required.contains(&key.as_str());
                let has_default = prop.get("default").is_some();
                let has_required_children = prop.get("required").is_some();

                if is_required || has_default || has_required_children {
                    result.insert(key.clone(), build_value_from_schema(prop));
                }
            }

            // If no fields matched, include all properties (flat schema like header_mutation)
            if result.is_empty() {
                for (key, prop) in properties {
                    result.insert(key.clone(), build_value_from_schema(prop));
                }
            }

            serde_json::Value::Object(result)
        }
        "array" => {
            // For arrays with items schema, generate one example item
            if let Some(items_schema) = schema.get("items") {
                serde_json::Value::Array(vec![build_value_from_schema(items_schema)])
            } else {
                serde_json::Value::Array(Vec::new())
            }
        }
        _ => placeholder_for_type(type_str),
    }
}

/// Write YAML output by walking the schema and emitting comments from descriptions
/// alongside recursively-built default values. This produces the same values as the
/// JSON scaffold path (build_value_from_schema) but with human-readable comments.
fn write_yaml_from_schema(
    writer: &mut impl Write,
    config_schema: &Option<serde_json::Value>,
    indent: usize,
) -> Result<()> {
    let schema = match config_schema {
        Some(s) => s,
        None => return Ok(()),
    };
    write_yaml_object_fields(writer, schema, indent)
}

/// Recursively write YAML fields for an object schema node.
fn write_yaml_object_fields(
    writer: &mut impl Write,
    schema: &serde_json::Value,
    indent: usize,
) -> Result<()> {
    let properties = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return Ok(()),
    };

    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let pad = " ".repeat(indent);

    for (key, prop) in properties {
        let is_required = required.contains(&key.as_str());
        let has_default = prop.get("default").is_some();
        let has_required_children = prop.get("required").is_some();

        // Same inclusion logic as build_value_from_schema for objects
        if !is_required && !has_default && !has_required_children {
            continue;
        }

        write_yaml_field(writer, key, prop, &pad, indent)?;
    }

    // If no fields matched (no required/default/required_children), include all properties.
    // This handles flat schemas like header_mutation where no field is individually required.
    let any_written = properties.iter().any(|(key, prop)| {
        let is_required = required.contains(&key.as_str());
        let has_default = prop.get("default").is_some();
        let has_required_children = prop.get("required").is_some();
        is_required || has_default || has_required_children
    });
    if !any_written {
        for (key, prop) in properties {
            write_yaml_field(writer, key, prop, &pad, indent)?;
        }
    }

    Ok(())
}

/// Write a single YAML field with its description comment and recursively expanded value.
fn write_yaml_field(
    writer: &mut impl Write,
    key: &str,
    prop: &serde_json::Value,
    pad: &str,
    indent: usize,
) -> Result<()> {
    if let Some(desc) = prop.get("description").and_then(|d| d.as_str()) {
        writeln!(writer, "{pad}# {desc}")?;
    }

    let type_str = prop.get("type").and_then(|t| t.as_str()).unwrap_or("string");
    match type_str {
        "object" if prop.get("properties").is_some() => {
            writeln!(writer, "{pad}{key}:")?;
            write_yaml_object_fields(writer, prop, indent + 2)?;
        }
        "array" => {
            if let Some(items_schema) = prop.get("items") {
                let items_type =
                    items_schema.get("type").and_then(|t| t.as_str()).unwrap_or("string");
                if items_type == "object" && items_schema.get("properties").is_some() {
                    writeln!(writer, "{pad}{key}:")?;
                    write_yaml_array_object_item(writer, items_schema, indent + 2)?;
                } else {
                    let example = build_value_from_schema(items_schema);
                    writeln!(writer, "{pad}{key}:")?;
                    writeln!(writer, "{pad}  - {}", yaml_value_str(&example))?;
                }
            } else {
                writeln!(writer, "{pad}{key}: []")?;
            }
        }
        _ => {
            let value = build_value_from_schema(prop);
            writeln!(writer, "{pad}{key}: {}", yaml_value_str(&value))?;
        }
    }
    Ok(())
}

/// Write a single example array item with `- key: value` YAML syntax.
fn write_yaml_array_object_item(
    writer: &mut impl Write,
    items_schema: &serde_json::Value,
    indent: usize,
) -> Result<()> {
    let properties = match items_schema.get("properties").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return Ok(()),
    };

    let pad = " ".repeat(indent);
    let mut first = true;
    for (key, prop) in properties {
        let value = build_value_from_schema(prop);
        if first {
            writeln!(writer, "{pad}- {key}: {}", yaml_value_str(&value))?;
            first = false;
        } else {
            writeln!(writer, "{pad}  {key}: {}", yaml_value_str(&value))?;
        }
    }
    Ok(())
}

/// Extract properties as (key, description, yaml_default_string) tuples
#[allow(dead_code)]
fn extract_properties(
    config_schema: &Option<serde_json::Value>,
) -> Vec<(String, Option<String>, String)> {
    let schema = match config_schema {
        Some(s) => s,
        None => return Vec::new(),
    };

    let properties = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return Vec::new(),
    };

    properties
        .iter()
        .map(|(key, prop)| {
            let desc = prop.get("description").and_then(|d| d.as_str()).map(|s| s.to_string());
            let default_str = if let Some(default) = prop.get("default") {
                yaml_value_str(default)
            } else {
                yaml_placeholder_for_type(
                    prop.get("type").and_then(|t| t.as_str()).unwrap_or("string"),
                )
            };
            (key.clone(), desc, default_str)
        })
        .collect()
}

fn placeholder_for_type(type_str: &str) -> serde_json::Value {
    match type_str {
        "integer" | "number" => serde_json::Value::Number(serde_json::Number::from(0)),
        "boolean" => serde_json::Value::Bool(false),
        "array" => serde_json::Value::Array(Vec::new()),
        "object" => serde_json::Value::Object(serde_json::Map::new()),
        _ => serde_json::Value::String("<your-value>".to_string()),
    }
}

fn yaml_placeholder_for_type(type_str: &str) -> String {
    match type_str {
        "integer" | "number" => "0".to_string(),
        "boolean" => "false".to_string(),
        "array" => "[]".to_string(),
        "object" => "{}".to_string(),
        _ => "\"<your-value>\"".to_string(),
    }
}

fn yaml_value_str(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => format!("\"{s}\""),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Array(arr) if arr.is_empty() => "[]".to_string(),
        serde_json::Value::Object(obj) if obj.is_empty() => "{}".to_string(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "\"<your-value>\"".to_string()),
    }
}

fn print_filter_types_table(types: &[FilterTypeInfo]) {
    if types.is_empty() {
        println!("No filter types found");
        return;
    }

    println!();
    println!("{:<25} {:<25} {:<50}", "Name", "Display Name", "Description");
    println!("{}", "-".repeat(100));

    for ft in types {
        println!(
            "{:<25} {:<25} {:<50}",
            truncate(&ft.name, 23),
            truncate(&ft.display_name, 23),
            truncate(&ft.description, 48),
        );
    }
    println!();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_type_info(
        filter_type: &str,
        config_schema: Option<serde_json::Value>,
    ) -> FilterTypeInfo {
        FilterTypeInfo {
            name: filter_type.to_string(),
            display_name: filter_type.to_string(),
            description: format!("Test {filter_type} filter"),
            version: "1.0".to_string(),
            is_implemented: true,
            source: "built-in".to_string(),
            config_schema,
            extra: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    #[test]
    fn scaffold_json_includes_filter_type() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "traffic_mode": {
                    "type": "string",
                    "default": "pass_through"
                }
            }
        });
        let type_info = make_type_info("mcp", Some(schema));

        let mut buf = Vec::new();
        scaffold_filter_to_writer(&mut buf, &type_info, "mcp", "json")
            .expect("scaffold should succeed");
        let output = String::from_utf8(buf).expect("valid utf8");

        let parsed: serde_json::Value =
            serde_json::from_str(&output).expect("output should be valid JSON");
        assert_eq!(
            parsed.get("filterType").and_then(|v| v.as_str()),
            Some("mcp"),
            "JSON scaffold must include filterType field. Got: {output}"
        );
        assert_eq!(parsed.get("kind").and_then(|v| v.as_str()), Some("Filter"),);
    }

    #[test]
    fn scaffold_yaml_includes_filter_type() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "traffic_mode": {
                    "type": "string",
                    "default": "pass_through"
                }
            }
        });
        let type_info = make_type_info("mcp", Some(schema));

        let mut buf = Vec::new();
        scaffold_filter_to_writer(&mut buf, &type_info, "mcp", "yaml")
            .expect("scaffold should succeed");
        let output = String::from_utf8(buf).expect("valid utf8");

        assert!(
            output.contains("filterType: \"mcp\""),
            "YAML scaffold must include filterType field. Got:\n{output}"
        );
        assert!(output.contains("kind: Filter"));
    }

    #[test]
    fn scaffold_json_includes_filter_type_for_all_filter_types() {
        let filter_types = [
            "header_mutation",
            "cors",
            "custom_response",
            "rbac",
            "mcp",
            "local_rate_limit",
            "compressor",
            "ext_authz",
            "jwt_auth",
            "oauth2",
        ];

        for ft in &filter_types {
            let type_info =
                make_type_info(ft, Some(serde_json::json!({"type": "object", "properties": {}})));
            let mut buf = Vec::new();
            scaffold_filter_to_writer(&mut buf, &type_info, ft, "json")
                .expect("scaffold should succeed");
            let output = String::from_utf8(buf).expect("valid utf8");
            let parsed: serde_json::Value = serde_json::from_str(&output)
                .unwrap_or_else(|e| panic!("Invalid JSON for {ft}: {e}\n{output}"));

            assert_eq!(
                parsed.get("filterType").and_then(|v| v.as_str()),
                Some(*ft),
                "filterType missing for filter type '{ft}'"
            );
        }
    }

    #[test]
    fn build_config_handles_nested_required_fields() {
        // Simulates compressor schema: compressor_library.type is required
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "compressor_library": {
                    "type": "object",
                    "required": ["type"],
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["gzip"],
                            "default": "gzip"
                        },
                        "memory_level": {
                            "type": "integer",
                            "default": 5
                        }
                    }
                }
            }
        });
        let result = build_config_from_schema(&Some(schema));
        let lib = result.get("compressor_library").expect("compressor_library should exist");
        assert_eq!(
            lib.get("type").and_then(|v| v.as_str()),
            Some("gzip"),
            "compressor_library.type should be populated"
        );
    }

    #[test]
    fn build_config_handles_enum_first_value() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["service"],
            "properties": {
                "service": {
                    "type": "object",
                    "required": ["type"],
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["grpc", "http"]
                        }
                    }
                }
            }
        });
        let result = build_config_from_schema(&Some(schema));
        let svc = result.get("service").expect("service should exist");
        assert_eq!(
            svc.get("type").and_then(|v| v.as_str()),
            Some("grpc"),
            "Should use first enum value as placeholder"
        );
    }

    #[test]
    fn build_config_handles_additional_properties() {
        // Simulates jwt_auth providers (additionalProperties map)
        let schema = serde_json::json!({
            "type": "object",
            "required": ["providers"],
            "properties": {
                "providers": {
                    "type": "object",
                    "additionalProperties": {
                        "type": "object",
                        "required": ["jwks"],
                        "properties": {
                            "issuer": {"type": "string"},
                            "jwks": {
                                "type": "object",
                                "properties": {
                                    "type": {"type": "string", "default": "remote"}
                                }
                            }
                        }
                    }
                }
            }
        });
        let result = build_config_from_schema(&Some(schema));
        let providers = result.get("providers").expect("providers should exist");
        assert!(
            providers.as_object().map(|m| !m.is_empty()).unwrap_or(false),
            "providers should have at least one example entry"
        );
    }

    #[test]
    fn build_config_local_rate_limit_includes_required_fields() {
        // Exact local_rate_limit schema structure
        let schema = serde_json::json!({
            "type": "object",
            "required": ["stat_prefix", "token_bucket"],
            "properties": {
                "stat_prefix": {
                    "type": "string",
                    "minLength": 1
                },
                "token_bucket": {
                    "type": "object",
                    "required": ["max_tokens", "fill_interval_ms"],
                    "properties": {
                        "max_tokens": {"type": "integer", "default": 100},
                        "tokens_per_fill": {"type": "integer"},
                        "fill_interval_ms": {"type": "integer", "default": 1000}
                    }
                },
                "filter_enabled": {
                    "type": "object",
                    "required": ["numerator"],
                    "properties": {
                        "numerator": {"type": "integer", "default": 100},
                        "denominator": {"type": "string", "enum": ["hundred", "ten_thousand", "million"], "default": "hundred"}
                    }
                },
                "filter_enforced": {
                    "type": "object",
                    "required": ["numerator"],
                    "properties": {
                        "numerator": {"type": "integer", "default": 100},
                        "denominator": {"type": "string", "enum": ["hundred", "ten_thousand", "million"], "default": "hundred"}
                    }
                }
            }
        });
        let result = build_config_from_schema(&Some(schema));

        // Required top-level fields must be present
        assert!(result.get("stat_prefix").is_some(), "stat_prefix must be present");
        assert!(result.get("token_bucket").is_some(), "token_bucket must be present");

        // token_bucket must include required sub-fields with defaults
        let tb = result.get("token_bucket").unwrap();
        assert_eq!(tb.get("max_tokens"), Some(&serde_json::json!(100)));
        assert_eq!(tb.get("fill_interval_ms"), Some(&serde_json::json!(1000)));

        // filter_enabled/enforced must include numerator when present (has required children)
        let fe = result.get("filter_enabled").expect("filter_enabled should be present");
        assert_eq!(
            fe.get("numerator"),
            Some(&serde_json::json!(100)),
            "filter_enabled.numerator must be populated"
        );
        let fenf = result.get("filter_enforced").expect("filter_enforced should be present");
        assert_eq!(
            fenf.get("numerator"),
            Some(&serde_json::json!(100)),
            "filter_enforced.numerator must be populated"
        );
    }

    #[test]
    fn build_config_compressor_includes_library_type() {
        // Exact compressor schema structure
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "response_direction_config": {
                    "type": "object",
                    "properties": {
                        "common_config": {
                            "type": "object",
                            "properties": {
                                "min_content_length": {"type": "integer", "default": 30}
                            }
                        }
                    }
                },
                "compressor_library": {
                    "type": "object",
                    "required": ["type"],
                    "properties": {
                        "type": {"type": "string", "enum": ["gzip"], "default": "gzip"},
                        "memory_level": {"type": "integer", "default": 5},
                        "compression_level": {"type": "string", "default": "best_speed"}
                    }
                }
            }
        });
        let result = build_config_from_schema(&Some(schema));

        let lib = result.get("compressor_library").expect("compressor_library must be present");
        assert_eq!(
            lib.get("type").and_then(|v| v.as_str()),
            Some("gzip"),
            "compressor_library.type must be 'gzip'. Got: {:?}",
            lib
        );
    }

    #[test]
    fn build_config_ext_authz_includes_service_type() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["service"],
            "properties": {
                "service": {
                    "type": "object",
                    "required": ["type"],
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["grpc", "http"]
                        },
                        "target_uri": {"type": "string"},
                        "timeout_ms": {"type": "integer", "default": 200}
                    }
                },
                "failure_mode_allow": {"type": "boolean", "default": false}
            }
        });
        let result = build_config_from_schema(&Some(schema));

        let service = result.get("service").expect("service must be present");
        assert_eq!(
            service.get("type").and_then(|v| v.as_str()),
            Some("grpc"),
            "service.type must use first enum value 'grpc'. Got: {:?}",
            service
        );
    }

    #[test]
    fn build_config_jwt_auth_includes_providers() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["providers"],
            "properties": {
                "providers": {
                    "type": "object",
                    "additionalProperties": {
                        "type": "object",
                        "required": ["jwks"],
                        "properties": {
                            "issuer": {"type": "string"},
                            "jwks": {
                                "type": "object",
                                "properties": {
                                    "type": {"type": "string", "default": "remote"}
                                }
                            }
                        }
                    }
                }
            }
        });
        let result = build_config_from_schema(&Some(schema));

        let providers = result.get("providers").expect("providers must be present");
        let providers_map = providers.as_object().expect("providers must be an object");
        assert!(!providers_map.is_empty(), "providers must have at least one example entry");
        // The example entry should have a jwks sub-object
        let first_provider = providers_map.values().next().unwrap();
        assert!(
            first_provider.get("jwks").is_some(),
            "Provider entry must include required jwks. Got: {:?}",
            first_provider
        );
    }

    #[test]
    fn build_config_oauth2_includes_required_fields() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["token_endpoint", "authorization_endpoint", "credentials", "redirect_uri"],
            "properties": {
                "token_endpoint": {
                    "type": "object",
                    "required": ["uri", "cluster"],
                    "properties": {
                        "uri": {"type": "string"},
                        "cluster": {"type": "string"},
                        "timeout_ms": {"type": "integer", "default": 5000}
                    }
                },
                "authorization_endpoint": {"type": "string"},
                "credentials": {
                    "type": "object",
                    "required": ["client_id"],
                    "properties": {
                        "client_id": {"type": "string"},
                        "token_secret": {
                            "type": "object",
                            "required": ["name"],
                            "properties": {
                                "name": {"type": "string"}
                            }
                        }
                    }
                },
                "redirect_uri": {"type": "string"}
            }
        });
        let result = build_config_from_schema(&Some(schema));

        assert!(result.get("token_endpoint").is_some(), "token_endpoint must be present");
        assert!(
            result.get("authorization_endpoint").is_some(),
            "authorization_endpoint must be present"
        );
        assert!(result.get("redirect_uri").is_some(), "redirect_uri must be present");

        let creds = result.get("credentials").expect("credentials must be present");
        assert!(
            creds.get("client_id").is_some(),
            "credentials.client_id must be present. Got: {:?}",
            creds
        );

        let token_ep = result.get("token_endpoint").unwrap();
        assert!(
            token_ep.get("uri").is_some(),
            "token_endpoint.uri must be present. Got: {:?}",
            token_ep
        );
        assert!(
            token_ep.get("cluster").is_some(),
            "token_endpoint.cluster must be present. Got: {:?}",
            token_ep
        );

        // token_secret has required: [name], so should be included
        let token_secret = creds.get("token_secret").expect("token_secret should be present");
        assert!(
            token_secret.get("name").is_some(),
            "token_secret.name must be present. Got: {:?}",
            token_secret
        );
    }
}
