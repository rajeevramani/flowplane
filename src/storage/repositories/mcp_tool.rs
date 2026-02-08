//! MCP tool repository for managing MCP tool definitions
//!
//! This module provides CRUD operations for MCP tools, which represent callable
//! tools exposed to AI assistants via the Model Context Protocol.

use crate::domain::{McpToolCategory, McpToolId, McpToolSourceType, RouteId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::instrument;

/// Database row structure for MCP tools
#[derive(Debug, Clone, FromRow)]
struct McpToolRow {
    pub id: String,
    pub team: String,
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub source_type: String,
    pub input_schema: String,
    pub output_schema: Option<String>,
    pub learned_schema_id: Option<i64>,
    pub schema_source: Option<String>,
    pub route_id: Option<String>,
    pub http_method: Option<String>,
    pub http_path: Option<String>,
    pub cluster_name: Option<String>,
    pub listener_port: Option<i64>,
    pub host_header: Option<String>,
    pub enabled: bool,
    pub confidence: Option<f64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// MCP tool data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolData {
    pub id: McpToolId,
    pub team: String,
    pub name: String,
    pub description: Option<String>,
    pub category: McpToolCategory,
    pub source_type: McpToolSourceType,
    pub input_schema: serde_json::Value,
    pub output_schema: Option<serde_json::Value>,
    pub learned_schema_id: Option<i64>,
    pub schema_source: Option<String>,
    pub route_id: Option<RouteId>,
    pub http_method: Option<String>,
    pub http_path: Option<String>,
    pub cluster_name: Option<String>,
    pub listener_port: Option<i64>,
    /// Host header to use when executing this tool (for upstream routing)
    pub host_header: Option<String>,
    pub enabled: bool,
    pub confidence: Option<f64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl crate::api::handlers::TeamOwned for McpToolData {
    fn team(&self) -> Option<&str> {
        Some(&self.team)
    }

    fn resource_name(&self) -> &str {
        &self.name
    }

    fn resource_type() -> &'static str {
        "MCP tool"
    }

    fn resource_type_metric() -> &'static str {
        "mcp_tools"
    }
}

impl TryFrom<McpToolRow> for McpToolData {
    type Error = FlowplaneError;

    fn try_from(row: McpToolRow) -> Result<Self> {
        let category = row
            .category
            .parse()
            .map_err(|e| FlowplaneError::validation(format!("Invalid category: {}", e)))?;

        let source_type = row
            .source_type
            .parse()
            .map_err(|e| FlowplaneError::validation(format!("Invalid source_type: {}", e)))?;

        let input_schema = serde_json::from_str(&row.input_schema)
            .map_err(|e| FlowplaneError::validation(format!("Invalid input_schema JSON: {}", e)))?;

        let output_schema =
            row.output_schema.as_ref().map(|s| serde_json::from_str(s)).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid output_schema JSON: {}", e)),
            )?;

        Ok(Self {
            id: McpToolId::from_string(row.id),
            team: row.team,
            name: row.name,
            description: row.description,
            category,
            source_type,
            input_schema,
            output_schema,
            learned_schema_id: row.learned_schema_id,
            schema_source: row.schema_source,
            route_id: row.route_id.map(RouteId::from_string),
            http_method: row.http_method,
            http_path: row.http_path,
            cluster_name: row.cluster_name,
            listener_port: row.listener_port,
            host_header: row.host_header,
            enabled: row.enabled,
            confidence: row.confidence,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// Create MCP tool request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMcpToolRequest {
    pub team: String,
    pub name: String,
    pub description: Option<String>,
    pub category: McpToolCategory,
    pub source_type: McpToolSourceType,
    pub input_schema: serde_json::Value,
    pub output_schema: Option<serde_json::Value>,
    pub learned_schema_id: Option<i64>,
    pub schema_source: Option<String>,
    pub route_id: Option<RouteId>,
    pub http_method: Option<String>,
    pub http_path: Option<String>,
    pub cluster_name: Option<String>,
    pub listener_port: Option<i64>,
    /// Host header to use when executing this tool (for upstream routing)
    pub host_header: Option<String>,
    pub enabled: bool,
    pub confidence: Option<f64>,
}

/// Update MCP tool request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMcpToolRequest {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub category: Option<McpToolCategory>,
    pub source_type: Option<McpToolSourceType>,
    pub input_schema: Option<serde_json::Value>,
    pub output_schema: Option<Option<serde_json::Value>>,
    pub learned_schema_id: Option<Option<i64>>,
    pub schema_source: Option<Option<String>>,
    pub route_id: Option<Option<RouteId>>,
    pub http_method: Option<Option<String>>,
    pub http_path: Option<Option<String>>,
    pub cluster_name: Option<Option<String>>,
    pub listener_port: Option<Option<i64>>,
    /// Host header to use when executing this tool (for upstream routing)
    pub host_header: Option<Option<String>>,
    pub enabled: Option<bool>,
    pub confidence: Option<Option<f64>>,
}

/// Repository for MCP tool data access
#[derive(Debug, Clone)]
pub struct McpToolRepository {
    pool: DbPool,
}

impl McpToolRepository {
    /// Create a new MCP tool repository
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create a new MCP tool
    #[instrument(skip(self, request), fields(tool_name = %request.name, team = %request.team), name = "db_create_mcp_tool")]
    pub async fn create(&self, request: CreateMcpToolRequest) -> Result<McpToolData> {
        let id = McpToolId::new();
        let now = chrono::Utc::now();

        let input_schema_json = serde_json::to_string(&request.input_schema)
            .map_err(|e| FlowplaneError::validation(format!("Invalid input_schema JSON: {}", e)))?;

        let output_schema_json =
            request.output_schema.as_ref().map(serde_json::to_string).transpose().map_err(|e| {
                FlowplaneError::validation(format!("Invalid output_schema JSON: {}", e))
            })?;

        let result = sqlx::query(
            "INSERT INTO mcp_tools (
                id, team, name, description, category, source_type, input_schema, output_schema,
                learned_schema_id, schema_source, route_id, http_method, http_path, cluster_name,
                listener_port, host_header, enabled, confidence, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)",
        )
        .bind(&id)
        .bind(&request.team)
        .bind(&request.name)
        .bind(&request.description)
        .bind(request.category.to_string())
        .bind(request.source_type.to_string())
        .bind(&input_schema_json)
        .bind(&output_schema_json)
        .bind(request.learned_schema_id)
        .bind(&request.schema_source)
        .bind(request.route_id.as_ref().map(|id| id.as_str()))
        .bind(&request.http_method)
        .bind(&request.http_path)
        .bind(&request.cluster_name)
        .bind(request.listener_port)
        .bind(&request.host_header)
        .bind(request.enabled)
        .bind(request.confidence)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, tool_name = %request.name, team = %request.team, "Failed to create MCP tool");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create MCP tool '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create MCP tool"));
        }

        tracing::info!(
            mcp_tool_id = %id,
            tool_name = %request.name,
            team = %request.team,
            category = %request.category,
            "Created MCP tool"
        );

        self.get_by_id(&id)
            .await?
            .ok_or_else(|| FlowplaneError::internal("Failed to retrieve created MCP tool"))
    }

    /// Get MCP tool by ID
    #[instrument(skip(self), fields(mcp_tool_id = %id), name = "db_get_mcp_tool_by_id")]
    pub async fn get_by_id(&self, id: &McpToolId) -> Result<Option<McpToolData>> {
        let row = sqlx::query_as::<sqlx::Postgres, McpToolRow>(
            "SELECT id, team, name, description, category, source_type, input_schema, output_schema,
                    learned_schema_id, schema_source, route_id, http_method, http_path, cluster_name,
                    listener_port, host_header, enabled, confidence, created_at, updated_at
             FROM mcp_tools WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, mcp_tool_id = %id, "Failed to get MCP tool by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get MCP tool with ID '{}'", id),
            }
        })?;

        row.map(McpToolData::try_from).transpose()
    }

    /// Get MCP tool by name and team
    #[instrument(skip(self), fields(team = %team, tool_name = %name), name = "db_get_mcp_tool_by_name")]
    pub async fn get_by_name(&self, team: &str, name: &str) -> Result<Option<McpToolData>> {
        let row = sqlx::query_as::<sqlx::Postgres, McpToolRow>(
            "SELECT id, team, name, description, category, source_type, input_schema, output_schema,
                    learned_schema_id, schema_source, route_id, http_method, http_path, cluster_name,
                    listener_port, host_header, enabled, confidence, created_at, updated_at
             FROM mcp_tools WHERE team = $1 AND name = $2",
        )
        .bind(team)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, tool_name = %name, "Failed to get MCP tool by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get MCP tool '{}' for team '{}'", name, team),
            }
        })?;

        row.map(McpToolData::try_from).transpose()
    }

    /// Get MCP tool by route ID
    #[instrument(skip(self), fields(route_id = %route_id), name = "db_get_mcp_tool_by_route_id")]
    pub async fn get_by_route_id(&self, route_id: &RouteId) -> Result<Option<McpToolData>> {
        let row = sqlx::query_as::<sqlx::Postgres, McpToolRow>(
            "SELECT id, team, name, description, category, source_type, input_schema, output_schema,
                    learned_schema_id, schema_source, route_id, http_method, http_path, cluster_name,
                    listener_port, host_header, enabled, confidence, created_at, updated_at
             FROM mcp_tools WHERE route_id = $1",
        )
        .bind(route_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %route_id, "Failed to get MCP tool by route ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get MCP tool for route '{}'", route_id),
            }
        })?;

        row.map(McpToolData::try_from).transpose()
    }

    /// List MCP tools by team with optional enabled filter
    #[instrument(skip(self), fields(team = %team, enabled_only = enabled_only), name = "db_list_mcp_tools_by_team")]
    pub async fn list_by_team(&self, team: &str, enabled_only: bool) -> Result<Vec<McpToolData>> {
        let query = if enabled_only {
            "SELECT id, team, name, description, category, source_type, input_schema, output_schema,
                    learned_schema_id, schema_source, route_id, http_method, http_path, cluster_name,
                    listener_port, host_header, enabled, confidence, created_at, updated_at
             FROM mcp_tools WHERE team = $1 AND enabled = 1
             ORDER BY created_at DESC"
        } else {
            "SELECT id, team, name, description, category, source_type, input_schema, output_schema,
                    learned_schema_id, schema_source, route_id, http_method, http_path, cluster_name,
                    listener_port, host_header, enabled, confidence, created_at, updated_at
             FROM mcp_tools WHERE team = $1
             ORDER BY created_at DESC"
        };

        let rows = sqlx::query_as::<sqlx::Postgres, McpToolRow>(query)
            .bind(team)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, team = %team, "Failed to list MCP tools by team");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to list MCP tools for team '{}'", team),
                }
            })?;

        rows.into_iter().map(McpToolData::try_from).collect()
    }

    /// List MCP tools by category
    #[instrument(skip(self), fields(team = %team, category = %category), name = "db_list_mcp_tools_by_category")]
    pub async fn list_by_category(
        &self,
        team: &str,
        category: McpToolCategory,
    ) -> Result<Vec<McpToolData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, McpToolRow>(
            "SELECT id, team, name, description, category, source_type, input_schema, output_schema,
                    learned_schema_id, schema_source, route_id, http_method, http_path, cluster_name,
                    listener_port, host_header, enabled, confidence, created_at, updated_at
             FROM mcp_tools WHERE team = $1 AND category = $2
             ORDER BY created_at DESC",
        )
        .bind(team)
        .bind(category.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, category = %category, "Failed to list MCP tools by category");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list MCP tools for team '{}' and category '{}'", team, category),
            }
        })?;

        rows.into_iter().map(McpToolData::try_from).collect()
    }

    /// Update MCP tool
    #[instrument(skip(self, request), fields(mcp_tool_id = %id), name = "db_update_mcp_tool")]
    pub async fn update(
        &self,
        id: &McpToolId,
        request: UpdateMcpToolRequest,
    ) -> Result<McpToolData> {
        // Get current tool
        let current = self.get_by_id(id).await?.ok_or_else(|| {
            FlowplaneError::not_found_msg(format!("MCP tool with ID '{}' not found", id))
        })?;

        let new_name = request.name.unwrap_or(current.name);
        let new_description = request.description.unwrap_or(current.description);
        let new_category = request.category.unwrap_or(current.category);
        let new_source_type = request.source_type.unwrap_or(current.source_type);
        let new_input_schema = request.input_schema.unwrap_or(current.input_schema);
        let new_output_schema = request.output_schema.unwrap_or(current.output_schema);
        let new_learned_schema_id = request.learned_schema_id.unwrap_or(current.learned_schema_id);
        let new_schema_source = request.schema_source.unwrap_or(current.schema_source);
        let new_route_id = request.route_id.unwrap_or(current.route_id);
        let new_http_method = request.http_method.unwrap_or(current.http_method);
        let new_http_path = request.http_path.unwrap_or(current.http_path);
        let new_cluster_name = request.cluster_name.unwrap_or(current.cluster_name);
        let new_listener_port = request.listener_port.unwrap_or(current.listener_port);
        let new_host_header = request.host_header.unwrap_or(current.host_header);
        let new_enabled = request.enabled.unwrap_or(current.enabled);
        let new_confidence = request.confidence.unwrap_or(current.confidence);

        let now = chrono::Utc::now();

        let input_schema_json = serde_json::to_string(&new_input_schema)
            .map_err(|e| FlowplaneError::validation(format!("Invalid input_schema JSON: {}", e)))?;

        let output_schema_json =
            new_output_schema.as_ref().map(serde_json::to_string).transpose().map_err(|e| {
                FlowplaneError::validation(format!("Invalid output_schema JSON: {}", e))
            })?;

        let result = sqlx::query(
            "UPDATE mcp_tools SET
                name = $1, description = $2, category = $3, source_type = $4, input_schema = $5,
                output_schema = $6, learned_schema_id = $7, schema_source = $8, route_id = $9,
                http_method = $10, http_path = $11, cluster_name = $12, listener_port = $13,
                host_header = $14, enabled = $15, confidence = $16, updated_at = $17
             WHERE id = $18",
        )
        .bind(&new_name)
        .bind(&new_description)
        .bind(new_category.to_string())
        .bind(new_source_type.to_string())
        .bind(&input_schema_json)
        .bind(&output_schema_json)
        .bind(new_learned_schema_id)
        .bind(&new_schema_source)
        .bind(new_route_id.as_ref().map(|id| id.as_str()))
        .bind(&new_http_method)
        .bind(&new_http_path)
        .bind(&new_cluster_name)
        .bind(new_listener_port)
        .bind(&new_host_header)
        .bind(new_enabled)
        .bind(new_confidence)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, mcp_tool_id = %id, "Failed to update MCP tool");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update MCP tool with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "MCP tool with ID '{}' not found",
                id
            )));
        }

        tracing::info!(
            mcp_tool_id = %id,
            tool_name = %new_name,
            "Updated MCP tool"
        );

        self.get_by_id(id).await?.ok_or_else(|| {
            FlowplaneError::not_found_msg(format!("MCP tool with ID '{}' not found", id))
        })
    }

    /// Delete MCP tool
    #[instrument(skip(self), fields(mcp_tool_id = %id), name = "db_delete_mcp_tool")]
    pub async fn delete(&self, id: &McpToolId) -> Result<()> {
        // Check if tool exists first
        let tool = self.get_by_id(id).await?.ok_or_else(|| {
            FlowplaneError::not_found_msg(format!("MCP tool with ID '{}' not found", id))
        })?;

        let result = sqlx::query("DELETE FROM mcp_tools WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, mcp_tool_id = %id, "Failed to delete MCP tool");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete MCP tool with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "MCP tool with ID '{}' not found",
                id
            )));
        }

        tracing::info!(
            mcp_tool_id = %id,
            tool_name = %tool.name,
            "Deleted MCP tool"
        );

        Ok(())
    }

    /// Count enabled MCP tools for a team.
    #[instrument(skip(self), fields(team = %team), name = "db_count_enabled_mcp_tools_by_team")]
    pub async fn count_enabled_by_team(&self, team: &str) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM mcp_tools WHERE team = $1 AND enabled = 1",
        )
        .bind(team)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to count enabled MCP tools by team");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count enabled MCP tools for team '{}'", team),
            }
        })?;

        Ok(count)
    }

    /// Count enabled MCP tools for multiple teams (supports admin bypass).
    ///
    /// If `teams` is empty, counts ALL enabled MCP tools across all teams (admin bypass).
    #[instrument(skip(self), fields(teams = ?teams.len()), name = "db_count_enabled_mcp_tools_by_teams")]
    pub async fn count_enabled_by_teams(&self, teams: &[String]) -> Result<i64> {
        // Single team: use existing optimized method
        if teams.len() == 1 {
            return self.count_enabled_by_team(&teams[0]).await;
        }

        // Admin bypass: empty teams = count all enabled MCP tools
        if teams.is_empty() {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mcp_tools WHERE enabled = 1")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to count all enabled MCP tools (admin bypass)");
                    FlowplaneError::Database {
                        source: e,
                        context: "Failed to count all enabled MCP tools".to_string(),
                    }
                })?;
            return Ok(count);
        }

        // Build IN clause for team filtering
        let placeholders: Vec<String> = (1..=teams.len()).map(|i| format!("${}", i)).collect();
        let query_str = format!(
            "SELECT COUNT(*) FROM mcp_tools WHERE team IN ({}) AND enabled = 1",
            placeholders.join(", ")
        );

        let mut query = sqlx::query_scalar::<sqlx::Postgres, i64>(&query_str);
        for team in teams {
            query = query.bind(team);
        }

        let count: i64 = query.fetch_one(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, teams = ?teams, "Failed to count enabled MCP tools by teams");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count enabled MCP tools for teams {:?}", teams),
            }
        })?;

        Ok(count)
    }

    /// Set enabled status for an MCP tool
    #[instrument(skip(self), fields(mcp_tool_id = %id, enabled = enabled), name = "db_set_mcp_tool_enabled")]
    pub async fn set_enabled(&self, id: &McpToolId, enabled: bool) -> Result<()> {
        let now = chrono::Utc::now();

        let result = sqlx::query(
            "UPDATE mcp_tools SET enabled = $1, updated_at = $2 WHERE id = $3",
        )
        .bind(enabled)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, mcp_tool_id = %id, "Failed to set MCP tool enabled status");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to set enabled status for MCP tool with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found_msg(format!(
                "MCP tool with ID '{}' not found",
                id
            )));
        }

        tracing::info!(
            mcp_tool_id = %id,
            enabled = enabled,
            "Set MCP tool enabled status"
        );

        Ok(())
    }

    /// Get MCP tool by name and team with gateway_host from dataplane.
    ///
    /// This method joins through listeners and dataplanes to resolve the gateway_host
    /// for MCP tool execution. If the tool's listener doesn't have a dataplane assigned,
    /// gateway_host will be None.
    #[instrument(skip(self), fields(team = %team, tool_name = %name), name = "db_get_mcp_tool_with_gateway")]
    pub async fn get_by_name_with_gateway(
        &self,
        team: &str,
        name: &str,
    ) -> Result<Option<McpToolWithGateway>> {
        // Query that joins mcp_tools -> listeners -> dataplanes to get gateway_host
        let row = sqlx::query_as::<sqlx::Postgres, McpToolWithGatewayRow>(
            r#"
            SELECT
                t.id, t.team, t.name, t.description, t.category, t.source_type,
                t.input_schema, t.output_schema, t.learned_schema_id, t.schema_source,
                t.route_id, t.http_method, t.http_path, t.cluster_name, t.listener_port,
                t.host_header, t.enabled, t.confidence, t.created_at, t.updated_at,
                d.gateway_host
            FROM mcp_tools t
            LEFT JOIN listeners l ON l.port = t.listener_port AND l.team = t.team
            LEFT JOIN dataplanes d ON d.id = l.dataplane_id
            WHERE t.team = $1 AND t.name = $2
            "#,
        )
        .bind(team)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, tool_name = %name, "Failed to get MCP tool with gateway");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get MCP tool '{}' with gateway for team '{}'", name, team),
            }
        })?;

        row.map(McpToolWithGateway::try_from).transpose()
    }
}

/// Database row structure for MCP tool with gateway_host
#[derive(Debug, Clone, FromRow)]
struct McpToolWithGatewayRow {
    pub id: String,
    pub team: String,
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub source_type: String,
    pub input_schema: String,
    pub output_schema: Option<String>,
    pub learned_schema_id: Option<i64>,
    pub schema_source: Option<String>,
    pub route_id: Option<String>,
    pub http_method: Option<String>,
    pub http_path: Option<String>,
    pub cluster_name: Option<String>,
    pub listener_port: Option<i64>,
    pub host_header: Option<String>,
    pub enabled: bool,
    pub confidence: Option<f64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub gateway_host: Option<String>,
}

/// MCP tool data with gateway_host resolved from dataplane
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolWithGateway {
    pub id: McpToolId,
    pub team: String,
    pub name: String,
    pub description: Option<String>,
    pub category: McpToolCategory,
    pub source_type: McpToolSourceType,
    pub input_schema: serde_json::Value,
    pub output_schema: Option<serde_json::Value>,
    pub learned_schema_id: Option<i64>,
    pub schema_source: Option<String>,
    pub route_id: Option<RouteId>,
    pub http_method: Option<String>,
    pub http_path: Option<String>,
    pub cluster_name: Option<String>,
    pub listener_port: Option<i64>,
    /// Host header to use when executing this tool (for upstream routing)
    pub host_header: Option<String>,
    pub enabled: bool,
    pub confidence: Option<f64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub gateway_host: Option<String>,
}

impl TryFrom<McpToolWithGatewayRow> for McpToolWithGateway {
    type Error = FlowplaneError;

    fn try_from(row: McpToolWithGatewayRow) -> Result<Self> {
        let category = row
            .category
            .parse()
            .map_err(|e| FlowplaneError::validation(format!("Invalid category: {}", e)))?;

        let source_type = row
            .source_type
            .parse()
            .map_err(|e| FlowplaneError::validation(format!("Invalid source_type: {}", e)))?;

        let input_schema = serde_json::from_str(&row.input_schema)
            .map_err(|e| FlowplaneError::validation(format!("Invalid input_schema JSON: {}", e)))?;

        let output_schema =
            row.output_schema.as_ref().map(|s| serde_json::from_str(s)).transpose().map_err(
                |e| FlowplaneError::validation(format!("Invalid output_schema JSON: {}", e)),
            )?;

        Ok(Self {
            id: McpToolId::from_string(row.id),
            team: row.team,
            name: row.name,
            description: row.description,
            category,
            source_type,
            input_schema,
            output_schema,
            learned_schema_id: row.learned_schema_id,
            schema_source: row.schema_source,
            route_id: row.route_id.map(RouteId::from_string),
            http_method: row.http_method,
            http_path: row.http_path,
            cluster_name: row.cluster_name,
            listener_port: row.listener_port,
            host_header: row.host_header,
            enabled: row.enabled,
            confidence: row.confidence,
            created_at: row.created_at,
            updated_at: row.updated_at,
            gateway_host: row.gateway_host,
        })
    }
}
