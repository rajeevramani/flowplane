//! Route Config Operations for Internal API
//!
//! This module provides the unified route config operations layer that sits between
//! HTTP/MCP handlers and the RouteService. It handles:
//! - Request validation
//! - Team-based access control
//! - Error mapping
//! - Response formatting

use std::sync::Arc;
use tracing::{info, instrument};

use crate::domain::RouteConfigId;
use crate::internal_api::auth::{verify_team_access, InternalAuthContext};
use crate::internal_api::error::InternalError;
use crate::internal_api::types::{
    CreateRouteConfigRequest, ListRouteConfigsRequest, ListRouteConfigsResponse, OperationResult,
    UpdateRouteConfigRequest,
};
use crate::openapi::defaults::is_default_gateway_route;
use crate::services::RouteService;
use crate::storage::RouteConfigData;
use crate::xds::route::RouteConfig;
use crate::xds::XdsState;
use serde_json::{json, Value};

/// Route config operations for the internal API layer
///
/// This struct provides all CRUD operations for route configs with unified
/// validation and access control.
pub struct RouteConfigOperations {
    xds_state: Arc<XdsState>,
}

impl RouteConfigOperations {
    /// Create a new RouteConfigOperations instance
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// Create a new route config
    ///
    /// # Arguments
    /// * `req` - The create route config request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the created route config on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(route_config_name = %req.name, team = ?req.team))]
    pub async fn create(
        &self,
        req: CreateRouteConfigRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<RouteConfigData>, InternalError> {
        // 1. Verify team access (can create in this team?)
        if !auth.can_create_for_team(req.team.as_deref()) {
            return Err(InternalError::forbidden(format!(
                "Cannot create route config for team '{}'",
                req.team.as_deref().unwrap_or("global")
            )));
        }

        // 2. Summarize the configuration for storage
        let (path_prefix, cluster_summary) = summarize_config(&req.config);

        // 3. Call service layer
        let service = RouteService::new(self.xds_state.clone());
        let created = service
            .create_route(
                req.name.clone(),
                path_prefix,
                cluster_summary,
                req.config.clone(),
                req.team,
            )
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("already exists") || err_str.contains("UNIQUE constraint") {
                    InternalError::already_exists("Route config", &req.name)
                } else {
                    InternalError::from(e)
                }
            })?;

        // 4. Sync route hierarchy if available
        if let Err(e) = self.sync_route_hierarchy(&created, &req.config).await {
            tracing::warn!(
                route_config_id = %created.id,
                error = %e,
                "Failed to sync route hierarchy after creation"
            );
        }

        info!(
            route_config_id = %created.id,
            route_config_name = %created.name,
            "Route config created via internal API"
        );

        Ok(OperationResult::with_message(
            created,
            "Route config created successfully. xDS configuration has been refreshed.",
        ))
    }

    /// Get a route config by name
    ///
    /// # Arguments
    /// * `name` - The route config name
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(RouteConfigData)` if found and accessible
    /// * `Err(InternalError::NotFound)` if not found or not accessible
    #[instrument(skip(self, auth), fields(route_config_name = %name))]
    pub async fn get(
        &self,
        name: &str,
        auth: &InternalAuthContext,
    ) -> Result<RouteConfigData, InternalError> {
        let service = RouteService::new(self.xds_state.clone());

        // Get the route config
        let route_config = service.get_route(name).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("not found") {
                InternalError::not_found("Route config", name)
            } else {
                InternalError::from(e)
            }
        })?;

        // Verify team access
        verify_team_access(route_config, auth).await
    }

    /// List route configs with pagination
    ///
    /// # Arguments
    /// * `req` - List request with pagination options
    /// * `auth` - Authentication context for filtering
    ///
    /// # Returns
    /// * `Ok(ListRouteConfigsResponse)` with filtered route configs
    #[instrument(skip(self, auth), fields(limit = ?req.limit, offset = ?req.offset))]
    pub async fn list(
        &self,
        req: ListRouteConfigsRequest,
        auth: &InternalAuthContext,
    ) -> Result<ListRouteConfigsResponse, InternalError> {
        let repository = self.xds_state.route_config_repository.as_ref().ok_or_else(|| {
            InternalError::service_unavailable("Route config repository unavailable")
        })?;

        // Use team filtering - empty allowed_teams means admin access to all
        let routes = repository
            .list_by_teams(&auth.allowed_teams, req.include_defaults, req.limit, req.offset)
            .await
            .map_err(|e| InternalError::database(e.to_string()))?;

        let count = routes.len();

        Ok(ListRouteConfigsResponse { routes, count, limit: req.limit, offset: req.offset })
    }

    /// Update an existing route config
    ///
    /// # Arguments
    /// * `name` - The route config name to update
    /// * `req` - The update request
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` with the updated route config on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, req, auth), fields(route_config_name = %name))]
    pub async fn update(
        &self,
        name: &str,
        req: UpdateRouteConfigRequest,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<RouteConfigData>, InternalError> {
        // 1. Get existing route config and verify access
        let _existing = self.get(name, auth).await?;

        // 2. Summarize the new configuration
        let (path_prefix, cluster_summary) = summarize_config(&req.config);

        // 3. Update via service layer
        let service = RouteService::new(self.xds_state.clone());
        let updated = service
            .update_route(name, path_prefix, cluster_summary, req.config.clone())
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                if err_str.contains("not found") {
                    InternalError::not_found("Route config", name)
                } else {
                    InternalError::from(e)
                }
            })?;

        // 4. Sync route hierarchy if available
        if let Err(e) = self.sync_route_hierarchy(&updated, &req.config).await {
            tracing::warn!(
                route_config_id = %updated.id,
                error = %e,
                "Failed to sync route hierarchy after update"
            );
        }

        info!(
            route_config_id = %updated.id,
            route_config_name = %updated.name,
            "Route config updated via internal API"
        );

        Ok(OperationResult::with_message(
            updated,
            "Route config updated successfully. xDS configuration has been refreshed.",
        ))
    }

    /// Delete a route config
    ///
    /// # Arguments
    /// * `name` - The route config name to delete
    /// * `auth` - Authentication context for access control
    ///
    /// # Returns
    /// * `Ok(OperationResult)` on success
    /// * `Err(InternalError)` on failure
    #[instrument(skip(self, auth), fields(route_config_name = %name))]
    pub async fn delete(
        &self,
        name: &str,
        auth: &InternalAuthContext,
    ) -> Result<OperationResult<()>, InternalError> {
        // 1. Check for default route config protection
        if is_default_gateway_route(name) {
            return Err(InternalError::forbidden(
                "The default gateway route configuration cannot be deleted".to_string(),
            ));
        }

        // 2. Get existing route config and verify access
        let _existing = self.get(name, auth).await?;

        // 3. Delete via service layer
        let service = RouteService::new(self.xds_state.clone());
        service.delete_route(name).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("default gateway") || err_str.contains("cannot be deleted") {
                InternalError::forbidden(err_str)
            } else {
                InternalError::from(e)
            }
        })?;

        info!(route_config_name = %name, "Route config deleted via internal API");

        Ok(OperationResult::with_message(
            (),
            "Route config deleted successfully. xDS configuration has been refreshed.",
        ))
    }

    /// Sync route hierarchy to normalized tables (virtual_hosts, routes)
    async fn sync_route_hierarchy(
        &self,
        route_config_data: &RouteConfigData,
        config: &Value,
    ) -> Result<(), InternalError> {
        if let Some(ref sync_service) = self.xds_state.route_hierarchy_sync_service {
            // Parse the configuration
            let xds_config: RouteConfig = serde_json::from_value(config.clone())
                .map_err(|e| InternalError::validation(format!("Invalid route config: {}", e)))?;

            let route_config_id = RouteConfigId::from_string(route_config_data.id.to_string());
            sync_service
                .sync(&route_config_id, &xds_config)
                .await
                .map_err(|e| InternalError::internal(e.to_string()))?;
        }
        Ok(())
    }
}

/// Summarize the route configuration for storage
///
/// Extracts the first path prefix and collects all referenced cluster names.
fn summarize_config(config: &Value) -> (String, String) {
    let mut paths = Vec::new();
    let mut clusters = std::collections::HashSet::new();

    // Handle both formats: MCP format and internal format
    let virtual_hosts = config.get("virtual_hosts").or_else(|| config.get("virtualHosts"));

    if let Some(hosts) = virtual_hosts.and_then(|h| h.as_array()) {
        for host in hosts {
            if let Some(routes) = host.get("routes").and_then(|r| r.as_array()) {
                for route in routes {
                    // Extract path from various formats
                    extract_path_from_route(route, &mut paths);

                    // Extract cluster from various formats
                    extract_clusters_from_route(route, &mut clusters);
                }
            }
        }
    }

    let path_prefix = paths.first().cloned().unwrap_or_else(|| "/".to_string());
    let cluster_summary = if clusters.is_empty() {
        "none".to_string()
    } else {
        clusters.into_iter().collect::<Vec<_>>().join(", ")
    };

    (path_prefix, cluster_summary)
}

/// Extract path from route configuration
fn extract_path_from_route(route: &Value, paths: &mut Vec<String>) {
    // Check for match.path in various formats
    if let Some(match_obj) = route.get("match") {
        if let Some(path) = match_obj.get("path") {
            // MCP format: {"type": "prefix", "value": "/api"}
            if let Some(value) = path.get("value").and_then(|v| v.as_str()) {
                paths.push(value.to_string());
            }
            // MCP template format
            else if let Some(template) = path.get("template").and_then(|t| t.as_str()) {
                paths.push(format!("template:{}", template));
            }
            // Internal format: {"Prefix": "/api"}
            else if let Some(prefix) = path.get("Prefix").and_then(|p| p.as_str()) {
                paths.push(prefix.to_string());
            } else if let Some(exact) = path.get("Exact").and_then(|e| e.as_str()) {
                paths.push(exact.to_string());
            } else if let Some(template) = path.get("Template").and_then(|t| t.as_str()) {
                paths.push(format!("template:{}", template));
            }
        }
    }
}

/// Extract clusters from route configuration
fn extract_clusters_from_route(route: &Value, clusters: &mut std::collections::HashSet<String>) {
    if let Some(action) = route.get("action") {
        // MCP forward format: {"type": "forward", "cluster": "x"}
        if let Some(cluster) = action.get("cluster").and_then(|c| c.as_str()) {
            clusters.insert(cluster.to_string());
        }

        // Internal Cluster format: {"Cluster": {"name": "x"}}
        if let Some(cluster_obj) = action.get("Cluster") {
            if let Some(name) = cluster_obj.get("name").and_then(|n| n.as_str()) {
                clusters.insert(name.to_string());
            }
        }

        // MCP weighted format: {"type": "weighted", "clusters": [...]}
        if let Some(weighted) = action.get("clusters").and_then(|c| c.as_array()) {
            for wc in weighted {
                if let Some(name) = wc.get("name").and_then(|n| n.as_str()) {
                    clusters.insert(name.to_string());
                }
            }
        }

        // Internal WeightedClusters format
        if let Some(weighted_obj) = action.get("WeightedClusters") {
            if let Some(weighted) = weighted_obj.get("clusters").and_then(|c| c.as_array()) {
                for wc in weighted {
                    if let Some(name) = wc.get("name").and_then(|n| n.as_str()) {
                        clusters.insert(name.to_string());
                    }
                }
            }
        }
    }
}

/// Transform virtual hosts from MCP-friendly format to internal format.
///
/// MCP schema uses:
/// - path: `{"type": "prefix", "value": "/api"}`
/// - action: `{"type": "forward", "cluster": "x", "prefixRewrite": "/y"}`
///
/// Internal format uses Rust enum serialization:
/// - path: `{"Prefix": "/api"}`
/// - action: `{"Cluster": {"name": "x", "prefix_rewrite": "/y"}}`
pub fn transform_virtual_hosts_for_internal(virtual_hosts: &Value) -> Value {
    let Some(hosts) = virtual_hosts.as_array() else {
        return virtual_hosts.clone();
    };

    let transformed_hosts: Vec<Value> = hosts
        .iter()
        .map(|host| {
            let mut new_host = host.clone();
            if let Some(routes) = host.get("routes").and_then(|r| r.as_array()) {
                let transformed_routes: Vec<Value> = routes.iter().map(transform_route).collect();
                new_host["routes"] = json!(transformed_routes);
            }
            new_host
        })
        .collect();

    json!(transformed_hosts)
}

/// Transform a single route from MCP format to internal format.
fn transform_route(route: &Value) -> Value {
    let mut new_route = serde_json::Map::new();

    // Copy name if present
    if let Some(name) = route.get("name") {
        new_route.insert("name".to_string(), name.clone());
    }

    // Transform match.path
    if let Some(match_obj) = route.get("match") {
        let mut new_match = serde_json::Map::new();

        if let Some(path) = match_obj.get("path") {
            new_match.insert("path".to_string(), transform_path(path));
        }

        // Copy headers if present
        if let Some(headers) = match_obj.get("headers") {
            new_match.insert("headers".to_string(), headers.clone());
        }

        // Copy query_parameters if present
        if let Some(qp) = match_obj.get("query_parameters") {
            new_match.insert("query_parameters".to_string(), qp.clone());
        }

        new_route.insert("match".to_string(), json!(new_match));
    }

    // Transform action
    if let Some(action) = route.get("action") {
        new_route.insert("action".to_string(), transform_action(action));
    }

    // Copy typed_per_filter_config if present
    if let Some(tpfc) = route.get("typed_per_filter_config") {
        new_route.insert("typed_per_filter_config".to_string(), tpfc.clone());
    }

    json!(new_route)
}

/// Transform path from MCP format to internal enum format.
fn transform_path(path: &Value) -> Value {
    let path_type = path.get("type").and_then(|t| t.as_str()).unwrap_or("prefix");

    match path_type {
        "prefix" => {
            let value = path.get("value").and_then(|v| v.as_str()).unwrap_or("/");
            json!({"Prefix": value})
        }
        "exact" => {
            let value = path.get("value").and_then(|v| v.as_str()).unwrap_or("/");
            json!({"Exact": value})
        }
        "regex" => {
            let value = path.get("value").and_then(|v| v.as_str()).unwrap_or(".*");
            json!({"Regex": value})
        }
        "template" => {
            let value = path
                .get("template")
                .or_else(|| path.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or("/");
            json!({"Template": value})
        }
        _ => {
            // If already in internal format (e.g., {"Prefix": "/api"}), pass through
            path.clone()
        }
    }
}

/// Transform action from MCP format to internal enum format.
fn transform_action(action: &Value) -> Value {
    let action_type = action.get("type").and_then(|t| t.as_str()).unwrap_or("forward");

    match action_type {
        "forward" => {
            let mut cluster_obj = serde_json::Map::new();

            if let Some(cluster) = action.get("cluster").and_then(|c| c.as_str()) {
                cluster_obj.insert("name".to_string(), json!(cluster));
            }
            if let Some(timeout) = action.get("timeoutSeconds").and_then(|t| t.as_u64()) {
                cluster_obj.insert("timeout".to_string(), json!(timeout));
            }
            if let Some(prefix_rewrite) = action.get("prefixRewrite").and_then(|p| p.as_str()) {
                cluster_obj.insert("prefix_rewrite".to_string(), json!(prefix_rewrite));
            }
            if let Some(path_rewrite) = action.get("pathTemplateRewrite").and_then(|p| p.as_str()) {
                cluster_obj.insert("path_template_rewrite".to_string(), json!(path_rewrite));
            }
            if let Some(retry) = action.get("retryPolicy") {
                cluster_obj.insert("retry_policy".to_string(), retry.clone());
            }

            json!({"Cluster": cluster_obj})
        }
        "weighted" => {
            let mut weighted_obj = serde_json::Map::new();

            if let Some(clusters) = action.get("clusters").and_then(|c| c.as_array()) {
                weighted_obj.insert("clusters".to_string(), json!(clusters));
            }
            if let Some(total_weight) = action.get("totalWeight").and_then(|t| t.as_u64()) {
                weighted_obj.insert("total_weight".to_string(), json!(total_weight as u32));
            }

            json!({"WeightedClusters": weighted_obj})
        }
        "redirect" => {
            let mut redirect_obj = serde_json::Map::new();

            if let Some(host) = action.get("hostRedirect").and_then(|h| h.as_str()) {
                redirect_obj.insert("host_redirect".to_string(), json!(host));
            }
            if let Some(path) = action.get("pathRedirect").and_then(|p| p.as_str()) {
                redirect_obj.insert("path_redirect".to_string(), json!(path));
            }
            if let Some(code) = action.get("responseCode").and_then(|c| c.as_u64()) {
                redirect_obj.insert("response_code".to_string(), json!(code as u32));
            }

            json!({"Redirect": redirect_obj})
        }
        _ => {
            // If already in internal format, pass through
            action.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SimpleXdsConfig;
    use crate::storage::{create_pool, DatabaseConfig};
    use sqlx::Executor;

    fn create_test_config() -> DatabaseConfig {
        DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            auto_migrate: false,
            ..Default::default()
        }
    }

    async fn setup_state() -> Arc<XdsState> {
        let pool = create_pool(&create_test_config()).await.expect("pool");

        pool.execute(
            r#"
            CREATE TABLE IF NOT EXISTS route_configs (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                path_prefix TEXT NOT NULL,
                cluster_name TEXT NOT NULL,
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL DEFAULT 'native_api' CHECK (source IN ('native_api', 'openapi_import')),
                team TEXT,
                import_id TEXT,
                route_order INTEGER,
                headers TEXT,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
        "#,
        )
        .await
        .expect("create table");

        Arc::new(XdsState::with_database(SimpleXdsConfig::default(), pool))
    }

    fn sample_config() -> Value {
        json!({
            "name": "test-routes",
            "virtual_hosts": [{
                "name": "default",
                "domains": ["*"],
                "routes": [{
                    "name": "api",
                    "match": {"path": {"Prefix": "/api"}},
                    "action": {"Cluster": {"name": "api-cluster"}}
                }]
            }]
        })
    }

    #[tokio::test]
    async fn test_create_route_config_admin() {
        let state = setup_state().await;
        let ops = RouteConfigOperations::new(state);
        let auth = InternalAuthContext::admin();

        let req = CreateRouteConfigRequest {
            name: "test-routes".to_string(),
            team: Some("test-team".to_string()),
            config: sample_config(),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_ok());

        let op_result = result.unwrap();
        assert_eq!(op_result.data.name, "test-routes");
        assert!(op_result.message.is_some());
    }

    #[tokio::test]
    async fn test_create_route_config_wrong_team() {
        let state = setup_state().await;
        let ops = RouteConfigOperations::new(state);
        let auth = InternalAuthContext::for_team("team-a");

        let req = CreateRouteConfigRequest {
            name: "wrong-team-routes".to_string(),
            team: Some("team-b".to_string()), // Different team
            config: sample_config(),
        };

        let result = ops.create(req, &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::Forbidden { .. }));
    }

    #[tokio::test]
    async fn test_get_route_config_not_found() {
        let state = setup_state().await;
        let ops = RouteConfigOperations::new(state);
        let auth = InternalAuthContext::admin();

        let result = ops.get("nonexistent", &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_delete_default_route_blocked() {
        let state = setup_state().await;
        let ops = RouteConfigOperations::new(state);
        let auth = InternalAuthContext::admin();

        // Note: The default route is "default-gateway-routes" (with an "s")
        let result = ops.delete("default-gateway-routes", &auth).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), InternalError::Forbidden { .. }));
    }

    #[test]
    fn test_transform_mcp_path() {
        let mcp_path = json!({"type": "prefix", "value": "/api"});
        let internal = transform_path(&mcp_path);
        assert_eq!(internal, json!({"Prefix": "/api"}));

        let mcp_exact = json!({"type": "exact", "value": "/health"});
        let internal_exact = transform_path(&mcp_exact);
        assert_eq!(internal_exact, json!({"Exact": "/health"}));
    }

    #[test]
    fn test_transform_mcp_action() {
        let mcp_action = json!({
            "type": "forward",
            "cluster": "api-cluster",
            "prefixRewrite": "/v1"
        });
        let internal = transform_action(&mcp_action);
        assert_eq!(internal, json!({"Cluster": {"name": "api-cluster", "prefix_rewrite": "/v1"}}));

        let mcp_weighted = json!({
            "type": "weighted",
            "clusters": [
                {"name": "stable", "weight": 90},
                {"name": "canary", "weight": 10}
            ]
        });
        let internal_weighted = transform_action(&mcp_weighted);
        let expected = json!({"WeightedClusters": {"clusters": [{"name": "stable", "weight": 90}, {"name": "canary", "weight": 10}]}});
        assert_eq!(internal_weighted, expected);
    }

    #[test]
    fn test_summarize_config() {
        let config = json!({
            "virtual_hosts": [{
                "name": "default",
                "domains": ["*"],
                "routes": [
                    {
                        "match": {"path": {"Prefix": "/api"}},
                        "action": {"Cluster": {"name": "api-cluster"}}
                    },
                    {
                        "match": {"path": {"Prefix": "/admin"}},
                        "action": {"Cluster": {"name": "admin-cluster"}}
                    }
                ]
            }]
        });

        let (path_prefix, cluster_summary) = summarize_config(&config);
        assert_eq!(path_prefix, "/api");
        assert!(cluster_summary.contains("api-cluster"));
        assert!(cluster_summary.contains("admin-cluster"));
    }
}
