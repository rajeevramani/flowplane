//! Reporting repository for platform visibility queries
//!
//! This module provides specialized queries for reporting and monitoring purposes,
//! joining data across multiple resource tables to provide comprehensive visibility.
//!
//! Includes:
//! - `list_route_flows()` — basic route flow listing (existing)
//! - `trace_request_path()` — multi-table JOIN tracing a request through the full path
//! - `full_topology()` — complete gateway topology with orphan detection
//! - `service_summary()` — aggregate view for a single cluster/service

use crate::errors::{FlowplaneError, Result};
use crate::storage::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tracing::instrument;

// ============================================================================
// Row types
// ============================================================================

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

/// A single match in a request trace — one path through the gateway.
///
/// Represents: listener → route_config → virtual_host → route → cluster.
/// Endpoints are fetched separately via `TraceEndpointRow`.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RequestTraceRow {
    pub listener_name: String,
    pub listener_address: String,
    pub listener_port: Option<i64>,
    pub route_config_name: String,
    pub route_config_path_prefix: String,
    pub cluster_name: String,
    pub virtual_host_name: String,
    pub virtual_host_domains: String,
    pub route_name: String,
    pub route_path_pattern: String,
    pub route_match_type: String,
}

/// Endpoint data associated with a traced cluster.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct TraceEndpointRow {
    pub cluster_name: String,
    pub address: String,
    pub port: i32,
    pub health_status: String,
}

/// Combined result from `trace_request_path()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestTraceResult {
    pub matches: Vec<RequestTraceRow>,
    pub endpoints: Vec<TraceEndpointRow>,
}

/// Flattened topology row from the listener → route_config → virtual_host → route chain.
///
/// Uses LEFT JOINs so listeners without route_configs still appear (with None fields).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct TopologyRow {
    pub listener_name: String,
    pub listener_address: String,
    pub listener_port: Option<i64>,
    pub route_config_name: Option<String>,
    pub route_config_path_prefix: Option<String>,
    pub route_config_cluster_name: Option<String>,
    pub virtual_host_name: Option<String>,
    pub route_name: Option<String>,
    pub route_path_pattern: Option<String>,
    pub route_match_type: Option<String>,
}

/// Cluster with no route_configs referencing it.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct OrphanClusterRow {
    pub name: String,
    pub service_name: String,
}

/// Route config with no listener bound to it.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct OrphanRouteConfigRow {
    pub name: String,
    pub path_prefix: String,
    pub cluster_name: String,
}

/// Summary counts for the topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologySummary {
    pub listener_count: i64,
    pub route_config_count: i64,
    pub cluster_count: i64,
    pub route_count: i64,
    pub orphan_cluster_count: i64,
    pub orphan_route_config_count: i64,
}

/// Combined result from `full_topology()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyResult {
    pub rows: Vec<TopologyRow>,
    pub orphan_clusters: Vec<OrphanClusterRow>,
    pub orphan_route_configs: Vec<OrphanRouteConfigRow>,
    pub summary: TopologySummary,
    pub truncated: bool,
}

/// Cluster-level aggregate for `service_summary()`.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ServiceClusterInfo {
    pub cluster_name: String,
    pub service_name: String,
    pub endpoints_total: i64,
    pub endpoints_healthy: i64,
}

/// Route config referencing a cluster (for service summary).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ServiceRouteConfigRow {
    pub name: String,
    pub path_prefix: String,
}

/// Listener bound to a cluster's route configs (for service summary).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ServiceListenerRow {
    pub name: String,
    pub port: Option<i64>,
}

/// Combined result from `service_summary()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSummaryResult {
    pub cluster: Option<ServiceClusterInfo>,
    pub route_configs: Vec<ServiceRouteConfigRow>,
    pub listeners: Vec<ServiceListenerRow>,
}

/// Topology scope filter for `full_topology()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyScope {
    Full,
    Listener,
    Cluster,
    RouteConfig,
}

impl TopologyScope {
    pub fn from_str_opt(s: Option<&str>) -> Self {
        match s {
            Some("listener") => Self::Listener,
            Some("cluster") => Self::Cluster,
            Some("route_config") => Self::RouteConfig,
            _ => Self::Full,
        }
    }
}

// ============================================================================
// Repository
// ============================================================================

/// Repository for reporting queries
#[derive(Debug, Clone)]
pub struct ReportingRepository {
    pool: DbPool,
}

impl ReportingRepository {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    // ========================================================================
    // trace_request_path
    // ========================================================================

    /// Trace a request path through the full gateway chain.
    ///
    /// Joins: listeners → listener_route_configs → route_configs → virtual_hosts
    ///        → routes → clusters (via route_configs.cluster_name) → cluster_endpoints
    ///
    /// # Arguments
    /// * `team` - Team ID (mandatory, compile-time isolation)
    /// * `path` - Request path to trace (e.g., "/api/users")
    /// * `port` - Optional listener port filter
    ///
    /// # Returns
    /// `RequestTraceResult` with matching trace rows and associated endpoints.
    #[instrument(skip(self), fields(team = %team, path = %path, port = ?port), name = "db_trace_request_path")]
    pub async fn trace_request_path(
        &self,
        team: &str,
        path: &str,
        port: Option<i64>,
    ) -> Result<RequestTraceResult> {
        let matches = self.trace_path_matches(team, path, port).await?;

        // Collect unique cluster names to fetch endpoints
        let cluster_names: Vec<String> = matches
            .iter()
            .map(|m| m.cluster_name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        let endpoints = if cluster_names.is_empty() {
            Vec::new()
        } else {
            self.trace_endpoints(team, &cluster_names).await?
        };

        Ok(RequestTraceResult { matches, endpoints })
    }

    /// Internal: query trace path matches.
    async fn trace_path_matches(
        &self,
        team: &str,
        path: &str,
        port: Option<i64>,
    ) -> Result<Vec<RequestTraceRow>> {
        let query = r#"
            SELECT
                l.name  AS listener_name,
                l.address AS listener_address,
                l.port  AS listener_port,
                rc.name AS route_config_name,
                rc.path_prefix AS route_config_path_prefix,
                rc.cluster_name,
                vh.name AS virtual_host_name,
                vh.domains AS virtual_host_domains,
                r.name  AS route_name,
                r.path_pattern AS route_path_pattern,
                r.match_type AS route_match_type
            FROM listeners l
            INNER JOIN listener_route_configs lrc ON l.id = lrc.listener_id
            INNER JOIN route_configs rc ON lrc.route_config_id = rc.id
            INNER JOIN virtual_hosts vh ON vh.route_config_id = rc.id
            INNER JOIN routes r ON r.virtual_host_id = vh.id
            WHERE (l.team = $1 OR l.team IS NULL)
              AND (rc.team = $1 OR rc.team IS NULL)
              AND ($2::BIGINT IS NULL OR l.port = $2)
              AND (
                  (r.match_type = 'prefix' AND LEFT($3, LENGTH(r.path_pattern)) = r.path_pattern)
                  OR (r.match_type = 'exact' AND $3 = r.path_pattern)
                  OR (r.match_type = 'regex' AND $3 ~ r.path_pattern)
              )
            ORDER BY l.name, lrc.route_order, vh.rule_order, r.rule_order
        "#;

        let rows = sqlx::query_as::<sqlx::Postgres, RequestTraceRow>(query)
            .bind(team)
            .bind(port)
            .bind(path)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to trace request path");
                FlowplaneError::Database {
                    source: e,
                    context: "Failed to trace request path".to_string(),
                }
            })?;

        Ok(rows)
    }

    /// Internal: fetch endpoints for the given cluster names (team-scoped).
    async fn trace_endpoints(
        &self,
        team: &str,
        cluster_names: &[String],
    ) -> Result<Vec<TraceEndpointRow>> {
        if cluster_names.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders = cluster_names
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");

        let query = format!(
            r#"
            SELECT
                c.name AS cluster_name,
                ce.address,
                ce.port,
                ce.health_status
            FROM clusters c
            INNER JOIN cluster_endpoints ce ON ce.cluster_id = c.id
            WHERE (c.team = $1 OR c.team IS NULL)
              AND c.name IN ({})
            ORDER BY c.name, ce.address, ce.port
            "#,
            placeholders
        );

        let mut q = sqlx::query_as::<sqlx::Postgres, TraceEndpointRow>(&query).bind(team);

        for name in cluster_names {
            q = q.bind(name);
        }

        let rows = q.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, "Failed to fetch trace endpoints");
            FlowplaneError::Database {
                source: e,
                context: "Failed to fetch trace endpoints".to_string(),
            }
        })?;

        Ok(rows)
    }

    // ========================================================================
    // full_topology
    // ========================================================================

    /// Build the full gateway topology for a team.
    ///
    /// Built from scratch using `listener_route_configs` junction table.
    /// Supports scoped queries (full, listener, cluster, route_config) and orphan detection.
    ///
    /// # Arguments
    /// * `team` - Team ID (mandatory)
    /// * `scope` - Topology scope filter
    /// * `name` - Optional resource name filter (used with non-full scopes)
    /// * `limit` - Max rows per level (default 50)
    #[instrument(skip(self), fields(team = %team, scope = ?scope, name = ?name, limit = ?limit), name = "db_full_topology")]
    pub async fn full_topology(
        &self,
        team: &str,
        scope: Option<&str>,
        name: Option<&str>,
        limit: Option<i64>,
    ) -> Result<TopologyResult> {
        let limit = limit.unwrap_or(50);
        let scope = TopologyScope::from_str_opt(scope);

        let rows = self.topology_rows(team, &scope, name, limit).await?;
        let orphan_clusters = self.orphan_clusters(team).await?;
        let orphan_route_configs = self.orphan_route_configs(team).await?;
        let summary = self.topology_summary(team).await?;

        let truncated = rows.len() as i64 >= limit;

        Ok(TopologyResult { rows, orphan_clusters, orphan_route_configs, summary, truncated })
    }

    /// Internal: fetch topology rows with optional scope filtering.
    async fn topology_rows(
        &self,
        team: &str,
        scope: &TopologyScope,
        name: Option<&str>,
        limit: i64,
    ) -> Result<Vec<TopologyRow>> {
        let (where_clause, needs_name_bind) = match scope {
            TopologyScope::Full => (String::new(), false),
            TopologyScope::Listener => {
                if name.is_some() {
                    (" AND l.name = $3".to_string(), true)
                } else {
                    (String::new(), false)
                }
            }
            TopologyScope::RouteConfig => {
                if name.is_some() {
                    (" AND rc.name = $3".to_string(), true)
                } else {
                    (String::new(), false)
                }
            }
            TopologyScope::Cluster => {
                if name.is_some() {
                    (" AND rc.cluster_name = $3".to_string(), true)
                } else {
                    (String::new(), false)
                }
            }
        };

        let query = format!(
            r#"
            SELECT
                l.name    AS listener_name,
                l.address AS listener_address,
                l.port    AS listener_port,
                rc.name   AS route_config_name,
                rc.path_prefix AS route_config_path_prefix,
                rc.cluster_name AS route_config_cluster_name,
                vh.name   AS virtual_host_name,
                r.name    AS route_name,
                r.path_pattern AS route_path_pattern,
                r.match_type   AS route_match_type
            FROM listeners l
            LEFT JOIN listener_route_configs lrc ON l.id = lrc.listener_id
            LEFT JOIN route_configs rc ON lrc.route_config_id = rc.id
                AND (rc.team = $1 OR rc.team IS NULL)
            LEFT JOIN virtual_hosts vh ON vh.route_config_id = rc.id
            LEFT JOIN routes r ON r.virtual_host_id = vh.id
            WHERE (l.team = $1 OR l.team IS NULL)
            {}
            ORDER BY l.name, lrc.route_order, vh.rule_order, r.rule_order
            LIMIT $2
            "#,
            where_clause
        );

        let mut q = sqlx::query_as::<sqlx::Postgres, TopologyRow>(&query).bind(team).bind(limit);

        if needs_name_bind {
            if let Some(n) = name {
                q = q.bind(n);
            }
        }

        let rows = q.fetch_all(&self.pool).await.map_err(|e| {
            tracing::error!(error = %e, "Failed to fetch topology rows");
            FlowplaneError::Database {
                source: e,
                context: "Failed to fetch topology rows".to_string(),
            }
        })?;

        Ok(rows)
    }

    /// Internal: find clusters with no route_configs referencing them.
    async fn orphan_clusters(&self, team: &str) -> Result<Vec<OrphanClusterRow>> {
        let query = r#"
            SELECT c.name, c.service_name
            FROM clusters c
            WHERE (c.team = $1 OR c.team IS NULL)
              AND NOT EXISTS (
                  SELECT 1 FROM route_configs rc
                  WHERE rc.cluster_name = c.name
              )
            ORDER BY c.name
        "#;

        let rows = sqlx::query_as::<sqlx::Postgres, OrphanClusterRow>(query)
            .bind(team)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to find orphan clusters");
                FlowplaneError::Database {
                    source: e,
                    context: "Failed to find orphan clusters".to_string(),
                }
            })?;

        Ok(rows)
    }

    /// Internal: find route_configs with no listener bound to them.
    async fn orphan_route_configs(&self, team: &str) -> Result<Vec<OrphanRouteConfigRow>> {
        let query = r#"
            SELECT rc.name, rc.path_prefix, rc.cluster_name
            FROM route_configs rc
            WHERE (rc.team = $1 OR rc.team IS NULL)
              AND NOT EXISTS (
                  SELECT 1 FROM listener_route_configs lrc
                  WHERE lrc.route_config_id = rc.id
              )
            ORDER BY rc.name
        "#;

        let rows = sqlx::query_as::<sqlx::Postgres, OrphanRouteConfigRow>(query)
            .bind(team)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to find orphan route configs");
                FlowplaneError::Database {
                    source: e,
                    context: "Failed to find orphan route configs".to_string(),
                }
            })?;

        Ok(rows)
    }

    /// Internal: compute topology summary counts.
    async fn topology_summary(&self, team: &str) -> Result<TopologySummary> {
        let query = r#"
            SELECT
                (SELECT COUNT(*) FROM listeners WHERE team = $1 OR team IS NULL) AS listener_count,
                (SELECT COUNT(*) FROM route_configs WHERE team = $1 OR team IS NULL) AS route_config_count,
                (SELECT COUNT(*) FROM clusters WHERE team = $1 OR team IS NULL) AS cluster_count,
                (SELECT COUNT(*)
                 FROM routes r
                 INNER JOIN virtual_hosts vh ON r.virtual_host_id = vh.id
                 INNER JOIN route_configs rc ON vh.route_config_id = rc.id
                 WHERE rc.team = $1 OR rc.team IS NULL
                ) AS route_count
        "#;

        let row: (i64, i64, i64, i64) =
            sqlx::query_as::<sqlx::Postgres, (i64, i64, i64, i64)>(query)
                .bind(team)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to compute topology summary");
                    FlowplaneError::Database {
                        source: e,
                        context: "Failed to compute topology summary".to_string(),
                    }
                })?;

        // Compute orphan counts from the data we already have (or re-query).
        // For simplicity, we'll compute here.
        let orphan_cluster_count: i64 = sqlx::query_scalar::<sqlx::Postgres, i64>(
            r#"
            SELECT COUNT(*) FROM clusters c
            WHERE (c.team = $1 OR c.team IS NULL)
              AND NOT EXISTS (SELECT 1 FROM route_configs rc WHERE rc.cluster_name = c.name)
            "#,
        )
        .bind(team)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to count orphan clusters".to_string(),
        })?;

        let orphan_route_config_count: i64 = sqlx::query_scalar::<sqlx::Postgres, i64>(
            r#"
            SELECT COUNT(*) FROM route_configs rc
            WHERE (rc.team = $1 OR rc.team IS NULL)
              AND NOT EXISTS (SELECT 1 FROM listener_route_configs lrc WHERE lrc.route_config_id = rc.id)
            "#,
        )
        .bind(team)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FlowplaneError::Database {
            source: e,
            context: "Failed to count orphan route configs".to_string(),
        })?;

        Ok(TopologySummary {
            listener_count: row.0,
            route_config_count: row.1,
            cluster_count: row.2,
            route_count: row.3,
            orphan_cluster_count,
            orphan_route_config_count,
        })
    }

    // ========================================================================
    // service_summary
    // ========================================================================

    /// Aggregate service view for a single cluster.
    ///
    /// Returns: cluster info + endpoint counts + route_configs referencing it + listeners
    /// bound to those route_configs.
    ///
    /// # Arguments
    /// * `cluster_name` - Cluster name to summarize
    /// * `team` - Team ID (mandatory)
    #[instrument(skip(self), fields(cluster_name = %cluster_name, team = %team), name = "db_service_summary")]
    pub async fn service_summary(
        &self,
        cluster_name: &str,
        team: &str,
    ) -> Result<ServiceSummaryResult> {
        // 1. Cluster info with endpoint counts
        let cluster = self.service_cluster_info(cluster_name, team).await?;

        // 2. Route configs referencing this cluster
        let route_configs = self.service_route_configs(cluster_name, team).await?;

        // 3. Listeners bound to those route configs
        let listeners = self.service_listeners(cluster_name, team).await?;

        Ok(ServiceSummaryResult { cluster, route_configs, listeners })
    }

    /// Internal: cluster info with endpoint health counts.
    async fn service_cluster_info(
        &self,
        cluster_name: &str,
        team: &str,
    ) -> Result<Option<ServiceClusterInfo>> {
        let query = r#"
            SELECT
                c.name AS cluster_name,
                c.service_name,
                COALESCE((SELECT COUNT(*) FROM cluster_endpoints ce WHERE ce.cluster_id = c.id), 0) AS endpoints_total,
                COALESCE((SELECT COUNT(*) FROM cluster_endpoints ce WHERE ce.cluster_id = c.id AND ce.health_status = 'healthy'), 0) AS endpoints_healthy
            FROM clusters c
            WHERE c.name = $1
              AND (c.team = $2 OR c.team IS NULL)
        "#;

        let row = sqlx::query_as::<sqlx::Postgres, ServiceClusterInfo>(query)
            .bind(cluster_name)
            .bind(team)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to fetch service cluster info");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to fetch service info for cluster '{}'", cluster_name),
                }
            })?;

        Ok(row)
    }

    /// Internal: route configs that reference the given cluster.
    async fn service_route_configs(
        &self,
        cluster_name: &str,
        team: &str,
    ) -> Result<Vec<ServiceRouteConfigRow>> {
        let query = r#"
            SELECT rc.name, rc.path_prefix
            FROM route_configs rc
            WHERE rc.cluster_name = $1
              AND (rc.team = $2 OR rc.team IS NULL)
            ORDER BY rc.name
        "#;

        let rows = sqlx::query_as::<sqlx::Postgres, ServiceRouteConfigRow>(query)
            .bind(cluster_name)
            .bind(team)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to fetch service route configs");
                FlowplaneError::Database {
                    source: e,
                    context: format!(
                        "Failed to fetch route configs for cluster '{}'",
                        cluster_name
                    ),
                }
            })?;

        Ok(rows)
    }

    /// Internal: listeners bound to route configs that reference the given cluster.
    async fn service_listeners(
        &self,
        cluster_name: &str,
        team: &str,
    ) -> Result<Vec<ServiceListenerRow>> {
        let query = r#"
            SELECT DISTINCT l.name, l.port
            FROM listeners l
            INNER JOIN listener_route_configs lrc ON l.id = lrc.listener_id
            INNER JOIN route_configs rc ON lrc.route_config_id = rc.id
            WHERE rc.cluster_name = $1
              AND (l.team = $2 OR l.team IS NULL)
              AND (rc.team = $2 OR rc.team IS NULL)
            ORDER BY l.name
        "#;

        let rows = sqlx::query_as::<sqlx::Postgres, ServiceListenerRow>(query)
            .bind(cluster_name)
            .bind(team)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to fetch service listeners");
                FlowplaneError::Database {
                    source: e,
                    context: format!("Failed to fetch listeners for cluster '{}'", cluster_name),
                }
            })?;

        Ok(rows)
    }

    // ========================================================================
    // Existing: list_route_flows
    // ========================================================================

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
                FROM route_configs r
                INNER JOIN clusters c ON r.cluster_name = c.name
                CROSS JOIN listeners l
                WHERE l.name LIKE 'default%'
                ORDER BY r.created_at DESC
                LIMIT $1 OFFSET $2
            "#
            .to_string();

            let rows = sqlx::query_as::<sqlx::Postgres, RouteFlowRow>(&query_str)
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
                FROM route_configs r
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

            let mut query = sqlx::query_as::<sqlx::Postgres, RouteFlowRow>(&query_str);

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
            sqlx::query_scalar::<sqlx::Postgres, i64>(
                r#"
                SELECT COUNT(DISTINCT r.name)
                FROM route_configs r
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
                FROM route_configs r
                INNER JOIN clusters c ON r.cluster_name = c.name
                WHERE r.team IN ({}) OR r.team IS NULL
                "#,
                placeholders
            );

            let mut query = sqlx::query_scalar::<sqlx::Postgres, i64>(&query_str);

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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::{seed_reporting_data, TestDatabase, TEAM_A_ID, TEAM_B_ID};

    // Helper: build a ReportingRepository from a TestDatabase
    fn repo(db: &TestDatabase) -> ReportingRepository {
        ReportingRepository::new(db.pool.clone())
    }

    // Helper: create a TestDatabase with reporting seed data
    async fn test_db(name: &str) -> TestDatabase {
        let db = TestDatabase::new(name).await;
        seed_reporting_data(&db.pool).await;
        db
    }

    // ========================================================================
    // trace_request_path tests
    // ========================================================================

    #[tokio::test]
    async fn test_trace_request_path_happy_path() {
        let db = test_db("trace_happy").await;
        let r = repo(&db);

        let result = r
            .trace_request_path(TEAM_A_ID, "/api/orders", Some(8080))
            .await
            .expect("trace should succeed");

        assert!(!result.matches.is_empty(), "should find at least one match");

        let m = &result.matches[0];
        assert_eq!(m.listener_name, "http-8080");
        assert_eq!(m.route_config_name, "orders-routes");
        assert_eq!(m.cluster_name, "orders-svc");
        assert_eq!(m.route_path_pattern, "/api/orders");
        assert_eq!(m.route_match_type, "prefix");

        // Should have endpoints for orders-svc
        assert!(!result.endpoints.is_empty(), "should have endpoints");
        let ep_cluster_names: Vec<&str> =
            result.endpoints.iter().map(|e| e.cluster_name.as_str()).collect();
        assert!(ep_cluster_names.contains(&"orders-svc"));
    }

    #[tokio::test]
    async fn test_trace_request_path_prefix_subpath() {
        let db = test_db("trace_subpath").await;
        let r = repo(&db);

        // /api/orders/123 should match the /api/orders prefix route
        let result = r
            .trace_request_path(TEAM_A_ID, "/api/orders/123", Some(8080))
            .await
            .expect("trace should succeed");

        assert!(!result.matches.is_empty(), "prefix match should work for subpaths");
        assert_eq!(result.matches[0].route_path_pattern, "/api/orders");
    }

    #[tokio::test]
    async fn test_trace_request_path_no_match() {
        let db = test_db("trace_no_match").await;
        let r = repo(&db);

        let result = r
            .trace_request_path(TEAM_A_ID, "/nonexistent/path", Some(8080))
            .await
            .expect("trace should succeed even with no matches");

        assert!(result.matches.is_empty(), "should find no matches");
        assert!(result.endpoints.is_empty(), "should have no endpoints");
    }

    #[tokio::test]
    async fn test_trace_request_path_wrong_port() {
        let db = test_db("trace_wrong_port").await;
        let r = repo(&db);

        // team-a listener is on port 8080, querying port 9999
        let result = r
            .trace_request_path(TEAM_A_ID, "/api/orders", Some(9999))
            .await
            .expect("trace should succeed");

        assert!(result.matches.is_empty(), "wrong port should yield no matches");
    }

    #[tokio::test]
    async fn test_trace_request_path_no_port_filter() {
        let db = test_db("trace_no_port").await;
        let r = repo(&db);

        // No port filter — should still find matches
        let result = r
            .trace_request_path(TEAM_A_ID, "/api/orders", None)
            .await
            .expect("trace should succeed");

        assert!(!result.matches.is_empty(), "no port filter should return matches");
    }

    #[tokio::test]
    async fn test_trace_request_path_team_isolation() {
        let db = test_db("trace_isolation").await;
        let r = repo(&db);

        // team-a should NOT see team-b's /api/payments route
        let result = r
            .trace_request_path(TEAM_A_ID, "/api/payments", Some(9090))
            .await
            .expect("trace should succeed");

        assert!(result.matches.is_empty(), "team-a must not see team-b routes");

        // team-b should see its own /api/payments route
        let result_b = r
            .trace_request_path(TEAM_B_ID, "/api/payments", Some(9090))
            .await
            .expect("trace should succeed");

        assert!(!result_b.matches.is_empty(), "team-b should see its own routes");
    }

    #[tokio::test]
    async fn test_trace_request_path_empty_database() {
        let db = TestDatabase::new("trace_empty").await;
        let r = repo(&db);

        // Use a team ID that has no resources
        let result = r
            .trace_request_path("nonexistent-team-id", "/anything", None)
            .await
            .expect("trace should succeed on empty data");

        assert!(result.matches.is_empty());
        assert!(result.endpoints.is_empty());
    }

    // ========================================================================
    // full_topology tests
    // ========================================================================

    #[tokio::test]
    async fn test_full_topology_happy_path() {
        let db = test_db("topo_happy").await;
        let r = repo(&db);

        let result =
            r.full_topology(TEAM_A_ID, None, None, None).await.expect("topology should succeed");

        // team-a should have at least one listener row
        assert!(!result.rows.is_empty(), "should have topology rows");

        // Check summary counts are positive
        assert!(result.summary.listener_count > 0, "should have listeners");
        assert!(result.summary.route_config_count > 0, "should have route configs");
        assert!(result.summary.cluster_count > 0, "should have clusters");
    }

    #[tokio::test]
    async fn test_full_topology_orphan_detection() {
        let db = test_db("topo_orphans").await;
        let r = repo(&db);

        let result =
            r.full_topology(TEAM_A_ID, None, None, None).await.expect("topology should succeed");

        // Should detect the orphan cluster (no routes pointing to it)
        let orphan_names: Vec<&str> =
            result.orphan_clusters.iter().map(|c| c.name.as_str()).collect();
        assert!(
            orphan_names.contains(&"orphan-cluster"),
            "should detect orphan-cluster, found: {:?}",
            orphan_names
        );

        // Should detect the orphan route config (no listener bound)
        let orphan_rc_names: Vec<&str> =
            result.orphan_route_configs.iter().map(|rc| rc.name.as_str()).collect();
        assert!(
            orphan_rc_names.contains(&"orphan-rc"),
            "should detect orphan-rc, found: {:?}",
            orphan_rc_names
        );

        // Summary should reflect orphan counts
        assert!(result.summary.orphan_cluster_count >= 1, "orphan cluster count should be >= 1");
        assert!(
            result.summary.orphan_route_config_count >= 1,
            "orphan route config count should be >= 1"
        );
    }

    #[tokio::test]
    async fn test_full_topology_team_isolation() {
        let db = test_db("topo_isolation").await;
        let r = repo(&db);

        let result_a =
            r.full_topology(TEAM_A_ID, None, None, None).await.expect("topology should succeed");

        let result_b =
            r.full_topology(TEAM_B_ID, None, None, None).await.expect("topology should succeed");

        // team-a listeners should not include team-b's listener
        let a_listeners: Vec<&str> =
            result_a.rows.iter().map(|row| row.listener_name.as_str()).collect();
        assert!(!a_listeners.contains(&"http-9090"), "team-a should not see team-b listener");

        // team-b listeners should not include team-a's listener
        let b_listeners: Vec<&str> =
            result_b.rows.iter().map(|row| row.listener_name.as_str()).collect();
        assert!(!b_listeners.contains(&"http-8080"), "team-b should not see team-a listener");
    }

    #[tokio::test]
    async fn test_full_topology_scoped_listener() {
        let db = test_db("topo_scope_listener").await;
        let r = repo(&db);

        let result = r
            .full_topology(TEAM_A_ID, Some("listener"), Some("http-8080"), None)
            .await
            .expect("scoped topology should succeed");

        // All rows should be for the specified listener
        for row in &result.rows {
            assert_eq!(row.listener_name, "http-8080");
        }
    }

    #[tokio::test]
    async fn test_full_topology_limit() {
        let db = test_db("topo_limit").await;
        let r = repo(&db);

        let result = r
            .full_topology(TEAM_A_ID, None, None, Some(1))
            .await
            .expect("topology with limit should succeed");

        assert!(result.rows.len() <= 1, "should respect the limit");
        assert!(result.truncated, "should be marked as truncated");
    }

    #[tokio::test]
    async fn test_full_topology_empty_team() {
        let db = TestDatabase::new("topo_empty_team").await;
        let r = repo(&db);

        let result = r
            .full_topology("nonexistent-team-id", None, None, None)
            .await
            .expect("topology should succeed for empty team");

        // Might have rows from NULL-team resources (global), but no team-specific ones
        // The key assertion is that it doesn't error
        assert!(result.summary.listener_count >= 0);
    }

    // ========================================================================
    // service_summary tests
    // ========================================================================

    #[tokio::test]
    async fn test_service_summary_happy_path() {
        let db = test_db("svc_happy").await;
        let r = repo(&db);

        let result = r
            .service_summary("orders-svc", TEAM_A_ID)
            .await
            .expect("service summary should succeed");

        // Should find the cluster
        assert!(result.cluster.is_some(), "should find orders-svc cluster");

        let cluster = result.cluster.as_ref().expect("cluster exists");
        assert_eq!(cluster.cluster_name, "orders-svc");
        assert_eq!(cluster.endpoints_total, 2, "orders-svc should have 2 endpoints");
        assert_eq!(cluster.endpoints_healthy, 1, "orders-svc should have 1 healthy endpoint");

        // Should find route configs referencing this cluster
        assert!(!result.route_configs.is_empty(), "should have route configs");
        assert_eq!(result.route_configs[0].name, "orders-routes");

        // Should find listeners bound to those route configs
        assert!(!result.listeners.is_empty(), "should have listeners");
        assert_eq!(result.listeners[0].name, "http-8080");
    }

    #[tokio::test]
    async fn test_service_summary_not_found() {
        let db = test_db("svc_not_found").await;
        let r = repo(&db);

        let result = r
            .service_summary("nonexistent-cluster", TEAM_A_ID)
            .await
            .expect("service summary should succeed even for missing cluster");

        assert!(result.cluster.is_none(), "should not find the cluster");
        assert!(result.route_configs.is_empty());
        assert!(result.listeners.is_empty());
    }

    #[tokio::test]
    async fn test_service_summary_team_isolation() {
        let db = test_db("svc_isolation").await;
        let r = repo(&db);

        // team-a should NOT see team-b's payments-svc cluster
        let result = r
            .service_summary("payments-svc", TEAM_A_ID)
            .await
            .expect("service summary should succeed");

        assert!(result.cluster.is_none(), "team-a should not see team-b's cluster");

        // team-b should see its own cluster
        let result_b = r
            .service_summary("payments-svc", TEAM_B_ID)
            .await
            .expect("service summary should succeed");

        assert!(result_b.cluster.is_some(), "team-b should see its own cluster");
    }

    #[tokio::test]
    async fn test_service_summary_orphan_cluster() {
        let db = test_db("svc_orphan").await;
        let r = repo(&db);

        // orphan-cluster exists but has no route configs or listeners
        let result = r
            .service_summary("orphan-cluster", TEAM_A_ID)
            .await
            .expect("service summary should succeed");

        assert!(result.cluster.is_some(), "orphan cluster should still be found");
        assert!(result.route_configs.is_empty(), "orphan has no route configs");
        assert!(result.listeners.is_empty(), "orphan has no listeners");
    }

    #[tokio::test]
    async fn test_service_summary_empty_database() {
        let db = TestDatabase::new("svc_empty").await;
        let r = repo(&db);

        let result = r
            .service_summary("anything", "nonexistent-team-id")
            .await
            .expect("service summary should succeed on empty data");

        assert!(result.cluster.is_none());
    }
}
