//! Reporting repository for platform visibility queries
//!
//! This module provides specialized queries for reporting and monitoring purposes,
//! joining data across multiple resource tables to provide comprehensive visibility.

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite};
use tracing::instrument;

/// Route flow data combining information from routes, clusters, and listeners
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RouteFlowRow {
    // Route information
    pub route_name: String,
    pub path_prefix: String,
    pub route_team: Option<String>,

    // Cluster information
    pub cluster_name: String,
    pub cluster_configuration: String, // JSON with endpoint information

    // Listener information
    pub listener_name: String,
    pub listener_address: String,
    pub listener_port: Option<i64>,
}

/// Repository for reporting queries
#[derive(Debug, Clone)]
pub struct ReportingRepository {
    pool: DbPool,
}

impl ReportingRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// List route flows showing how requests flow through the system
    ///
    /// Returns comprehensive data showing: listener → route → cluster → endpoints
    ///
    /// # Arguments
    /// * `teams` - List of team names to filter by (empty = all routes for admin)
    /// * `limit` - Maximum number of results
    /// * `offset` - Number of results to skip
    ///
    /// # Returns
    /// Tuple of (route_flows, total_count)
    #[instrument(skip(self), fields(teams = ?teams, limit = limit, offset = offset), name = "db_list_route_flows")]
    pub async fn list_route_flows(
        &self,
        teams: &[String],
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<RouteFlowRow>, i64)> {
        // First, get the total count for pagination
        let total_count = self.count_route_flows(teams).await?;

        // Build the query based on whether we're filtering by teams
        let (query_str, rows) = if teams.is_empty() {
            // Admin query - get all routes
            let query_str = r#"
                SELECT
                    r.name as route_name,
                    r.path_prefix,
                    r.team as route_team,
                    c.name as cluster_name,
                    c.configuration as cluster_configuration,
                    l.name as listener_name,
                    l.address as listener_address,
                    l.port as listener_port
                FROM routes r
                INNER JOIN clusters c ON r.cluster_name = c.name
                CROSS JOIN listeners l
                WHERE l.name LIKE 'default%'
                ORDER BY r.created_at DESC
                LIMIT $1 OFFSET $2
            "#
            .to_string();

            let rows = sqlx::query_as::<Sqlite, RouteFlowRow>(&query_str)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to list route flows");
                    FlowplaneError::Database {
                        source: e,
                        context: "Failed to list route flows".to_string(),
                    }
                })?;

            (query_str, rows)
        } else {
            // Team-scoped query - filter by teams
            let placeholders = teams
                .iter()
                .enumerate()
                .map(|(i, _)| format!("${}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");

            let query_str = format!(
                r#"
                SELECT
                    r.name as route_name,
                    r.path_prefix,
                    r.team as route_team,
                    c.name as cluster_name,
                    c.configuration as cluster_configuration,
                    l.name as listener_name,
                    l.address as listener_address,
                    l.port as listener_port
                FROM routes r
                INNER JOIN clusters c ON r.cluster_name = c.name
                CROSS JOIN listeners l
                WHERE (r.team IN ({}) OR r.team IS NULL)
                AND l.name LIKE 'default%'
                ORDER BY r.created_at DESC
                LIMIT ${} OFFSET ${}
                "#,
                placeholders,
                teams.len() + 1,
                teams.len() + 2
            );

            let mut query = sqlx::query_as::<Sqlite, RouteFlowRow>(&query_str);

            // Bind team names
            for team in teams {
                query = query.bind(team);
            }

            // Bind limit and offset
            query = query.bind(limit).bind(offset);

            let rows = query.fetch_all(&self.pool).await.map_err(|e| {
                tracing::error!(error = %e, teams = ?teams, "Failed to list route flows by teams");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to list route flows for teams: {:?}", teams),
                }
            })?;

            (query_str, rows)
        };

        tracing::debug!(
            query = %query_str,
            teams = ?teams,
            row_count = rows.len(),
            total = total_count,
            "Retrieved route flows"
        );

        Ok((rows, total_count))
    }

    /// Count total route flows (for pagination)
    #[instrument(skip(self), fields(teams = ?teams), name = "db_count_route_flows")]
    async fn count_route_flows(&self, teams: &[String]) -> Result<i64> {
        let count = if teams.is_empty() {
            // Admin query - count all routes
            sqlx::query_scalar::<Sqlite, i64>(
                r#"
                SELECT COUNT(DISTINCT r.name)
                FROM routes r
                INNER JOIN clusters c ON r.cluster_name = c.name
                "#,
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to count route flows");
                FlowplaneError::Database {
                    source: e,
                    context: "Failed to count route flows".to_string(),
                }
            })?
        } else {
            // Team-scoped query - count routes for specific teams
            let placeholders = teams
                .iter()
                .enumerate()
                .map(|(i, _)| format!("${}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");

            let query_str = format!(
                r#"
                SELECT COUNT(DISTINCT r.name)
                FROM routes r
                INNER JOIN clusters c ON r.cluster_name = c.name
                WHERE r.team IN ({}) OR r.team IS NULL
                "#,
                placeholders
            );

            let mut query = sqlx::query_scalar::<Sqlite, i64>(&query_str);

            for team in teams {
                query = query.bind(team);
            }

            query.fetch_one(&self.pool).await.map_err(|e| {
                tracing::error!(error = %e, teams = ?teams, "Failed to count route flows by teams");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to count route flows for teams: {:?}", teams),
                }
            })?
        };

        Ok(count)
    }

    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
}
