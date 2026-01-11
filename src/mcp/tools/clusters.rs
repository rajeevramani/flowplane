//! MCP Tools for Cluster Control Plane Operations
//!
//! Provides tools for querying and inspecting cluster configurations via the MCP protocol.

use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::storage::repositories::cluster::ClusterRepository;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tracing::instrument;

/// Returns the MCP tool definition for listing clusters.
///
/// This tool supports pagination via `limit` and `offset` parameters.
pub fn cp_list_clusters_tool() -> Tool {
    Tool {
        name: "cp_list_clusters".to_string(),
        description: "List all clusters in the Flowplane control plane. Returns cluster configurations with names, service names, and metadata. Supports pagination via limit and offset parameters.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of clusters to return (default: 50, max: 1000)",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 50
                },
                "offset": {
                    "type": "integer",
                    "description": "Number of clusters to skip for pagination (default: 0)",
                    "minimum": 0,
                    "default": 0
                }
            }
        }),
    }
}

/// Returns the MCP tool definition for getting a cluster by name.
///
/// Requires a `name` parameter to identify the cluster.
pub fn cp_get_cluster_tool() -> Tool {
    Tool {
        name: "cp_get_cluster".to_string(),
        description: "Get detailed information about a specific cluster by name. Returns the cluster's complete configuration, metadata, and current version.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the cluster to retrieve"
                }
            },
            "required": ["name"]
        }),
    }
}

/// Execute the cp_list_clusters tool.
///
/// Lists clusters with pagination, returning pretty-printed JSON output.
///
/// # Arguments
///
/// * `db_pool` - Database connection pool
/// * `team` - Team identifier for multi-tenancy filtering
/// * `args` - Tool arguments containing optional `limit` and `offset`
///
/// # Returns
///
/// A `ToolCallResult` with cluster list as pretty-printed JSON text.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_list_clusters")]
pub async fn execute_list_clusters(
    db_pool: &SqlitePool,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let limit = args.get("limit").and_then(|v| v.as_i64()).map(|v| v as i32).or(Some(50));

    let offset = args.get("offset").and_then(|v| v.as_i64()).map(|v| v as i32).or(Some(0));

    tracing::debug!(
        team = %team,
        limit = ?limit,
        offset = ?offset,
        "Listing clusters for team"
    );

    let repo = ClusterRepository::new(db_pool.clone());

    // For team-based queries, use list_by_teams to enforce multi-tenancy
    let clusters = if team.is_empty() {
        repo.list(limit, offset).await
    } else {
        repo.list_by_teams(&[team.to_string()], true, limit, offset).await
    }
    .map_err(|e| McpError::DatabaseError(sqlx::Error::Protocol(e.to_string())))?;

    // Build output with cluster summaries
    let cluster_summaries: Vec<Value> = clusters
        .iter()
        .map(|cluster| {
            let mut summary = json!({
                "name": cluster.name,
                "service_name": cluster.service_name,
                "version": cluster.version,
                "source": cluster.source,
                "team": cluster.team,
                "created_at": cluster.created_at.to_rfc3339(),
                "updated_at": cluster.updated_at.to_rfc3339(),
            });

            // Parse configuration to extract description/tags if present
            if let Ok(config) = serde_json::from_str::<Value>(&cluster.configuration) {
                if let Some(description) = config.get("description") {
                    summary["description"] = description.clone();
                }
                if let Some(tags) = config.get("tags") {
                    summary["tags"] = tags.clone();
                }
            }

            summary
        })
        .collect();

    let output = json!({
        "clusters": cluster_summaries,
        "count": clusters.len(),
        "limit": limit,
        "offset": offset,
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        cluster_count = clusters.len(),
        "Successfully listed clusters"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute the cp_get_cluster tool.
///
/// Retrieves a specific cluster by name, returning detailed configuration.
///
/// # Arguments
///
/// * `db_pool` - Database connection pool
/// * `team` - Team identifier for access control
/// * `args` - Tool arguments containing required `name` field
///
/// # Returns
///
/// A `ToolCallResult` with cluster details as pretty-printed JSON, or
/// `ResourceNotFound` error if the cluster doesn't exist.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_get_cluster")]
pub async fn execute_get_cluster(
    db_pool: &SqlitePool,
    team: &str,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("Missing required parameter: name".to_string()))?;

    tracing::debug!(
        team = %team,
        cluster_name = %name,
        "Getting cluster by name"
    );

    let repo = ClusterRepository::new(db_pool.clone());
    let cluster = repo.get_by_name(name).await.map_err(|e| {
        if e.to_string().contains("not found") {
            McpError::ResourceNotFound(format!("Cluster '{}' not found", name))
        } else {
            McpError::DatabaseError(sqlx::Error::Protocol(e.to_string()))
        }
    })?;

    // Verify team access if team is specified
    if !team.is_empty() {
        if let Some(cluster_team) = &cluster.team {
            if cluster_team != team {
                return Err(McpError::ResourceNotFound(format!("Cluster '{}' not found", name)));
            }
        }
    }

    // Parse configuration JSON for pretty output
    let configuration: Value =
        serde_json::from_str(&cluster.configuration).map_err(McpError::SerializationError)?;

    let output = json!({
        "id": cluster.id.to_string(),
        "name": cluster.name,
        "service_name": cluster.service_name,
        "configuration": configuration,
        "version": cluster.version,
        "source": cluster.source,
        "team": cluster.team,
        "import_id": cluster.import_id,
        "created_at": cluster.created_at.to_rfc3339(),
        "updated_at": cluster.updated_at.to_rfc3339(),
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;

    tracing::info!(
        team = %team,
        cluster_name = %name,
        "Successfully retrieved cluster"
    );

    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseConfig;
    use crate::storage::create_pool;
    use crate::storage::repositories::cluster::CreateClusterRequest;

    async fn setup_test_db() -> SqlitePool {
        let config = DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            max_connections: 5,
            min_connections: 1,
            connect_timeout_seconds: 5,
            idle_timeout_seconds: 0,
            auto_migrate: false,
        };
        let pool = create_pool(&config).await.expect("Failed to create pool");

        // Run migrations
        sqlx::migrate!("./migrations").run(&pool).await.expect("Failed to run migrations");

        pool
    }

    /// Create a test team in the database
    async fn create_test_team(pool: &SqlitePool, team_name: &str) {
        let team_id = format!("team-{}", uuid::Uuid::new_v4());
        sqlx::query("INSERT INTO teams (id, name, display_name, status) VALUES ($1, $2, $3, $4)")
            .bind(&team_id)
            .bind(team_name)
            .bind(format!("Test {}", team_name))
            .bind("active")
            .execute(pool)
            .await
            .expect("Failed to create test team");
    }

    #[tokio::test]
    async fn test_cp_list_clusters_tool_definition() {
        let tool = cp_list_clusters_tool();
        assert_eq!(tool.name, "cp_list_clusters");
        assert!(tool.description.contains("List all clusters"));
        assert!(tool.input_schema.get("properties").is_some());
    }

    #[tokio::test]
    async fn test_cp_get_cluster_tool_definition() {
        let tool = cp_get_cluster_tool();
        assert_eq!(tool.name, "cp_get_cluster");
        assert!(tool.description.contains("Get detailed information"));
        assert!(tool.input_schema.get("required").is_some());
    }

    #[tokio::test]
    async fn test_execute_list_clusters_empty() {
        let pool = setup_test_db().await;
        let args = json!({});

        let result = execute_list_clusters(&pool, "test-team", args).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        assert_eq!(tool_result.content.len(), 1);

        if let ContentBlock::Text { text } = &tool_result.content[0] {
            let output: Value = serde_json::from_str(text).unwrap();
            assert_eq!(output["count"], 0);
        } else {
            panic!("Expected text content block");
        }
    }

    #[tokio::test]
    async fn test_execute_list_clusters_with_data() {
        let pool = setup_test_db().await;

        // Create the team first (required by foreign key constraint)
        create_test_team(&pool, "test-team").await;

        let repo = ClusterRepository::new(pool.clone());

        // Create test clusters
        repo.create(CreateClusterRequest {
            name: "test-cluster-1".to_string(),
            service_name: "service-1".to_string(),
            configuration: json!({"endpoints": []}),
            team: Some("test-team".to_string()),
            import_id: None,
        })
        .await
        .expect("Failed to create cluster");

        let args = json!({"limit": 10, "offset": 0});
        let result = execute_list_clusters(&pool, "test-team", args).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        if let ContentBlock::Text { text } = &tool_result.content[0] {
            let output: Value = serde_json::from_str(text).unwrap();
            assert_eq!(output["count"], 1);
            assert_eq!(output["clusters"][0]["name"], "test-cluster-1");
        }
    }

    #[tokio::test]
    async fn test_execute_get_cluster_not_found() {
        let pool = setup_test_db().await;
        let args = json!({"name": "non-existent-cluster"});

        let result = execute_get_cluster(&pool, "test-team", args).await;
        assert!(result.is_err());

        if let Err(McpError::ResourceNotFound(msg)) = result {
            assert!(msg.contains("not found"));
        } else {
            panic!("Expected ResourceNotFound error");
        }
    }

    #[tokio::test]
    async fn test_execute_get_cluster_success() {
        let pool = setup_test_db().await;

        // Create the team first (required by foreign key constraint)
        create_test_team(&pool, "test-team").await;

        let repo = ClusterRepository::new(pool.clone());

        // Create test cluster
        repo.create(CreateClusterRequest {
            name: "test-cluster".to_string(),
            service_name: "test-service".to_string(),
            configuration: json!({"endpoints": [], "description": "Test cluster"}),
            team: Some("test-team".to_string()),
            import_id: None,
        })
        .await
        .expect("Failed to create cluster");

        let args = json!({"name": "test-cluster"});
        let result = execute_get_cluster(&pool, "test-team", args).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        if let ContentBlock::Text { text } = &tool_result.content[0] {
            let output: Value = serde_json::from_str(text).unwrap();
            assert_eq!(output["name"], "test-cluster");
            assert_eq!(output["service_name"], "test-service");
            assert_eq!(output["configuration"]["description"], "Test cluster");
        }
    }

    #[tokio::test]
    async fn test_execute_get_cluster_missing_name() {
        let pool = setup_test_db().await;
        let args = json!({});

        let result = execute_get_cluster(&pool, "test-team", args).await;
        assert!(result.is_err());

        if let Err(McpError::InvalidParams(msg)) = result {
            assert!(msg.contains("Missing required parameter: name"));
        } else {
            panic!("Expected InvalidParams error");
        }
    }
}
