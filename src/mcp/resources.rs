//! MCP Resources Module
//!
//! Provides resource listing and reading for Flowplane configuration entities.
//! Resources are exposed via URIs in the format: `flowplane://{type}/{team}/{name}`

use crate::mcp::error::McpError;
use crate::mcp::protocol::{Resource, ResourceContent, ResourcesListResult};
use crate::storage::DbPool;
use sqlx::FromRow;
use std::sync::Arc;
use tracing::{debug, error};

/// Resource types supported by Flowplane
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceType {
    Cluster,
    Listener,
    Route,
    Filter,
}

impl ResourceType {
    /// Get the URI scheme prefix for this resource type
    pub fn scheme(&self) -> &'static str {
        match self {
            ResourceType::Cluster => "flowplane://clusters",
            ResourceType::Listener => "flowplane://listeners",
            ResourceType::Route => "flowplane://routes",
            ResourceType::Filter => "flowplane://filters",
        }
    }

    /// Get the MIME type for this resource
    pub fn mime_type(&self) -> &'static str {
        "application/json"
    }
}

/// Parsed resource URI
#[derive(Debug, Clone)]
pub struct ResourceUri {
    pub resource_type: ResourceType,
    pub team: String,
    pub name: String,
}

impl ResourceUri {
    /// Parse a resource URI string
    ///
    /// Expected format: `flowplane://{type}/{team}/{name}`
    pub fn parse(uri: &str) -> Result<Self, McpError> {
        // Check scheme prefix
        if !uri.starts_with("flowplane://") {
            return Err(McpError::InvalidParams(format!(
                "Invalid resource URI scheme. Expected 'flowplane://', got: {}",
                uri
            )));
        }

        let path = &uri["flowplane://".len()..];
        let parts: Vec<&str> = path.split('/').collect();

        if parts.len() != 3 {
            return Err(McpError::InvalidParams(format!(
                "Invalid resource URI format. Expected 'flowplane://{{type}}/{{team}}/{{name}}', got: {}",
                uri
            )));
        }

        let resource_type = match parts[0] {
            "clusters" => ResourceType::Cluster,
            "listeners" => ResourceType::Listener,
            "routes" => ResourceType::Route,
            "filters" => ResourceType::Filter,
            other => {
                return Err(McpError::InvalidParams(format!(
                    "Unknown resource type '{}'. Valid types: clusters, listeners, routes, filters",
                    other
                )));
            }
        };

        let team = parts[1].to_string();
        let name = parts[2].to_string();

        if team.is_empty() {
            return Err(McpError::InvalidParams(
                "Team name cannot be empty in resource URI".to_string(),
            ));
        }

        if name.is_empty() {
            return Err(McpError::InvalidParams(
                "Resource name cannot be empty in resource URI".to_string(),
            ));
        }

        Ok(Self { resource_type, team, name })
    }

    /// Build a resource URI string
    pub fn to_uri(&self) -> String {
        format!("{}/{}/{}", self.resource_type.scheme(), self.team, self.name)
    }
}

// -----------------------------------------------------------------------------
// Database Row Types
// -----------------------------------------------------------------------------

#[derive(FromRow)]
struct ClusterListRow {
    name: String,
    lb_policy: String,
    description: Option<String>,
}

#[derive(FromRow)]
struct ClusterDetailRow {
    name: String,
    lb_policy: String,
    connect_timeout_secs: Option<i64>,
    description: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(FromRow)]
struct ListenerListRow {
    name: String,
    address: String,
    port: i64,
    description: Option<String>,
}

#[derive(FromRow)]
struct ListenerDetailRow {
    name: String,
    address: String,
    port: i64,
    protocol: String,
    enabled: bool,
    description: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(FromRow)]
struct RouteListRow {
    name: String,
    cluster_name: String,
}

#[derive(FromRow)]
struct RouteDetailRow {
    name: String,
    cluster_name: String,
    path_prefix: Option<String>,
    path_exact: Option<String>,
    path_regex: Option<String>,
    headers: Option<String>,
    query_params: Option<String>,
    match_type: String,
    rule_order: i64,
    enabled: bool,
    created_at: String,
    updated_at: String,
}

#[derive(FromRow)]
struct FilterListRow {
    name: String,
    filter_type: String,
}

#[derive(FromRow)]
struct FilterDetailRow {
    name: String,
    filter_type: String,
    config: Option<String>,
    enabled: bool,
    created_at: String,
    updated_at: String,
}

// -----------------------------------------------------------------------------
// Resource Listing and Reading Functions
// -----------------------------------------------------------------------------

/// List all resources for a team
pub async fn list_resources(
    db_pool: &Arc<DbPool>,
    team: &str,
) -> Result<ResourcesListResult, McpError> {
    debug!(team = %team, "Listing resources for team");

    let mut resources = Vec::new();

    // List clusters
    match list_cluster_resources(db_pool, team).await {
        Ok(mut r) => resources.append(&mut r),
        Err(e) => error!(error = %e, "Failed to list cluster resources"),
    }

    // List listeners
    match list_listener_resources(db_pool, team).await {
        Ok(mut r) => resources.append(&mut r),
        Err(e) => error!(error = %e, "Failed to list listener resources"),
    }

    // List routes
    match list_route_resources(db_pool, team).await {
        Ok(mut r) => resources.append(&mut r),
        Err(e) => error!(error = %e, "Failed to list route resources"),
    }

    // List filters
    match list_filter_resources(db_pool, team).await {
        Ok(mut r) => resources.append(&mut r),
        Err(e) => error!(error = %e, "Failed to list filter resources"),
    }

    debug!(count = resources.len(), team = %team, "Listed resources");

    Ok(ResourcesListResult { resources, next_cursor: None })
}

/// Read a specific resource by URI
pub async fn read_resource(db_pool: &Arc<DbPool>, uri: &str) -> Result<ResourceContent, McpError> {
    let parsed = ResourceUri::parse(uri)?;

    debug!(
        uri = %uri,
        resource_type = ?parsed.resource_type,
        team = %parsed.team,
        name = %parsed.name,
        "Reading resource"
    );

    let content = match parsed.resource_type {
        ResourceType::Cluster => read_cluster(db_pool, &parsed.team, &parsed.name).await,
        ResourceType::Listener => read_listener(db_pool, &parsed.team, &parsed.name).await,
        ResourceType::Route => read_route(db_pool, &parsed.team, &parsed.name).await,
        ResourceType::Filter => read_filter(db_pool, &parsed.team, &parsed.name).await,
    }?;

    Ok(ResourceContent {
        uri: uri.to_string(),
        mime_type: Some("application/json".to_string()),
        text: Some(content),
        blob: None,
    })
}

// -----------------------------------------------------------------------------
// Cluster Resources
// -----------------------------------------------------------------------------

async fn list_cluster_resources(
    db_pool: &Arc<DbPool>,
    team: &str,
) -> Result<Vec<Resource>, McpError> {
    let rows: Vec<ClusterListRow> = sqlx::query_as(
        "SELECT name, lb_policy, description FROM clusters WHERE team = $1 ORDER BY name",
    )
    .bind(team)
    .fetch_all(db_pool.as_ref())
    .await
    .map_err(McpError::DatabaseError)?;

    let resources = rows
        .into_iter()
        .map(|row| Resource {
            uri: format!("flowplane://clusters/{}/{}", team, row.name),
            name: row.name.clone(),
            description: row
                .description
                .or_else(|| Some(format!("Cluster: {} ({})", row.name, row.lb_policy))),
            mime_type: Some("application/json".to_string()),
        })
        .collect();

    Ok(resources)
}

async fn read_cluster(db_pool: &Arc<DbPool>, team: &str, name: &str) -> Result<String, McpError> {
    let row: ClusterDetailRow = sqlx::query_as(
        "SELECT name, lb_policy, connect_timeout_secs, description, created_at, updated_at \
         FROM clusters WHERE team = $1 AND name = $2",
    )
    .bind(team)
    .bind(name)
    .fetch_optional(db_pool.as_ref())
    .await
    .map_err(McpError::DatabaseError)?
    .ok_or_else(|| McpError::ResourceNotFound(format!("flowplane://clusters/{}/{}", team, name)))?;

    let data = serde_json::json!({
        "name": row.name,
        "team": team,
        "lb_policy": row.lb_policy,
        "connect_timeout_secs": row.connect_timeout_secs,
        "description": row.description,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    });

    serde_json::to_string_pretty(&data).map_err(McpError::SerializationError)
}

// -----------------------------------------------------------------------------
// Listener Resources
// -----------------------------------------------------------------------------

async fn list_listener_resources(
    db_pool: &Arc<DbPool>,
    team: &str,
) -> Result<Vec<Resource>, McpError> {
    let rows: Vec<ListenerListRow> = sqlx::query_as(
        "SELECT name, address, port, description FROM listeners WHERE team = $1 ORDER BY name",
    )
    .bind(team)
    .fetch_all(db_pool.as_ref())
    .await
    .map_err(McpError::DatabaseError)?;

    let resources = rows
        .into_iter()
        .map(|row| Resource {
            uri: format!("flowplane://listeners/{}/{}", team, row.name),
            name: row.name.clone(),
            description: row
                .description
                .or_else(|| Some(format!("Listener: {} ({}:{})", row.name, row.address, row.port))),
            mime_type: Some("application/json".to_string()),
        })
        .collect();

    Ok(resources)
}

async fn read_listener(db_pool: &Arc<DbPool>, team: &str, name: &str) -> Result<String, McpError> {
    let row: ListenerDetailRow = sqlx::query_as(
        "SELECT name, address, port, protocol, enabled, description, created_at, updated_at \
         FROM listeners WHERE team = $1 AND name = $2",
    )
    .bind(team)
    .bind(name)
    .fetch_optional(db_pool.as_ref())
    .await
    .map_err(McpError::DatabaseError)?
    .ok_or_else(|| {
        McpError::ResourceNotFound(format!("flowplane://listeners/{}/{}", team, name))
    })?;

    let data = serde_json::json!({
        "name": row.name,
        "team": team,
        "address": row.address,
        "port": row.port,
        "protocol": row.protocol,
        "enabled": row.enabled,
        "description": row.description,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    });

    serde_json::to_string_pretty(&data).map_err(McpError::SerializationError)
}

// -----------------------------------------------------------------------------
// Route Resources
// -----------------------------------------------------------------------------

async fn list_route_resources(
    db_pool: &Arc<DbPool>,
    team: &str,
) -> Result<Vec<Resource>, McpError> {
    let rows: Vec<RouteListRow> = sqlx::query_as(
        "SELECT name, cluster_name FROM route_configs WHERE team = $1 ORDER BY name",
    )
    .bind(team)
    .fetch_all(db_pool.as_ref())
    .await
    .map_err(McpError::DatabaseError)?;

    let resources = rows
        .into_iter()
        .map(|row| Resource {
            uri: format!("flowplane://routes/{}/{}", team, row.name),
            name: row.name.clone(),
            description: Some(format!("Route: {} (cluster: {})", row.name, row.cluster_name)),
            mime_type: Some("application/json".to_string()),
        })
        .collect();

    Ok(resources)
}

async fn read_route(db_pool: &Arc<DbPool>, team: &str, name: &str) -> Result<String, McpError> {
    let row: RouteDetailRow = sqlx::query_as(
        "SELECT name, cluster_name, path_prefix, path_exact, path_regex, \
         headers, query_params, match_type, rule_order, enabled, created_at, updated_at \
         FROM route_configs WHERE team = $1 AND name = $2",
    )
    .bind(team)
    .bind(name)
    .fetch_optional(db_pool.as_ref())
    .await
    .map_err(McpError::DatabaseError)?
    .ok_or_else(|| McpError::ResourceNotFound(format!("flowplane://routes/{}/{}", team, name)))?;

    // Parse headers and query_params if they exist
    let headers: Option<serde_json::Value> =
        row.headers.as_ref().and_then(|h| serde_json::from_str(h).ok());
    let query_params: Option<serde_json::Value> =
        row.query_params.as_ref().and_then(|q| serde_json::from_str(q).ok());

    let data = serde_json::json!({
        "name": row.name,
        "team": team,
        "cluster_name": row.cluster_name,
        "path_prefix": row.path_prefix,
        "path_exact": row.path_exact,
        "path_regex": row.path_regex,
        "headers": headers,
        "query_params": query_params,
        "match_type": row.match_type,
        "rule_order": row.rule_order,
        "enabled": row.enabled,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    });

    serde_json::to_string_pretty(&data).map_err(McpError::SerializationError)
}

// -----------------------------------------------------------------------------
// Filter Resources
// -----------------------------------------------------------------------------

async fn list_filter_resources(
    db_pool: &Arc<DbPool>,
    team: &str,
) -> Result<Vec<Resource>, McpError> {
    let rows: Vec<FilterListRow> =
        sqlx::query_as("SELECT name, filter_type FROM filters WHERE team = $1 ORDER BY name")
            .bind(team)
            .fetch_all(db_pool.as_ref())
            .await
            .map_err(McpError::DatabaseError)?;

    let resources = rows
        .into_iter()
        .map(|row| Resource {
            uri: format!("flowplane://filters/{}/{}", team, row.name),
            name: row.name.clone(),
            description: Some(format!("Filter: {} (type: {})", row.name, row.filter_type)),
            mime_type: Some("application/json".to_string()),
        })
        .collect();

    Ok(resources)
}

async fn read_filter(db_pool: &Arc<DbPool>, team: &str, name: &str) -> Result<String, McpError> {
    let row: FilterDetailRow = sqlx::query_as(
        "SELECT name, filter_type, config, enabled, created_at, updated_at \
         FROM filters WHERE team = $1 AND name = $2",
    )
    .bind(team)
    .bind(name)
    .fetch_optional(db_pool.as_ref())
    .await
    .map_err(McpError::DatabaseError)?
    .ok_or_else(|| McpError::ResourceNotFound(format!("flowplane://filters/{}/{}", team, name)))?;

    // Parse config JSON
    let config: Option<serde_json::Value> =
        row.config.as_ref().and_then(|c| serde_json::from_str(c).ok());

    let data = serde_json::json!({
        "name": row.name,
        "team": team,
        "filter_type": row.filter_type,
        "config": config,
        "enabled": row.enabled,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    });

    serde_json::to_string_pretty(&data).map_err(McpError::SerializationError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cluster_uri() {
        let uri = "flowplane://clusters/my-team/backend-cluster";
        let parsed = ResourceUri::parse(uri).unwrap();

        assert_eq!(parsed.resource_type, ResourceType::Cluster);
        assert_eq!(parsed.team, "my-team");
        assert_eq!(parsed.name, "backend-cluster");
    }

    #[test]
    fn test_parse_listener_uri() {
        let uri = "flowplane://listeners/prod/http-listener";
        let parsed = ResourceUri::parse(uri).unwrap();

        assert_eq!(parsed.resource_type, ResourceType::Listener);
        assert_eq!(parsed.team, "prod");
        assert_eq!(parsed.name, "http-listener");
    }

    #[test]
    fn test_parse_route_uri() {
        let uri = "flowplane://routes/test-team/api-route";
        let parsed = ResourceUri::parse(uri).unwrap();

        assert_eq!(parsed.resource_type, ResourceType::Route);
        assert_eq!(parsed.team, "test-team");
        assert_eq!(parsed.name, "api-route");
    }

    #[test]
    fn test_parse_filter_uri() {
        let uri = "flowplane://filters/staging/jwt-auth";
        let parsed = ResourceUri::parse(uri).unwrap();

        assert_eq!(parsed.resource_type, ResourceType::Filter);
        assert_eq!(parsed.team, "staging");
        assert_eq!(parsed.name, "jwt-auth");
    }

    #[test]
    fn test_parse_invalid_scheme() {
        let uri = "http://clusters/team/name";
        let result = ResourceUri::parse(uri);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::InvalidParams(_)));
    }

    #[test]
    fn test_parse_invalid_format_missing_parts() {
        let uri = "flowplane://clusters/team";
        let result = ResourceUri::parse(uri);

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_unknown_resource_type() {
        let uri = "flowplane://unknown/team/name";
        let result = ResourceUri::parse(uri);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown resource type"));
    }

    #[test]
    fn test_parse_empty_team() {
        let uri = "flowplane://clusters//name";
        let result = ResourceUri::parse(uri);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Team name cannot be empty"));
    }

    #[test]
    fn test_parse_empty_name() {
        let uri = "flowplane://clusters/team/";
        let result = ResourceUri::parse(uri);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Resource name cannot be empty"));
    }

    #[test]
    fn test_resource_uri_to_uri() {
        let resource = ResourceUri {
            resource_type: ResourceType::Cluster,
            team: "my-team".to_string(),
            name: "my-cluster".to_string(),
        };

        assert_eq!(resource.to_uri(), "flowplane://clusters/my-team/my-cluster");
    }

    #[test]
    fn test_resource_type_scheme() {
        assert_eq!(ResourceType::Cluster.scheme(), "flowplane://clusters");
        assert_eq!(ResourceType::Listener.scheme(), "flowplane://listeners");
        assert_eq!(ResourceType::Route.scheme(), "flowplane://routes");
        assert_eq!(ResourceType::Filter.scheme(), "flowplane://filters");
    }

    #[test]
    fn test_resource_type_mime_type() {
        assert_eq!(ResourceType::Cluster.mime_type(), "application/json");
        assert_eq!(ResourceType::Listener.mime_type(), "application/json");
        assert_eq!(ResourceType::Route.mime_type(), "application/json");
        assert_eq!(ResourceType::Filter.mime_type(), "application/json");
    }
}
