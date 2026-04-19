//! Listener CLI commands
//!
//! Provides command-line interface for managing listener configurations

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
pub enum ListenerCommands {
    /// Create a new Envoy listener configuration
    #[command(
        long_about = "Create a new listener by providing a JSON file with the listener specification.\n\nListeners define how Envoy accepts incoming connections, including address, port, protocol, and filter chains.",
        after_help = "EXAMPLES:\n    # Create a listener from a JSON file\n    flowplane listener create --file listener-spec.json\n\n    # Create and output as YAML\n    flowplane listener create --file listener-spec.json --output yaml\n\n    # With authentication\n    flowplane listener create --file listener-spec.json --token your-token"
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
        after_help = "EXAMPLES:\n    # List all listeners\n    flowplane listener list\n\n    # List with table output\n    flowplane listener list --output table\n\n    # Filter by protocol\n    flowplane listener list --protocol http\n\n    # Paginate results\n    flowplane listener list --limit 10 --offset 20"
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
        after_help = "EXAMPLES:\n    # Get listener details in JSON format\n    flowplane listener get http-listener\n\n    # Get listener in YAML format\n    flowplane listener get http-listener --output yaml\n\n    # With authentication\n    flowplane listener get http-listener --token your-token --base-url https://api.example.com"
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
        after_help = "EXAMPLES:\n    # Update a listener from JSON file\n    flowplane listener update http-listener --file updated-listener.json\n\n    # Update and output as YAML\n    flowplane listener update http-listener --file updated-listener.json --output yaml\n\n    # With authentication\n    flowplane listener update http-listener --file updated-listener.json --token your-token"
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
        after_help = "EXAMPLES:\n    # Delete a listener (with confirmation)\n    flowplane listener delete http-listener\n\n    # Delete without confirmation prompt\n    flowplane listener delete http-listener --yes\n\n    # With authentication\n    flowplane listener delete http-listener --token your-token"
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

fn scaffold_listener_to_writer(output: &str, writer: &mut impl Write) -> Result<()> {
    if output == "json" {
        let scaffold = serde_json::json!({
            "kind": "Listener",
            "name": "<your-listener-name>",
            "address": "0.0.0.0",
            "port": 10001,
            "dataplaneId": "<your-dataplane-id>",
            "protocol": "TCP",
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
                            ],
                            "accessLog": {
                                "path": "/dev/stdout"
                            }
                        }
                    ]
                }
            ]
        });
        let json =
            serde_json::to_string_pretty(&scaffold).context("Failed to serialize scaffold")?;
        writeln!(writer, "{json}")?;
    } else {
        writeln!(writer, "# Listener scaffold")?;
        writeln!(writer, "#")?;
        writeln!(writer, "# Use with: flowplane listener create -f <file>")?;
        writeln!(writer, "#       or: flowplane apply -f <file>")?;
        writeln!(writer)?;
        writeln!(writer, "kind: Listener")?;
        writeln!(writer)?;
        writeln!(writer, "# [REQUIRED] Unique name for the listener")?;
        writeln!(writer, "name: \"<your-listener-name>\"")?;
        writeln!(writer)?;
        writeln!(writer, "# [REQUIRED] Bind address (0.0.0.0 for all interfaces)")?;
        writeln!(writer, "address: \"0.0.0.0\"")?;
        writeln!(writer)?;
        writeln!(writer, "# [REQUIRED] Port to listen on (10000-10020 for Envoy)")?;
        writeln!(writer, "port: 10001")?;
        writeln!(writer)?;
        writeln!(
            writer,
            "# [REQUIRED] Dataplane ID (run 'flowplane dataplane list' to find yours)"
        )?;
        writeln!(writer, "dataplaneId: \"<your-dataplane-id>\"")?;
        writeln!(writer)?;
        writeln!(writer, "# [OPTIONAL] Protocol: TCP, UDP (default: TCP)")?;
        writeln!(writer, "# protocol: TCP")?;
        writeln!(writer)?;
        writeln!(
            writer,
            "# [REQUIRED] Filter chains (at least one with an HTTP connection manager or TCP proxy)"
        )?;
        writeln!(writer, "filterChains:")?;
        writeln!(writer, "  - name: \"default\"")?;
        writeln!(writer, "    filters:")?;
        writeln!(writer, "      - name: \"envoy.filters.network.http_connection_manager\"")?;
        writeln!(writer, "        # Filter type: httpConnectionManager or tcpProxy")?;
        writeln!(writer, "        type: httpConnectionManager")?;
        writeln!(writer)?;
        writeln!(writer, "        # [REQUIRED for HCM] Route config to use")?;
        writeln!(writer, "        routeConfigName: \"<your-route-config-name>\"")?;
        writeln!(writer)?;
        writeln!(
            writer,
            "        # [OPTIONAL] Inline route config (alternative to routeConfigName)"
        )?;
        writeln!(writer, "        # inlineRouteConfig: {{}}")?;
        writeln!(writer)?;
        writeln!(writer, "        # [OPTIONAL] HTTP filters in the connection manager")?;
        writeln!(writer, "        httpFilters:")?;
        writeln!(writer, "          - filter:")?;
        writeln!(writer, "              type: router")?;
        writeln!(writer)?;
        writeln!(writer, "        # [OPTIONAL] Access logging")?;
        writeln!(writer, "        # accessLog:")?;
        writeln!(writer, "        #   path: \"/dev/stdout\"")?;
        writeln!(
            writer,
            "        #   format: \"%START_TIME% %REQ(:METHOD)% %REQ(X-ENVOY-ORIGINAL-PATH?:PATH)% %PROTOCOL% %RESPONSE_CODE%\""
        )?;
        writeln!(writer)?;
        writeln!(writer, "        # [OPTIONAL] Distributed tracing")?;
        writeln!(writer, "        # tracing:")?;
        writeln!(writer, "        #   provider:")?;
        writeln!(writer, "        #     type: open_telemetry")?;
        writeln!(writer, "        #     service_name: \"my-service\"")?;
        writeln!(writer, "        #     grpc_cluster: \"otel-collector\"")?;
        writeln!(writer, "        #   randomSamplingPercentage: 100.0")?;
        writeln!(writer, "        #   spawnUpstreamSpan: true")?;
        writeln!(writer, "        #   customTags:")?;
        writeln!(writer, "        #     environment: \"production\"")?;
        writeln!(writer)?;
        writeln!(writer, "    # [OPTIONAL] TLS termination")?;
        writeln!(writer, "    # tlsContext:")?;
        writeln!(writer, "    #   certChainFile: \"/etc/certs/server.crt\"")?;
        writeln!(writer, "    #   privateKeyFile: \"/etc/certs/server.key\"")?;
        writeln!(writer, "    #   caCertFile: \"/etc/certs/ca.crt\"")?;
        writeln!(writer, "    #   requireClientCertificate: false")?;
        writeln!(writer)?;
        writeln!(writer, "# --- Alternative: TCP proxy filter ---")?;
        writeln!(writer, "# filters:")?;
        writeln!(writer, "#   - name: \"envoy.filters.network.tcp_proxy\"")?;
        writeln!(writer, "#     type: tcpProxy")?;
        writeln!(writer, "#     cluster: \"<your-cluster-name>\"")?;
        writeln!(writer, "#     accessLog:")?;
        writeln!(writer, "#       path: \"/dev/stdout\"")?;
    }
    Ok(())
}

fn scaffold_listener(output: &str) -> Result<()> {
    let mut stdout = std::io::stdout();
    scaffold_listener_to_writer(output, &mut stdout)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scaffold_yaml() -> String {
        let mut buf = Vec::new();
        scaffold_listener_to_writer("yaml", &mut buf).expect("scaffold yaml");
        String::from_utf8(buf).expect("valid utf8")
    }

    fn scaffold_json() -> String {
        let mut buf = Vec::new();
        scaffold_listener_to_writer("json", &mut buf).expect("scaffold json");
        String::from_utf8(buf).expect("valid utf8")
    }

    #[test]
    fn yaml_kind_is_listener() {
        let yaml = scaffold_yaml();
        assert!(yaml.contains("kind: Listener"), "missing kind: Listener");
    }

    #[test]
    fn json_kind_is_listener() {
        let json = scaffold_json();
        assert!(json.contains("\"kind\": \"Listener\""), "missing kind: Listener in JSON");
    }

    #[test]
    fn yaml_contains_all_fields_and_annotations() {
        let yaml = scaffold_yaml();

        assert!(yaml.contains("[REQUIRED]"), "missing [REQUIRED] annotation");
        assert!(yaml.contains("[OPTIONAL]"), "missing [OPTIONAL] annotation");

        // All camelCase field names must be present
        for field in [
            "name:",
            "address:",
            "port:",
            "dataplaneId:",
            "filterChains:",
            "routeConfigName:",
            "httpFilters:",
            "accessLog:",
            "tracing:",
            "randomSamplingPercentage:",
            "spawnUpstreamSpan:",
            "customTags:",
            "tlsContext:",
            "certChainFile:",
            "privateKeyFile:",
            "caCertFile:",
            "requireClientCertificate:",
            "tcpProxy",
            "httpConnectionManager",
            "protocol:",
            "inlineRouteConfig:",
        ] {
            assert!(yaml.contains(field), "missing field: {field}");
        }
    }

    #[test]
    fn yaml_field_names_are_camel_case() {
        let yaml = scaffold_yaml();

        // These snake_case variants must NOT appear
        assert!(!yaml.contains("dataplane_id"), "found snake_case: dataplane_id");
        assert!(!yaml.contains("filter_chains"), "found snake_case: filter_chains");
        assert!(!yaml.contains("route_config_name"), "found snake_case: route_config_name");
        assert!(!yaml.contains("http_filters"), "found snake_case: http_filters");
        assert!(!yaml.contains("access_log"), "found snake_case: access_log");
        assert!(
            !yaml.contains("random_sampling_percentage"),
            "found snake_case: random_sampling_percentage"
        );
        assert!(!yaml.contains("spawn_upstream_span"), "found snake_case: spawn_upstream_span");
        assert!(!yaml.contains("custom_tags"), "found snake_case: custom_tags");
        assert!(!yaml.contains("tls_context"), "found snake_case: tls_context");
        assert!(!yaml.contains("cert_chain_file"), "found snake_case: cert_chain_file");
        assert!(!yaml.contains("private_key_file"), "found snake_case: private_key_file");
        assert!(!yaml.contains("ca_cert_file"), "found snake_case: ca_cert_file");
        assert!(
            !yaml.contains("require_client_certificate"),
            "found snake_case: require_client_certificate"
        );
        assert!(!yaml.contains("inline_route_config"), "found snake_case: inline_route_config");
    }

    #[test]
    fn yaml_uncommented_lines_are_parseable() {
        let yaml = scaffold_yaml();

        // Extract only non-comment, non-empty lines
        let uncommented: String = yaml
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.is_empty() && !trimmed.starts_with('#')
            })
            .map(|line| format!("{line}\n"))
            .collect();

        let parsed: serde_json::Value =
            serde_yaml::from_str(&uncommented).expect("uncommented YAML should parse");

        // Verify top-level keys
        assert_eq!(parsed["kind"], "Listener");
        assert!(parsed.get("name").is_some(), "missing 'name' key");
        assert!(parsed.get("address").is_some(), "missing 'address' key");
        assert!(parsed.get("port").is_some(), "missing 'port' key");
        assert!(parsed.get("dataplaneId").is_some(), "missing 'dataplaneId' key");
        assert!(parsed.get("filterChains").is_some(), "missing 'filterChains' key");

        // Verify filterChains[0].filters[0] has name and type keys
        let chains = parsed["filterChains"].as_array().expect("filterChains should be array");
        assert!(!chains.is_empty(), "filterChains should not be empty");
        let filters = chains[0]["filters"].as_array().expect("filters should be array");
        assert!(!filters.is_empty(), "filters should not be empty");
        assert!(filters[0].get("name").is_some(), "filter missing 'name' key");
        assert!(filters[0].get("type").is_some(), "filter missing 'type' key");
    }

    #[test]
    fn json_output_is_valid_json_with_all_keys() {
        let json_str = scaffold_json();
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("scaffold JSON should parse");

        for key in ["kind", "name", "address", "port", "dataplaneId", "protocol", "filterChains"] {
            assert!(parsed.get(key).is_some(), "missing key in JSON: {key}");
        }

        // filterChains structure
        let chains = parsed["filterChains"].as_array().expect("filterChains should be array");
        assert!(!chains.is_empty(), "filterChains should not be empty");

        let filter = &chains[0]["filters"][0];
        assert!(filter.get("name").is_some(), "filter missing 'name' key");
        assert!(filter.get("type").is_some(), "filter missing 'type' key");
        assert!(filter.get("routeConfigName").is_some(), "filter missing 'routeConfigName'");
        assert!(filter.get("httpFilters").is_some(), "filter missing 'httpFilters'");
        assert!(filter.get("accessLog").is_some(), "filter missing 'accessLog'");

        // Tracing and tlsContext are omitted from JSON scaffold (documented in YAML
        // comments only). Tracing references external clusters; tlsContext with null
        // cert paths causes server errors.
    }
}
