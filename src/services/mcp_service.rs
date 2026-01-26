//! MCP enablement service
//!
//! This service provides business logic for enabling/disabling MCP on routes,
//! checking readiness status, and managing MCP tool definitions.

use crate::domain::{RouteConfigId, RouteId, RouteMetadataSourceType};
use crate::errors::FlowplaneError;
use crate::mcp::error::McpError;
use crate::mcp::gateway::GatewayToolGenerator;
use crate::storage::repositories::aggregated_schema::AggregatedSchemaRepository;
use crate::storage::repositories::listener::ListenerRepository;
use crate::storage::repositories::listener_route_config::ListenerRouteConfigRepository;
use crate::storage::repositories::mcp_tool::{
    McpToolData, McpToolRepository, UpdateMcpToolRequest,
};
use crate::storage::repositories::route::RouteRepository;
use crate::storage::repositories::route_config::RouteConfigRepository;
use crate::storage::repositories::route_metadata::{
    RouteMetadataData, RouteMetadataRepository, UpdateRouteMetadataRequest,
};
use crate::storage::repositories::virtual_host::VirtualHostRepository;
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::instrument;

/// MCP service for managing MCP enablement on routes
pub struct McpService {
    route_repo: RouteRepository,
    route_metadata_repo: RouteMetadataRepository,
    virtual_host_repo: VirtualHostRepository,
    route_config_repo: RouteConfigRepository,
    mcp_tool_repo: McpToolRepository,
    aggregated_schema_repo: AggregatedSchemaRepository,
    listener_repo: ListenerRepository,
    listener_route_config_repo: ListenerRouteConfigRepository,
    gateway_tool_generator: GatewayToolGenerator,
}

/// MCP status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStatusResponse {
    /// Whether the route is ready for MCP enablement
    pub ready: bool,
    /// Whether MCP is currently enabled on the route
    pub enabled: bool,
    /// List of missing required fields
    pub missing_fields: Vec<String>,
    /// The tool name if MCP is enabled
    pub tool_name: Option<String>,
    /// Recommended source for schema information
    pub recommended_source: String,
}

/// Request to enable MCP on a route
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnableMcpRequest {
    /// Optional custom tool name
    pub tool_name: Option<String>,
    /// Optional custom description
    pub description: Option<String>,
    /// Optional schema source identifier
    pub schema_source: Option<String>,
    /// Summary for the route (used to create metadata if missing)
    pub summary: Option<String>,
    /// HTTP method for the route (used to create metadata if missing)
    pub http_method: Option<String>,
}

/// Result of refreshing schema from learning module
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshSchemaResult {
    /// Whether the refresh was successful
    pub success: bool,
    /// Message describing the result
    pub message: String,
}

/// Result of applying learned schema to a route's MCP metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyLearnedSchemaResponse {
    /// Updated route metadata
    pub metadata: RouteMetadataData,
    /// Previous source type before applying
    pub previous_source: RouteMetadataSourceType,
    /// ID of the learned schema that was applied
    pub learned_schema_id: i64,
    /// Confidence score of the learned schema
    pub confidence: f64,
    /// Number of samples used to learn the schema
    pub sample_count: i64,
}

/// Information about a learned schema's availability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedSchemaAvailability {
    /// Whether a learned schema is available
    pub available: bool,
    /// The learned schema info if available
    pub schema: Option<LearnedSchemaInfo>,
    /// Current source type of the route metadata
    pub current_source: RouteMetadataSourceType,
    /// Whether the learned schema can be applied (confidence >= 0.8)
    pub can_apply: bool,
    /// Whether force flag is required (current source is OpenAPI)
    pub requires_force: bool,
}

/// Information about a learned schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedSchemaInfo {
    /// Schema ID
    pub id: i64,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f64,
    /// Number of samples used to learn the schema
    pub sample_count: i64,
    /// Schema version
    pub version: i64,
    /// When the schema was last observed
    pub last_observed: chrono::DateTime<chrono::Utc>,
}

/// MCP service error types
#[derive(Debug, thiserror::Error)]
pub enum McpServiceError {
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<FlowplaneError> for McpServiceError {
    fn from(err: FlowplaneError) -> Self {
        match err {
            FlowplaneError::NotFound { resource_type, id } => {
                McpServiceError::NotFound(format!("{} with ID '{}' not found", resource_type, id))
            }
            FlowplaneError::Validation { message, .. } => McpServiceError::Validation(message),
            FlowplaneError::Database { source, context } => {
                tracing::error!(error = %source, context = %context, "Database error in MCP service");
                McpServiceError::Database(source)
            }
            _ => McpServiceError::Internal(err.to_string()),
        }
    }
}

impl From<McpError> for McpServiceError {
    fn from(err: McpError) -> Self {
        McpServiceError::Internal(err.to_string())
    }
}

impl McpService {
    /// Create a new MCP service
    pub fn new(db_pool: Arc<DbPool>) -> Self {
        let route_repo = RouteRepository::new((*db_pool).clone());
        let route_metadata_repo = RouteMetadataRepository::new((*db_pool).clone());
        let virtual_host_repo = VirtualHostRepository::new((*db_pool).clone());
        let route_config_repo = RouteConfigRepository::new((*db_pool).clone());
        let mcp_tool_repo = McpToolRepository::new((*db_pool).clone());
        let aggregated_schema_repo = AggregatedSchemaRepository::new((*db_pool).clone());
        let listener_repo = ListenerRepository::new((*db_pool).clone());
        let listener_route_config_repo = ListenerRouteConfigRepository::new((*db_pool).clone());
        let gateway_tool_generator = GatewayToolGenerator::new();

        Self {
            route_repo,
            route_metadata_repo,
            virtual_host_repo,
            route_config_repo,
            mcp_tool_repo,
            aggregated_schema_repo,
            listener_repo,
            listener_route_config_repo,
            gateway_tool_generator,
        }
    }

    /// Get MCP status for a route
    #[instrument(skip(self), fields(team = %team, route_id = %route_id))]
    pub async fn get_status(
        &self,
        team: &str,
        route_id: &str,
    ) -> std::result::Result<McpStatusResponse, McpServiceError> {
        let route_id = RouteId::from_string(route_id.to_string());

        // Get the route
        let route = self.route_repo.get_by_id(&route_id).await?;

        // Verify team access via virtual host and route config
        let virtual_host = self.virtual_host_repo.get_by_id(&route.virtual_host_id).await?;
        let route_config = self.route_config_repo.get_by_id(&virtual_host.route_config_id).await?;

        if let Some(route_team) = &route_config.team {
            if route_team != team {
                return Err(McpServiceError::NotFound(format!(
                    "Route '{}' not found in team '{}'",
                    route_id, team
                )));
            }
        }

        // Check if MCP tool already exists
        let existing_tool = self.mcp_tool_repo.get_by_route_id(&route_id).await?;

        // Get metadata if it exists
        let metadata = self.route_metadata_repo.get_by_route_id(&route_id).await?;

        // Check missing fields
        let missing_fields = self.check_missing_fields(&metadata);
        let ready = missing_fields.is_empty();

        // Determine recommended source
        let recommended_source =
            if metadata.is_some() { "metadata".to_string() } else { "learning".to_string() };

        Ok(McpStatusResponse {
            ready,
            enabled: existing_tool.is_some(),
            missing_fields,
            tool_name: existing_tool.as_ref().map(|t| t.name.clone()),
            recommended_source,
        })
    }

    /// Enable MCP on a route with enrichment chain
    ///
    /// Enrichment priority:
    /// 1. Existing route_metadata (from OpenAPI import)
    /// 2. Learning session data (from aggregated_api_schemas if confidence >= 0.8)
    /// 3. User-provided data from request
    /// 4. Auto-generated fallback
    #[instrument(skip(self, request), fields(team = %team, route_id = %route_id))]
    pub async fn enable(
        &self,
        team: &str,
        route_id: &str,
        request: EnableMcpRequest,
    ) -> std::result::Result<McpToolData, McpServiceError> {
        let route_id = RouteId::from_string(route_id.to_string());

        // Get the route and verify team access
        let route = self.route_repo.get_by_id(&route_id).await?;
        let virtual_host = self.virtual_host_repo.get_by_id(&route.virtual_host_id).await?;
        let route_config = self.route_config_repo.get_by_id(&virtual_host.route_config_id).await?;

        if let Some(route_team) = &route_config.team {
            if route_team != team {
                return Err(McpServiceError::NotFound(format!(
                    "Route '{}' not found in team '{}'",
                    route_id, team
                )));
            }
        }

        // Get or create metadata using enrichment chain
        let metadata = match self.route_metadata_repo.get_by_route_id(&route_id).await? {
            Some(mut existing) => {
                // Check if existing metadata has missing required fields
                let missing = self.check_missing_fields(&Some(existing.clone()));

                if missing.is_empty() {
                    tracing::info!(
                        route_id = %route_id,
                        source_type = ?existing.source_type,
                        "Using existing route metadata"
                    );
                    existing
                } else {
                    // Fill in missing fields from request or auto-generate
                    tracing::info!(
                        route_id = %route_id,
                        missing_fields = ?missing,
                        "Existing metadata incomplete, filling missing fields"
                    );

                    let http_method = existing
                        .http_method
                        .as_deref()
                        .or(request.http_method.as_deref())
                        .unwrap_or("GET");

                    let update_request = UpdateRouteMetadataRequest {
                        operation_id: if existing.operation_id.is_none() {
                            Some(request.tool_name.clone().or_else(|| {
                                let method = http_method.to_lowercase();
                                let path_parts: Vec<&str> = route
                                    .path_pattern
                                    .split('/')
                                    .filter(|s| !s.is_empty())
                                    .map(|s| s.trim_matches('{').trim_matches('}'))
                                    .collect();
                                Some(format!("{}_{}", method, path_parts.join("_")))
                            }))
                        } else {
                            None
                        },
                        summary: if existing.summary.is_none() {
                            Some(request.summary.clone().or_else(|| {
                                Some(format!(
                                    "{} {}",
                                    http_method.to_uppercase(),
                                    route.path_pattern
                                ))
                            }))
                        } else {
                            None
                        },
                        description: if existing.description.is_none() {
                            Some(request.description.clone().or_else(|| {
                                Some(format!("API endpoint for {}", route.path_pattern))
                            }))
                        } else {
                            None
                        },
                        tags: None,
                        http_method: if existing.http_method.is_none() {
                            Some(Some(http_method.to_string()))
                        } else {
                            None
                        },
                        request_body_schema: None,
                        response_schemas: None,
                        learning_schema_id: None,
                        enriched_from_learning: None,
                        source_type: None,
                        confidence: None,
                    };

                    // Update the metadata with missing fields
                    match self.route_metadata_repo.update(&existing.id, update_request).await {
                        Ok(updated) => {
                            existing = updated;
                        }
                        Err(e) => {
                            tracing::warn!(
                                route_id = %route_id,
                                error = %e,
                                "Failed to update incomplete metadata, using as-is"
                            );
                        }
                    }
                    existing
                }
            }
            None => {
                // Try to enrich from learning session first
                let http_method = request.http_method.as_deref().unwrap_or("GET");
                let learned_schema =
                    self.try_enrich_from_learning(team, &route.path_pattern, http_method).await;

                // Build metadata from enrichment or fallback
                let (
                    operation_id,
                    summary,
                    description,
                    request_body_schema,
                    response_schemas,
                    learning_schema_id,
                    enriched_from_learning,
                    source_type,
                    confidence,
                ) = if let Some(schema) = learned_schema {
                    tracing::info!(
                        route_id = %route_id,
                        schema_id = schema.id,
                        confidence = schema.confidence_score,
                        "Enriching metadata from learning session"
                    );
                    (
                        request.tool_name.clone().unwrap_or_else(|| {
                            let method = http_method.to_lowercase();
                            let path_parts: Vec<&str> = route
                                .path_pattern
                                .split('/')
                                .filter(|s| !s.is_empty())
                                .map(|s| s.trim_matches('{').trim_matches('}'))
                                .collect();
                            format!("{}_{}", method, path_parts.join("_"))
                        }),
                        request.summary.clone().or_else(|| {
                            Some(format!(
                                "{} {} (learned)",
                                http_method.to_uppercase(),
                                route.path_pattern
                            ))
                        }),
                        request.description.clone().or_else(|| {
                            Some(format!(
                                "API endpoint for {} (schema learned from traffic)",
                                route.path_pattern
                            ))
                        }),
                        schema.request_schema.clone(),
                        schema.response_schemas.clone(),
                        Some(schema.id),
                        true,
                        RouteMetadataSourceType::Learned,
                        schema.confidence_score,
                    )
                } else {
                    tracing::info!(
                        route_id = %route_id,
                        "No learning data available, using manual metadata"
                    );
                    // Fallback to manual generation
                    let operation_id = request.tool_name.clone().unwrap_or_else(|| {
                        let method = http_method.to_lowercase();
                        let path_parts: Vec<&str> = route
                            .path_pattern
                            .split('/')
                            .filter(|s| !s.is_empty())
                            .map(|s| s.trim_matches('{').trim_matches('}'))
                            .collect();
                        format!("{}_{}", method, path_parts.join("_"))
                    });

                    let summary = request.summary.clone().or_else(|| {
                        Some(format!("{} {}", http_method.to_uppercase(), route.path_pattern))
                    });

                    let description = request
                        .description
                        .clone()
                        .or_else(|| Some(format!("API endpoint for {}", route.path_pattern)));

                    (
                        operation_id,
                        summary,
                        description,
                        None,
                        None,
                        None,
                        false,
                        RouteMetadataSourceType::Manual,
                        1.0,
                    )
                };

                let create_request =
                    crate::storage::repositories::route_metadata::CreateRouteMetadataRequest {
                        route_id: route_id.clone(),
                        operation_id: Some(operation_id),
                        summary,
                        description,
                        tags: None,
                        http_method: Some(http_method.to_string()),
                        request_body_schema,
                        response_schemas,
                        learning_schema_id,
                        enriched_from_learning,
                        source_type,
                        confidence: Some(confidence),
                    };

                tracing::info!(
                    route_id = %route_id,
                    source_type = ?create_request.source_type,
                    "Creating route metadata for MCP enablement"
                );

                self.route_metadata_repo.create(create_request).await.map_err(|e| {
                    McpServiceError::Internal(format!("Failed to create route metadata: {}", e))
                })?
            }
        };

        // Validate metadata completeness (should always pass now since we create complete metadata)
        let missing_fields = self.check_missing_fields(&Some(metadata.clone()));
        if !missing_fields.is_empty() {
            return Err(McpServiceError::Validation(format!(
                "Route metadata incomplete. Missing fields: {}",
                missing_fields.join(", ")
            )));
        }

        // Check if tool already exists
        if let Some(existing_tool) = self.mcp_tool_repo.get_by_route_id(&route_id).await? {
            // Refresh listener port in case it changed since tool was created
            let listener_port =
                self.get_listener_port_for_route_config(&virtual_host.route_config_id).await?;

            // Extract non-wildcard domain from virtual host for Host header
            let host_header = virtual_host.domains.iter().find(|d| *d != "*").cloned();

            // Update existing tool to enabled with refreshed listener port and host header
            let update_request = UpdateMcpToolRequest {
                enabled: Some(true),
                name: request.tool_name,
                description: request.description.map(Some),
                schema_source: request.schema_source.map(Some),
                category: None,
                source_type: None,
                input_schema: None,
                output_schema: None,
                learned_schema_id: None,
                route_id: None,
                http_method: None,
                http_path: None,
                cluster_name: None,
                listener_port: Some(Some(listener_port as i64)),
                host_header: Some(host_header),
                confidence: None,
            };
            return Ok(self.mcp_tool_repo.update(&existing_tool.id, update_request).await?);
        }

        // Get listener port dynamically from the route config's associated listeners
        let listener_port =
            self.get_listener_port_for_route_config(&virtual_host.route_config_id).await?;

        // Extract non-wildcard domain from virtual host for Host header
        // Priority: first non-wildcard domain, otherwise None
        let host_header = virtual_host.domains.iter().find(|d| *d != "*").cloned();

        // Generate tool using GatewayToolGenerator
        let mut tool_request = self.gateway_tool_generator.generate_tool(
            &route,
            &metadata,
            listener_port,
            team,
            host_header,
        )?;

        // Apply custom overrides from request
        if let Some(tool_name) = request.tool_name {
            tool_request.name = tool_name;
        }
        if let Some(description) = request.description {
            tool_request.description = Some(description);
        }
        if let Some(schema_source) = request.schema_source {
            tool_request.schema_source = Some(schema_source);
        }

        // Create the MCP tool
        let tool = self.mcp_tool_repo.create(tool_request).await?;

        tracing::info!(
            tool_id = %tool.id,
            tool_name = %tool.name,
            route_id = %route_id,
            team = %team,
            "MCP enabled on route"
        );

        Ok(tool)
    }

    /// Disable MCP on a route (soft disable)
    #[instrument(skip(self), fields(team = %team, route_id = %route_id))]
    pub async fn disable(
        &self,
        team: &str,
        route_id: &str,
    ) -> std::result::Result<(), McpServiceError> {
        let route_id = RouteId::from_string(route_id.to_string());

        // Get the route and verify team access
        let route = self.route_repo.get_by_id(&route_id).await?;
        let virtual_host = self.virtual_host_repo.get_by_id(&route.virtual_host_id).await?;
        let route_config = self.route_config_repo.get_by_id(&virtual_host.route_config_id).await?;

        if let Some(route_team) = &route_config.team {
            if route_team != team {
                return Err(McpServiceError::NotFound(format!(
                    "Route '{}' not found in team '{}'",
                    route_id, team
                )));
            }
        }

        // Find the MCP tool
        let tool = self.mcp_tool_repo.get_by_route_id(&route_id).await?.ok_or_else(|| {
            McpServiceError::NotFound(format!("MCP tool not found for route '{}'", route_id))
        })?;

        // Soft disable by setting enabled = false
        self.mcp_tool_repo.set_enabled(&tool.id, false).await?;

        tracing::info!(
            tool_id = %tool.id,
            route_id = %route_id,
            team = %team,
            "MCP disabled on route"
        );

        Ok(())
    }

    /// Refresh schema from learning module
    #[instrument(skip(self), fields(team = %team, route_id = %route_id))]
    pub async fn refresh_schema(
        &self,
        team: &str,
        route_id: &str,
    ) -> std::result::Result<RefreshSchemaResult, McpServiceError> {
        let route_id = RouteId::from_string(route_id.to_string());

        // Get the route and verify team access
        let route = self.route_repo.get_by_id(&route_id).await?;
        let virtual_host = self.virtual_host_repo.get_by_id(&route.virtual_host_id).await?;
        let route_config = self.route_config_repo.get_by_id(&virtual_host.route_config_id).await?;

        if let Some(route_team) = &route_config.team {
            if route_team != team {
                return Err(McpServiceError::NotFound(format!(
                    "Route '{}' not found in team '{}'",
                    route_id, team
                )));
            }
        }

        // Get existing metadata
        let metadata = match self.route_metadata_repo.get_by_route_id(&route_id).await? {
            Some(m) => m,
            None => {
                return Ok(RefreshSchemaResult {
                    success: false,
                    message: "No route metadata exists. Enable MCP first to create metadata."
                        .to_string(),
                });
            }
        };

        // Extract HTTP method (default to GET)
        let http_method = metadata.http_method.as_deref().unwrap_or("GET");

        // Get the route path pattern
        let path_pattern = route.path_pattern.as_str();

        // Query aggregated schema from learning module
        let aggregated_schema =
            self.aggregated_schema_repo.get_latest(team, path_pattern, http_method).await.map_err(
                |e| McpServiceError::Internal(format!("Failed to query aggregated schema: {}", e)),
            )?;

        match aggregated_schema {
            Some(schema) if schema.confidence_score >= 0.8 => {
                // Update route metadata with learned schema
                let update_request = UpdateRouteMetadataRequest {
                    operation_id: None,
                    summary: None,
                    description: None,
                    tags: None,
                    http_method: None,
                    request_body_schema: Some(schema.request_schema.clone()),
                    response_schemas: Some(schema.response_schemas.clone()),
                    learning_schema_id: Some(Some(schema.id)),
                    enriched_from_learning: Some(true),
                    source_type: Some(RouteMetadataSourceType::Learned),
                    confidence: Some(Some(schema.confidence_score)),
                };

                let updated_metadata =
                    self.route_metadata_repo.update(&metadata.id, update_request).await.map_err(
                        |e| McpServiceError::Internal(format!("Failed to update metadata: {}", e)),
                    )?;

                // Also regenerate the MCP tool with the new schemas
                if let Some(existing_tool) = self.mcp_tool_repo.get_by_route_id(&route_id).await? {
                    // Get listener port to regenerate the tool
                    let listener_port = self
                        .get_listener_port_for_route_config(&virtual_host.route_config_id)
                        .await?;

                    // Extract non-wildcard domain from virtual host for Host header
                    let host_header = virtual_host.domains.iter().find(|d| *d != "*").cloned();

                    // Generate new tool with updated metadata
                    let new_tool_request = self.gateway_tool_generator.generate_tool(
                        &route,
                        &updated_metadata,
                        listener_port,
                        team,
                        host_header.clone(),
                    )?;

                    // Update the existing tool with new schemas and refreshed listener port
                    let tool_update =
                        crate::storage::repositories::mcp_tool::UpdateMcpToolRequest {
                            name: None,
                            description: None,
                            schema_source: Some(new_tool_request.schema_source),
                            category: None,
                            source_type: Some(new_tool_request.source_type),
                            input_schema: Some(new_tool_request.input_schema),
                            output_schema: Some(new_tool_request.output_schema),
                            learned_schema_id: Some(new_tool_request.learned_schema_id),
                            route_id: None,
                            http_method: None,
                            http_path: None,
                            cluster_name: None,
                            listener_port: Some(new_tool_request.listener_port),
                            host_header: Some(host_header),
                            enabled: None, // Keep existing enabled state
                            confidence: Some(new_tool_request.confidence),
                        };

                    self.mcp_tool_repo.update(&existing_tool.id, tool_update).await.map_err(
                        |e| McpServiceError::Internal(format!("Failed to update MCP tool: {}", e)),
                    )?;

                    tracing::info!(
                        tool_id = %existing_tool.id,
                        "MCP tool regenerated with new learned schemas"
                    );
                }

                tracing::info!(
                    route_id = %route_id,
                    schema_id = schema.id,
                    confidence = schema.confidence_score,
                    sample_count = schema.sample_count,
                    "Refreshed schema from learning module"
                );

                Ok(RefreshSchemaResult {
                    success: true,
                    message: format!(
                        "Schema updated from learning module (confidence: {:.0}%, {} samples)",
                        schema.confidence_score * 100.0,
                        schema.sample_count
                    ),
                })
            }
            Some(schema) => Ok(RefreshSchemaResult {
                success: false,
                message: format!(
                    "Schema found but confidence too low ({:.0}% < 80%). Collect more samples.",
                    schema.confidence_score * 100.0
                ),
            }),
            None => Ok(RefreshSchemaResult {
                success: false,
                message: "No learned schema found. Start a learning session to collect samples."
                    .to_string(),
            }),
        }
    }

    /// Get listener port for a route config
    ///
    /// Returns an error if no listeners are bound or if the listener has no port configured.
    /// This ensures MCP tools always have valid, executable listener ports.
    ///
    /// Resolution strategy:
    /// 1. First, check the listener_route_configs junction table
    /// 2. If empty, fall back to finding listeners by route_config_name in their HCM config
    async fn get_listener_port_for_route_config(
        &self,
        route_config_id: &RouteConfigId,
    ) -> std::result::Result<i32, McpServiceError> {
        // Strategy 1: Check the junction table for listener bindings
        let listener_ids = self
            .listener_route_config_repo
            .list_listener_ids_by_route_config(route_config_id)
            .await
            .map_err(|e| McpServiceError::Internal(format!("Failed to get listeners: {}", e)))?;

        if let Some(listener_id) = listener_ids.first() {
            // Found via junction table
            let listener = self.listener_repo.get_by_id(listener_id).await.map_err(|e| {
                McpServiceError::Internal(format!(
                    "Failed to get listener '{}': {}",
                    listener_id, e
                ))
            })?;

            let port = listener.port.ok_or_else(|| {
                McpServiceError::Validation(format!(
                    "Cannot enable MCP: listener '{}' has no port configured. \
                     MCP tools require listeners with explicit ports.",
                    listener.name
                ))
            })? as i32;

            if listener_ids.len() > 1 {
                tracing::warn!(
                    route_config_id = %route_config_id,
                    listener_count = listener_ids.len(),
                    selected_listener = %listener_id,
                    selected_port = port,
                    "Multiple listeners found for route config, using first listener's port"
                );
            }

            tracing::info!(
                route_config_id = %route_config_id,
                listener_id = %listener_id,
                listener_name = %listener.name,
                port = port,
                "Resolved listener port via junction table"
            );

            return Ok(port);
        }

        // Strategy 2: Junction table empty - find listener by route_config_name in HCM config
        tracing::debug!(
            route_config_id = %route_config_id,
            "Junction table empty, falling back to route_config_name lookup"
        );

        // Get the route_config to find its name
        let route_config =
            self.route_config_repo.get_by_id(route_config_id).await.map_err(|e| {
                McpServiceError::Internal(format!(
                    "Failed to get route config '{}': {}",
                    route_config_id, e
                ))
            })?;

        // Find listeners that reference this route_config_name in their HCM configuration
        let listeners =
            self.listener_repo.find_by_route_config_name(&route_config.name, &[]).await.map_err(
                |e| {
                    McpServiceError::Internal(format!(
                        "Failed to find listeners for route config '{}': {}",
                        route_config.name, e
                    ))
                },
            )?;

        let listener = listeners.first().ok_or_else(|| {
            McpServiceError::Validation(format!(
                "Cannot enable MCP: no listener found that references route config '{}'. \
                 Please ensure a listener is configured with this route config.",
                route_config.name
            ))
        })?;

        let port = listener.port.ok_or_else(|| {
            McpServiceError::Validation(format!(
                "Cannot enable MCP: listener '{}' has no port configured. \
                 MCP tools require listeners with explicit ports.",
                listener.name
            ))
        })? as i32;

        if listeners.len() > 1 {
            tracing::warn!(
                route_config_id = %route_config_id,
                route_config_name = %route_config.name,
                listener_count = listeners.len(),
                selected_listener = %listener.id,
                selected_port = port,
                "Multiple listeners reference this route config, using first listener's port"
            );
        }

        tracing::info!(
            route_config_id = %route_config_id,
            route_config_name = %route_config.name,
            listener_id = %listener.id,
            listener_name = %listener.name,
            port = port,
            "Resolved listener port via route_config_name lookup"
        );

        Ok(port)
    }

    /// Check which required fields are missing from metadata
    fn check_missing_fields(&self, metadata: &Option<RouteMetadataData>) -> Vec<String> {
        let mut missing = Vec::new();

        match metadata {
            None => {
                missing.push("metadata".to_string());
                missing.push("operation_id".to_string());
                missing.push("summary".to_string());
                missing.push("description".to_string());
            }
            Some(meta) => {
                if meta.operation_id.is_none() {
                    missing.push("operation_id".to_string());
                }
                if meta.summary.is_none() {
                    missing.push("summary".to_string());
                }
                if meta.description.is_none() {
                    missing.push("description".to_string());
                }
            }
        }

        missing
    }

    /// Try to enrich metadata from learning session data
    ///
    /// Queries the aggregated_api_schemas table for learned schemas matching
    /// the team, path, and HTTP method. Returns the schema if confidence >= 0.8.
    async fn try_enrich_from_learning(
        &self,
        team: &str,
        path_pattern: &str,
        http_method: &str,
    ) -> Option<crate::storage::repositories::aggregated_schema::AggregatedSchemaData> {
        match self.aggregated_schema_repo.get_latest(team, path_pattern, http_method).await {
            Ok(Some(schema)) if schema.confidence_score >= 0.8 => {
                tracing::debug!(
                    team = %team,
                    path = %path_pattern,
                    method = %http_method,
                    confidence = schema.confidence_score,
                    sample_count = schema.sample_count,
                    "Found learned schema with sufficient confidence"
                );
                Some(schema)
            }
            Ok(Some(schema)) => {
                tracing::debug!(
                    team = %team,
                    path = %path_pattern,
                    method = %http_method,
                    confidence = schema.confidence_score,
                    "Found learned schema but confidence too low (< 0.8)"
                );
                None
            }
            Ok(None) => {
                tracing::debug!(
                    team = %team,
                    path = %path_pattern,
                    method = %http_method,
                    "No learned schema found for route"
                );
                None
            }
            Err(e) => {
                tracing::warn!(
                    team = %team,
                    path = %path_pattern,
                    method = %http_method,
                    error = %e,
                    "Failed to query learned schema"
                );
                None
            }
        }
    }

    /// Check if a learned schema is available for a route
    ///
    /// Returns information about the learned schema availability, including
    /// whether it can be applied and if force flag is required.
    #[tracing::instrument(skip(self), fields(team = %team, route_id = %route_id))]
    pub async fn check_learned_schema_availability(
        &self,
        team: &str,
        route_id: &RouteId,
    ) -> Result<LearnedSchemaAvailability, McpServiceError> {
        // Get the route (returns Result<RouteData>, not Option)
        let route = self.route_repo.get_by_id(route_id).await?;

        // Navigate through virtual_host to get route_config for team verification
        let virtual_host = self.virtual_host_repo.get_by_id(&route.virtual_host_id).await?;
        let route_config = self.route_config_repo.get_by_id(&virtual_host.route_config_id).await?;

        if route_config.team.as_deref() != Some(team) {
            return Err(McpServiceError::NotFound(format!("Route {} not found", route_id)));
        }

        // Get metadata to check current source (returns Result<Option<RouteMetadataData>>)
        let metadata = self.route_metadata_repo.get_by_route_id(route_id).await?;
        let current_source =
            metadata.as_ref().map(|m| m.source_type).unwrap_or(RouteMetadataSourceType::Manual);

        // Query for learned schema
        let http_method = metadata
            .as_ref()
            .and_then(|m| m.http_method.clone())
            .unwrap_or_else(|| "GET".to_string());

        let learned_schema = self
            .aggregated_schema_repo
            .get_latest(team, &route.path_pattern, &http_method)
            .await
            .ok()
            .flatten();

        match learned_schema {
            Some(schema) => {
                let can_apply = schema.confidence_score >= 0.8;
                let requires_force = current_source == RouteMetadataSourceType::Openapi;

                Ok(LearnedSchemaAvailability {
                    available: true,
                    schema: Some(LearnedSchemaInfo {
                        id: schema.id,
                        confidence: schema.confidence_score,
                        sample_count: schema.sample_count,
                        version: schema.version,
                        last_observed: schema.last_observed,
                    }),
                    current_source,
                    can_apply,
                    requires_force,
                })
            }
            None => Ok(LearnedSchemaAvailability {
                available: false,
                schema: None,
                current_source,
                can_apply: false,
                requires_force: false,
            }),
        }
    }

    /// Apply a learned schema to a route's MCP metadata
    ///
    /// # Arguments
    /// * `team` - Team identifier
    /// * `route_id` - Route to update
    /// * `force` - If true, override even if source_type is "openapi"
    ///
    /// # Returns
    /// Updated metadata or error
    #[tracing::instrument(skip(self), fields(team = %team, route_id = %route_id, force = %force))]
    pub async fn apply_learned_schema(
        &self,
        team: &str,
        route_id: &RouteId,
        force: bool,
    ) -> Result<ApplyLearnedSchemaResponse, McpServiceError> {
        // Get the route (returns Result<RouteData>, not Option)
        let route = self.route_repo.get_by_id(route_id).await?;

        // Navigate through virtual_host to get route_config for team verification
        let virtual_host = self.virtual_host_repo.get_by_id(&route.virtual_host_id).await?;
        let route_config = self.route_config_repo.get_by_id(&virtual_host.route_config_id).await?;

        if route_config.team.as_deref() != Some(team) {
            return Err(McpServiceError::NotFound(format!("Route {} not found", route_id)));
        }

        // Get existing metadata (must exist - route must be MCP enabled)
        // Returns Result<Option<RouteMetadataData>>
        let existing_metadata =
            self.route_metadata_repo.get_by_route_id(route_id).await?.ok_or_else(|| {
                McpServiceError::Validation(
                    "MCP is not enabled for this route. Enable MCP first.".to_string(),
                )
            })?;

        let previous_source = existing_metadata.source_type;

        // Check if force is required
        if previous_source == RouteMetadataSourceType::Openapi && !force {
            return Err(McpServiceError::Validation(
                "Route has OpenAPI-sourced metadata. Use force=true to override.".to_string(),
            ));
        }

        // Get learned schema
        let http_method =
            existing_metadata.http_method.clone().unwrap_or_else(|| "GET".to_string());

        let learned_schema = self
            .aggregated_schema_repo
            .get_latest(team, &route.path_pattern, &http_method)
            .await?
            .ok_or_else(|| {
                McpServiceError::NotFound(format!(
                    "No learned schema available for {} {}",
                    http_method, route.path_pattern
                ))
            })?;

        // Check confidence threshold
        if learned_schema.confidence_score < 0.8 {
            return Err(McpServiceError::Validation(format!(
                "Learned schema confidence ({:.0}%) is below required threshold (80%)",
                learned_schema.confidence_score * 100.0
            )));
        }

        // Update metadata with learned schema
        let update_request = UpdateRouteMetadataRequest {
            operation_id: None, // Keep existing
            summary: Some(Some(format!(
                "{} {} (learned)",
                http_method.to_uppercase(),
                route.path_pattern
            ))),
            description: Some(Some(format!(
                "API endpoint for {} (schema learned from {} samples)",
                route.path_pattern, learned_schema.sample_count
            ))),
            tags: None,
            http_method: None, // Keep existing
            request_body_schema: Some(learned_schema.request_schema.clone()),
            response_schemas: Some(learned_schema.response_schemas.clone()),
            learning_schema_id: Some(Some(learned_schema.id)),
            enriched_from_learning: Some(true),
            source_type: Some(RouteMetadataSourceType::Learned),
            confidence: Some(Some(learned_schema.confidence_score)),
        };

        let updated_metadata =
            self.route_metadata_repo.update(&existing_metadata.id, update_request).await?;

        tracing::info!(
            route_id = %route_id,
            previous_source = ?previous_source,
            schema_id = learned_schema.id,
            confidence = learned_schema.confidence_score,
            "Applied learned schema to route metadata"
        );

        Ok(ApplyLearnedSchemaResponse {
            metadata: updated_metadata,
            previous_source,
            learned_schema_id: learned_schema.id,
            confidence: learned_schema.confidence_score,
            sample_count: learned_schema.sample_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::RouteMetadataSourceType;

    async fn create_test_service() -> McpService {
        // Create a minimal in-memory pool for tests
        let pool =
            sqlx::SqlitePool::connect("sqlite::memory:").await.expect("Failed to create test pool");
        McpService::new(Arc::new(pool))
    }

    #[tokio::test]
    async fn test_check_missing_fields_no_metadata() {
        let service = create_test_service().await;
        let missing = service.check_missing_fields(&None);

        assert_eq!(missing.len(), 4);
        assert!(missing.contains(&"metadata".to_string()));
        assert!(missing.contains(&"operation_id".to_string()));
        assert!(missing.contains(&"summary".to_string()));
        assert!(missing.contains(&"description".to_string()));
    }

    #[tokio::test]
    async fn test_check_missing_fields_incomplete_metadata() {
        let service = create_test_service().await;

        let metadata = RouteMetadataData {
            id: crate::domain::RouteMetadataId::new(),
            route_id: RouteId::new(),
            operation_id: Some("test_op".to_string()),
            summary: None,
            description: None,
            tags: None,
            http_method: Some("GET".to_string()),
            request_body_schema: None,
            response_schemas: None,
            learning_schema_id: None,
            enriched_from_learning: false,
            source_type: RouteMetadataSourceType::Manual,
            confidence: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let missing = service.check_missing_fields(&Some(metadata));

        assert_eq!(missing.len(), 2);
        assert!(missing.contains(&"summary".to_string()));
        assert!(missing.contains(&"description".to_string()));
    }

    #[tokio::test]
    async fn test_check_missing_fields_complete_metadata() {
        let service = create_test_service().await;

        let metadata = RouteMetadataData {
            id: crate::domain::RouteMetadataId::new(),
            route_id: RouteId::new(),
            operation_id: Some("test_op".to_string()),
            summary: Some("Test operation".to_string()),
            description: Some("Test description".to_string()),
            tags: None,
            http_method: Some("GET".to_string()),
            request_body_schema: None,
            response_schemas: None,
            learning_schema_id: None,
            enriched_from_learning: false,
            source_type: RouteMetadataSourceType::Manual,
            confidence: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let missing = service.check_missing_fields(&Some(metadata));

        assert_eq!(missing.len(), 0);
    }
}
