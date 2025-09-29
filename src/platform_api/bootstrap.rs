use std::fs;
use std::path::PathBuf;

use crate::errors::{Error, Result};
use crate::openapi::defaults::DEFAULT_GATEWAY_PORT;
use crate::storage::{
    ApiDefinitionData, ApiDefinitionRepository, ApiRouteData, UpdateBootstrapMetadataRequest,
};

const BOOTSTRAP_DIR: &str = "data/bootstrap";

/// Compute a bootstrap artifact URI for the given API definition identifier.
pub fn compute_bootstrap_uri(definition_id: &str) -> String {
    format!("/bootstrap/api-definitions/{}.yaml", definition_id)
}

/// Materialise a bootstrap configuration on disk and return the relative URI.
fn write_bootstrap_file(definition: &ApiDefinitionData, routes: &[ApiRouteData]) -> Result<String> {
    fs::create_dir_all(BOOTSTRAP_DIR).map_err(Error::from)?;
    let filename = format!("{}.yaml", definition.id);
    let path = PathBuf::from(BOOTSTRAP_DIR).join(&filename);
    let contents = render_bootstrap_yaml(definition, routes);
    fs::write(&path, contents).map_err(Error::from)?;
    Ok(path.to_string_lossy().to_string())
}

fn render_bootstrap_yaml(definition: &ApiDefinitionData, routes: &[ApiRouteData]) -> String {
    let listener_name = format!("{}-listener", definition.id);
    let route_config_name = format!("{}-routes", definition.id);

    let mut routes_yaml = String::new();
    let mut clusters = Vec::new();

    for route in routes {
        routes_yaml.push_str("                        - match:\n");
        match route.match_type.as_str() {
            "prefix" => {
                routes_yaml.push_str(&format!(
                    "                            prefix: \"{}\"\n",
                    route.match_value
                ));
            }
            "path" => {
                routes_yaml.push_str(&format!(
                    "                            path: \"{}\"\n",
                    route.match_value
                ));
            }
            other => {
                routes_yaml.push_str(&format!(
                    "                            safe_regex: \"{}:{}\"\n",
                    other, route.match_value
                ));
            }
        }

        let (cluster_name, host, port) = extract_primary_target(&route.upstream_targets);

        if !clusters.iter().any(|(name, _, _)| name == &cluster_name) {
            clusters.push((cluster_name.clone(), host.clone(), port));
        }

        routes_yaml.push_str("                          route:\n");
        routes_yaml.push_str(&format!("                            cluster: {}\n", cluster_name));
        if let Some(timeout) = route.timeout_seconds {
            routes_yaml.push_str(&format!("                            timeout: {}s\n", timeout));
        }
        if let Some(prefix) = route.rewrite_prefix.as_ref() {
            routes_yaml
                .push_str(&format!("                            prefix_rewrite: \"{}\"\n", prefix));
        }
        routes_yaml.push('\n');
    }

    let mut yaml = String::new();
    yaml.push_str("admin:\n");
    yaml.push_str("  access_log_path: /tmp/envoy_admin.log\n");
    yaml.push_str("  address:\n");
    yaml.push_str("    socket_address:\n");
    yaml.push_str("      address: 127.0.0.1\n");
    yaml.push_str("      port_value: 9901\n");
    yaml.push_str("static_resources:\n");
    yaml.push_str("  listeners:\n");
    yaml.push_str(&format!("    - name: {}\n", listener_name));
    yaml.push_str("      address:\n");
    yaml.push_str("        socket_address:\n");
    yaml.push_str("          address: 0.0.0.0\n");
    yaml.push_str(&format!("          port_value: {}\n", DEFAULT_GATEWAY_PORT));
    yaml.push_str("      filter_chains:\n");
    yaml.push_str("        - filters:\n");
    yaml.push_str("            - name: envoy.filters.network.http_connection_manager\n");
    yaml.push_str("              typed_config:\n");
    yaml.push_str("                \"@type\": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager\n");
    yaml.push_str("                stat_prefix: ingress_http\n");
    yaml.push_str("                route_config:\n");
    yaml.push_str(&format!("                  name: {}\n", route_config_name));
    yaml.push_str("                  virtual_hosts:\n");
    yaml.push_str(&format!("                    - name: {}\n", definition.domain));
    yaml.push_str(&format!(
        "                      domains:\n                        - \"{}\"\n",
        definition.domain
    ));
    yaml.push_str("                      routes:\n");
    yaml.push_str(&routes_yaml);

    yaml.push_str("  clusters:\n");
    for (cluster_name, host, port) in clusters {
        yaml.push_str(&format!("    - name: {}\n", cluster_name));
        yaml.push_str("      connect_timeout: 5s\n");
        yaml.push_str("      type: STRICT_DNS\n");
        yaml.push_str("      load_assignment:\n");
        yaml.push_str(&format!("        cluster_name: {}\n", cluster_name));
        yaml.push_str("        endpoints:\n");
        yaml.push_str("          - lb_endpoints:\n");
        yaml.push_str("              - endpoint:\n");
        yaml.push_str("                  address:\n");
        yaml.push_str("                    socket_address:\n");
        yaml.push_str(&format!("                      address: {}\n", host));
        yaml.push_str(&format!("                      port_value: {}\n", port));
    }

    yaml
}

fn extract_primary_target(targets: &serde_json::Value) -> (String, String, u16) {
    let name = targets
        .get("targets")
        .and_then(|value| value.get(0))
        .and_then(|value| value.get("name"))
        .and_then(|value| value.as_str())
        .unwrap_or("primary-upstream")
        .to_string();

    let endpoint = targets
        .get("targets")
        .and_then(|value| value.get(0))
        .and_then(|value| value.get("endpoint"))
        .and_then(|value| value.as_str())
        .unwrap_or("localhost:8080");

    let mut parts = endpoint.split(':');
    let host = parts.next().unwrap_or("localhost").to_string();
    let port = parts.next().and_then(|p| p.parse::<u16>().ok()).unwrap_or(80);

    (name, host, port)
}

/// Persist bootstrap metadata and return the updated API definition alongside the URI.
pub async fn persist_bootstrap_metadata(
    repository: &ApiDefinitionRepository,
    definition: &ApiDefinitionData,
    routes: &[ApiRouteData],
) -> Result<(ApiDefinitionData, String)> {
    let _file_path = write_bootstrap_file(definition, routes)?;
    let uri = compute_bootstrap_uri(&definition.id);
    let updated = repository
        .update_bootstrap_metadata(UpdateBootstrapMetadataRequest {
            definition_id: definition.id.clone(),
            bootstrap_uri: Some(uri.clone()),
            bootstrap_revision: definition.bootstrap_revision + 1,
        })
        .await?;
    Ok((updated, uri))
}
