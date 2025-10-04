//! # Repository Pattern for Data Access
//!
//! Provides repository implementations using runtime queries with structured types
//! for development velocity while maintaining type safety.

use crate::auth::models::{
    NewPersonalAccessToken, PersonalAccessToken, TokenStatus, UpdatePersonalAccessToken,
};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use std::str::FromStr;
use uuid::Uuid;

/// Database row structure for clusters
#[derive(Debug, Clone, FromRow)]
struct ClusterRow {
    pub id: String,
    pub name: String,
    pub service_name: String,
    pub configuration: String,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Database row structure for listeners
#[derive(Debug, Clone, FromRow)]
struct ListenerRow {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: Option<i64>,
    pub protocol: String,
    pub configuration: String,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Listener configuration data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerData {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: Option<i64>,
    pub protocol: String,
    pub configuration: String,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ListenerRow> for ListenerData {
    fn from(row: ListenerRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            address: row.address,
            port: row.port,
            protocol: row.protocol,
            configuration: row.configuration,
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Cluster configuration data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterData {
    pub id: String,
    pub name: String,
    pub service_name: String,
    pub configuration: String, // JSON serialized
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ClusterRow> for ClusterData {
    fn from(row: ClusterRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            service_name: row.service_name,
            configuration: row.configuration,
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Database row structure for routes
#[derive(Debug, Clone, FromRow)]
struct RouteRow {
    pub id: String,
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub configuration: String,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Route configuration data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteData {
    pub id: String,
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub configuration: String,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<RouteRow> for RouteData {
    fn from(row: RouteRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            path_prefix: row.path_prefix,
            cluster_name: row.cluster_name,
            configuration: row.configuration,
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
struct ApiDefinitionRow {
    pub id: String,
    pub team: String,
    pub domain: String,
    pub listener_isolation: i64,
    pub tls_config: Option<String>,
    pub metadata: Option<String>,
    pub bootstrap_uri: Option<String>,
    pub bootstrap_revision: i64,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Persisted API definition record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDefinitionData {
    pub id: String,
    pub team: String,
    pub domain: String,
    pub listener_isolation: bool,
    pub tls_config: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
    pub bootstrap_uri: Option<String>,
    pub bootstrap_revision: i64,
    pub version: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ApiDefinitionRow> for ApiDefinitionData {
    fn from(row: ApiDefinitionRow) -> Self {
        Self {
            id: row.id,
            team: row.team,
            domain: row.domain,
            listener_isolation: row.listener_isolation != 0,
            tls_config: row.tls_config.and_then(|json| serde_json::from_str(&json).ok()),
            metadata: row.metadata.and_then(|json| serde_json::from_str(&json).ok()),
            bootstrap_uri: row.bootstrap_uri,
            bootstrap_revision: row.bootstrap_revision,
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
struct ApiRouteRow {
    pub id: String,
    pub api_definition_id: String,
    pub match_type: String,
    pub match_value: String,
    pub case_sensitive: i64,
    pub rewrite_prefix: Option<String>,
    pub rewrite_regex: Option<String>,
    pub rewrite_substitution: Option<String>,
    pub upstream_targets: String,
    pub timeout_seconds: Option<i64>,
    pub override_config: Option<String>,
    pub deployment_note: Option<String>,
    pub route_order: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Persisted API route record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiRouteData {
    pub id: String,
    pub api_definition_id: String,
    pub match_type: String,
    pub match_value: String,
    pub case_sensitive: bool,
    pub rewrite_prefix: Option<String>,
    pub rewrite_regex: Option<String>,
    pub rewrite_substitution: Option<String>,
    pub upstream_targets: serde_json::Value,
    pub timeout_seconds: Option<i64>,
    pub override_config: Option<serde_json::Value>,
    pub deployment_note: Option<String>,
    pub route_order: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ApiRouteRow> for ApiRouteData {
    fn from(row: ApiRouteRow) -> Self {
        Self {
            id: row.id,
            api_definition_id: row.api_definition_id,
            match_type: row.match_type,
            match_value: row.match_value,
            case_sensitive: row.case_sensitive != 0,
            rewrite_prefix: row.rewrite_prefix,
            rewrite_regex: row.rewrite_regex,
            rewrite_substitution: row.rewrite_substitution,
            upstream_targets: serde_json::from_str(&row.upstream_targets)
                .unwrap_or(serde_json::Value::Null),
            timeout_seconds: row.timeout_seconds,
            override_config: row.override_config.and_then(|json| serde_json::from_str(&json).ok()),
            deployment_note: row.deployment_note,
            route_order: row.route_order,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Create API definition request payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApiDefinitionRequest {
    pub team: String,
    pub domain: String,
    pub listener_isolation: bool,
    pub tls_config: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
}

/// Create API route request payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApiRouteRequest {
    pub api_definition_id: String,
    pub match_type: String,
    pub match_value: String,
    pub case_sensitive: bool,
    pub rewrite_prefix: Option<String>,
    pub rewrite_regex: Option<String>,
    pub rewrite_substitution: Option<String>,
    pub upstream_targets: serde_json::Value,
    pub timeout_seconds: Option<i64>,
    pub override_config: Option<serde_json::Value>,
    pub deployment_note: Option<String>,
    pub route_order: i64,
}

/// Parameters for updating bootstrap metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateBootstrapMetadataRequest {
    pub definition_id: String,
    pub bootstrap_uri: Option<String>,
    pub bootstrap_revision: i64,
}

/// Repository encapsulating persistence for API definitions and routes
#[derive(Debug, Clone)]
pub struct ApiDefinitionRepository {
    pool: DbPool,
}

impl ApiDefinitionRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> DbPool {
        self.pool.clone()
    }

    fn serialize_optional(value: &Option<serde_json::Value>) -> Result<Option<String>> {
        value
            .as_ref()
            .map(|val| {
                serde_json::to_string(val).map_err(|e| {
                    FlowplaneError::validation(format!("Failed to serialize JSON payload: {}", e))
                })
            })
            .transpose()
    }

    fn serialize_required(value: &serde_json::Value) -> Result<String> {
        serde_json::to_string(value).map_err(|e| {
            FlowplaneError::validation(format!("Failed to serialize JSON payload: {}", e))
        })
    }

    /// Insert a new API definition record
    pub async fn create_definition(
        &self,
        request: CreateApiDefinitionRequest,
    ) -> Result<ApiDefinitionData> {
        let id = Uuid::new_v4().to_string();
        let tls_config = Self::serialize_optional(&request.tls_config)?;
        let metadata = Self::serialize_optional(&request.metadata)?;
        let listener_isolation: i64 = if request.listener_isolation { 1 } else { 0 };

        let now = chrono::Utc::now();

        sqlx::query::<Sqlite>(
            "INSERT INTO api_definitions (
                id, team, domain, listener_isolation, tls_config, metadata, bootstrap_uri,
                bootstrap_revision, version, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(&id)
        .bind(&request.team)
        .bind(&request.domain)
        .bind(listener_isolation)
        .bind(tls_config)
        .bind(metadata)
        .bind(Option::<String>::None)
        .bind(0_i64)
        .bind(1_i64)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to insert API definition".to_string(),
        })?;

        self.get_definition(&id).await
    }

    /// Delete an API definition by ID (cascades to routes)
    pub async fn delete_definition(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM api_definitions WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: "Failed to delete API definition".to_string(),
            })?;
        Ok(())
    }

    /// Fetch an API definition by identifier
    pub async fn get_definition(&self, id: &str) -> Result<ApiDefinitionData> {
        let row = sqlx::query_as::<Sqlite, ApiDefinitionRow>(
            "SELECT id, team, domain, listener_isolation, tls_config, metadata, bootstrap_uri,
                    bootstrap_revision, version, created_at, updated_at
             FROM api_definitions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to load API definition '{}'", id),
        })?;

        row.map(ApiDefinitionData::from)
            .ok_or_else(|| FlowplaneError::not_found(format!("API definition '{}' not found", id)))
    }

    /// Fetch an API definition by team/domain (if present)
    pub async fn find_by_team_domain(
        &self,
        team: &str,
        domain: &str,
    ) -> Result<Option<ApiDefinitionData>> {
        let row = sqlx::query_as::<Sqlite, ApiDefinitionRow>(
            "SELECT id, team, domain, listener_isolation, tls_config, metadata, bootstrap_uri,
                    bootstrap_revision, version, created_at, updated_at
             FROM api_definitions WHERE team = $1 AND domain = $2 LIMIT 1",
        )
        .bind(team)
        .bind(domain)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to query API definition by team/domain".to_string(),
        })?;

        Ok(row.map(ApiDefinitionData::from))
    }

    /// Fetch an API definition by domain regardless of owning team
    pub async fn find_by_domain(&self, domain: &str) -> Result<Option<ApiDefinitionData>> {
        let row = sqlx::query_as::<Sqlite, ApiDefinitionRow>(
            "SELECT id, team, domain, listener_isolation, tls_config, metadata, bootstrap_uri,
                    bootstrap_revision, version, created_at, updated_at
             FROM api_definitions WHERE domain = $1 LIMIT 1",
        )
        .bind(domain)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to query API definition by domain".to_string(),
        })?;

        Ok(row.map(ApiDefinitionData::from))
    }

    /// Append a new route to an API definition
    pub async fn create_route(&self, request: CreateApiRouteRequest) -> Result<ApiRouteData> {
        let id = Uuid::new_v4().to_string();
        let upstream_json = Self::serialize_required(&request.upstream_targets)?;
        let overrides_json = Self::serialize_optional(&request.override_config)?;
        let case_sensitive = if request.case_sensitive { 1 } else { 0 };

        let now = chrono::Utc::now();

        sqlx::query::<Sqlite>(
            "INSERT INTO api_routes (
                id,
                api_definition_id,
                match_type,
                match_value,
                case_sensitive,
                rewrite_prefix,
                rewrite_regex,
                rewrite_substitution,
                upstream_targets,
                timeout_seconds,
                override_config,
                deployment_note,
                route_order,
                created_at,
                updated_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15
            )",
        )
        .bind(&id)
        .bind(&request.api_definition_id)
        .bind(&request.match_type)
        .bind(&request.match_value)
        .bind(case_sensitive)
        .bind(&request.rewrite_prefix)
        .bind(&request.rewrite_regex)
        .bind(&request.rewrite_substitution)
        .bind(upstream_json)
        .bind(request.timeout_seconds)
        .bind(overrides_json)
        .bind(&request.deployment_note)
        .bind(request.route_order)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to insert API route".to_string(),
        })?;

        self.get_route(&id).await
    }

    /// Retrieve a route by identifier
    pub async fn get_route(&self, id: &str) -> Result<ApiRouteData> {
        let row = sqlx::query_as::<Sqlite, ApiRouteRow>(
            "SELECT id, api_definition_id, match_type, match_value, case_sensitive, rewrite_prefix,
                    rewrite_regex, rewrite_substitution, upstream_targets, timeout_seconds,
                    override_config, deployment_note, route_order, created_at, updated_at
             FROM api_routes WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to load API route '{}'", id),
        })?;

        row.map(ApiRouteData::from)
            .ok_or_else(|| FlowplaneError::not_found(format!("API route '{}' not found", id)))
    }

    /// List routes for a given definition ordered by insertion order
    pub async fn list_routes(&self, api_definition_id: &str) -> Result<Vec<ApiRouteData>> {
        let rows = sqlx::query_as::<Sqlite, ApiRouteRow>(
            "SELECT id, api_definition_id, match_type, match_value, case_sensitive, rewrite_prefix,
                    rewrite_regex, rewrite_substitution, upstream_targets, timeout_seconds,
                    override_config, deployment_note, route_order, created_at, updated_at
             FROM api_routes WHERE api_definition_id = $1
             ORDER BY route_order ASC, created_at ASC",
        )
        .bind(api_definition_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to list API routes".to_string(),
        })?;

        Ok(rows.into_iter().map(ApiRouteData::from).collect())
    }

    /// Update bootstrap metadata for a definition
    pub async fn update_bootstrap_metadata(
        &self,
        request: UpdateBootstrapMetadataRequest,
    ) -> Result<ApiDefinitionData> {
        let now = chrono::Utc::now();

        sqlx::query(
            "UPDATE api_definitions
             SET bootstrap_uri = $2,
                 bootstrap_revision = $3,
                 updated_at = $4,
                 version = version + 1
             WHERE id = $1",
        )
        .bind(&request.definition_id)
        .bind(&request.bootstrap_uri)
        .bind(request.bootstrap_revision)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to update bootstrap metadata".to_string(),
        })?;

        self.get_definition(&request.definition_id).await
    }

    pub async fn list_definitions(&self) -> Result<Vec<ApiDefinitionData>> {
        let rows = sqlx::query_as::<Sqlite, ApiDefinitionRow>(
            "SELECT id, team, domain, listener_isolation, tls_config, metadata, bootstrap_uri,
                    bootstrap_revision, version, created_at, updated_at
             FROM api_definitions",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to list API definitions".to_string(),
        })?;

        Ok(rows.into_iter().map(ApiDefinitionData::from).collect())
    }

    pub async fn list_all_routes(&self) -> Result<Vec<ApiRouteData>> {
        let rows = sqlx::query_as::<Sqlite, ApiRouteRow>(
            "SELECT id, api_definition_id, match_type, match_value, case_sensitive, rewrite_prefix,
                    rewrite_regex, rewrite_substitution, upstream_targets, timeout_seconds,
                    override_config, deployment_note, route_order, created_at, updated_at
             FROM api_routes ORDER BY api_definition_id, route_order",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to list API routes".to_string(),
        })?;

        Ok(rows.into_iter().map(ApiRouteData::from).collect())
    }
}

/// Create route request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRouteRequest {
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
    pub configuration: serde_json::Value,
}

/// Update route request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRouteRequest {
    pub path_prefix: Option<String>,
    pub cluster_name: Option<String>,
    pub configuration: Option<serde_json::Value>,
}

/// Create cluster request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateClusterRequest {
    pub name: String,
    pub service_name: String,
    pub configuration: serde_json::Value,
}

/// Update cluster request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateClusterRequest {
    pub service_name: Option<String>,
    pub configuration: Option<serde_json::Value>,
}

/// Create listener request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateListenerRequest {
    pub name: String,
    pub address: String,
    pub port: Option<i64>,
    pub protocol: Option<String>,
    pub configuration: serde_json::Value,
}

/// Update listener request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateListenerRequest {
    pub address: Option<String>,
    pub port: Option<Option<i64>>,
    pub protocol: Option<String>,
    pub configuration: Option<serde_json::Value>,
}

/// Repository for cluster data access (simplified version)
#[derive(Debug, Clone)]
pub struct ClusterRepository {
    pool: DbPool,
}

impl ClusterRepository {
    /// Create a new cluster repository
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create a new cluster
    pub async fn create(&self, request: CreateClusterRequest) -> Result<ClusterData> {
        let id = Uuid::new_v4().to_string();
        let configuration_json = serde_json::to_string(&request.configuration).map_err(|e| {
            FlowplaneError::validation(format!("Invalid configuration JSON: {}", e))
        })?;
        let now = chrono::Utc::now();

        // Use parameterized query with positional parameters (works with both SQLite and PostgreSQL)
        let result = sqlx::query(
            "INSERT INTO clusters (id, name, service_name, configuration, version, created_at, updated_at) VALUES ($1, $2, $3, $4, 1, $5, $6)"
        )
        .bind(&id)
        .bind(&request.name)
        .bind(&request.service_name)
        .bind(&configuration_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_name = %request.name, "Failed to create cluster");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create cluster '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create cluster"));
        }

        tracing::info!(
            cluster_id = %id,
            cluster_name = %request.name,
            service_name = %request.service_name,
            "Created new cluster"
        );

        // Return the created cluster
        self.get_by_id(&id).await
    }

    /// Get cluster by ID
    pub async fn get_by_id(&self, id: &str) -> Result<ClusterData> {
        let row = sqlx::query_as::<Sqlite, ClusterRow>(
            "SELECT id, name, service_name, configuration, version, created_at, updated_at FROM clusters WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_id = %id, "Failed to get cluster by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get cluster with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => Ok(ClusterData::from(row)),
            None => Err(FlowplaneError::not_found(format!("Cluster with ID '{}' not found", id))),
        }
    }

    /// Get cluster by name
    pub async fn get_by_name(&self, name: &str) -> Result<ClusterData> {
        let row = sqlx::query_as::<Sqlite, ClusterRow>(
            "SELECT id, name, service_name, configuration, version, created_at, updated_at FROM clusters WHERE name = $1 ORDER BY version DESC LIMIT 1"
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_name = %name, "Failed to get cluster by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get cluster with name '{}'", name),
            }
        })?;

        match row {
            Some(row) => Ok(ClusterData::from(row)),
            None => {
                Err(FlowplaneError::not_found(format!("Cluster with name '{}' not found", name)))
            }
        }
    }

    /// List all clusters
    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<ClusterData>> {
        let limit = limit.unwrap_or(100).min(1000); // Max 1000 results
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<Sqlite, ClusterRow>(
            "SELECT id, name, service_name, configuration, version, created_at, updated_at FROM clusters ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list clusters");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list clusters".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(ClusterData::from).collect())
    }

    /// Update cluster
    pub async fn update(&self, id: &str, request: UpdateClusterRequest) -> Result<ClusterData> {
        // Get current cluster to check if it exists and get current values
        let current = self.get_by_id(id).await?;

        let new_service_name = request.service_name.unwrap_or(current.service_name);
        let new_configuration = if let Some(config) = request.configuration {
            serde_json::to_string(&config).map_err(|e| {
                FlowplaneError::validation(format!("Invalid configuration JSON: {}", e))
            })?
        } else {
            current.configuration
        };

        let now = chrono::Utc::now();
        let new_version = current.version + 1;

        let result = sqlx::query(
            "UPDATE clusters SET service_name = $1, configuration = $2, version = $3, updated_at = $4 WHERE id = $5"
        )
        .bind(&new_service_name)
        .bind(&new_configuration)
        .bind(new_version)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, cluster_id = %id, "Failed to update cluster");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update cluster with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found(format!("Cluster with ID '{}' not found", id)));
        }

        tracing::info!(
            cluster_id = %id,
            cluster_name = %current.name,
            new_version = new_version,
            "Updated cluster"
        );

        // Return the updated cluster
        self.get_by_id(id).await
    }

    /// Delete cluster
    pub async fn delete(&self, id: &str) -> Result<()> {
        // Check if cluster exists first
        let cluster = self.get_by_id(id).await?;

        let result = sqlx::query("DELETE FROM clusters WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, cluster_id = %id, "Failed to delete cluster");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete cluster with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found(format!("Cluster with ID '{}' not found", id)));
        }

        tracing::info!(
            cluster_id = %id,
            cluster_name = %cluster.name,
            "Deleted cluster"
        );

        Ok(())
    }

    /// Check if cluster exists by name
    pub async fn exists_by_name(&self, name: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clusters WHERE name = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, cluster_name = %name, "Failed to check cluster existence");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to check existence of cluster '{}'", name),
                }
            })?;

        Ok(count > 0)
    }

    pub async fn delete_by_name(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM clusters WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, cluster_name = %name, "Failed to delete cluster by name");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete cluster '{}'", name),
                }
            })?;

        Ok(())
    }

    /// Get cluster count
    pub async fn count(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clusters")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get cluster count");
                FlowplaneError::Database {
                    source: e,
                    context: "Failed to get cluster count".to_string(),
                }
            })?;

        Ok(count)
    }

    /// Access the underlying pool (used by background watchers)
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
}

/// Repository for listener data access
#[derive(Debug, Clone)]
pub struct ListenerRepository {
    pool: DbPool,
}

impl ListenerRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, request: CreateListenerRequest) -> Result<ListenerData> {
        let id = Uuid::new_v4().to_string();
        let configuration_json = serde_json::to_string(&request.configuration).map_err(|e| {
            FlowplaneError::validation(format!("Invalid listener configuration JSON: {}", e))
        })?;
        let now = chrono::Utc::now();
        let protocol = request.protocol.unwrap_or_else(|| "HTTP".to_string());

        let result = sqlx::query(
            "INSERT INTO listeners (id, name, address, port, protocol, configuration, version, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, 1, $7, $8)"
        )
        .bind(&id)
        .bind(&request.name)
        .bind(&request.address)
        .bind(request.port)
        .bind(&protocol)
        .bind(&configuration_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_name = %request.name, "Failed to create listener");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create listener '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create listener"));
        }

        tracing::info!(listener_id = %id, listener_name = %request.name, "Created new listener");

        self.get_by_id(&id).await
    }

    // removed create_tx variant to avoid cross-transaction complexity; use create()

    pub async fn get_by_id(&self, id: &str) -> Result<ListenerData> {
        let row = sqlx::query_as::<Sqlite, ListenerRow>(
            "SELECT id, name, address, port, protocol, configuration, version, created_at, updated_at FROM listeners WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %id, "Failed to get listener by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get listener with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => Ok(ListenerData::from(row)),
            None => Err(FlowplaneError::not_found(format!("Listener with ID '{}' not found", id))),
        }
    }

    pub async fn get_by_name(&self, name: &str) -> Result<ListenerData> {
        let row = sqlx::query_as::<Sqlite, ListenerRow>(
            "SELECT id, name, address, port, protocol, configuration, version, created_at, updated_at FROM listeners WHERE name = $1 ORDER BY version DESC LIMIT 1"
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_name = %name, "Failed to get listener by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get listener with name '{}'", name),
            }
        })?;

        match row {
            Some(row) => Ok(ListenerData::from(row)),
            None => {
                Err(FlowplaneError::not_found(format!("Listener with name '{}' not found", name)))
            }
        }
    }

    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<ListenerData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<Sqlite, ListenerRow>(
            "SELECT id, name, address, port, protocol, configuration, version, created_at, updated_at FROM listeners ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list listeners");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list listeners".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(ListenerData::from).collect())
    }

    pub async fn update(&self, id: &str, request: UpdateListenerRequest) -> Result<ListenerData> {
        let current = self.get_by_id(id).await?;

        let current_address = current.address.clone();
        let current_protocol = current.protocol.clone();
        let current_configuration = current.configuration.clone();
        let current_name = current.name.clone();

        let new_address = request.address.unwrap_or(current_address);
        let new_port = match request.port {
            Some(value) => value,
            None => current.port,
        };
        let new_protocol = request.protocol.unwrap_or(current_protocol);
        let new_configuration = if let Some(config) = request.configuration {
            serde_json::to_string(&config).map_err(|e| {
                FlowplaneError::validation(format!("Invalid listener configuration JSON: {}", e))
            })?
        } else {
            current_configuration
        };

        let now = chrono::Utc::now();
        let new_version = current.version + 1;

        let result = sqlx::query(
            "UPDATE listeners SET address = $1, port = $2, protocol = $3, configuration = $4, version = $5, updated_at = $6 WHERE id = $7"
        )
        .bind(&new_address)
        .bind(new_port)
        .bind(&new_protocol)
        .bind(&new_configuration)
        .bind(new_version)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, listener_id = %id, "Failed to update listener");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update listener with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found(format!("Listener with ID '{}' not found", id)));
        }

        tracing::info!(listener_id = %id, listener_name = %current_name, new_version = new_version, "Updated listener");

        self.get_by_id(id).await
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let listener = self.get_by_id(id).await?;

        let result = sqlx::query("DELETE FROM listeners WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, listener_id = %id, "Failed to delete listener");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete listener with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found(format!("Listener with ID '{}' not found", id)));
        }

        tracing::info!(listener_id = %id, listener_name = %listener.name, "Deleted listener");

        Ok(())
    }

    pub async fn delete_by_name(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM listeners WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| {
            tracing::error!(error = %e, listener_name = %name, "Failed to delete listener by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to delete listener '{}'", name),
            }
        })?;

        Ok(())
    }

    pub async fn exists_by_name(&self, name: &str) -> Result<bool> {
        let count = sqlx::query_scalar::<Sqlite, i64>("SELECT COUNT(*) FROM listeners WHERE name = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, listener_name = %name, "Failed to check listener existence");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to check existence of listener '{}'", name),
                }
            })?;

        Ok(count > 0)
    }

    pub async fn count(&self) -> Result<i64> {
        let count = sqlx::query_scalar::<Sqlite, i64>("SELECT COUNT(*) FROM listeners")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get listener count");
                FlowplaneError::Database {
                    source: e,
                    context: "Failed to get listener count".to_string(),
                }
            })?;

        Ok(count)
    }

    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
}

/// Repository for route data access
#[derive(Debug, Clone)]
pub struct RouteRepository {
    pool: DbPool,
}

impl RouteRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, request: CreateRouteRequest) -> Result<RouteData> {
        let id = Uuid::new_v4().to_string();
        let configuration_json = serde_json::to_string(&request.configuration).map_err(|e| {
            FlowplaneError::validation(format!("Invalid route configuration JSON: {}", e))
        })?;
        let now = chrono::Utc::now();

        let result = sqlx::query(
            "INSERT INTO routes (id, name, path_prefix, cluster_name, configuration, version, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, 1, $6, $7)"
        )
        .bind(&id)
        .bind(&request.name)
        .bind(&request.path_prefix)
        .bind(&request.cluster_name)
        .bind(&configuration_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_name = %request.name, "Failed to create route");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create route '{}'", request.name),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::validation("Failed to create route"));
        }

        tracing::info!(route_id = %id, route_name = %request.name, "Created new route");

        self.get_by_id(&id).await
    }

    pub async fn get_by_id(&self, id: &str) -> Result<RouteData> {
        let row = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, created_at, updated_at FROM routes WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %id, "Failed to get route by ID");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get route with ID '{}'", id),
            }
        })?;

        match row {
            Some(row) => Ok(RouteData::from(row)),
            None => Err(FlowplaneError::not_found(format!("Route with ID '{}' not found", id))),
        }
    }

    pub async fn get_by_name(&self, name: &str) -> Result<RouteData> {
        let row = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, created_at, updated_at FROM routes WHERE name = $1 ORDER BY version DESC LIMIT 1"
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_name = %name, "Failed to get route by name");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to get route with name '{}'", name),
            }
        })?;

        match row {
            Some(row) => Ok(RouteData::from(row)),
            None => Err(FlowplaneError::not_found(format!("Route with name '{}' not found", name))),
        }
    }

    pub async fn exists_by_name(&self, name: &str) -> Result<bool> {
        let count = sqlx::query_scalar::<Sqlite, i64>(
            "SELECT COUNT(*) FROM routes WHERE name = $1",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_name = %name, "Failed to check route existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check existence of route '{}'", name),
            }
        })?;

        Ok(count > 0)
    }

    pub async fn list(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<RouteData>> {
        let limit = limit.unwrap_or(100).min(1000);
        let offset = offset.unwrap_or(0);

        let rows = sqlx::query_as::<Sqlite, RouteRow>(
            "SELECT id, name, path_prefix, cluster_name, configuration, version, created_at, updated_at FROM routes ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list routes");
            FlowplaneError::Database {
                source: e,
                context: "Failed to list routes".to_string(),
            }
        })?;

        Ok(rows.into_iter().map(RouteData::from).collect())
    }

    pub async fn update(&self, id: &str, request: UpdateRouteRequest) -> Result<RouteData> {
        let current = self.get_by_id(id).await?;

        let new_path_prefix = request.path_prefix.unwrap_or(current.path_prefix);
        let new_cluster_name = request.cluster_name.unwrap_or(current.cluster_name);
        let new_configuration = if let Some(config) = request.configuration {
            serde_json::to_string(&config).map_err(|e| {
                FlowplaneError::validation(format!("Invalid route configuration JSON: {}", e))
            })?
        } else {
            current.configuration
        };

        let now = chrono::Utc::now();
        let new_version = current.version + 1;

        let result = sqlx::query(
            "UPDATE routes SET path_prefix = $1, cluster_name = $2, configuration = $3, version = $4, updated_at = $5 WHERE id = $6"
        )
        .bind(&new_path_prefix)
        .bind(&new_cluster_name)
        .bind(&new_configuration)
        .bind(new_version)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_id = %id, "Failed to update route");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update route with ID '{}'", id),
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found(format!("Route with ID '{}' not found", id)));
        }

        tracing::info!(route_id = %id, route_name = %current.name, new_version = new_version, "Updated route");

        self.get_by_id(id).await
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let route = self.get_by_id(id).await?;

        let result = sqlx::query("DELETE FROM routes WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, route_id = %id, "Failed to delete route");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete route with ID '{}'", id),
                }
            })?;

        if result.rows_affected() == 0 {
            return Err(FlowplaneError::not_found(format!("Route with ID '{}' not found", id)));
        }

        tracing::info!(route_id = %id, route_name = %route.name, "Deleted route");

        Ok(())
    }

    pub async fn delete_by_name(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM routes WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, route_name = %name, "Failed to delete route by name");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete route '{}'", name),
                }
            })?;

        Ok(())
    }

    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseConfig;
    use crate::storage::create_pool;

    async fn create_test_pool() -> crate::storage::DbPool {
        let config = DatabaseConfig {
            url: "sqlite://:memory:".to_string(),
            auto_migrate: false,
            ..Default::default()
        };
        let pool = create_pool(&config).await.unwrap();

        // Run basic schema creation for testing
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS clusters (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                service_name TEXT NOT NULL,
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(name, version)
            )
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS routes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                path_prefix TEXT NOT NULL,
                cluster_name TEXT NOT NULL,
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(name, version)
            )
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS listeners (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                address TEXT NOT NULL,
                port INTEGER,
                protocol TEXT NOT NULL DEFAULT 'HTTP',
                configuration TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(name, version)
            )
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    #[tokio::test]
    async fn test_cluster_crud_operations() {
        let pool = create_test_pool().await;
        let repo = ClusterRepository::new(pool);

        // Create a test cluster
        let create_request = CreateClusterRequest {
            name: "test_cluster".to_string(),
            service_name: "test_service".to_string(),
            configuration: serde_json::json!({
                "type": "EDS",
                "endpoints": ["127.0.0.1:8080"]
            }),
        };

        let created = repo.create(create_request).await.unwrap();
        assert_eq!(created.name, "test_cluster");
        assert_eq!(created.service_name, "test_service");
        assert_eq!(created.version, 1);

        // Get by ID
        let fetched = repo.get_by_id(&created.id).await.unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.name, created.name);

        // Get by name
        let fetched_by_name = repo.get_by_name("test_cluster").await.unwrap();
        assert_eq!(fetched_by_name.id, created.id);

        // Update cluster
        let update_request = UpdateClusterRequest {
            service_name: Some("updated_service".to_string()),
            configuration: Some(serde_json::json!({
                "type": "EDS",
                "endpoints": ["127.0.0.1:9090"]
            })),
        };

        let updated = repo.update(&created.id, update_request).await.unwrap();
        assert_eq!(updated.service_name, "updated_service");
        assert_eq!(updated.version, 2);

        // List clusters
        let clusters = repo.list(None, None).await.unwrap();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].id, created.id);

        // Check existence
        assert!(repo.exists_by_name("test_cluster").await.unwrap());
        assert!(!repo.exists_by_name("nonexistent").await.unwrap());

        // Get count
        let count = repo.count().await.unwrap();
        assert_eq!(count, 1);

        // Delete cluster
        repo.delete(&created.id).await.unwrap();

        // Verify deletion
        assert!(repo.get_by_id(&created.id).await.is_err());
        let count_after_delete = repo.count().await.unwrap();
        assert_eq!(count_after_delete, 0);
    }

    #[tokio::test]
    async fn test_cluster_not_found() {
        let pool = create_test_pool().await;
        let repo = ClusterRepository::new(pool);

        let result = repo.get_by_id("nonexistent-id").await;
        assert!(result.is_err());

        let result = repo.get_by_name("nonexistent-name").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_route_crud_operations() {
        let pool = create_test_pool().await;
        let repo = RouteRepository::new(pool.clone());

        let create_request = CreateRouteRequest {
            name: "test_route".to_string(),
            path_prefix: "/api".to_string(),
            cluster_name: "cluster-a".to_string(),
            configuration: serde_json::json!({
                "name": "test_route",
                "virtualHosts": [
                    {
                        "name": "default",
                        "domains": ["*"],
                        "routes": [
                            {
                                "name": "api",
                                "match": {
                                    "path": { "Prefix": "/api" }
                                },
                                "action": {
                                    "Cluster": {
                                        "name": "cluster-a"
                                    }
                                }
                            }
                        ]
                    }
                ]
            }),
        };

        let created = repo.create(create_request).await.unwrap();
        assert_eq!(created.name, "test_route");
        assert_eq!(created.version, 1);

        let fetched = repo.get_by_id(&created.id).await.unwrap();
        assert_eq!(fetched.id, created.id);

        let fetched_by_name = repo.get_by_name("test_route").await.unwrap();
        assert_eq!(fetched_by_name.id, created.id);

        let update_request = UpdateRouteRequest {
            path_prefix: Some("/api/v2".to_string()),
            cluster_name: Some("cluster-b".to_string()),
            configuration: Some(serde_json::json!({
                "name": "test_route",
                "virtualHosts": [
                    {
                        "name": "default",
                        "domains": ["*"],
                        "routes": [
                            {
                                "name": "api",
                                "match": {
                                    "path": { "Prefix": "/api/v2" }
                                },
                                "action": {
                                    "WeightedClusters": {
                                        "clusters": [
                                            {"name": "cluster-b", "weight": 70},
                                            {"name": "cluster-c", "weight": 30}
                                        ]
                                    }
                                }
                            }
                        ]
                    }
                ]
            })),
        };

        let updated = repo.update(&created.id, update_request).await.unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(updated.path_prefix, "/api/v2");
        assert_eq!(updated.cluster_name, "cluster-b");

        let listed = repo.list(None, None).await.unwrap();
        assert_eq!(listed.len(), 1);

        repo.delete(&created.id).await.unwrap();
        assert!(repo.get_by_id(&created.id).await.is_err());
    }

    #[tokio::test]
    async fn test_listener_crud_operations() {
        let pool = create_test_pool().await;
        let repo = ListenerRepository::new(pool);

        let create_request = CreateListenerRequest {
            name: "test-listener".to_string(),
            address: "0.0.0.0".to_string(),
            port: Some(8080),
            protocol: Some("HTTP".to_string()),
            configuration: serde_json::json!({
                "name": "test-listener",
                "address": "0.0.0.0",
                "port": 8080
            }),
        };

        let created = repo.create(create_request).await.unwrap();
        assert_eq!(created.name, "test-listener");
        assert_eq!(created.port, Some(8080));
        assert_eq!(created.protocol, "HTTP");
        assert_eq!(created.version, 1);

        let fetched = repo.get_by_id(&created.id).await.unwrap();
        assert_eq!(fetched.id, created.id);

        let fetched_by_name = repo.get_by_name("test-listener").await.unwrap();
        assert_eq!(fetched_by_name.id, created.id);

        let update_request = UpdateListenerRequest {
            address: Some("127.0.0.1".to_string()),
            port: Some(Some(9090)),
            protocol: Some("TCP".to_string()),
            configuration: Some(serde_json::json!({
                "name": "test-listener",
                "address": "127.0.0.1",
                "port": 9090
            })),
        };

        let updated = repo.update(&created.id, update_request).await.unwrap();
        assert_eq!(updated.address, "127.0.0.1");
        assert_eq!(updated.port, Some(9090));
        assert_eq!(updated.protocol, "TCP");
        assert_eq!(updated.version, 2);

        let listeners = repo.list(None, None).await.unwrap();
        assert_eq!(listeners.len(), 1);

        assert!(repo.exists_by_name("test-listener").await.unwrap());
        assert!(!repo.exists_by_name("missing").await.unwrap());

        assert_eq!(repo.count().await.unwrap(), 1);

        repo.delete(&created.id).await.unwrap();
        assert!(repo.get_by_id(&created.id).await.is_err());
        assert_eq!(repo.count().await.unwrap(), 0);
    }
}

/// Audit event descriptor for authentication activity logging.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub action: String,
    pub resource_id: Option<String>,
    pub resource_name: Option<String>,
    pub metadata: serde_json::Value,
}

impl AuditEvent {
    pub fn token(
        action: &str,
        resource_id: Option<&str>,
        resource_name: Option<&str>,
        metadata: serde_json::Value,
    ) -> Self {
        Self {
            action: action.to_string(),
            resource_id: resource_id.map(|value| value.to_string()),
            resource_name: resource_name.map(|value| value.to_string()),
            metadata,
        }
    }
}

/// Repository for audit log interactions (scaffold for auth events).
#[derive(Debug, Clone)]
pub struct AuditLogRepository {
    pool: DbPool,
}

impl AuditLogRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    async fn record_event(&self, resource_type: &str, event: AuditEvent) -> Result<()> {
        let now = chrono::Utc::now();
        let metadata_json = serde_json::to_string(&event.metadata).map_err(|err| {
            FlowplaneError::validation(format!("Invalid audit metadata JSON: {}", err))
        })?;
        let resource_name = event.resource_name.unwrap_or_else(|| event.action.clone());

        sqlx::query(
            "INSERT INTO audit_log (resource_type, resource_id, resource_name, action, old_configuration, new_configuration, user_id, client_ip, user_agent, created_at) \
             VALUES ($1, $2, $3, $4, NULL, $5, NULL, NULL, NULL, $6)"
        )
        .bind(resource_type)
        .bind(event.resource_id.as_deref())
        .bind(&resource_name)
        .bind(event.action.as_str())
        .bind(metadata_json)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to write authentication audit event".to_string(),
        })?;

        Ok(())
    }

    /// Record an authentication-related audit event.
    pub async fn record_auth_event(&self, event: AuditEvent) -> Result<()> {
        self.record_event("auth.token", event).await
    }

    /// Record a Platform API lifecycle event.
    pub async fn record_platform_event(&self, event: AuditEvent) -> Result<()> {
        self.record_event("platform.api", event).await
    }
}

#[derive(Debug, Clone, FromRow)]
struct PersonalAccessTokenRow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub token_hash: String,
    pub status: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_by: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, FromRow)]
struct TokenScopeRow {
    pub scope: String,
}

#[async_trait]
pub trait TokenRepository: Send + Sync {
    async fn create_token(&self, token: NewPersonalAccessToken) -> Result<PersonalAccessToken>;
    async fn list_tokens(&self, limit: i64, offset: i64) -> Result<Vec<PersonalAccessToken>>;
    async fn get_token(&self, id: &str) -> Result<PersonalAccessToken>;
    async fn update_metadata(
        &self,
        id: &str,
        update: UpdatePersonalAccessToken,
    ) -> Result<PersonalAccessToken>;
    async fn rotate_secret(&self, id: &str, hashed_secret: String) -> Result<()>;
    async fn update_last_used(&self, id: &str, when: chrono::DateTime<chrono::Utc>) -> Result<()>;
    async fn find_active_for_auth(&self, id: &str)
        -> Result<Option<(PersonalAccessToken, String)>>;
    async fn count_tokens(&self) -> Result<i64>;
    async fn count_active_tokens(&self) -> Result<i64>;
}

#[derive(Debug, Clone)]
pub struct SqlxTokenRepository {
    pool: DbPool,
}

impl SqlxTokenRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    fn to_model(
        &self,
        row: PersonalAccessTokenRow,
        scopes: Vec<String>,
    ) -> Result<PersonalAccessToken> {
        let status = TokenStatus::from_str(&row.status).map_err(|_| {
            FlowplaneError::validation(format!(
                "Unknown token status '{}' for token {}",
                row.status, row.id
            ))
        })?;

        Ok(PersonalAccessToken {
            id: row.id,
            name: row.name,
            description: row.description,
            status,
            expires_at: row.expires_at,
            last_used_at: row.last_used_at,
            created_by: row.created_by,
            created_at: row.created_at,
            updated_at: row.updated_at,
            scopes,
        })
    }

    async fn scopes_for_token(&self, id: &str) -> Result<Vec<String>> {
        let rows: Vec<TokenScopeRow> =
            sqlx::query_as("SELECT scope FROM token_scopes WHERE token_id = $1 ORDER BY scope")
                .bind(id)
                .fetch_all(&self.pool)
                .await
                .map_err(|err| FlowplaneError::Database {
                    source: err,
                    context: "Failed to fetch token scopes".to_string(),
                })?;

        Ok(rows.into_iter().map(|row| row.scope).collect())
    }
}

#[async_trait]
impl TokenRepository for SqlxTokenRepository {
    async fn create_token(&self, token: NewPersonalAccessToken) -> Result<PersonalAccessToken> {
        let mut tx = self.pool.begin().await.map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to begin transaction for token creation".to_string(),
        })?;

        sqlx::query(
            "INSERT INTO personal_access_tokens (id, name, token_hash, description, status, expires_at, created_by, created_at, updated_at)              VALUES ($1, $2, $3, $4, $5, $6, $7, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"
        )
        .bind(&token.id)
        .bind(&token.name)
        .bind(&token.hashed_secret)
        .bind(token.description.as_ref())
        .bind(token.status.as_str())
        .bind(token.expires_at)
        .bind(token.created_by.as_ref())
        .execute(&mut *tx)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to insert personal access token".to_string(),
        })?;

        for scope in &token.scopes {
            sqlx::query(
                "INSERT INTO token_scopes (id, token_id, scope, created_at) VALUES ($1, $2, $3, CURRENT_TIMESTAMP)"
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&token.id)
            .bind(scope)
            .execute(&mut *tx)
            .await
            .map_err(|err| FlowplaneError::Database {
                source: err,
                context: "Failed to insert token scope".to_string(),
            })?;
        }

        tx.commit().await.map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to commit token creation".to_string(),
        })?;

        self.get_token(&token.id).await
    }

    async fn list_tokens(&self, limit: i64, offset: i64) -> Result<Vec<PersonalAccessToken>> {
        let limit = limit.clamp(1, 1000);
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM personal_access_tokens ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to list personal access tokens".to_string(),
        })?;

        let mut tokens = Vec::with_capacity(ids.len());
        for id in ids {
            tokens.push(self.get_token(&id).await?);
        }
        Ok(tokens)
    }

    async fn get_token(&self, id: &str) -> Result<PersonalAccessToken> {
        let row: PersonalAccessTokenRow = sqlx::query_as(
            "SELECT id, name, description, token_hash, status, expires_at, last_used_at, created_by, created_at, updated_at              FROM personal_access_tokens WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to fetch personal access token".to_string(),
        })?
        .ok_or_else(|| FlowplaneError::not_found(format!("Token '{}' not found", id)))?;

        let scopes = self.scopes_for_token(id).await?;
        self.to_model(row, scopes)
    }

    async fn update_metadata(
        &self,
        id: &str,
        update: UpdatePersonalAccessToken,
    ) -> Result<PersonalAccessToken> {
        let mut tx = self.pool.begin().await.map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to begin transaction for token update".to_string(),
        })?;

        let existing: PersonalAccessTokenRow = sqlx::query_as(
            "SELECT id, name, description, token_hash, status, expires_at, last_used_at, created_by, created_at, updated_at              FROM personal_access_tokens WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to fetch personal access token".to_string(),
        })?
        .ok_or_else(|| FlowplaneError::not_found(format!("Token '{}' not found", id)))?;

        let base_status = TokenStatus::from_str(&existing.status).map_err(|_| {
            FlowplaneError::validation(format!(
                "Unknown token status '{}' for token {}",
                existing.status, existing.id
            ))
        })?;

        let name = update.name.unwrap_or(existing.name.clone());
        let description = update.description.or(existing.description.clone());
        let status = update.status.unwrap_or(base_status);
        let expires_at = update.expires_at.unwrap_or(existing.expires_at);

        sqlx::query(
            "UPDATE personal_access_tokens SET name = $1, description = $2, status = $3, expires_at = $4, updated_at = CURRENT_TIMESTAMP WHERE id = $5"
        )
        .bind(&name)
        .bind(description.as_ref())
        .bind(status.as_str())
        .bind(expires_at)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to update personal access token".to_string(),
        })?;

        if let Some(scopes) = update.scopes {
            sqlx::query("DELETE FROM token_scopes WHERE token_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|err| FlowplaneError::Database {
                    source: err,
                    context: "Failed to delete token scopes".to_string(),
                })?;

            for scope in scopes {
                sqlx::query(
                    "INSERT INTO token_scopes (id, token_id, scope, created_at) VALUES ($1, $2, $3, CURRENT_TIMESTAMP)"
                )
                .bind(Uuid::new_v4().to_string())
                .bind(id)
                .bind(&scope)
                .execute(&mut *tx)
                .await
                .map_err(|err| FlowplaneError::Database {
                    source: err,
                    context: "Failed to insert token scope".to_string(),
                })?;
            }
        }

        tx.commit().await.map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to commit token update".to_string(),
        })?;

        self.get_token(id).await
    }

    async fn rotate_secret(&self, id: &str, hashed_secret: String) -> Result<()> {
        sqlx::query(
            "UPDATE personal_access_tokens SET token_hash = $1, updated_at = CURRENT_TIMESTAMP WHERE id = $2"
        )
        .bind(&hashed_secret)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to rotate token secret".to_string(),
        })?;
        Ok(())
    }

    async fn update_last_used(&self, id: &str, when: chrono::DateTime<chrono::Utc>) -> Result<()> {
        sqlx::query(
            "UPDATE personal_access_tokens SET last_used_at = $1, updated_at = CURRENT_TIMESTAMP WHERE id = $2"
        )
        .bind(when)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to update token last_used_at".to_string(),
        })?;
        Ok(())
    }

    async fn find_active_for_auth(
        &self,
        id: &str,
    ) -> Result<Option<(PersonalAccessToken, String)>> {
        let row: Option<PersonalAccessTokenRow> = sqlx::query_as(
            "SELECT id, name, description, token_hash, status, expires_at, last_used_at, created_by, created_at, updated_at              FROM personal_access_tokens WHERE id = $1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to fetch personal access token".to_string(),
        })?;

        let Some(row) = row else {
            return Ok(None);
        };

        let hashed = row.token_hash.clone();
        let scopes = self.scopes_for_token(&row.id).await?;
        let model = self.to_model(row, scopes)?;
        Ok(Some((model, hashed)))
    }

    async fn count_tokens(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM personal_access_tokens")
            .fetch_one(&self.pool)
            .await
            .map_err(|err| FlowplaneError::Database {
                source: err,
                context: "Failed to count personal access tokens".to_string(),
            })?;
        Ok(count)
    }

    async fn count_active_tokens(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM personal_access_tokens WHERE status = 'active'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|err| FlowplaneError::Database {
            source: err,
            context: "Failed to count active personal access tokens".to_string(),
        })?;
        Ok(count)
    }
}
