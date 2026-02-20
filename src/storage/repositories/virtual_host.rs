//! Virtual Host Repository
//!
//! This module provides CRUD operations for virtual hosts extracted from route configurations.
//! Virtual hosts are synchronized when route configs are created/updated.

use crate::domain::{RouteConfigId, VirtualHostId};
use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::instrument;

/// Internal database row structure for virtual_hosts.
#[derive(Debug, Clone, FromRow)]
struct VirtualHostRow {
    pub id: String,
    pub route_config_id: String,
    pub name: String,
    pub domains: String, // JSON array
    pub rule_order: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Virtual host data returned from the repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualHostData {
    pub id: VirtualHostId,
    pub route_config_id: RouteConfigId,
    pub name: String,
    pub domains: Vec<String>,
    pub rule_order: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TryFrom<VirtualHostRow> for VirtualHostData {
    type Error = FlowplaneError;

    fn try_from(row: VirtualHostRow) -> Result<Self> {
        let domains: Vec<String> = serde_json::from_str(&row.domains).map_err(|e| {
            FlowplaneError::internal(format!("Failed to parse virtual host domains: {}", e))
        })?;

        Ok(Self {
            id: VirtualHostId::from_string(row.id),
            route_config_id: RouteConfigId::from_string(row.route_config_id),
            name: row.name,
            domains,
            rule_order: row.rule_order,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// Request to create a new virtual host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVirtualHostRequest {
    pub route_config_id: RouteConfigId,
    pub name: String,
    pub domains: Vec<String>,
    pub rule_order: i32,
}

/// Request to update a virtual host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateVirtualHostRequest {
    pub domains: Option<Vec<String>>,
    pub rule_order: Option<i32>,
}

/// Repository for virtual host operations.
#[derive(Debug, Clone)]
pub struct VirtualHostRepository {
    pool: DbPool,
}

impl VirtualHostRepository {
    /// Creates a new repository with the given database pool.
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Create a new virtual host.
    #[instrument(skip(self, request), fields(route_config_id = %request.route_config_id, name = %request.name), name = "db_create_virtual_host")]
    pub async fn create(&self, request: CreateVirtualHostRequest) -> Result<VirtualHostData> {
        let id = VirtualHostId::new();
        let now = Utc::now();
        let domains_json = serde_json::to_string(&request.domains)
            .map_err(|e| FlowplaneError::internal(format!("Failed to serialize domains: {}", e)))?;

        sqlx::query(
            "INSERT INTO virtual_hosts (id, route_config_id, name, domains, rule_order, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)"
        )
        .bind(id.as_str())
        .bind(request.route_config_id.as_str())
        .bind(&request.name)
        .bind(&domains_json)
        .bind(request.rule_order)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_id = %request.route_config_id, name = %request.name, "Failed to create virtual host");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to create virtual host '{}' for route config '{}'", request.name, request.route_config_id),
            }
        })?;

        tracing::info!(
            id = %id,
            route_config_id = %request.route_config_id,
            name = %request.name,
            "Created virtual host"
        );

        Ok(VirtualHostData {
            id,
            route_config_id: request.route_config_id,
            name: request.name,
            domains: request.domains,
            rule_order: request.rule_order,
            created_at: now,
            updated_at: now,
        })
    }

    /// Get a virtual host by ID.
    #[instrument(skip(self), fields(id = %id), name = "db_get_virtual_host_by_id")]
    pub async fn get_by_id(&self, id: &VirtualHostId) -> Result<VirtualHostData> {
        let row = sqlx::query_as::<sqlx::Postgres, VirtualHostRow>(
            "SELECT id, route_config_id, name, domains, rule_order, created_at, updated_at \
             FROM virtual_hosts WHERE id = $1",
        )
        .bind(id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => FlowplaneError::not_found("VirtualHost", id.as_str()),
            _ => {
                tracing::error!(error = %e, id = %id, "Failed to get virtual host by ID");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to get virtual host by ID: {}", id),
                }
            }
        })?;

        VirtualHostData::try_from(row)
    }

    /// Get a virtual host by route config ID and name.
    #[instrument(skip(self), fields(route_config_id = %route_config_id, name = %name), name = "db_get_virtual_host_by_name")]
    pub async fn get_by_route_config_and_name(
        &self,
        route_config_id: &RouteConfigId,
        name: &str,
    ) -> Result<VirtualHostData> {
        let row = sqlx::query_as::<sqlx::Postgres, VirtualHostRow>(
            "SELECT id, route_config_id, name, domains, rule_order, created_at, updated_at \
             FROM virtual_hosts WHERE route_config_id = $1 AND name = $2"
        )
        .bind(route_config_id.as_str())
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                FlowplaneError::not_found("VirtualHost", format!("{}:{}", route_config_id, name))
            }
            _ => {
                tracing::error!(error = %e, route_config_id = %route_config_id, name = %name, "Failed to get virtual host by name");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to get virtual host '{}' for route config '{}'", name, route_config_id),
                }
            }
        })?;

        VirtualHostData::try_from(row)
    }

    /// List all virtual hosts for a route config.
    #[instrument(skip(self), fields(route_config_id = %route_config_id), name = "db_list_virtual_hosts_by_route_config")]
    pub async fn list_by_route_config(
        &self,
        route_config_id: &RouteConfigId,
    ) -> Result<Vec<VirtualHostData>> {
        let rows = sqlx::query_as::<sqlx::Postgres, VirtualHostRow>(
            "SELECT id, route_config_id, name, domains, rule_order, created_at, updated_at \
             FROM virtual_hosts WHERE route_config_id = $1 ORDER BY rule_order ASC",
        )
        .bind(route_config_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_id = %route_config_id, "Failed to list virtual hosts");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to list virtual hosts for route config '{}'", route_config_id),
            }
        })?;

        rows.into_iter().map(VirtualHostData::try_from).collect()
    }

    /// Update a virtual host.
    #[instrument(skip(self, request), fields(id = %id), name = "db_update_virtual_host")]
    pub async fn update(
        &self,
        id: &VirtualHostId,
        request: UpdateVirtualHostRequest,
    ) -> Result<VirtualHostData> {
        let current = self.get_by_id(id).await?;
        let now = Utc::now();

        let new_domains = request.domains.unwrap_or(current.domains);
        let new_rule_order = request.rule_order.unwrap_or(current.rule_order);

        let domains_json = serde_json::to_string(&new_domains)
            .map_err(|e| FlowplaneError::internal(format!("Failed to serialize domains: {}", e)))?;

        let result = sqlx::query(
            "UPDATE virtual_hosts SET domains = $1, rule_order = $2, updated_at = $3 WHERE id = $4",
        )
        .bind(&domains_json)
        .bind(new_rule_order)
        .bind(now)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, id = %id, "Failed to update virtual host");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to update virtual host '{}'", id),
            }
        })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found("VirtualHost", id.as_str()));
        }

        self.get_by_id(id).await
    }

    /// Delete a virtual host by ID.
    #[instrument(skip(self), fields(id = %id), name = "db_delete_virtual_host")]
    pub async fn delete(&self, id: &VirtualHostId) -> Result<()> {
        let result = sqlx::query("DELETE FROM virtual_hosts WHERE id = $1")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, id = %id, "Failed to delete virtual host");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete virtual host '{}'", id),
                }
            })?;

        if result.rows_affected() != 1 {
            return Err(FlowplaneError::not_found("VirtualHost", id.as_str()));
        }

        tracing::info!(id = %id, "Deleted virtual host");
        Ok(())
    }

    /// Delete all virtual hosts for a route config.
    /// Used during route config sync to clear old data before re-populating.
    #[instrument(skip(self), fields(route_config_id = %route_config_id), name = "db_delete_virtual_hosts_by_route_config")]
    pub async fn delete_by_route_config(&self, route_config_id: &RouteConfigId) -> Result<u64> {
        let result = sqlx::query("DELETE FROM virtual_hosts WHERE route_config_id = $1")
            .bind(route_config_id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, route_config_id = %route_config_id, "Failed to delete virtual hosts by route config");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to delete virtual hosts for route config '{}'", route_config_id),
                }
            })?;

        let deleted = result.rows_affected();
        if deleted > 0 {
            tracing::info!(route_config_id = %route_config_id, deleted = deleted, "Deleted virtual hosts");
        }

        Ok(deleted)
    }

    /// Check if a virtual host exists by route config and name.
    #[instrument(skip(self), fields(route_config_id = %route_config_id, name = %name), name = "db_exists_virtual_host")]
    pub async fn exists(&self, route_config_id: &RouteConfigId, name: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM virtual_hosts WHERE route_config_id = $1 AND name = $2"
        )
        .bind(route_config_id.as_str())
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, route_config_id = %route_config_id, name = %name, "Failed to check virtual host existence");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to check virtual host existence for route config '{}'", route_config_id),
            }
        })?;

        Ok(count > 0)
    }

    /// Count all virtual hosts for a team (joins through route_config).
    #[instrument(skip(self), fields(team = %team), name = "db_count_virtual_hosts_by_team")]
    pub async fn count_by_team(&self, team: &str) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) \
             FROM virtual_hosts vh \
             INNER JOIN route_configs rc ON vh.route_config_id = rc.id \
             WHERE rc.team = $1",
        )
        .bind(team)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, team = %team, "Failed to count virtual hosts by team");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count virtual hosts for team '{}'", team),
            }
        })?;

        Ok(count)
    }

    /// Count all virtual hosts for multiple teams (supports admin bypass).
    ///
    /// If `teams` is empty, counts ALL virtual hosts across all teams (admin bypass).
    #[instrument(skip(self), fields(teams = ?teams.len()), name = "db_count_virtual_hosts_by_teams")]
    pub async fn count_by_teams(&self, teams: &[String]) -> Result<i64> {
        // Single team: use existing optimized method
        if teams.len() == 1 {
            return self.count_by_team(&teams[0]).await;
        }

        // Admin bypass: empty teams = count all virtual hosts
        if teams.is_empty() {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM virtual_hosts")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to count all virtual hosts (admin bypass)");
                    FlowplaneError::Database {
                        source: e,
                        context: "Failed to count all virtual hosts".to_string(),
                    }
                })?;
            return Ok(count);
        }

        // Build IN clause for team filtering
        let placeholders: Vec<String> = (1..=teams.len()).map(|i| format!("${}", i)).collect();
        let query_str = format!(
            "SELECT COUNT(*) \
             FROM virtual_hosts vh \
             INNER JOIN route_configs rc ON vh.route_config_id = rc.id \
             WHERE rc.team IN ({})",
            placeholders.join(", ")
        );

        let mut query = sqlx::query_scalar::<sqlx::Postgres, i64>(&query_str);
        for team in teams {
            query = query.bind(team);
        }

        let count: i64 = query.fetch_one(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, teams = ?teams, "Failed to count virtual hosts by teams");
            FlowplaneError::Database {
                source: e,
                context: format!("Failed to count virtual hosts for teams {:?}", teams),
            }
        })?;

        Ok(count)
    }
}
