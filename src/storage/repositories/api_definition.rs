//! API Definition repository for managing OpenAPI definitions
//!
//! This module handles persistence for API definitions and their associated routes,
//! supporting both Platform API and Native API resource creation.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use uuid::Uuid;

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
    pub generated_listener_id: Option<String>,
    pub target_listeners: Option<String>,
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
    pub generated_listener_id: Option<String>,
    pub target_listeners: Option<Vec<String>>,
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
            generated_listener_id: row.generated_listener_id,
            target_listeners: row
                .target_listeners
                .and_then(|json| serde_json::from_str(&json).ok()),
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
    pub headers: Option<String>,
    pub rewrite_prefix: Option<String>,
    pub rewrite_regex: Option<String>,
    pub rewrite_substitution: Option<String>,
    pub upstream_targets: String,
    pub timeout_seconds: Option<i64>,
    pub override_config: Option<String>,
    pub deployment_note: Option<String>,
    pub route_order: i64,
    pub generated_route_id: Option<String>,
    pub generated_cluster_id: Option<String>,
    pub filter_config: Option<String>,
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
    pub headers: Option<serde_json::Value>,
    pub rewrite_prefix: Option<String>,
    pub rewrite_regex: Option<String>,
    pub rewrite_substitution: Option<String>,
    pub upstream_targets: serde_json::Value,
    pub timeout_seconds: Option<i64>,
    pub override_config: Option<serde_json::Value>,
    pub deployment_note: Option<String>,
    pub route_order: i64,
    pub generated_route_id: Option<String>,
    pub generated_cluster_id: Option<String>,
    pub filter_config: Option<serde_json::Value>,
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
            headers: row.headers.and_then(|json| serde_json::from_str(&json).ok()),
            rewrite_prefix: row.rewrite_prefix,
            rewrite_regex: row.rewrite_regex,
            rewrite_substitution: row.rewrite_substitution,
            upstream_targets: serde_json::from_str(&row.upstream_targets)
                .unwrap_or(serde_json::Value::Null),
            timeout_seconds: row.timeout_seconds,
            override_config: row.override_config.and_then(|json| serde_json::from_str(&json).ok()),
            deployment_note: row.deployment_note,
            route_order: row.route_order,
            generated_route_id: row.generated_route_id,
            generated_cluster_id: row.generated_cluster_id,
            filter_config: row.filter_config.and_then(|json| serde_json::from_str(&json).ok()),
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
    pub target_listeners: Option<Vec<String>>,
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
    pub headers: Option<serde_json::Value>,
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

/// Request for updating an API definition
#[derive(Debug, Clone)]
pub struct UpdateApiDefinitionRequest {
    pub domain: Option<String>,
    pub tls_config: Option<serde_json::Value>,
    pub target_listeners: Option<Vec<String>>,
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
        let target_listeners = Self::serialize_optional(
            &request.target_listeners.as_ref().map(|v| serde_json::to_value(v).unwrap()),
        )?;
        let listener_isolation: i64 = if request.listener_isolation { 1 } else { 0 };

        let now = chrono::Utc::now();

        sqlx::query::<Sqlite>(
            "INSERT INTO api_definitions (
                id, team, domain, listener_isolation, target_listeners, tls_config, metadata, bootstrap_uri,
                bootstrap_revision, version, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(&id)
        .bind(&request.team)
        .bind(&request.domain)
        .bind(listener_isolation)
        .bind(target_listeners)
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

    /// Delete an API route by ID
    pub async fn delete_route(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM api_routes WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| FlowplaneError::Database {
                source: e,
                context: format!("Failed to delete API route '{}'", id),
            })?;
        Ok(())
    }

    /// Fetch an API definition by identifier
    pub async fn get_definition(&self, id: &str) -> Result<ApiDefinitionData> {
        let row = sqlx::query_as::<Sqlite, ApiDefinitionRow>(
            "SELECT id, team, domain, listener_isolation, tls_config, metadata, bootstrap_uri,
                    bootstrap_revision, generated_listener_id, target_listeners, version, created_at, updated_at
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
                    bootstrap_revision, generated_listener_id, target_listeners, version, created_at, updated_at
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
                    bootstrap_revision, generated_listener_id, target_listeners, version, created_at, updated_at
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
        let headers_json = Self::serialize_optional(&request.headers)?;
        let case_sensitive = if request.case_sensitive { 1 } else { 0 };

        let now = chrono::Utc::now();

        sqlx::query::<Sqlite>(
            "INSERT INTO api_routes (
                id,
                api_definition_id,
                match_type,
                match_value,
                case_sensitive,
                headers,
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
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16
            )",
        )
        .bind(&id)
        .bind(&request.api_definition_id)
        .bind(&request.match_type)
        .bind(&request.match_value)
        .bind(case_sensitive)
        .bind(headers_json)
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
            "SELECT id, api_definition_id, match_type, match_value, case_sensitive, headers, rewrite_prefix,
                    rewrite_regex, rewrite_substitution, upstream_targets, timeout_seconds,
                    override_config, deployment_note, route_order, generated_route_id, generated_cluster_id, filter_config, created_at, updated_at
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
            "SELECT id, api_definition_id, match_type, match_value, case_sensitive, headers, rewrite_prefix,
                    rewrite_regex, rewrite_substitution, upstream_targets, timeout_seconds,
                    override_config, deployment_note, route_order, generated_route_id, generated_cluster_id, filter_config, created_at, updated_at
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

    /// Update generated listener ID for an API definition
    pub async fn update_generated_listener_id(
        &self,
        definition_id: &str,
        listener_id: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE api_definitions
             SET generated_listener_id = $2,
                 updated_at = $3
             WHERE id = $1",
        )
        .bind(definition_id)
        .bind(listener_id)
        .bind(chrono::Utc::now())
        .execute(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!(
                "Failed to update generated_listener_id for definition '{}'",
                definition_id
            ),
        })?;

        Ok(())
    }

    /// Update generated route and cluster IDs for an API route
    pub async fn update_generated_resource_ids(
        &self,
        route_id: &str,
        generated_route_id: Option<&str>,
        generated_cluster_id: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE api_routes
             SET generated_route_id = $2,
                 generated_cluster_id = $3,
                 updated_at = $4
             WHERE id = $1",
        )
        .bind(route_id)
        .bind(generated_route_id)
        .bind(generated_cluster_id)
        .bind(chrono::Utc::now())
        .execute(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to update generated resource IDs for route '{}'", route_id),
        })?;

        Ok(())
    }

    /// Update an API definition's mutable fields
    pub async fn update_definition(
        &self,
        definition_id: &str,
        request: UpdateApiDefinitionRequest,
    ) -> Result<ApiDefinitionData> {
        // Get current definition to merge with updates
        let current = self.get_definition(definition_id).await?;

        let now = chrono::Utc::now();

        // Use current values if not provided in request
        let domain = request.domain.as_ref().unwrap_or(&current.domain);

        let tls_json = if let Some(tls) = &request.tls_config {
            Some(serde_json::to_string(tls).map_err(|e| {
                FlowplaneError::validation(format!("Invalid TLS configuration JSON: {}", e))
            })?)
        } else {
            current.tls_config.as_ref().map(|v| serde_json::to_string(v).unwrap())
        };

        let target_listeners_json = if let Some(listeners) = &request.target_listeners {
            Some(serde_json::to_string(listeners).map_err(|e| {
                FlowplaneError::validation(format!("Invalid target_listeners JSON: {}", e))
            })?)
        } else {
            current.target_listeners.as_ref().map(|v| serde_json::to_string(v).unwrap())
        };

        sqlx::query(
            "UPDATE api_definitions
             SET domain = $2,
                 tls_config = $3,
                 target_listeners = $4,
                 version = version + 1,
                 updated_at = $5
             WHERE id = $1",
        )
        .bind(definition_id)
        .bind(domain)
        .bind(tls_json.as_deref())
        .bind(target_listeners_json.as_deref())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: format!("Failed to update API definition '{}'", definition_id),
        })?;

        self.get_definition(definition_id).await
    }

    pub async fn list_definitions(
        &self,
        team: Option<String>,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<ApiDefinitionData>> {
        let base_query =
            "SELECT id, team, domain, listener_isolation, tls_config, metadata, bootstrap_uri,
                    bootstrap_revision, generated_listener_id, target_listeners, version, created_at, updated_at
             FROM api_definitions";

        let mut query_builder = sqlx::QueryBuilder::<Sqlite>::new(base_query);

        if let Some(ref t) = team {
            query_builder.push(" WHERE team = ");
            query_builder.push_bind(t);
        }

        query_builder.push(" ORDER BY created_at DESC");

        if let Some(lim) = limit {
            query_builder.push(" LIMIT ");
            query_builder.push_bind(lim);
        }

        if let Some(off) = offset {
            query_builder.push(" OFFSET ");
            query_builder.push_bind(off);
        }

        let rows = query_builder
            .build_query_as::<ApiDefinitionRow>()
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
            "SELECT id, api_definition_id, match_type, match_value, case_sensitive, headers, rewrite_prefix,
                    rewrite_regex, rewrite_substitution, upstream_targets, timeout_seconds,
                    override_config, deployment_note, route_order, generated_route_id, generated_cluster_id, filter_config, created_at, updated_at
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
