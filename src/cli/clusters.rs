//! Cluster CLI commands
//!
//! Provides command-line interface for managing cluster configurations

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::FlowplaneClient;
use super::config_file;
use super::output::{print_output, truncate};
use crate::api::handlers::PaginatedResponse;

#[derive(Subcommand)]
pub enum ClusterCommands {
    /// Create a new Envoy cluster configuration
    #[command(
        long_about = "Create a new cluster configuration that defines upstream service endpoints.\n\nClusters specify how to connect to backend services, including endpoints, load balancing, and health checking.",
        after_help = "EXAMPLES:\n    # Create a cluster from JSON file\n    flowplane-cli cluster create --file cluster-spec.json\n\n    # Create with YAML output\n    flowplane-cli cluster create --file cluster.json --output yaml\n\n    # With authentication\n    flowplane-cli cluster create --file cluster.json --token your-token"
    )]
    Create {
        /// Path to YAML or JSON file with resource spec
        #[arg(short, long, value_name = "FILE", help = config_file::FILE_ARG_HELP)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// List all cluster configurations
    #[command(
        long_about = "List all cluster configurations in the system with optional filtering and pagination.",
        after_help = "EXAMPLES:\n    # List all clusters\n    flowplane-cli cluster list\n\n    # List with table output\n    flowplane-cli cluster list --output table\n\n    # Filter by service name\n    flowplane-cli cluster list --service backend-api\n\n    # Paginate results\n    flowplane-cli cluster list --limit 10 --offset 20"
    )]
    List {
        /// Filter by service name
        #[arg(long, value_name = "SERVICE")]
        service: Option<String>,

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

    /// Get details of a specific cluster by name
    #[command(
        long_about = "Retrieve detailed information about a specific cluster configuration.",
        after_help = "EXAMPLES:\n    # Get cluster details\n    flowplane-cli cluster get my-backend-cluster\n\n    # Get with YAML output\n    flowplane-cli cluster get my-cluster --output yaml\n\n    # Get with table output\n    flowplane-cli cluster get my-cluster --output table"
    )]
    Get {
        /// Cluster name
        #[arg(value_name = "NAME")]
        name: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Update an existing cluster configuration
    #[command(
        long_about = "Update an existing cluster configuration with new settings.\n\nProvide a JSON file with the updated configuration fields.",
        after_help = "EXAMPLES:\n    # Update a cluster\n    flowplane-cli cluster update my-cluster --file updated-cluster.json\n\n    # Update with YAML output\n    flowplane-cli cluster update my-cluster --file update.json --output yaml"
    )]
    Update {
        /// Cluster name
        #[arg(value_name = "NAME")]
        name: String,

        /// Path to YAML or JSON file with resource spec
        #[arg(short, long, value_name = "FILE", help = config_file::FILE_ARG_HELP)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Delete a cluster configuration
    #[command(
        long_about = "Delete a specific cluster configuration from the system.\n\nWARNING: This will remove the cluster and may affect routing if it's in use.",
        after_help = "EXAMPLES:\n    # Delete a cluster (with confirmation prompt)\n    flowplane-cli cluster delete my-cluster\n\n    # Delete without confirmation\n    flowplane-cli cluster delete my-cluster --yes"
    )]
    Delete {
        /// Cluster name
        #[arg(value_name = "NAME")]
        name: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Generate a template cluster manifest
    Scaffold {
        /// Output format (yaml or json)
        #[arg(short, long, default_value = "yaml", value_parser = ["json", "yaml"])]
        output: String,
    },
}

/// Cluster response structure matching API response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterResponse {
    pub name: String,
    pub team: String,
    pub service_name: String,
    #[serde(default)]
    pub version: Option<i64>,
    #[serde(default)]
    pub import_id: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

/// Handle cluster commands
pub async fn handle_cluster_command(
    command: ClusterCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        ClusterCommands::Create { file, output } => {
            create_cluster(client, team, file, &output).await?
        }
        ClusterCommands::List { service, limit, offset, output } => {
            list_clusters(client, team, service, limit, offset, &output).await?
        }
        ClusterCommands::Get { name, output } => get_cluster(client, team, &name, &output).await?,
        ClusterCommands::Update { name, file, output } => {
            update_cluster(client, team, &name, file, &output).await?
        }
        ClusterCommands::Delete { name, yes } => delete_cluster(client, team, &name, yes).await?,
        ClusterCommands::Scaffold { output } => scaffold_cluster(&output)?,
    }

    Ok(())
}

async fn create_cluster(
    client: &FlowplaneClient,
    team: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let mut body = config_file::load_config_file(&file)?;
    config_file::strip_kind_field(&mut body);

    let path = format!("/api/v1/teams/{team}/clusters");
    let response: ClusterResponse = client.post_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn list_clusters(
    client: &FlowplaneClient,
    team: &str,
    service: Option<String>,
    limit: Option<i32>,
    offset: Option<i32>,
    output: &str,
) -> Result<()> {
    let mut path = format!("/api/v1/teams/{team}/clusters?");
    let mut params = Vec::new();

    if let Some(ref s) = service {
        params.push(format!("service={}", s));
    }
    if let Some(l) = limit {
        params.push(format!("limit={}", l));
    }
    if let Some(o) = offset {
        params.push(format!("offset={}", o));
    }

    path.push_str(&params.join("&"));

    let response: PaginatedResponse<ClusterResponse> = client.get_json(&path).await?;

    // Client-side filtering by service name (API doesn't support this filter yet)
    let items: Vec<ClusterResponse> = if let Some(ref svc) = service {
        response.items.into_iter().filter(|c| c.service_name.eq_ignore_ascii_case(svc)).collect()
    } else {
        response.items
    };

    if output == "table" {
        print_clusters_table(&items);
    } else {
        print_output(&items, output)?;
    }

    Ok(())
}

async fn get_cluster(client: &FlowplaneClient, team: &str, name: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/clusters/{name}");
    let response: ClusterResponse = client.get_json(&path).await?;

    if output == "table" {
        print_clusters_table(&[response]);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn update_cluster(
    client: &FlowplaneClient,
    team: &str,
    name: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let mut body = config_file::load_config_file(&file)?;
    config_file::strip_kind_field(&mut body);

    let path = format!("/api/v1/teams/{team}/clusters/{name}");
    let response: ClusterResponse = client.put_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn delete_cluster(client: &FlowplaneClient, team: &str, name: &str, yes: bool) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete cluster '{}'? (y/N)", name);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/teams/{team}/clusters/{name}");
    client.delete_no_content(&path).await?;

    println!("Cluster '{}' deleted successfully", name);
    Ok(())
}

fn scaffold_cluster_to_writer(output: &str, writer: &mut impl std::io::Write) -> Result<()> {
    if output == "json" {
        let scaffold = serde_json::json!({
            "kind": "Cluster",
            "name": "<your-cluster-name>",
            "serviceName": "<your-service-name>",
            "endpoints": [
                {
                    "host": "<host-or-ip>",
                    "port": 8080
                }
            ],
            "connectTimeoutSeconds": 5,
            "useTls": false,
            "tlsServerName": "",
            "dnsLookupFamily": "AUTO",
            "lbPolicy": "ROUND_ROBIN",
            "protocolType": "",
            "healthChecks": [],
            "circuitBreakers": {
                "default": {
                    "maxConnections": 100,
                    "maxPendingRequests": 50,
                    "maxRequests": 200,
                    "maxRetries": 3
                },
                "high": {
                    "maxConnections": 200,
                    "maxPendingRequests": 100,
                    "maxRequests": 400,
                    "maxRetries": 5
                }
            },
            "outlierDetection": {
                "consecutive5xx": 5,
                "intervalSeconds": 30,
                "baseEjectionTimeSeconds": 30,
                "maxEjectionPercent": 50,
                "minHosts": 3
            }
        });
        let json =
            serde_json::to_string_pretty(&scaffold).context("Failed to serialize scaffold")?;
        writeln!(writer, "{json}")?;
    } else {
        writeln!(writer, "# Cluster scaffold")?;
        writeln!(writer, "#")?;
        writeln!(writer, "# Use with: flowplane cluster create -f <file>")?;
        writeln!(writer, "#       or: flowplane apply -f <file>")?;
        writeln!(writer)?;
        writeln!(writer, "kind: Cluster")?;
        writeln!(writer)?;
        writeln!(writer, "# [REQUIRED] Unique name for the cluster")?;
        writeln!(writer, "name: \"<your-cluster-name>\"")?;
        writeln!(writer)?;
        writeln!(writer, "# [OPTIONAL] Service identifier exposed to clients (defaults to name)")?;
        writeln!(writer, "serviceName: \"<your-service-name>\"")?;
        writeln!(writer)?;
        writeln!(writer, "# [REQUIRED] Upstream endpoints (at least one)")?;
        writeln!(writer, "endpoints:")?;
        writeln!(writer, "  - host: \"<host-or-ip>\"")?;
        writeln!(writer, "    port: 8080")?;
        writeln!(writer)?;
        writeln!(writer, "# [OPTIONAL] Connection timeout in seconds (default: 5)")?;
        writeln!(writer, "connectTimeoutSeconds: 5")?;
        writeln!(writer)?;
        writeln!(writer, "# [OPTIONAL] Enable TLS for upstream connections (default: false)")?;
        writeln!(writer, "useTls: false")?;
        writeln!(writer)?;
        writeln!(writer, "# [OPTIONAL] SNI server name for TLS handshake")?;
        writeln!(writer, "# tlsServerName: \"service.example.com\"")?;
        writeln!(writer)?;
        writeln!(
            writer,
            "# [OPTIONAL] DNS lookup family: AUTO, V4_ONLY, V6_ONLY, V4_PREFERRED, ALL"
        )?;
        writeln!(writer, "# dnsLookupFamily: AUTO")?;
        writeln!(writer)?;
        writeln!(writer, "# [OPTIONAL] Load balancing policy: ROUND_ROBIN, LEAST_REQUEST, RANDOM, RING_HASH, MAGLEV")?;
        writeln!(writer, "lbPolicy: ROUND_ROBIN")?;
        writeln!(writer)?;
        writeln!(
            writer,
            "# [OPTIONAL] Protocol type: HTTP2, GRPC (defaults to HTTP/1.1 if not set)"
        )?;
        writeln!(writer, "# protocolType: GRPC")?;
        writeln!(writer)?;
        writeln!(writer, "# [OPTIONAL] Active health checks")?;
        writeln!(writer, "# healthChecks:")?;
        writeln!(writer, "#   - type: http")?;
        writeln!(writer, "#     path: /health")?;
        writeln!(writer, "#     method: GET")?;
        writeln!(writer, "#     intervalSeconds: 10")?;
        writeln!(writer, "#     timeoutSeconds: 5")?;
        writeln!(writer, "#     healthyThreshold: 2")?;
        writeln!(writer, "#     unhealthyThreshold: 3")?;
        writeln!(writer, "#     expectedStatuses:")?;
        writeln!(writer, "#       - 200")?;
        writeln!(writer, "#       - 204")?;
        writeln!(writer)?;
        writeln!(writer, "# [OPTIONAL] Circuit breaker thresholds")?;
        writeln!(writer, "# circuitBreakers:")?;
        writeln!(writer, "#   default:")?;
        writeln!(writer, "#     maxConnections: 100")?;
        writeln!(writer, "#     maxPendingRequests: 50")?;
        writeln!(writer, "#     maxRequests: 200")?;
        writeln!(writer, "#     maxRetries: 3")?;
        writeln!(writer, "#   high:")?;
        writeln!(writer, "#     maxConnections: 200")?;
        writeln!(writer, "#     maxPendingRequests: 100")?;
        writeln!(writer, "#     maxRequests: 400")?;
        writeln!(writer, "#     maxRetries: 5")?;
        writeln!(writer)?;
        writeln!(writer, "# [OPTIONAL] Passive outlier detection")?;
        writeln!(writer, "# outlierDetection:")?;
        writeln!(writer, "#   consecutive5xx: 5")?;
        writeln!(writer, "#   intervalSeconds: 30")?;
        writeln!(writer, "#   baseEjectionTimeSeconds: 30")?;
        writeln!(writer, "#   maxEjectionPercent: 50")?;
        writeln!(writer, "#   minHosts: 3")?;
    }
    Ok(())
}

fn scaffold_cluster(output: &str) -> Result<()> {
    let mut stdout = std::io::stdout();
    scaffold_cluster_to_writer(output, &mut stdout)
}

fn print_clusters_table(clusters: &[ClusterResponse]) {
    if clusters.is_empty() {
        println!("No clusters found");
        return;
    }

    println!();
    println!("{:<35} {:<20} {:<35}", "Name", "Team", "Service");
    println!("{}", "-".repeat(95));

    for cluster in clusters {
        println!(
            "{:<35} {:<20} {:<35}",
            truncate(&cluster.name, 33),
            truncate(&cluster.team, 18),
            truncate(&cluster.service_name, 33),
        );
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaffold_yaml() -> String {
        let mut buf = Vec::new();
        scaffold_cluster_to_writer("yaml", &mut buf).expect("scaffold yaml");
        String::from_utf8(buf).expect("valid utf8")
    }

    fn scaffold_json() -> String {
        let mut buf = Vec::new();
        scaffold_cluster_to_writer("json", &mut buf).expect("scaffold json");
        String::from_utf8(buf).expect("valid utf8")
    }

    #[test]
    fn yaml_contains_all_fields_and_annotations() {
        let yaml = scaffold_yaml();

        // Required fields
        assert!(yaml.contains("[REQUIRED]"), "missing [REQUIRED] annotation");
        assert!(yaml.contains("[OPTIONAL]"), "missing [OPTIONAL] annotation");

        // All field names must be present (camelCase)
        for field in [
            "name:",
            "serviceName:",
            "endpoints:",
            "connectTimeoutSeconds:",
            "useTls:",
            "tlsServerName:",
            "dnsLookupFamily:",
            "lbPolicy:",
            "protocolType:",
            "healthChecks:",
            "circuitBreakers:",
            "outlierDetection:",
        ] {
            assert!(yaml.contains(field), "missing field: {field}");
        }

        // Health check sub-fields
        for field in
            ["intervalSeconds:", "timeoutSeconds:", "healthyThreshold:", "unhealthyThreshold:"]
        {
            assert!(yaml.contains(field), "missing health check field: {field}");
        }

        // Circuit breaker sub-fields
        for field in ["maxConnections:", "maxPendingRequests:", "maxRequests:", "maxRetries:"] {
            assert!(yaml.contains(field), "missing circuit breaker field: {field}");
        }

        // Outlier detection sub-fields
        for field in
            ["consecutive5xx:", "baseEjectionTimeSeconds:", "maxEjectionPercent:", "minHosts:"]
        {
            assert!(yaml.contains(field), "missing outlier detection field: {field}");
        }
    }

    #[test]
    fn yaml_field_names_are_camel_case() {
        let yaml = scaffold_yaml();

        // These snake_case variants must NOT appear
        assert!(
            !yaml.contains("connect_timeout_seconds"),
            "found snake_case: connect_timeout_seconds"
        );
        assert!(!yaml.contains("use_tls"), "found snake_case: use_tls");
        assert!(!yaml.contains("tls_server_name"), "found snake_case: tls_server_name");
        assert!(!yaml.contains("dns_lookup_family"), "found snake_case: dns_lookup_family");
        assert!(!yaml.contains("lb_policy"), "found snake_case: lb_policy");
        assert!(!yaml.contains("protocol_type"), "found snake_case: protocol_type");
        assert!(!yaml.contains("health_checks"), "found snake_case: health_checks");
        assert!(!yaml.contains("circuit_breakers"), "found snake_case: circuit_breakers");
        assert!(!yaml.contains("outlier_detection"), "found snake_case: outlier_detection");
        assert!(!yaml.contains("service_name"), "found snake_case: service_name");
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

        // Verify structure
        assert!(parsed.get("kind").is_some(), "missing 'kind' key");
        assert_eq!(parsed["kind"], "Cluster");
        assert!(parsed.get("name").is_some(), "missing 'name' key");
        assert!(parsed.get("endpoints").is_some(), "missing 'endpoints' key");
        assert!(parsed["endpoints"].is_array(), "'endpoints' should be an array");
    }

    #[test]
    fn json_output_is_valid_json_with_all_keys() {
        let json_str = scaffold_json();
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("scaffold JSON should parse");

        for key in [
            "kind",
            "name",
            "serviceName",
            "endpoints",
            "connectTimeoutSeconds",
            "useTls",
            "tlsServerName",
            "dnsLookupFamily",
            "lbPolicy",
            "protocolType",
            "healthChecks",
            "circuitBreakers",
            "outlierDetection",
        ] {
            assert!(parsed.get(key).is_some(), "missing key in JSON: {key}");
        }

        // healthChecks is an empty array in JSON
        assert!(parsed["healthChecks"].is_array(), "healthChecks should be an array");
        assert_eq!(parsed["healthChecks"].as_array().map(|a| a.len()), Some(0));
    }
}
