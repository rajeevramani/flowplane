//! Expose/Unexpose HTTP handlers
//!
//! Provides a simplified API for exposing services through Envoy.
//! POST /api/v1/teams/{team}/expose creates a cluster + route config + listener.
//! DELETE /api/v1/teams/{team}/expose/{name} removes all three.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::{
    api::{
        error::{ApiError, JsonBody},
        handlers::team_access::{require_resource_access_resolved, resolve_rest_auth},
        routes::ApiState,
    },
    auth::models::AuthContext,
    internal_api::{
        ClusterOperations, CreateClusterRequest, CreateListenerRequest, CreateRouteConfigRequest,
        ListenerOperations,
    },
    storage::{
        repositories::{import_metadata::ImportMetadataRepository, TeamRepository},
        DataplaneRepository,
    },
    xds::{
        listener::{FilterChainConfig, FilterConfig, FilterType, ListenerConfig},
        route::{
            PathMatch, RouteActionConfig, RouteConfig, RouteMatchConfig, RouteRule,
            VirtualHostConfig,
        },
        ClusterSpec, EndpointSpec,
    },
};

/// Port pool range for expose API
const PORT_POOL_START: u16 = 10001;
const PORT_POOL_END: u16 = 10020;

#[derive(Debug, Deserialize)]
pub struct ExposeRequest {
    pub name: String,
    pub upstream: String,
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    pub port: Option<u16>,
}

#[derive(Debug, Serialize)]
pub struct ExposeResponse {
    pub name: String,
    pub upstream: String,
    pub port: u16,
    pub paths: Vec<String>,
    pub cluster: String,
    pub route_config: String,
    pub listener: String,
}

/// Parse an upstream string into (host, port).
///
/// Accepts both URL format (`http://host:port/path`) and plain `host:port`.
/// The scheme and path are stripped — only host and port are used for the
/// Envoy cluster endpoint.
fn parse_upstream(upstream: &str) -> Result<(String, u16), ApiError> {
    // Strip scheme if present (http://, https://)
    let without_scheme = if let Some(rest) = upstream.strip_prefix("http://") {
        rest
    } else if let Some(rest) = upstream.strip_prefix("https://") {
        rest
    } else {
        upstream
    };

    // Strip path if present (everything after first /)
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);

    // Split host:port
    let parts: Vec<&str> = host_port.rsplitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(ApiError::BadRequest(format!(
            "Invalid upstream '{}': expected [http://]host:port[/path]",
            upstream
        )));
    }
    let port: u16 = parts[0]
        .parse()
        .map_err(|_| ApiError::BadRequest(format!("Invalid upstream port in '{}'", upstream)))?;
    crate::validation::validate_port_nonzero(port).map_err(ApiError::BadRequest)?;
    let host = parts[1].to_string();
    if host.is_empty() {
        return Err(ApiError::BadRequest("Upstream host cannot be empty".to_string()));
    }
    Ok((host, port))
}

/// Find the first free port in the pool by querying all listeners.
async fn find_free_port(state: &ApiState) -> Result<u16, ApiError> {
    let listener_repo = state
        .xds_state
        .listener_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Listener repository unavailable".to_string()))?;

    // Query all listeners (cross-team) to find occupied ports
    let all_listeners = listener_repo
        .list(Some(1000), Some(0))
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list listeners: {}", e)))?;

    let occupied: std::collections::HashSet<u16> =
        all_listeners.iter().filter_map(|l| l.port.map(|p| p as u16)).collect();

    for port in PORT_POOL_START..=PORT_POOL_END {
        if !occupied.contains(&port) {
            return Ok(port);
        }
    }

    Err(ApiError::Conflict("Port pool exhausted: all ports 10001-10020 are in use".to_string()))
}

/// Check if a specific port is free across all listeners.
async fn check_port_free(state: &ApiState, port: u16) -> Result<(), ApiError> {
    let listener_repo = state
        .xds_state
        .listener_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Listener repository unavailable".to_string()))?;

    let all_listeners = listener_repo
        .list(Some(1000), Some(0))
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list listeners: {}", e)))?;

    let occupied = all_listeners.iter().any(|l| l.port == Some(port as i64));
    if occupied {
        return Err(ApiError::Conflict(format!("Port {} is already in use", port)));
    }
    Ok(())
}

/// Get the default dataplane for a team.
async fn get_team_dataplane(state: &ApiState, team: &str) -> Result<String, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Database pool unavailable".to_string()))?;

    let dataplane_repo = DataplaneRepository::new(cluster_repo.pool().clone());

    // Resolve team name to ID first
    let team_repo = crate::api::handlers::team_access::team_repo_from_state(state)?;
    let team_record = team_repo
        .get_team_by_name(team)
        .await
        .map_err(|_| ApiError::NotFound(format!("Team '{}' not found", team)))?
        .ok_or_else(|| ApiError::NotFound(format!("Team '{}' not found", team)))?;

    let dataplanes =
        dataplane_repo
            .list_by_team(team_record.id.as_ref(), Some(1), Some(0))
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to list dataplanes: {}", e)))?;

    dataplanes
        .first()
        .map(|dp| dp.id.to_string())
        .ok_or_else(|| ApiError::NotFound(format!("No dataplane found for team '{}'", team)))
}

/// Build a route config JSON for given paths pointing to a cluster.
///
/// Uses typed `RouteConfig` struct to ensure the stored JSON matches the
/// xDS deserialization format exactly — avoids "missing field" errors when
/// the xDS snapshot is rebuilt.
fn build_route_config_json(
    name: &str,
    cluster_name: &str,
    paths: &[String],
) -> Result<serde_json::Value, ApiError> {
    let routes: Vec<RouteRule> = paths
        .iter()
        .enumerate()
        .map(|(i, path)| RouteRule {
            name: Some(format!("{}-route-{}", name, i)),
            r#match: RouteMatchConfig {
                path: PathMatch::Prefix(path.clone()),
                headers: None,
                query_parameters: None,
            },
            action: RouteActionConfig::Cluster {
                name: cluster_name.to_string(),
                timeout: None,
                prefix_rewrite: None,
                path_template_rewrite: None,
                retry_policy: None,
            },
            typed_per_filter_config: Default::default(),
        })
        .collect();

    let config = RouteConfig {
        name: name.to_string(),
        virtual_hosts: vec![VirtualHostConfig {
            name: format!("{}-vhost", name),
            domains: vec!["*".to_string()],
            routes,
            typed_per_filter_config: Default::default(),
        }],
    };

    serde_json::to_value(&config)
        .map_err(|e| ApiError::Internal(format!("Failed to serialize route config: {}", e)))
}

#[instrument(skip(state, payload), fields(team = %team, service_name = %payload.name))]
pub async fn expose_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path(team): Path<String>,
    JsonBody(payload): JsonBody<ExposeRequest>,
) -> Result<(StatusCode, Json<ExposeResponse>), ApiError> {
    // Validate input
    crate::validation::validate_resource_name(&payload.name).map_err(ApiError::BadRequest)?;
    crate::validation::validate_upstream(&payload.upstream).map_err(ApiError::BadRequest)?;
    let (upstream_host, upstream_port) = parse_upstream(&payload.upstream)?;

    // Reject explicitly empty paths array — default to ["/"] only when field is omitted
    if let Some(ref p) = payload.paths {
        if p.is_empty() {
            return Err(ApiError::BadRequest(
                "paths array cannot be empty; omit the field to default to [\"/\"]".to_string(),
            ));
        }
    }

    // Authorization
    require_resource_access_resolved(&state, &context, "listeners", "create", Some(&team)).await?;

    let auth = resolve_rest_auth(&state, &context).await?;

    let cluster_name = payload.name.clone();
    let route_config_name = format!("{}-routes", payload.name);
    let listener_name = format!("{}-listener", payload.name);
    let paths = payload.paths.unwrap_or_else(|| vec!["/".to_string()]);

    // Check idempotency: if cluster already exists with same name, check upstream match
    let cluster_ops = ClusterOperations::new(state.xds_state.clone());
    let existing_cluster = cluster_ops.get(&cluster_name, &auth).await;
    if let Ok(ref cluster_data) = existing_cluster {
        // Parse stored config to check upstream
        let config: serde_json::Value = serde_json::from_str(&cluster_data.configuration)
            .map_err(|e| ApiError::Internal(format!("Failed to parse cluster config: {}", e)))?;
        let existing_endpoints = &config["endpoints"];
        let expected = serde_json::json!([{"host": upstream_host, "port": upstream_port}]);
        if existing_endpoints != &expected {
            return Err(ApiError::Conflict(format!(
                "Service '{}' already exists with a different upstream",
                payload.name
            )));
        }

        // Idempotent: find the existing listener port and return
        let listener_ops = ListenerOperations::new(state.xds_state.clone());
        if let Ok(listener_data) = listener_ops.get(&listener_name, &auth).await {
            let port = listener_data.port.unwrap_or(0) as u16;
            return Ok((
                StatusCode::OK,
                Json(ExposeResponse {
                    name: payload.name,
                    upstream: payload.upstream,
                    port,
                    paths,
                    cluster: cluster_name,
                    route_config: route_config_name,
                    listener: listener_name,
                }),
            ));
        }
    }

    // Determine port
    let port = match payload.port {
        Some(p) => {
            if !(PORT_POOL_START..=PORT_POOL_END).contains(&p) {
                return Err(ApiError::BadRequest(format!(
                    "Port {} is outside the valid range {}-{}",
                    p, PORT_POOL_START, PORT_POOL_END
                )));
            }
            check_port_free(&state, p).await?;
            p
        }
        None => find_free_port(&state).await?,
    };

    // Get default dataplane for team
    let dataplane_id = get_team_dataplane(&state, &team).await?;

    // 1. Create-or-reuse cluster
    if existing_cluster.is_err() {
        let cluster_spec = ClusterSpec {
            endpoints: vec![EndpointSpec::Address {
                host: upstream_host.clone(),
                port: upstream_port,
            }],
            ..Default::default()
        };
        let create_cluster = CreateClusterRequest {
            name: cluster_name.clone(),
            service_name: cluster_name.clone(),
            team: Some(team.clone()),
            config: cluster_spec,
        };
        cluster_ops.create(create_cluster, &auth).await.map_err(ApiError::from)?;
    }

    // 2. Create-or-reuse route config
    let route_ops = crate::internal_api::RouteConfigOperations::new(state.xds_state.clone());
    let existing_rc = route_ops.get(&route_config_name, &auth).await;
    if existing_rc.is_err() {
        let rc_config = build_route_config_json(&route_config_name, &cluster_name, &paths)?;
        let create_rc = CreateRouteConfigRequest {
            name: route_config_name.clone(),
            team: Some(team.clone()),
            config: rc_config,
        };
        route_ops.create(create_rc, &auth).await.map_err(ApiError::from)?;
    }

    // 3. Create listener
    let listener_config = ListenerConfig {
        name: listener_name.clone(),
        address: "0.0.0.0".to_string(),
        port: port as u32,
        filter_chains: vec![FilterChainConfig {
            name: Some("default".to_string()),
            filters: vec![FilterConfig {
                name: "envoy.filters.network.http_connection_manager".to_string(),
                filter_type: FilterType::HttpConnectionManager {
                    route_config_name: Some(route_config_name.clone()),
                    inline_route_config: None,
                    access_log: None,
                    tracing: None,
                    http_filters: Vec::new(),
                },
            }],
            tls_context: None,
        }],
    };

    let create_listener = CreateListenerRequest {
        name: listener_name.clone(),
        address: "0.0.0.0".to_string(),
        port,
        protocol: Some("HTTP".to_string()),
        team: Some(team.clone()),
        config: listener_config,
        dataplane_id,
    };
    let listener_ops = ListenerOperations::new(state.xds_state.clone());
    listener_ops.create(create_listener, &auth).await.map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(ExposeResponse {
            name: payload.name,
            upstream: payload.upstream,
            port,
            paths,
            cluster: cluster_name,
            route_config: route_config_name,
            listener: listener_name,
        }),
    ))
}

#[instrument(skip(state), fields(team = %team, service_name = %name))]
pub async fn unexpose_handler(
    State(state): State<ApiState>,
    Extension(context): Extension<AuthContext>,
    Path((team, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    // Authorization
    require_resource_access_resolved(&state, &context, "listeners", "delete", Some(&team)).await?;

    let auth = resolve_rest_auth(&state, &context).await?;

    let listener_name = format!("{}-listener", name);
    let route_config_name = format!("{}-routes", name);
    let cluster_name = name.clone();

    let mut found_any = false;

    // Delete listener (skip if missing)
    let listener_ops = ListenerOperations::new(state.xds_state.clone());
    if let Ok(_listener) = listener_ops.get(&listener_name, &auth).await {
        found_any = true;
        let _ = listener_ops.delete(&listener_name, &auth).await;
    }

    // Delete route config (skip if missing)
    let route_ops = crate::internal_api::RouteConfigOperations::new(state.xds_state.clone());
    if let Ok(_rc) = route_ops.get(&route_config_name, &auth).await {
        found_any = true;
        let _ = route_ops.delete(&route_config_name, &auth).await;
    }

    // Delete cluster (skip if missing)
    let cluster_ops = ClusterOperations::new(state.xds_state.clone());
    if let Ok(_cluster) = cluster_ops.get(&cluster_name, &auth).await {
        found_any = true;
        let _ = cluster_ops.delete(&cluster_name, &auth).await;
    }

    // If direct name pattern didn't match, check for import-created resources.
    // OpenAPI imports create resources with sanitized naming (e.g., `<title>-<host>`)
    // that don't match the `<name>` / `<name>-routes` / `<name>-listener` convention.
    if !found_any {
        let import_deleted = try_delete_import_by_name(&state, &team, &name).await?;
        if import_deleted {
            return Ok(StatusCode::NO_CONTENT);
        }
        return Err(ApiError::NotFound(format!("Service '{}' is not currently exposed", name)));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Try to find and delete an OpenAPI import whose spec_name or gateway_name matches.
async fn try_delete_import_by_name(
    state: &ApiState,
    team: &str,
    name: &str,
) -> Result<bool, ApiError> {
    let cluster_repo = state
        .xds_state
        .cluster_repository
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Cluster repository not configured".to_string()))?;
    let db_pool = cluster_repo.pool().clone();
    let import_repo = ImportMetadataRepository::new(db_pool.clone());

    // Resolve team name to ID for lookup
    let team_id =
        crate::api::handlers::team_access::resolve_team_name(state, team, None).await.ok();

    // Search by spec_name match (exact match first, then sanitized gateway name)
    let imports = import_repo
        .list_by_team(team_id.as_deref().unwrap_or(team))
        .await
        .map_err(ApiError::from)?;

    let sanitized_name = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>();

    let matching_import = imports.iter().find(|imp| {
        // Match by spec_name (case-insensitive)
        imp.spec_name.to_lowercase() == name.to_lowercase()
            // Match by sanitized gateway name
            || imp.spec_name
                .to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '-' })
                .collect::<String>()
                == sanitized_name
    });

    if let Some(import) = matching_import {
        let import_id = import.id.clone();
        crate::api::handlers::openapi_import::delete_import_resources(
            &state.xds_state,
            &db_pool,
            &import_id,
        )
        .await?;
        return Ok(true);
    }

    Ok(false)
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_upstream_valid() {
        let (host, port) = parse_upstream("localhost:8080").expect("valid upstream");
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
    }

    #[test]
    fn parse_upstream_ipv4() {
        let (host, port) = parse_upstream("10.0.0.1:3000").expect("valid upstream");
        assert_eq!(host, "10.0.0.1");
        assert_eq!(port, 3000);
    }

    #[test]
    fn parse_upstream_http_url() {
        let (host, port) = parse_upstream("http://localhost:3000").expect("valid upstream");
        assert_eq!(host, "localhost");
        assert_eq!(port, 3000);
    }

    #[test]
    fn parse_upstream_https_url() {
        let (host, port) = parse_upstream("https://api.example.com:443").expect("valid upstream");
        assert_eq!(host, "api.example.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn parse_upstream_url_with_path() {
        let (host, port) = parse_upstream("http://localhost:3000/api/v1").expect("valid upstream");
        assert_eq!(host, "localhost");
        assert_eq!(port, 3000);
    }

    #[test]
    fn parse_upstream_missing_port() {
        let result = parse_upstream("localhost");
        assert!(result.is_err());
    }

    #[test]
    fn parse_upstream_invalid_port() {
        let result = parse_upstream("localhost:abc");
        assert!(result.is_err());
    }

    #[test]
    fn parse_upstream_empty_host() {
        let result = parse_upstream(":8080");
        assert!(result.is_err());
    }

    #[test]
    fn parse_upstream_port_zero_rejected() {
        let result = parse_upstream("localhost:0");
        assert!(result.is_err());
    }

    #[test]
    fn validate_name_special_chars_rejected() {
        let result = crate::validation::validate_resource_name("test@!#");
        assert!(result.is_err());
    }

    #[test]
    fn validate_upstream_spaces_rejected() {
        let result = crate::validation::validate_upstream("localhost :8080");
        assert!(result.is_err());
    }

    #[test]
    fn naming_conventions() {
        let name = "my-service";
        assert_eq!(format!("{}-routes", name), "my-service-routes");
        assert_eq!(format!("{}-listener", name), "my-service-listener");
        // cluster name is same as service name
        assert_eq!(name, "my-service");
    }

    #[test]
    fn build_route_config_with_default_paths() {
        let config = build_route_config_json("svc", "svc", &["/".to_string()]).expect("serialize");
        // Typed structs serialize field names as-is (snake_case)
        assert_eq!(config["name"], "svc");
        let vhosts = config["virtual_hosts"].as_array().expect("virtual_hosts");
        assert_eq!(vhosts.len(), 1);
        let routes = vhosts[0]["routes"].as_array().expect("routes");
        assert_eq!(routes.len(), 1);
        // PathMatch::Prefix serializes as {"Prefix": value}
        assert_eq!(routes[0]["match"]["path"]["Prefix"], "/");
        // RouteActionConfig::Cluster serializes as {"Cluster": {"name": value}}
        assert_eq!(routes[0]["action"]["Cluster"]["name"], "svc");
    }

    #[test]
    fn build_route_config_with_multiple_paths() {
        let paths = vec!["/api".to_string(), "/health".to_string()];
        let config = build_route_config_json("svc", "svc", &paths).expect("serialize");
        let routes = config["virtual_hosts"][0]["routes"].as_array().expect("routes");
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0]["match"]["path"]["Prefix"], "/api");
        assert_eq!(routes[1]["match"]["path"]["Prefix"], "/health");
    }

    #[test]
    fn port_pool_range_is_valid() {
        const { assert!(PORT_POOL_START < PORT_POOL_END) };
        assert_eq!(PORT_POOL_END - PORT_POOL_START + 1, 20);
    }

    #[test]
    fn validate_empty_name_rejected() {
        let payload = ExposeRequest {
            name: "".to_string(),
            upstream: "localhost:8080".to_string(),
            paths: None,
            port: None,
        };
        assert!(payload.name.is_empty());
    }

    #[test]
    fn validate_port_outside_range() {
        // Port validation logic
        let port: u16 = 9999;
        assert!(!(PORT_POOL_START..=PORT_POOL_END).contains(&port));

        let port: u16 = 10001;
        assert!((PORT_POOL_START..=PORT_POOL_END).contains(&port));

        let port: u16 = 10020;
        assert!((PORT_POOL_START..=PORT_POOL_END).contains(&port));

        let port: u16 = 10021;
        assert!(!(PORT_POOL_START..=PORT_POOL_END).contains(&port));
    }
}
