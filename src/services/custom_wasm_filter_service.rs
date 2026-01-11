//! Custom WASM Filter business logic service
//!
//! This module contains the business logic for custom WASM filter operations,
//! including creating, listing, updating, and deleting custom filters.

use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, instrument};

use crate::{
    domain::{
        validate_wasm_binary, AttachmentPoint, CustomWasmFilterId, EnvoyFilterMetadata,
        FilterCapabilities, FilterSchemaDefinition, FilterSchemaSource, PerRouteBehavior,
    },
    errors::{FlowplaneError, Result},
    storage::{
        CreateCustomWasmFilterRequest, CustomWasmFilterData, CustomWasmFilterRepository,
        UpdateCustomWasmFilterRequest,
    },
    xds::XdsState,
};

/// Service for managing custom WASM filter business logic
pub struct CustomWasmFilterService {
    xds_state: Arc<XdsState>,
}

impl CustomWasmFilterService {
    /// Create a new custom WASM filter service
    pub fn new(xds_state: Arc<XdsState>) -> Self {
        Self { xds_state }
    }

    /// Get the custom WASM filter repository
    fn repository(&self) -> Result<&CustomWasmFilterRepository> {
        self.xds_state
            .custom_wasm_filter_repository
            .as_ref()
            .ok_or_else(|| FlowplaneError::internal("Custom WASM filter repository not configured"))
    }

    /// Create a new custom WASM filter
    ///
    /// Validates the WASM binary and configuration schema, then stores
    /// the filter in the database.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip(self, wasm_binary, config_schema), fields(filter_name = %name, team = %team))]
    pub async fn create_custom_filter(
        &self,
        name: String,
        display_name: String,
        description: Option<String>,
        wasm_binary: Vec<u8>,
        config_schema: serde_json::Value,
        per_route_config_schema: Option<serde_json::Value>,
        ui_hints: Option<serde_json::Value>,
        attachment_points: Option<Vec<String>>,
        runtime: Option<String>,
        failure_policy: Option<String>,
        team: String,
        created_by: Option<String>,
    ) -> Result<CustomWasmFilterData> {
        let repository = self.repository()?;

        // Validate WASM binary
        validate_wasm_binary(&wasm_binary)
            .map_err(|e| FlowplaneError::validation(format!("Invalid WASM binary: {}", e)))?;

        // Validate config_schema is a JSON object
        if !config_schema.is_object() {
            return Err(FlowplaneError::validation("Config schema must be a JSON object"));
        }

        // Validate name format
        if name.is_empty() {
            return Err(FlowplaneError::validation("Name cannot be empty"));
        }
        if !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            return Err(FlowplaneError::validation(
                "Name must contain only alphanumeric characters, underscores, or hyphens",
            ));
        }

        // Validate display name
        if display_name.trim().is_empty() {
            return Err(FlowplaneError::validation("Display name cannot be empty"));
        }

        // Check if name already exists for this team
        if repository.exists_by_name(&team, &name).await? {
            return Err(FlowplaneError::conflict(
                format!("Custom WASM filter '{}' already exists for team '{}'", name, team),
                "CustomWasmFilter",
            ));
        }

        // Default attachment points
        let attachment_points =
            attachment_points.unwrap_or_else(|| vec!["listener".to_string(), "route".to_string()]);

        // Validate attachment points
        for point in &attachment_points {
            if !["listener", "route", "cluster"].contains(&point.as_str()) {
                return Err(FlowplaneError::validation(format!(
                    "Invalid attachment point: '{}'. Valid values are: listener, route, cluster",
                    point
                )));
            }
        }

        let runtime = runtime.unwrap_or_else(|| "envoy.wasm.runtime.v8".to_string());
        let failure_policy = failure_policy.unwrap_or_else(|| "FAIL_CLOSED".to_string());

        let request = CreateCustomWasmFilterRequest {
            name: name.clone(),
            display_name: display_name.clone(),
            description,
            wasm_binary,
            config_schema,
            per_route_config_schema,
            ui_hints,
            attachment_points,
            runtime,
            failure_policy,
            team: team.clone(),
            created_by,
        };

        let created = repository.create(request).await?;

        info!(
            filter_id = %created.id,
            filter_name = %created.name,
            wasm_size = created.wasm_size_bytes,
            team = %created.team,
            "Custom WASM filter created"
        );

        Ok(created)
    }

    /// List custom WASM filters for the given teams
    #[instrument(skip(self, teams))]
    pub async fn list_custom_filters(
        &self,
        teams: &[String],
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CustomWasmFilterData>> {
        let repository = self.repository()?;

        if teams.is_empty() {
            return Ok(vec![]);
        }

        if teams.len() == 1 {
            repository.list_by_team(&teams[0], limit, offset).await
        } else {
            repository.list_by_teams(teams, limit, offset).await
        }
    }

    /// Get a custom WASM filter by ID
    #[instrument(skip(self))]
    pub async fn get_custom_filter(&self, id: &CustomWasmFilterId) -> Result<CustomWasmFilterData> {
        let repository = self.repository()?;
        repository.get_by_id(id).await
    }

    /// Get a custom WASM filter by name and team
    #[instrument(skip(self))]
    pub async fn get_custom_filter_by_name(
        &self,
        team: &str,
        name: &str,
    ) -> Result<CustomWasmFilterData> {
        let repository = self.repository()?;
        repository.get_by_name(team, name).await
    }

    /// Get the WASM binary for a custom filter
    #[instrument(skip(self))]
    pub async fn get_wasm_binary(&self, id: &CustomWasmFilterId) -> Result<Vec<u8>> {
        let repository = self.repository()?;
        repository.get_wasm_binary(id).await
    }

    /// Update a custom WASM filter's metadata
    #[instrument(skip(self, request))]
    pub async fn update_custom_filter(
        &self,
        id: &CustomWasmFilterId,
        request: UpdateCustomWasmFilterRequest,
    ) -> Result<CustomWasmFilterData> {
        let repository = self.repository()?;

        // Validate attachment points if provided
        if let Some(ref points) = request.attachment_points {
            let valid_points: &[&str] = &["listener", "route", "cluster"];
            for point in points.iter() {
                if !valid_points.contains(&point.as_str()) {
                    return Err(FlowplaneError::validation(format!(
                        "Invalid attachment point: '{}'. Valid values are: listener, route, cluster",
                        point
                    )));
                }
            }
        }

        // Validate config_schema if provided
        if let Some(ref schema) = request.config_schema {
            let schema_value: &serde_json::Value = schema;
            if !schema_value.is_object() {
                return Err(FlowplaneError::validation("Config schema must be a JSON object"));
            }
        }

        repository.update(id, request).await
    }

    /// Delete a custom WASM filter
    ///
    /// Note: This will fail if any filter instances are using this custom filter type.
    /// The caller should check for usage before calling this method.
    #[instrument(skip(self))]
    pub async fn delete_custom_filter(&self, id: &CustomWasmFilterId) -> Result<()> {
        let repository = self.repository()?;

        // TODO: Check if any filter instances are using this custom filter type
        // This requires checking the filters table for filter_type = "custom_wasm_{id}"

        repository.delete(id).await?;

        info!(filter_id = %id, "Custom WASM filter deleted");

        Ok(())
    }

    /// Count custom WASM filters for a team
    #[instrument(skip(self))]
    pub async fn count_by_team(&self, team: &str) -> Result<i64> {
        let repository = self.repository()?;
        repository.count_by_team(team).await
    }

    /// List all custom WASM filters (for startup initialization)
    #[instrument(skip(self))]
    pub async fn list_all(&self) -> Result<Vec<CustomWasmFilterData>> {
        let repository = self.repository()?;
        repository.list_all().await
    }

    /// Generate a FilterSchemaDefinition from custom WASM filter data
    ///
    /// This creates a dynamic schema definition that can be registered
    /// in the FilterSchemaRegistry for filter validation and xDS conversion.
    pub fn generate_schema_definition(data: &CustomWasmFilterData) -> FilterSchemaDefinition {
        // Parse attachment points
        let attachment_points: Vec<AttachmentPoint> = data
            .attachment_points
            .iter()
            .filter_map(|s: &String| match s.as_str() {
                "listener" => Some(AttachmentPoint::Listener),
                "route" => Some(AttachmentPoint::Route),
                "cluster" => Some(AttachmentPoint::Cluster),
                _ => None,
            })
            .collect();

        FilterSchemaDefinition {
            name: format!("custom_wasm_{}", data.id),
            display_name: data.display_name.clone(),
            description: data.description.clone().unwrap_or_default(),
            version: "1.0".to_string(),
            envoy: EnvoyFilterMetadata {
                http_filter_name: "envoy.filters.http.wasm".to_string(),
                type_url: "type.googleapis.com/envoy.extensions.filters.http.wasm.v3.Wasm"
                    .to_string(),
                per_route_type_url: Some(
                    "type.googleapis.com/envoy.extensions.filters.http.wasm.v3.PerRouteConfig"
                        .to_string(),
                ),
            },
            capabilities: FilterCapabilities {
                attachment_points,
                requires_listener_config: true,
                per_route_behavior: if data.per_route_config_schema.is_some() {
                    PerRouteBehavior::FullConfig
                } else {
                    PerRouteBehavior::NotSupported
                },
            },
            config_schema: data.config_schema.clone(),
            per_route_config_schema: data.per_route_config_schema.clone(),
            proto_mapping: HashMap::new(),
            ui_hints: None, // TODO: Parse ui_hints from data.ui_hints
            source: FilterSchemaSource::Custom,
            is_implemented: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_schema_definition() {
        let data = CustomWasmFilterData {
            id: CustomWasmFilterId::from_string("test-id".to_string()),
            name: "test-filter".to_string(),
            display_name: "Test Filter".to_string(),
            description: Some("A test filter".to_string()),
            wasm_sha256: "abc123".to_string(),
            wasm_size_bytes: 1024,
            config_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "header": {"type": "string"}
                }
            }),
            per_route_config_schema: None,
            ui_hints: None,
            attachment_points: vec!["listener".to_string(), "route".to_string()],
            runtime: "envoy.wasm.runtime.v8".to_string(),
            failure_policy: "FAIL_CLOSED".to_string(),
            version: 1,
            team: "test-team".to_string(),
            created_by: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let schema = CustomWasmFilterService::generate_schema_definition(&data);

        assert_eq!(schema.name, "custom_wasm_test-id");
        assert_eq!(schema.display_name, "Test Filter");
        assert_eq!(schema.envoy.http_filter_name, "envoy.filters.http.wasm");
        assert!(schema.capabilities.attachment_points.contains(&AttachmentPoint::Listener));
        assert!(schema.capabilities.attachment_points.contains(&AttachmentPoint::Route));
        assert!(!schema.capabilities.attachment_points.contains(&AttachmentPoint::Cluster));
        assert_eq!(schema.capabilities.per_route_behavior, PerRouteBehavior::NotSupported);
        assert!(schema.is_implemented);
    }

    #[test]
    fn test_generate_schema_definition_with_per_route() {
        let data = CustomWasmFilterData {
            id: CustomWasmFilterId::from_string("test-id-2".to_string()),
            name: "test-filter-2".to_string(),
            display_name: "Test Filter 2".to_string(),
            description: None,
            wasm_sha256: "def456".to_string(),
            wasm_size_bytes: 2048,
            config_schema: serde_json::json!({"type": "object"}),
            per_route_config_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "enabled": {"type": "boolean"}
                }
            })),
            ui_hints: None,
            attachment_points: vec![
                "listener".to_string(),
                "cluster".to_string(),
                "route".to_string(),
            ],
            runtime: "envoy.wasm.runtime.v8".to_string(),
            failure_policy: "FAIL_OPEN".to_string(),
            version: 1,
            team: "test-team".to_string(),
            created_by: Some("user-1".to_string()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let schema = CustomWasmFilterService::generate_schema_definition(&data);

        assert_eq!(schema.capabilities.per_route_behavior, PerRouteBehavior::FullConfig);
        assert!(schema.per_route_config_schema.is_some());
        assert!(schema.capabilities.attachment_points.contains(&AttachmentPoint::Cluster));
    }
}
