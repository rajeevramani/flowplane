//! Route CLI commands
//!
//! Provides command-line interface for managing route configurations

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

use super::client::FlowplaneClient;
use super::config_file;
use crate::api::handlers::PaginatedResponse;

#[derive(Subcommand)]
pub enum RouteCommands {
    /// Create a new route configuration
    #[command(
        long_about = "Create a new route by providing a JSON file with the route specification.\n\nThe JSON file should contain fields like name, path_prefix, cluster_name, and optional match conditions.",
        after_help = "EXAMPLES:\n    # Create a route from a JSON file\n    flowplane-cli route create --file route-spec.json\n\n    # Create and output as YAML\n    flowplane-cli route create --file route-spec.json --output yaml\n\n    # With authentication\n    flowplane-cli route create --file route-spec.json --token your-token"
    )]
    Create {
        /// Path to YAML or JSON file with resource spec
        #[arg(short, long, value_name = "FILE", help = config_file::FILE_ARG_HELP)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// List all route configurations
    #[command(
        long_about = "List all route configurations in the system with optional filtering and pagination.\n\nRoutes define path matching and routing rules for traffic to clusters.",
        after_help = "EXAMPLES:\n    # List all routes\n    flowplane-cli route list\n\n    # List with table output\n    flowplane-cli route list --output table\n\n    # Filter by cluster name\n    flowplane-cli route list --cluster backend-api\n\n    # Paginate results\n    flowplane-cli route list --limit 10 --offset 20"
    )]
    List {
        /// Filter by cluster name
        #[arg(long, value_name = "CLUSTER")]
        cluster: Option<String>,

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

    /// Get details of a specific route by name
    #[command(
        long_about = "Retrieve detailed information about a specific route configuration by its name.\n\nShows path matching rules, cluster association, and metadata.",
        after_help = "EXAMPLES:\n    # Get route details in JSON format\n    flowplane-cli route get my-api-route\n\n    # Get route in YAML format\n    flowplane-cli route get my-api-route --output yaml\n\n    # With authentication\n    flowplane-cli route get my-api-route --token your-token --base-url https://api.example.com"
    )]
    Get {
        /// Route name
        #[arg(value_name = "NAME")]
        name: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Update an existing route configuration
    #[command(
        long_about = "Update an existing route configuration by providing a JSON file with the updated specification.\n\nYou can modify path matching, cluster association, and other route properties.",
        after_help = "EXAMPLES:\n    # Update a route from JSON file\n    flowplane-cli route update my-api-route --file updated-route.json\n\n    # Update and output as YAML\n    flowplane-cli route update my-api-route --file updated-route.json --output yaml\n\n    # With authentication\n    flowplane-cli route update my-api-route --file updated-route.json --token your-token"
    )]
    Update {
        /// Route name
        #[arg(value_name = "NAME")]
        name: String,

        /// Path to YAML or JSON file with resource spec
        #[arg(short, long, value_name = "FILE", help = config_file::FILE_ARG_HELP)]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Delete a route configuration
    #[command(
        long_about = "Delete a route configuration by name.\n\nThis removes the route and stops traffic matching from being routed to the associated cluster.",
        after_help = "EXAMPLES:\n    # Delete a route (with confirmation)\n    flowplane-cli route delete my-api-route\n\n    # Delete without confirmation prompt\n    flowplane-cli route delete my-api-route --yes\n\n    # With authentication\n    flowplane-cli route delete my-api-route --token your-token"
    )]
    Delete {
        /// Route name
        #[arg(value_name = "NAME")]
        name: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Generate a template route manifest
    Scaffold {
        /// Output format (yaml or json)
        #[arg(short, long, default_value = "yaml", value_parser = ["json", "yaml"])]
        output: String,
    },
}

/// Route config response structure (matches API's RouteConfigResponse)
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteConfigResponse {
    pub name: String,
    pub team: String,
    pub path_prefix: String,
    pub cluster_targets: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_id: Option<String>,
    pub route_order: Option<i64>,
    pub config: serde_json::Value,
}

/// Handle route commands
pub async fn handle_route_command(
    command: RouteCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        RouteCommands::Create { file, output } => create_route(client, team, file, &output).await?,
        RouteCommands::List { cluster, limit, offset, output } => {
            list_routes(client, team, cluster, limit, offset, &output).await?
        }
        RouteCommands::Get { name, output } => get_route(client, team, &name, &output).await?,
        RouteCommands::Update { name, file, output } => {
            update_route(client, team, &name, file, &output).await?
        }
        RouteCommands::Delete { name, yes } => delete_route(client, team, &name, yes).await?,
        RouteCommands::Scaffold { output } => scaffold_route(&output)?,
    }

    Ok(())
}

async fn create_route(
    client: &FlowplaneClient,
    team: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let mut body = config_file::load_config_file(&file)?;
    config_file::strip_kind_field(&mut body);

    let path = format!("/api/v1/teams/{team}/route-configs");
    let response: RouteConfigResponse = client.post_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn list_routes(
    client: &FlowplaneClient,
    team: &str,
    cluster: Option<String>,
    limit: Option<i32>,
    offset: Option<i32>,
    output: &str,
) -> Result<()> {
    let mut path = format!("/api/v1/teams/{team}/route-configs?");
    let mut params = Vec::new();

    if let Some(c) = cluster {
        params.push(format!("cluster={}", c));
    }
    if let Some(l) = limit {
        params.push(format!("limit={}", l));
    }
    if let Some(o) = offset {
        params.push(format!("offset={}", o));
    }

    path.push_str(&params.join("&"));

    let response: PaginatedResponse<RouteConfigResponse> = client.get_json(&path).await?;

    if output == "table" {
        print_routes_table(&response.items);
    } else {
        print_output(&response.items, output)?;
    }

    Ok(())
}

async fn get_route(client: &FlowplaneClient, team: &str, name: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/route-configs/{name}");
    let response: RouteConfigResponse = client.get_json(&path).await?;

    if output == "table" {
        print_routes_table(&[response]);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn update_route(
    client: &FlowplaneClient,
    team: &str,
    name: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let mut body = config_file::load_config_file(&file)?;
    config_file::strip_kind_field(&mut body);

    let path = format!("/api/v1/teams/{team}/route-configs/{name}");
    let response: RouteConfigResponse = client.put_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn delete_route(client: &FlowplaneClient, team: &str, name: &str, yes: bool) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete route config '{}'? (y/N)", name);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/teams/{team}/route-configs/{name}");
    client.delete_no_content(&path).await?;

    println!("Route config '{}' deleted successfully", name);
    Ok(())
}

fn scaffold_route_to_writer(output: &str, writer: &mut impl Write) -> Result<()> {
    if output == "json" {
        let scaffold = serde_json::json!({
            "kind": "RouteConfig",
            "name": "<your-route-config-name>",
            "virtualHosts": [
                {
                    "name": "<your-vhost-name>",
                    "domains": ["*"],
                    "routes": [
                        {
                            "name": "<your-route-name>",
                            "match": {
                                "path": {
                                    "type": "prefix",
                                    "value": "/"
                                },
                                "headers": [],
                                "queryParameters": []
                            },
                            "action": {
                                "type": "forward",
                                "cluster": "<your-cluster-name>",
                                "timeoutSeconds": null,
                                "prefixRewrite": null,
                                "templateRewrite": null,
                                "retryPolicy": null
                            },
                            "typedPerFilterConfig": {}
                        }
                    ],
                    "typedPerFilterConfig": {}
                }
            ]
        });
        let json =
            serde_json::to_string_pretty(&scaffold).context("Failed to serialize scaffold")?;
        writeln!(writer, "{json}")?;
    } else {
        writeln!(writer, "# Route configuration scaffold")?;
        writeln!(writer, "#")?;
        writeln!(writer, "# Use with: flowplane route create -f <file>")?;
        writeln!(writer, "#       or: flowplane apply -f <file>")?;
        writeln!(writer)?;
        writeln!(writer, "kind: RouteConfig")?;
        writeln!(writer)?;
        writeln!(writer, "# [REQUIRED] Unique name for the route config")?;
        writeln!(writer, "name: \"<your-route-config-name>\"")?;
        writeln!(writer)?;
        writeln!(writer, "# [REQUIRED] Virtual hosts (at least one)")?;
        writeln!(writer, "virtualHosts:")?;
        writeln!(writer, "  - name: \"<your-vhost-name>\"")?;
        writeln!(writer, "    # [REQUIRED] Domains to match (\"*\" matches all)")?;
        writeln!(writer, "    domains:")?;
        writeln!(writer, "      - \"*\"")?;
        writeln!(writer, "    # [REQUIRED] Route rules (at least one, evaluated in order)")?;
        writeln!(writer, "    routes:")?;
        writeln!(writer, "      - name: \"<your-route-name>\"               # [OPTIONAL]")?;
        writeln!(writer, "        match:")?;
        writeln!(writer, "          path:")?;
        writeln!(writer, "            # [REQUIRED] Match type: prefix, exact, regex, or template")?;
        writeln!(writer, "            type: prefix")?;
        writeln!(writer, "            value: \"/\"")?;
        writeln!(writer, "          # [OPTIONAL] Header matchers")?;
        writeln!(writer, "          # headers:")?;
        writeln!(writer, "          #   - name: \"x-api-version\"")?;
        writeln!(writer, "          #     value: \"v2\"                  # exact match")?;
        writeln!(writer, "          #   - name: \"x-request-id\"")?;
        writeln!(writer, "          #     present: true               # just check header exists")?;
        writeln!(writer, "          # [OPTIONAL] Query parameter matchers")?;
        writeln!(writer, "          # queryParameters:")?;
        writeln!(writer, "          #   - name: \"debug\"")?;
        writeln!(writer, "          #     value: \"true\"               # exact match")?;
        writeln!(writer, "          #   - name: \"version\"")?;
        writeln!(writer, "          #     regex: \"^v[0-9]+$\"          # regex match")?;
        writeln!(writer, "        action:")?;
        writeln!(writer, "          # [REQUIRED] Action type: forward, weighted, or redirect")?;
        writeln!(writer, "          type: forward")?;
        writeln!(writer, "          # [REQUIRED] Target cluster name")?;
        writeln!(writer, "          cluster: \"<your-cluster-name>\"")?;
        writeln!(writer, "          # [OPTIONAL] Request timeout in seconds")?;
        writeln!(writer, "          # timeoutSeconds: 30")?;
        writeln!(writer, "          # [OPTIONAL] Rewrite path prefix")?;
        writeln!(writer, "          # prefixRewrite: \"/v2\"")?;
        writeln!(writer, "          # [OPTIONAL] Rewrite using path template")?;
        writeln!(writer, "          # templateRewrite: \"/{{version}}/{{resource}}\"")?;
        writeln!(writer, "          # [OPTIONAL] Retry policy")?;
        writeln!(writer, "          # retryPolicy:")?;
        writeln!(writer, "          #   retryOn:")?;
        writeln!(writer, "          #     - \"5xx\"")?;
        writeln!(writer, "          #     - \"reset\"")?;
        writeln!(writer, "          #     - \"connect-failure\"")?;
        writeln!(writer, "          #   maxRetries: 3")?;
        writeln!(writer, "          #   perTryTimeoutSeconds: 10")?;
        writeln!(writer, "          #   backoff:")?;
        writeln!(writer, "          #     baseIntervalMs: 100")?;
        writeln!(writer, "          #     maxIntervalMs: 1000")?;
        writeln!(writer, "        # [OPTIONAL] Per-filter config for this route")?;
        writeln!(writer, "        # typedPerFilterConfig: {{}}")?;
        writeln!(writer)?;
        writeln!(writer, "    # [OPTIONAL] Per-filter config for this virtual host")?;
        writeln!(writer, "    # typedPerFilterConfig: {{}}")?;
        writeln!(writer)?;
        writeln!(
            writer,
            "# --- Alternative action examples (uncomment one to replace forward) ---"
        )?;
        writeln!(writer)?;
        writeln!(writer, "# Weighted routing across multiple clusters:")?;
        writeln!(writer, "# action:")?;
        writeln!(writer, "#   type: weighted")?;
        writeln!(writer, "#   clusters:")?;
        writeln!(writer, "#     - name: \"cluster-a\"")?;
        writeln!(writer, "#       weight: 80")?;
        writeln!(writer, "#     - name: \"cluster-b\"")?;
        writeln!(writer, "#       weight: 20")?;
        writeln!(writer, "#   totalWeight: 100")?;
        writeln!(writer)?;
        writeln!(writer, "# Redirect:")?;
        writeln!(writer, "# action:")?;
        writeln!(writer, "#   type: redirect")?;
        writeln!(writer, "#   hostRedirect: \"new.example.com\"")?;
        writeln!(writer, "#   pathRedirect: \"/new-path\"")?;
        writeln!(writer, "#   responseCode: 301")?;
    }
    Ok(())
}

fn scaffold_route(output: &str) -> Result<()> {
    let mut stdout = std::io::stdout();
    scaffold_route_to_writer(output, &mut stdout)
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

fn print_routes_table(routes: &[RouteConfigResponse]) {
    if routes.is_empty() {
        println!("No route configs found");
        return;
    }

    println!();
    println!("{:<30} {:<15} {:<25} {:<25}", "Name", "Team", "Path Prefix", "Cluster Targets");
    println!("{}", "-".repeat(100));

    for route in routes {
        println!(
            "{:<30} {:<15} {:<25} {:<25}",
            truncate(&route.name, 28),
            truncate(&route.team, 13),
            truncate(&route.path_prefix, 23),
            truncate(&route.cluster_targets, 23),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scaffold_yaml() -> String {
        let mut buf = Vec::new();
        scaffold_route_to_writer("yaml", &mut buf).expect("scaffold yaml");
        String::from_utf8(buf).expect("valid utf8")
    }

    fn scaffold_json() -> String {
        let mut buf = Vec::new();
        scaffold_route_to_writer("json", &mut buf).expect("scaffold json");
        String::from_utf8(buf).expect("valid utf8")
    }

    #[test]
    fn yaml_kind_is_route_config() {
        let yaml = scaffold_yaml();
        assert!(yaml.contains("kind: RouteConfig"), "kind should be RouteConfig, not Route");
        assert!(!yaml.contains("kind: Route\n"), "must not contain bare 'kind: Route'");
    }

    #[test]
    fn json_kind_is_route_config() {
        let json_str = scaffold_json();
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("scaffold JSON should parse");
        assert_eq!(parsed["kind"], "RouteConfig", "JSON kind should be RouteConfig");
    }

    #[test]
    fn yaml_contains_all_fields_and_annotations() {
        let yaml = scaffold_yaml();

        assert!(yaml.contains("[REQUIRED]"), "missing [REQUIRED] annotation");
        assert!(yaml.contains("[OPTIONAL]"), "missing [OPTIONAL] annotation");

        // Top-level and structural fields
        for field in ["name:", "virtualHosts:", "domains:", "routes:"] {
            assert!(yaml.contains(field), "missing field: {field}");
        }

        // Match fields
        for field in ["path:", "type: prefix", "value:", "headers:", "queryParameters:"] {
            assert!(yaml.contains(field), "missing match field: {field}");
        }

        // Forward action fields
        for field in
            ["cluster:", "timeoutSeconds:", "prefixRewrite:", "templateRewrite:", "retryPolicy:"]
        {
            assert!(yaml.contains(field), "missing forward action field: {field}");
        }

        // Retry policy sub-fields
        for field in [
            "retryOn:",
            "maxRetries:",
            "perTryTimeoutSeconds:",
            "backoff:",
            "baseIntervalMs:",
            "maxIntervalMs:",
        ] {
            assert!(yaml.contains(field), "missing retry policy field: {field}");
        }

        // Alternative actions
        for field in ["totalWeight:", "hostRedirect:", "pathRedirect:", "responseCode:"] {
            assert!(yaml.contains(field), "missing alternative action field: {field}");
        }

        // Per-filter config at both levels
        assert!(
            yaml.matches("typedPerFilterConfig:").count() >= 2,
            "typedPerFilterConfig should appear at both virtual host and route levels"
        );
    }

    #[test]
    fn yaml_field_names_are_camel_case() {
        let yaml = scaffold_yaml();

        // These snake_case variants must NOT appear
        assert!(!yaml.contains("virtual_hosts"), "found snake_case: virtual_hosts");
        assert!(!yaml.contains("query_parameters"), "found snake_case: query_parameters");
        assert!(!yaml.contains("timeout_seconds"), "found snake_case: timeout_seconds");
        assert!(!yaml.contains("prefix_rewrite"), "found snake_case: prefix_rewrite");
        assert!(!yaml.contains("template_rewrite"), "found snake_case: template_rewrite");
        assert!(!yaml.contains("retry_policy"), "found snake_case: retry_policy");
        assert!(!yaml.contains("retry_on"), "found snake_case: retry_on");
        assert!(!yaml.contains("max_retries"), "found snake_case: max_retries");
        assert!(
            !yaml.contains("per_try_timeout_seconds"),
            "found snake_case: per_try_timeout_seconds"
        );
        assert!(!yaml.contains("host_redirect"), "found snake_case: host_redirect");
        assert!(!yaml.contains("path_redirect"), "found snake_case: path_redirect");
        assert!(!yaml.contains("response_code"), "found snake_case: response_code");
        assert!(!yaml.contains("total_weight"), "found snake_case: total_weight");
        assert!(
            !yaml.contains("typed_per_filter_config"),
            "found snake_case: typed_per_filter_config"
        );
        assert!(!yaml.contains("base_interval_ms"), "found snake_case: base_interval_ms");
        assert!(!yaml.contains("max_interval_ms"), "found snake_case: max_interval_ms");
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
        assert_eq!(parsed["kind"], "RouteConfig");
        assert!(parsed.get("name").is_some(), "missing 'name' key");
        assert!(parsed.get("virtualHosts").is_some(), "missing 'virtualHosts' key");
        assert!(parsed["virtualHosts"].is_array(), "'virtualHosts' should be an array");

        // Verify route rule structure
        let vhost = &parsed["virtualHosts"][0];
        assert!(vhost.get("routes").is_some(), "missing 'routes' in vhost");
        let route = &vhost["routes"][0];
        assert!(route.get("match").is_some(), "missing 'match' in route");
        assert!(route.get("action").is_some(), "missing 'action' in route");
    }

    #[test]
    fn json_output_is_valid_json_with_all_keys() {
        let json_str = scaffold_json();
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("scaffold JSON should parse");

        // Top-level keys
        for key in ["kind", "name", "virtualHosts"] {
            assert!(parsed.get(key).is_some(), "missing top-level key: {key}");
        }

        // Virtual host keys
        let vhost = &parsed["virtualHosts"][0];
        for key in ["name", "domains", "routes", "typedPerFilterConfig"] {
            assert!(vhost.get(key).is_some(), "missing vhost key: {key}");
        }

        // Route rule keys
        let route = &vhost["routes"][0];
        for key in ["name", "match", "action", "typedPerFilterConfig"] {
            assert!(route.get(key).is_some(), "missing route key: {key}");
        }

        // Match keys
        let match_obj = &route["match"];
        for key in ["path", "headers", "queryParameters"] {
            assert!(match_obj.get(key).is_some(), "missing match key: {key}");
        }

        // Forward action keys
        let action = &route["action"];
        for key in
            ["type", "cluster", "timeoutSeconds", "prefixRewrite", "templateRewrite", "retryPolicy"]
        {
            assert!(action.get(key).is_some(), "missing action key: {key}");
        }
    }
}
