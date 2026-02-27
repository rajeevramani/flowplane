//! Ops Agent MCP Tools
//!
//! Read-only diagnostic tools for tracing requests, viewing topology, and
//! validating configuration. These tools query the database directly
//! (no xds_state needed) via `ReportingRepository`.

use crate::domain::OrgId;
use crate::mcp::error::McpError;
use crate::mcp::protocol::{ContentBlock, Tool, ToolCallResult};
use crate::storage::repositories::{
    AuditLogFilters, AuditLogRepository, ClusterRepository, DataplaneRepository,
    NackEventRepository, ReportingRepository,
};
use crate::storage::DbPool;
use crate::xds::{ClusterSpec, HealthCheckSpec};
use chrono::DateTime;
use serde_json::{json, Value};
use tracing::instrument;

// =============================================================================
// TOOL DEFINITIONS
// =============================================================================

/// Ops tool: trace a request path through the full gateway chain.
pub fn ops_trace_request_tool() -> Tool {
    Tool::new(
        "ops_trace_request",
        r#"Trace how a request flows through the gateway: listener → route_config → virtual_host → route → cluster → endpoints.

PURPOSE: Diagnose routing issues by showing every resource a request touches.

USE CASES:
- Verify a path reaches the correct backend
- Debug 404s or unexpected routing
- Understand which listeners handle a request

PARAMETERS:
- path (required): Request path to trace (e.g., "/api/users")
- port (optional): Listener port filter (e.g., 8080)

EXAMPLE:
{ "path": "/api/orders", "port": 8080 }

RETURNS:
- matches: Array of trace rows (listener → route_config → virtual_host → route → cluster)
- endpoints: Array of backend endpoints for the matched clusters
- unmatched_reason: When no matches found, explains why (e.g., "no route matches path")

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Request path to trace (must start with '/', max 2048 chars)",
                    "maxLength": 2048
                },
                "port": {
                    "type": "integer",
                    "description": "Optional listener port filter"
                }
            },
            "required": ["path"]
        }),
    )
}

/// Ops tool: view the full gateway topology.
pub fn ops_topology_tool() -> Tool {
    Tool::new(
        "ops_topology",
        r#"View the full gateway topology: listeners, route_configs, virtual_hosts, routes, and orphan detection.

PURPOSE: Understand the complete gateway layout and find disconnected resources.

USE CASES:
- Get a bird's-eye view of the gateway
- Find orphan clusters (not referenced by any route_config)
- Find orphan route_configs (not bound to any listener)
- Scope to a specific listener, cluster, or route_config

PARAMETERS:
- scope (optional): Filter scope — "listener", "cluster", or "route_config". Omit for full topology.
- name (optional): Resource name filter (used with scope)
- limit (optional): Max rows per level (default: 50)
- include_details (optional): Include full topology rows (default: false). When false, returns summary only (<120 tokens).

EXAMPLE:
{ "scope": "listener", "name": "http-8080", "include_details": true }

RETURNS:
- summary: Counts for listeners, route_configs, clusters, routes, orphans (always included)
- orphan_clusters: Clusters with no route_configs referencing them (always included)
- orphan_route_configs: Route configs with no listener bound (always included)
- rows: Flattened topology rows (only when include_details=true)
- truncated: Whether the result was limited

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "scope": {
                    "type": "string",
                    "description": "Filter scope: 'listener', 'cluster', or 'route_config'. Omit for full topology.",
                    "enum": ["listener", "cluster", "route_config"]
                },
                "name": {
                    "type": "string",
                    "description": "Resource name filter (used with scope)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max rows per level (default: 50)",
                    "default": 50
                },
                "include_details": {
                    "type": "boolean",
                    "description": "Include full topology rows (default: false — summary only for token efficiency)",
                    "default": false
                }
            }
        }),
    )
}

/// Ops tool: validate gateway configuration and detect problems.
pub fn ops_config_validate_tool() -> Tool {
    Tool::new(
        "ops_config_validate",
        r#"Validate gateway configuration and detect Envoy proto constraint violations.

PURPOSE: Find misconfigurations that Envoy would reject (NACK), orphaned resources, and connectivity issues — before or after deployment.

USE CASES:
- Pre-deployment validation — catch proto violations before Envoy NACKs
- Post-deployment check — correlate issues with recent xDS NACKs
- Find orphan clusters (backends with no traffic)
- Find unbound route_configs (routing rules with no listener)
- Detect empty endpoint pools or invalid endpoints

CHECKS PERFORMED:
1. Connectivity: orphan clusters, orphan route_configs, empty listeners
2. Proto violations: empty endpoints, invalid endpoint format, health check thresholds, health check timeout/interval, empty HTTP health check path
3. xDS delivery: recent NACK events from Envoy dataplanes

PARAMETERS: None required. All checks run automatically for the current team.

RETURNS:
- valid: Boolean — true if no issues found
- issues: Array of detected problems, each with:
  - severity: "warning" or "error"
  - category: "orphan_cluster", "orphan_route_config", "empty_listener", "proto_violation", "xds_delivery"
  - message: Human-readable description
  - resource: Affected resource name
- summary: Counts for total issues, warnings, errors, and resource counts
- next_step: Suggested action based on findings

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {}
        }),
    )
}

/// Ops tool: query recent audit log entries (PII-stripped summaries).
pub fn ops_audit_query_tool() -> Tool {
    Tool::new(
        "ops_audit_query",
        r#"Query recent audit log entries for the current team.

PURPOSE: Review recent configuration changes and operations for troubleshooting or compliance.

SECURITY CONSTRAINTS:
- Team and org context are forced from the authenticated session — cannot be overridden
- Returns PII-stripped summaries only (no client_ip, user_agent, old/new configuration)
- Requires audit:read scope — cp:read does NOT grant access

PARAMETERS:
- resource_type (optional): Filter by resource type (e.g., "clusters", "listeners", "routes")
- action (optional): Filter by action (e.g., "create", "update", "delete")
- limit (optional): Max results (default: 20, max: 100)

EXAMPLE:
{ "resource_type": "clusters", "action": "delete", "limit": 10 }

RETURNS:
- entries: Array of audit summaries (id, resource_type, resource_id, resource_name, action, created_at)
- count: Number of entries returned
- message: Summary description

Authorization: Requires audit:read scope (NOT covered by cp:read)."#,
        json!({
            "type": "object",
            "properties": {
                "resource_type": {
                    "type": "string",
                    "description": "Filter by resource type (e.g., 'clusters', 'listeners', 'routes')"
                },
                "action": {
                    "type": "string",
                    "description": "Filter by action (e.g., 'create', 'update', 'delete')"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results (default: 20, max: 100)",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 20
                }
            }
        }),
    )
}

// =============================================================================
// EXECUTE FUNCTIONS
// =============================================================================

// validate_team_in_org is shared — use super::validate_team_in_org
use super::validate_team_in_org;

/// Execute ops_trace_request: trace a request path through the gateway.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_trace_request")]
pub async fn execute_ops_trace_request(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // Validate team belongs to caller's org
    if let Some(oid) = org_id {
        validate_team_in_org(db_pool, team, oid).await?;
    }

    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::InvalidParams("'path' is required".to_string()))?;

    // Input validation (R6): max 2048 chars, must start with '/', reject null bytes
    if path.len() > 2048 {
        return Err(McpError::InvalidParams(
            "'path' exceeds maximum length of 2048 characters".to_string(),
        ));
    }
    if !path.starts_with('/') {
        return Err(McpError::InvalidParams("'path' must start with '/'".to_string()));
    }
    if path.contains('\0') {
        return Err(McpError::InvalidParams("'path' must not contain null bytes".to_string()));
    }

    let port = args.get("port").and_then(|v| v.as_i64());

    let repo = ReportingRepository::new(db_pool.clone());
    let result = repo
        .trace_request_path(team, path, port)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to trace request path: {}", e)))?;

    let port_desc = port.map(|p| format!(" on port {}", p)).unwrap_or_default();

    let (message, unmatched_reason) = if result.matches.is_empty() {
        let reason = format!("No route matches path '{}'{} for the current team", path, port_desc);
        (format!("No routes match path '{}'{}", path, port_desc), Some(reason))
    } else {
        (
            format!(
                "Found {} route(s) matching path '{}'{}",
                result.matches.len(),
                path,
                port_desc
            ),
            None,
        )
    };

    let output = json!({
        "success": true,
        "path": path,
        "port": port,
        "match_count": result.matches.len(),
        "matches": result.matches,
        "endpoints": result.endpoints,
        "unmatched_reason": unmatched_reason,
        "message": message
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute ops_topology: view the full gateway topology.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_topology")]
pub async fn execute_ops_topology(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // Validate team belongs to caller's org
    if let Some(oid) = org_id {
        validate_team_in_org(db_pool, team, oid).await?;
    }

    let scope = args.get("scope").and_then(|v| v.as_str());
    let name = args.get("name").and_then(|v| v.as_str());
    let limit = args.get("limit").and_then(|v| v.as_i64());
    let include_details = args.get("include_details").and_then(|v| v.as_bool()).unwrap_or(false);

    let repo = ReportingRepository::new(db_pool.clone());
    let result = repo
        .full_topology(team, scope, name, limit)
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to get topology: {}", e)))?;

    let message = format!(
        "Topology: {} listeners, {} route_configs, {} clusters, {} routes ({} orphan clusters, {} orphan route_configs){}",
        result.summary.listener_count,
        result.summary.route_config_count,
        result.summary.cluster_count,
        result.summary.route_count,
        result.summary.orphan_cluster_count,
        result.summary.orphan_route_config_count,
        if result.truncated { " [TRUNCATED]" } else { "" }
    );

    // When include_details=false (default), omit rows for token efficiency (<120 tokens).
    // Orphans are always included since they're the most actionable part.
    let output = if include_details {
        json!({
            "success": true,
            "scope": scope.unwrap_or("full"),
            "name": name,
            "rows": result.rows,
            "orphan_clusters": result.orphan_clusters,
            "orphan_route_configs": result.orphan_route_configs,
            "summary": result.summary,
            "truncated": result.truncated,
            "message": message
        })
    } else {
        json!({
            "success": true,
            "scope": scope.unwrap_or("full"),
            "name": name,
            "orphan_clusters": result.orphan_clusters,
            "orphan_route_configs": result.orphan_route_configs,
            "summary": result.summary,
            "truncated": result.truncated,
            "message": message
        })
    };

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Check health check thresholds in a cluster's configuration JSON.
/// Returns issues for health checks with missing or zero thresholds.
fn check_health_check_thresholds(cluster_name: &str, configuration: &str) -> Vec<Value> {
    let mut issues = Vec::new();

    let spec: ClusterSpec = match serde_json::from_str(configuration) {
        Ok(s) => s,
        Err(_) => return issues, // can't parse — skip
    };

    for (i, hc) in spec.health_checks.iter().enumerate() {
        let (healthy_threshold, unhealthy_threshold) = match hc {
            HealthCheckSpec::Http { healthy_threshold, unhealthy_threshold, .. } => {
                (healthy_threshold, unhealthy_threshold)
            }
            HealthCheckSpec::Tcp { healthy_threshold, unhealthy_threshold, .. } => {
                (healthy_threshold, unhealthy_threshold)
            }
        };

        if healthy_threshold.is_none() || *healthy_threshold == Some(0) {
            issues.push(json!({
                "severity": "error",
                "category": "proto_violation",
                "message": format!(
                    "Cluster '{}' health check #{}: missing or zero healthy_threshold — Envoy will NACK this config",
                    cluster_name, i + 1
                ),
                "resource": cluster_name
            }));
        }

        if unhealthy_threshold.is_none() || *unhealthy_threshold == Some(0) {
            issues.push(json!({
                "severity": "error",
                "category": "proto_violation",
                "message": format!(
                    "Cluster '{}' health check #{}: missing or zero unhealthy_threshold — Envoy will NACK this config",
                    cluster_name, i + 1
                ),
                "resource": cluster_name
            }));
        }
    }

    issues
}

/// Check required fields that Envoy enforces at the proto level.
/// Returns issues for clusters with empty endpoints, invalid endpoints,
/// and health check misconfigurations.
fn check_cluster_required_fields(cluster_name: &str, configuration: &str) -> Vec<Value> {
    let mut issues = Vec::new();

    let spec: ClusterSpec = match serde_json::from_str(configuration) {
        Ok(s) => s,
        Err(_) => {
            issues.push(json!({
                "severity": "error",
                "category": "proto_violation",
                "message": format!(
                    "Cluster '{}': configuration JSON is malformed — Envoy cannot parse this",
                    cluster_name
                ),
                "resource": cluster_name
            }));
            return issues;
        }
    };

    // Empty endpoints — Envoy needs at least one endpoint in ClusterLoadAssignment
    if spec.endpoints.is_empty() {
        issues.push(json!({
            "severity": "error",
            "category": "proto_violation",
            "message": format!(
                "Cluster '{}': no endpoints defined — Envoy will have no backends to route to",
                cluster_name
            ),
            "resource": cluster_name
        }));
    }

    // Invalid endpoint format — can't resolve to host:port
    for (i, ep) in spec.endpoints.iter().enumerate() {
        if ep.to_host_port().is_none() {
            issues.push(json!({
                "severity": "error",
                "category": "proto_violation",
                "message": format!(
                    "Cluster '{}' endpoint #{} '{}': invalid format — must be 'host:port'",
                    cluster_name, i + 1, ep
                ),
                "resource": cluster_name
            }));
        }
    }

    // Health check HTTP with empty path — Envoy requires a non-empty path
    for (i, hc) in spec.health_checks.iter().enumerate() {
        if let HealthCheckSpec::Http { path, interval_seconds, timeout_seconds, .. } = hc {
            if path.is_empty() {
                issues.push(json!({
                    "severity": "error",
                    "category": "proto_violation",
                    "message": format!(
                        "Cluster '{}' health check #{}: HTTP health check has empty path — Envoy requires a path (e.g., '/healthz')",
                        cluster_name, i + 1
                    ),
                    "resource": cluster_name
                }));
            }

            // timeout > interval — Envoy rejects or behaves unexpectedly
            if let (Some(timeout), Some(interval)) = (timeout_seconds, interval_seconds) {
                if timeout > interval {
                    issues.push(json!({
                        "severity": "warning",
                        "category": "proto_violation",
                        "message": format!(
                            "Cluster '{}' health check #{}: timeout ({}s) exceeds interval ({}s) — health checks may overlap or Envoy may reject",
                            cluster_name, i + 1, timeout, interval
                        ),
                        "resource": cluster_name
                    }));
                }
            }
        }
    }

    issues
}

/// Execute ops_config_validate: validate gateway configuration.
#[instrument(skip(db_pool, _args), fields(team = %team), name = "mcp_execute_ops_config_validate")]
pub async fn execute_ops_config_validate(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    _args: Value,
) -> Result<ToolCallResult, McpError> {
    // Validate team belongs to caller's org
    if let Some(oid) = org_id {
        validate_team_in_org(db_pool, team, oid).await?;
    }

    let repo = ReportingRepository::new(db_pool.clone());

    // Get topology with orphan detection
    let topology = repo
        .full_topology(team, None, None, Some(200))
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to validate config: {}", e)))?;

    let mut issues: Vec<Value> = Vec::new();

    // Check for orphan clusters
    for orphan in &topology.orphan_clusters {
        issues.push(json!({
            "severity": "warning",
            "category": "orphan_cluster",
            "message": format!("Cluster '{}' (service: {}) is not referenced by any route_config — it receives no traffic", orphan.name, orphan.service_name),
            "resource": orphan.name
        }));
    }

    // Check for orphan route configs
    for orphan in &topology.orphan_route_configs {
        issues.push(json!({
            "severity": "warning",
            "category": "orphan_route_config",
            "message": format!("Route config '{}' (path: {}, cluster: {}) is not bound to any listener — its routes are unreachable", orphan.name, orphan.path_prefix, orphan.cluster_name),
            "resource": orphan.name
        }));
    }

    // Check for listeners with no route configs
    for row in &topology.rows {
        if row.route_config_name.is_none() {
            // Check if this listener is already reported (avoid duplicates)
            let already_reported = issues.iter().any(|i| {
                i.get("category").and_then(|c| c.as_str()) == Some("empty_listener")
                    && i.get("resource").and_then(|r| r.as_str()) == Some(&row.listener_name)
            });
            if !already_reported {
                issues.push(json!({
                    "severity": "warning",
                    "category": "empty_listener",
                    "message": format!("Listener '{}' ({}:{}) has no route configs bound — it cannot route traffic", row.listener_name, row.listener_address, row.listener_port.map(|p| p.to_string()).unwrap_or_else(|| "?".to_string())),
                    "resource": row.listener_name
                }));
            }
        }
    }

    // --- Proto violation checks ---
    let cluster_repo = ClusterRepository::new(db_pool.clone());
    let clusters =
        cluster_repo.list_by_teams(&[team.to_string()], true, Some(500), Some(0)).await.map_err(
            |e| McpError::InternalError(format!("Failed to list clusters for validation: {}", e)),
        )?;

    for cluster in &clusters {
        // Health check threshold checks (missing/zero thresholds → Envoy NACK)
        let mut hc_issues = check_health_check_thresholds(&cluster.name, &cluster.configuration);
        issues.append(&mut hc_issues);

        // Required field checks (empty endpoints, invalid format, HC path/timeout)
        let mut rf_issues = check_cluster_required_fields(&cluster.name, &cluster.configuration);
        issues.append(&mut rf_issues);
    }

    // --- xDS delivery: recent NACK warnings ---
    let nack_repo = NackEventRepository::new(db_pool.clone());
    let recent_nacks = nack_repo
        .list_recent(team, None, Some(5))
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to query recent NACKs: {}", e)))?;

    for nack in &recent_nacks {
        let resource_names: Option<Vec<String>> =
            nack.resource_names.as_ref().and_then(|r| serde_json::from_str(r).ok());

        issues.push(json!({
            "severity": "warning",
            "category": "xds_delivery",
            "message": format!(
                "NACK from '{}' on {} at {}: {}",
                nack.dataplane_name,
                type_url_to_label(&nack.type_url),
                nack.created_at.to_rfc3339(),
                nack.error_message
            ),
            "resource": resource_names.as_ref().and_then(|r| r.first().cloned()).unwrap_or_else(|| nack.dataplane_name.clone())
        }));
    }

    let warning_count = issues.iter().filter(|i| i["severity"] == "warning").count();
    let error_count = issues.iter().filter(|i| i["severity"] == "error").count();
    let proto_violation_count = issues
        .iter()
        .filter(|i| i.get("category").and_then(|c| c.as_str()) == Some("proto_violation"))
        .count();
    let nack_count = recent_nacks.len();
    let valid = issues.is_empty();

    let next_step = if error_count > 0 {
        "Fix proto_violation errors first — these will cause Envoy to NACK the config. Use cp_get_cluster to inspect affected clusters, then cp_update_cluster to fix.".to_string()
    } else if nack_count > 0 {
        "Recent NACKs detected. Use ops_nack_history to see full error details, then ops_xds_delivery_status to check current delivery state.".to_string()
    } else if warning_count > 0 {
        "Warnings found (orphan resources or connectivity gaps). Use ops_topology to see the full resource graph and identify unused resources.".to_string()
    } else {
        "Configuration looks good. No issues detected.".to_string()
    };

    let output = json!({
        "success": true,
        "valid": valid,
        "issues": issues,
        "summary": {
            "total_issues": issues.len(),
            "warnings": warning_count,
            "errors": error_count,
            "proto_violations": proto_violation_count,
            "recent_nacks": nack_count,
            "listeners": topology.summary.listener_count,
            "route_configs": topology.summary.route_config_count,
            "clusters": topology.summary.cluster_count,
            "routes": topology.summary.route_count
        },
        "message": if valid {
            "Configuration is valid — no issues detected".to_string()
        } else {
            format!("Found {} issue(s): {} error(s), {} warning(s) ({} proto violations, {} recent NACKs)",
                issues.len(), error_count, warning_count, proto_violation_count, nack_count)
        },
        "next_step": next_step
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Execute ops_audit_query: query recent audit log entries (PII-stripped).
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_audit_query")]
pub async fn execute_ops_audit_query(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    let resource_type = args.get("resource_type").and_then(|v| v.as_str()).map(|s| s.to_string());
    let action = args.get("action").and_then(|v| v.as_str()).map(|s| s.to_string());
    let limit =
        args.get("limit").and_then(|v| v.as_i64()).map(|v| (v as i32).min(100)).or(Some(20));

    // Force team_id and org_id from session — cannot be overridden by caller
    let filters = AuditLogFilters {
        resource_type,
        action,
        user_id: None, // No user_id filter exposed — PII protection
        org_id: org_id.map(|o| o.to_string()),
        team_id: Some(team.to_string()),
        start_date: None,
        end_date: None,
    };

    let repo = AuditLogRepository::new(db_pool.clone());
    let entries = repo
        .query_logs(Some(filters), limit, Some(0))
        .await
        .map_err(|e| McpError::InternalError(format!("Failed to query audit logs: {}", e)))?;

    // Convert to PII-stripped summaries
    let summaries: Vec<Value> = entries
        .into_iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "resource_type": entry.resource_type,
                "resource_id": entry.resource_id,
                "resource_name": entry.resource_name,
                "action": entry.action,
                "created_at": entry.created_at.to_rfc3339()
            })
        })
        .collect();

    let count = summaries.len();
    let output = json!({
        "success": true,
        "entries": summaries,
        "count": count,
        "message": format!("Found {} audit log entries", count)
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

/// Ops tool: view xDS delivery status per dataplane.
pub fn ops_xds_delivery_status_tool() -> Tool {
    Tool::new(
        "ops_xds_delivery_status",
        r#"Show xDS config delivery status for each dataplane: which resource types were accepted (ACK) or rejected (NACK) by Envoy.

PURPOSE: Quickly assess whether Envoy dataplanes are running the latest configuration or rejecting it.

USE CASES:
- After deploying config changes, verify Envoy accepted them
- Diagnose why a dataplane isn't picking up new routes/clusters
- Find dataplanes with persistent NACK errors

PARAMETERS:
- dataplane_name (optional): Filter to a specific dataplane. Omit to see all dataplanes.

EXAMPLE:
{ "dataplane_name": "my-dataplane" }

RETURNS:
- dataplanes: Array of dataplane status objects, each containing:
  - name: Dataplane name
  - resource_types: Object with CDS/RDS/LDS keys, each showing status (ACK or NACK), error details if NACK
- summary: Total dataplanes, how many have NACKs
- next_step: Suggested action based on findings

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "dataplane_name": {
                    "type": "string",
                    "description": "Filter to a specific dataplane name. Omit for all dataplanes."
                }
            }
        }),
    )
}

/// Ops tool: query recent NACK event history.
pub fn ops_nack_history_tool() -> Tool {
    Tool::new(
        "ops_nack_history",
        r#"Query recent xDS NACK events — configuration rejections from Envoy dataplanes.

PURPOSE: Investigate why Envoy rejected configuration changes by viewing the full error history.

USE CASES:
- After ops_xds_delivery_status shows NACKs, drill into the error details
- Filter by dataplane to isolate issues to a specific instance
- Filter by resource type (CDS/RDS/LDS/EDS) to narrow down the problem
- View recent NACKs after deploying config changes

PARAMETERS:
- limit (optional): Max events to return (default: 10, max: 100)
- dataplane_name (optional): Filter to a specific dataplane
- type_url (optional): Filter by resource type. Accepts short forms: "CDS", "RDS", "LDS", "EDS" or full type URLs.
- since (optional): ISO 8601 timestamp — only show events after this time (e.g., "2026-02-25T00:00:00Z")

EXAMPLES:
{ "dataplane_name": "my-dp", "limit": 5 }
{ "type_url": "CDS", "since": "2026-02-25T00:00:00Z" }

RETURNS:
- events: Array of NACK events with timestamp, dataplane_name, resource_type, error_message, error_code, resource_names, version_rejected
- count: Number of events returned
- message: Summary description

Authorization: Requires cp:read scope."#,
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Max events to return (default: 10, max: 100)",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 10
                },
                "dataplane_name": {
                    "type": "string",
                    "description": "Filter to a specific dataplane name"
                },
                "type_url": {
                    "type": "string",
                    "description": "Filter by resource type: 'CDS', 'RDS', 'LDS', 'EDS', or full type URL"
                },
                "since": {
                    "type": "string",
                    "description": "ISO 8601 timestamp — only show events after this time"
                }
            }
        }),
    )
}

// =============================================================================
// EXECUTE: ops_xds_delivery_status
// =============================================================================

/// Map a full xDS type URL to a short resource type label.
fn type_url_to_label(type_url: &str) -> &str {
    if type_url.contains("Cluster") {
        "CDS"
    } else if type_url.contains("RouteConfiguration") {
        "RDS"
    } else if type_url.contains("Listener") {
        "LDS"
    } else if type_url.contains("ClusterLoadAssignment") {
        "EDS"
    } else {
        type_url
    }
}

/// Execute ops_xds_delivery_status: show per-dataplane xDS delivery health.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_xds_delivery_status")]
pub async fn execute_ops_xds_delivery_status(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // Validate team belongs to caller's org
    if let Some(oid) = org_id {
        validate_team_in_org(db_pool, team, oid).await?;
    }

    let dataplane_filter =
        args.get("dataplane_name").or_else(|| args.get("dataplaneId")).and_then(|v| v.as_str());

    let dataplane_repo = DataplaneRepository::new(db_pool.clone());
    let nack_repo = NackEventRepository::new(db_pool.clone());

    // Get dataplanes for this team
    let dataplanes = match dataplane_filter {
        Some(name) => match dataplane_repo.get_by_name(team, name).await {
            Ok(Some(dp)) => vec![dp],
            Ok(None) => vec![],
            Err(e) => {
                return Err(McpError::InternalError(format!("Failed to get dataplane: {}", e)));
            }
        },
        None => dataplane_repo
            .list_by_team(team, Some(100), Some(0))
            .await
            .map_err(|e| McpError::InternalError(format!("Failed to list dataplanes: {}", e)))?,
    };

    let known_types = ["CDS", "RDS", "LDS"];
    let mut dataplane_statuses = Vec::new();
    let mut nack_count = 0;

    for dp in &dataplanes {
        // Get latest NACK per type_url for this dataplane
        let latest_nacks = nack_repo.latest_per_type_url(team, &dp.name).await.map_err(|e| {
            McpError::InternalError(format!(
                "Failed to get NACK status for dataplane '{}': {}",
                dp.name, e
            ))
        })?;

        let mut resource_types = json!({});
        let mut has_nack = false;

        // Build a map of type_url label → nack event
        let mut nack_map = std::collections::HashMap::new();
        for nack in &latest_nacks {
            let label = type_url_to_label(&nack.type_url);
            nack_map.insert(label.to_string(), nack);
        }

        // Report status for each known resource type
        for type_label in &known_types {
            if let Some(nack) = nack_map.get(*type_label) {
                has_nack = true;
                let resource_names: Option<Vec<String>> =
                    nack.resource_names.as_ref().and_then(|r| serde_json::from_str(r).ok());

                resource_types[type_label] = json!({
                    "status": "NACK",
                    "error_message": nack.error_message,
                    "error_code": nack.error_code,
                    "resource_names": resource_names,
                    "version_rejected": nack.version_rejected,
                    "nacked_at": nack.created_at.to_rfc3339()
                });
            } else {
                resource_types[type_label] = json!({
                    "status": "ACK"
                });
            }
        }

        // Include any unexpected type_urls not in the known set
        for nack in &latest_nacks {
            let label = type_url_to_label(&nack.type_url);
            if !known_types.contains(&label) {
                has_nack = true;
                resource_types[label] = json!({
                    "status": "NACK",
                    "error_message": nack.error_message,
                    "error_code": nack.error_code,
                    "nacked_at": nack.created_at.to_rfc3339()
                });
            }
        }

        if has_nack {
            nack_count += 1;
        }

        dataplane_statuses.push(json!({
            "name": dp.name,
            "resource_types": resource_types
        }));
    }

    let total = dataplanes.len();
    let healthy = total - nack_count;

    let message = if dataplanes.is_empty() {
        "No dataplanes found for this team".to_string()
    } else if nack_count == 0 {
        format!("All {} dataplane(s) healthy — no NACK events recorded", total)
    } else {
        format!(
            "{} of {} dataplane(s) have NACK events — Envoy rejected configuration",
            nack_count, total
        )
    };

    let next_step = if dataplanes.is_empty() {
        "Create a dataplane first, then bootstrap an Envoy instance to connect to it."
    } else if nack_count > 0 {
        "Use ops_nack_history to see full NACK details, then ops_config_validate to find the root cause."
    } else {
        "All dataplanes are healthy. No action needed."
    };

    let output = json!({
        "success": true,
        "dataplanes": dataplane_statuses,
        "summary": {
            "total_dataplanes": total,
            "healthy": healthy,
            "nacked": nack_count
        },
        "message": message,
        "next_step": next_step
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

// =============================================================================
// EXECUTE: ops_nack_history
// =============================================================================

/// Expand short-form type labels to full xDS type URLs.
fn expand_type_url(input: &str) -> String {
    match input.to_uppercase().as_str() {
        "CDS" => "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
        "RDS" => "type.googleapis.com/envoy.config.route.v3.RouteConfiguration".to_string(),
        "LDS" => "type.googleapis.com/envoy.config.listener.v3.Listener".to_string(),
        "EDS" => "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment".to_string(),
        _ => input.to_string(), // assume it's already a full type URL
    }
}

/// Execute ops_nack_history: query recent NACK events.
#[instrument(skip(db_pool, args), fields(team = %team), name = "mcp_execute_ops_nack_history")]
pub async fn execute_ops_nack_history(
    db_pool: &DbPool,
    team: &str,
    org_id: Option<&OrgId>,
    args: Value,
) -> Result<ToolCallResult, McpError> {
    // Validate team belongs to caller's org
    if let Some(oid) = org_id {
        validate_team_in_org(db_pool, team, oid).await?;
    }

    let limit =
        args.get("limit").and_then(|v| v.as_i64()).map(|v| (v as i32).min(100)).unwrap_or(10);
    let dataplane_name = args.get("dataplane_name").and_then(|v| v.as_str());
    let type_url_filter = args.get("type_url").and_then(|v| v.as_str());
    let since_str = args.get("since").and_then(|v| v.as_str());

    // Parse `since` timestamp if provided
    let since = match since_str {
        Some(s) => {
            let parsed: DateTime<chrono::Utc> = s.parse().map_err(|_| {
                McpError::InvalidParams(format!(
                    "Invalid 'since' timestamp '{}' — expected ISO 8601 format (e.g., 2026-02-25T00:00:00Z)",
                    s
                ))
            })?;
            Some(parsed)
        }
        None => None,
    };

    let nack_repo = NackEventRepository::new(db_pool.clone());

    // Choose the most specific query method based on filters
    let events =
        if let Some(dp_name) = dataplane_name {
            nack_repo.list_by_dataplane(team, dp_name, Some(limit)).await.map_err(|e| {
                McpError::InternalError(format!("Failed to query NACK events: {}", e))
            })?
        } else if let Some(tu) = type_url_filter {
            let full_url = expand_type_url(tu);
            nack_repo.list_by_type_url(team, &full_url, Some(limit)).await.map_err(|e| {
                McpError::InternalError(format!("Failed to query NACK events: {}", e))
            })?
        } else {
            nack_repo.list_recent(team, since, Some(limit)).await.map_err(|e| {
                McpError::InternalError(format!("Failed to query NACK events: {}", e))
            })?
        };

    // Apply `since` filter client-side for dataplane/type_url queries (those methods don't accept since)
    let events: Vec<_> =
        if since.is_some() && (dataplane_name.is_some() || type_url_filter.is_some()) {
            let since_time = since.as_ref().copied();
            events.into_iter().filter(|e| since_time.is_none_or(|s| e.created_at >= s)).collect()
        } else {
            events
        };

    let formatted: Vec<Value> = events
        .iter()
        .map(|e| {
            let resource_names: Option<Vec<String>> =
                e.resource_names.as_ref().and_then(|r| serde_json::from_str(r).ok());

            json!({
                "timestamp": e.created_at.to_rfc3339(),
                "dataplane_name": e.dataplane_name,
                "resource_type": type_url_to_label(&e.type_url),
                "error_message": e.error_message,
                "error_code": e.error_code,
                "resource_names": resource_names,
                "version_rejected": e.version_rejected
            })
        })
        .collect();

    let count = formatted.len();
    let message = if count == 0 {
        "No NACK events found matching the filters".to_string()
    } else {
        format!("Found {} NACK event(s)", count)
    };

    let output = json!({
        "success": true,
        "events": formatted,
        "count": count,
        "message": message
    });

    let text = serde_json::to_string_pretty(&output).map_err(McpError::SerializationError)?;
    Ok(ToolCallResult { content: vec![ContentBlock::Text { text }], is_error: None })
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_helpers::{seed_reporting_data, TestDatabase, TEAM_A_ID, TEAM_B_ID};

    // Helper: create a TestDatabase with reporting seed data
    async fn test_db(name: &str) -> TestDatabase {
        let db = TestDatabase::new(name).await;
        seed_reporting_data(&db.pool).await;
        db
    }

    // ========================================================================
    // Tool definition tests
    // ========================================================================

    #[test]
    fn test_ops_trace_request_tool_definition() {
        let tool = ops_trace_request_tool();
        assert_eq!(tool.name, "ops_trace_request");
        assert!(tool.description.as_ref().unwrap().contains("Trace"));

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["port"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "path");
    }

    #[test]
    fn test_ops_topology_tool_definition() {
        let tool = ops_topology_tool();
        assert_eq!(tool.name, "ops_topology");
        assert!(tool.description.as_ref().unwrap().contains("topology"));

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["scope"].is_object());
        assert!(schema["properties"]["name"].is_object());
        assert!(schema["properties"]["limit"].is_object());
        assert!(schema["properties"]["include_details"].is_object());

        // No required params
        assert!(
            schema["required"].is_null()
                || schema["required"].as_array().map(|a| a.is_empty()).unwrap_or(true)
        );
    }

    #[test]
    fn test_ops_config_validate_tool_definition() {
        let tool = ops_config_validate_tool();
        assert_eq!(tool.name, "ops_config_validate");
        assert!(tool.description.as_ref().unwrap().contains("Validate"));

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn test_all_ops_tools_have_valid_schemas() {
        let tools = vec![
            ops_trace_request_tool(),
            ops_topology_tool(),
            ops_config_validate_tool(),
            ops_xds_delivery_status_tool(),
        ];

        for tool in tools {
            assert!(!tool.name.is_empty(), "Tool name should not be empty");
            assert!(
                tool.description.as_ref().is_some_and(|d| !d.is_empty()),
                "Tool '{}' description should not be empty",
                tool.name
            );
            assert!(
                tool.input_schema.is_object(),
                "Tool '{}' schema should be an object",
                tool.name
            );
            assert_eq!(
                tool.input_schema["type"], "object",
                "Tool '{}' schema type should be 'object'",
                tool.name
            );
        }
    }

    // ========================================================================
    // Execute: ops_trace_request
    // ========================================================================

    #[tokio::test]
    async fn test_execute_ops_trace_request_happy_path() {
        let db = test_db("ops_trace_happy").await;

        let result = execute_ops_trace_request(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/api/orders", "port": 8080}),
        )
        .await
        .expect("should succeed");

        assert!(result.is_error.is_none());
        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert!(output["match_count"].as_i64().unwrap() > 0);
        assert!(!output["matches"].as_array().unwrap().is_empty());
        assert!(!output["endpoints"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_execute_ops_trace_request_no_match() {
        let db = test_db("ops_trace_no_match").await;

        let result = execute_ops_trace_request(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/nonexistent/path"}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert_eq!(output["match_count"], 0);
        assert!(output["matches"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_execute_ops_trace_request_missing_path() {
        let db = test_db("ops_trace_missing_path").await;

        let result = execute_ops_trace_request(&db.pool, TEAM_A_ID, None, json!({})).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidParams(msg) => assert!(msg.contains("path")),
            other => panic!("expected InvalidParams, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_execute_ops_trace_request_path_validation() {
        let db = test_db("ops_trace_validate").await;

        // Must start with '/'
        let result =
            execute_ops_trace_request(&db.pool, TEAM_A_ID, None, json!({"path": "no-slash"})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidParams(msg) => assert!(msg.contains("start with '/'")),
            other => panic!("expected InvalidParams for no-slash, got: {:?}", other),
        }

        // Max 2048 chars
        let long_path = format!("/{}", "a".repeat(2048));
        let result =
            execute_ops_trace_request(&db.pool, TEAM_A_ID, None, json!({"path": long_path})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidParams(msg) => assert!(msg.contains("2048")),
            other => panic!("expected InvalidParams for long path, got: {:?}", other),
        }

        // Reject null bytes
        let result =
            execute_ops_trace_request(&db.pool, TEAM_A_ID, None, json!({"path": "/api/\0bad"}))
                .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidParams(msg) => assert!(msg.contains("null")),
            other => panic!("expected InvalidParams for null byte, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_execute_ops_trace_request_unmatched_reason() {
        let db = test_db("ops_trace_unmatched").await;

        let result = execute_ops_trace_request(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/nonexistent/path"}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["match_count"], 0);
        assert!(
            output["unmatched_reason"].is_string(),
            "should have unmatched_reason when no match"
        );
        let reason = output["unmatched_reason"].as_str().unwrap();
        assert!(reason.contains("No route matches"), "reason should explain the miss");
    }

    #[tokio::test]
    async fn test_execute_ops_trace_request_team_isolation() {
        let db = test_db("ops_trace_team_iso").await;

        // team-a should not see team-b's payments route
        let result = execute_ops_trace_request(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"path": "/api/payments", "port": 9090}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["match_count"], 0, "team-a must not see team-b routes");

        // team-b should see its own payments route
        let result_b = execute_ops_trace_request(
            &db.pool,
            TEAM_B_ID,
            None,
            json!({"path": "/api/payments", "port": 9090}),
        )
        .await
        .expect("should succeed");

        let text_b = match &result_b.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output_b: Value = serde_json::from_str(text_b).expect("should be valid JSON");
        assert!(output_b["match_count"].as_i64().unwrap() > 0, "team-b should see its own routes");
    }

    // ========================================================================
    // Execute: ops_topology
    // ========================================================================

    #[tokio::test]
    async fn test_execute_ops_topology_summary_only() {
        let db = test_db("ops_topo_summary").await;

        // Default: include_details=false, returns summary only (no rows)
        let result = execute_ops_topology(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert!(output["summary"]["listener_count"].as_i64().unwrap() > 0);
        assert!(output.get("rows").is_none(), "summary mode should not include rows");
        // Orphans are always included
        assert!(output.get("orphan_clusters").is_some());
    }

    #[tokio::test]
    async fn test_execute_ops_topology_with_details() {
        let db = test_db("ops_topo_details").await;

        let result =
            execute_ops_topology(&db.pool, TEAM_A_ID, None, json!({"include_details": true}))
                .await
                .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert!(!output["rows"].as_array().unwrap().is_empty(), "detail mode should include rows");
        assert!(output["summary"]["listener_count"].as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_execute_ops_topology_scoped() {
        let db = test_db("ops_topo_scoped").await;

        let result = execute_ops_topology(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"scope": "listener", "name": "http-8080", "include_details": true}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["scope"], "listener");

        // All rows should be for the specified listener
        for row in output["rows"].as_array().unwrap() {
            assert_eq!(row["listener_name"], "http-8080");
        }
    }

    #[tokio::test]
    async fn test_execute_ops_topology_with_limit() {
        let db = test_db("ops_topo_limit").await;

        let result = execute_ops_topology(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"limit": 1, "include_details": true}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert!(output["rows"].as_array().unwrap().len() <= 1);
        assert_eq!(output["truncated"], true);
    }

    #[tokio::test]
    async fn test_execute_ops_topology_team_isolation() {
        let db = test_db("ops_topo_team_iso").await;

        let result =
            execute_ops_topology(&db.pool, TEAM_A_ID, None, json!({"include_details": true}))
                .await
                .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");

        // team-a should not see team-b's listener
        let listener_names: Vec<&str> = output["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["listener_name"].as_str())
            .collect();
        assert!(!listener_names.contains(&"http-9090"), "team-a must not see team-b listener");
    }

    // ========================================================================
    // Execute: ops_config_validate
    // ========================================================================

    #[tokio::test]
    async fn test_execute_ops_config_validate_detects_orphans() {
        let db = test_db("ops_validate_orphans").await;

        let result = execute_ops_config_validate(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert_eq!(output["valid"], false, "should detect issues");

        let issues = output["issues"].as_array().unwrap();

        // Should detect orphan cluster
        let orphan_cluster_issues: Vec<&Value> =
            issues.iter().filter(|i| i["category"] == "orphan_cluster").collect();
        assert!(!orphan_cluster_issues.is_empty(), "should detect orphan cluster");
        assert!(orphan_cluster_issues.iter().any(|i| i["resource"] == "orphan-cluster"));

        // Should detect orphan route config
        let orphan_rc_issues: Vec<&Value> =
            issues.iter().filter(|i| i["category"] == "orphan_route_config").collect();
        assert!(!orphan_rc_issues.is_empty(), "should detect orphan route config");
        assert!(orphan_rc_issues.iter().any(|i| i["resource"] == "orphan-rc"));
    }

    #[tokio::test]
    async fn test_execute_ops_config_validate_empty_team() {
        let db = TestDatabase::new("ops_validate_empty").await;

        let result = execute_ops_config_validate(&db.pool, "nonexistent-team", None, json!({}))
            .await
            .expect("should succeed on empty data");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        // No resources means no issues (or only global resource issues)
    }

    #[tokio::test]
    async fn test_execute_ops_config_validate_team_isolation() {
        let db = test_db("ops_validate_team_iso").await;

        // team-b should NOT see team-a's orphan cluster
        let result = execute_ops_config_validate(&db.pool, TEAM_B_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");

        let issues = output["issues"].as_array().unwrap();
        let orphan_names: Vec<&str> = issues
            .iter()
            .filter(|i| i["category"] == "orphan_cluster")
            .filter_map(|i| i["resource"].as_str())
            .collect();
        assert!(
            !orphan_names.contains(&"orphan-cluster"),
            "team-b must not see team-a's orphan-cluster"
        );
    }

    // ========================================================================
    // Tool definition: ops_audit_query
    // ========================================================================

    #[test]
    fn test_ops_audit_query_tool_definition() {
        let tool = ops_audit_query_tool();
        assert_eq!(tool.name, "ops_audit_query");
        assert!(tool.description.as_ref().unwrap().contains("audit"));
        assert!(tool.description.as_ref().unwrap().contains("audit:read"));

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["resource_type"].is_object());
        assert!(schema["properties"]["action"].is_object());
        assert!(schema["properties"]["limit"].is_object());

        // No required params
        assert!(
            schema["required"].is_null()
                || schema["required"].as_array().map(|a| a.is_empty()).unwrap_or(true)
        );
    }

    // ========================================================================
    // Execute: ops_audit_query
    // ========================================================================

    #[tokio::test]
    async fn test_execute_ops_audit_query_empty() {
        let db = TestDatabase::new("ops_audit_empty").await;

        let result = execute_ops_audit_query(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed on empty audit logs");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert_eq!(output["count"], 0);
        assert!(output["entries"].as_array().unwrap().is_empty());
    }

    /// Helper: insert a basic audit log entry for testing.
    async fn insert_audit_entry(
        pool: &crate::storage::DbPool,
        resource_type: &str,
        resource_name: &str,
        action: &str,
        team_id: &str,
    ) {
        sqlx::query(
            "INSERT INTO audit_log (resource_type, resource_id, resource_name, action, \
             user_id, client_ip, user_agent, org_id, team_id, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())",
        )
        .bind(resource_type)
        .bind(resource_name) // use name as id for simplicity
        .bind(resource_name)
        .bind(action)
        .bind(None::<&str>) // user_id
        .bind(None::<&str>) // client_ip
        .bind(None::<&str>) // user_agent
        .bind(None::<&str>) // org_id
        .bind(team_id)
        .execute(pool)
        .await
        .expect("insert audit entry");
    }

    /// Helper: insert an audit log entry with PII fields for testing PII stripping.
    async fn insert_audit_entry_with_pii(
        pool: &crate::storage::DbPool,
        resource_type: &str,
        resource_name: &str,
        action: &str,
        team_id: &str,
    ) {
        sqlx::query(
            "INSERT INTO audit_log (resource_type, resource_id, resource_name, action, \
             user_id, client_ip, user_agent, org_id, team_id, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())",
        )
        .bind(resource_type)
        .bind(resource_name)
        .bind(resource_name)
        .bind(action)
        .bind("user-secret") // PII: user_id
        .bind("10.0.0.1") // PII: client_ip
        .bind("curl/7.0") // PII: user_agent
        .bind(None::<&str>) // org_id
        .bind(team_id)
        .execute(pool)
        .await
        .expect("insert audit entry with pii");
    }

    #[tokio::test]
    async fn test_execute_ops_audit_query_with_data() {
        let db = TestDatabase::new("ops_audit_data").await;

        // Insert audit log entry with PII fields
        insert_audit_entry_with_pii(&db.pool, "clusters", "my-cluster", "create", TEAM_A_ID).await;

        let result = execute_ops_audit_query(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert!(output["count"].as_i64().unwrap() >= 1);

        // Verify PII is stripped — no client_ip, user_agent, user_id, old/new config
        let entry = &output["entries"][0];
        assert!(entry.get("client_ip").is_none(), "must NOT contain client_ip");
        assert!(entry.get("user_agent").is_none(), "must NOT contain user_agent");
        assert!(entry.get("user_id").is_none(), "must NOT contain user_id");
        assert!(entry.get("old_configuration").is_none(), "must NOT contain old_configuration");
        assert!(entry.get("new_configuration").is_none(), "must NOT contain new_configuration");

        // Should contain summary fields
        assert_eq!(entry["resource_type"], "clusters");
        assert_eq!(entry["action"], "create");
        assert_eq!(entry["resource_name"], "my-cluster");
    }

    #[tokio::test]
    async fn test_execute_ops_audit_query_team_isolation() {
        let db = TestDatabase::new("ops_audit_iso").await;

        // team-a event
        insert_audit_entry(&db.pool, "clusters", "team-a-cluster", "create", TEAM_A_ID).await;

        // team-b event
        insert_audit_entry(&db.pool, "clusters", "team-b-cluster", "delete", TEAM_B_ID).await;

        // team-a should only see its own entry
        let result = execute_ops_audit_query(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        let entries = output["entries"].as_array().unwrap();
        for entry in entries {
            assert_ne!(
                entry["resource_name"], "team-b-cluster",
                "team-a must NOT see team-b's audit entries"
            );
        }
    }

    #[tokio::test]
    async fn test_execute_ops_audit_query_with_filters() {
        let db = TestDatabase::new("ops_audit_filter").await;

        // Insert multiple events
        for action in &["create", "update", "delete"] {
            insert_audit_entry(&db.pool, "listeners", "test-listener", action, TEAM_A_ID).await;
        }

        // Filter by action
        let result = execute_ops_audit_query(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"action": "delete", "resource_type": "listeners"}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        let entries = output["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["action"], "delete");
    }

    // ========================================================================
    // Tool definition: ops_nack_history
    // ========================================================================

    #[test]
    fn test_ops_nack_history_tool_definition() {
        let tool = ops_nack_history_tool();
        assert_eq!(tool.name, "ops_nack_history");
        assert!(tool.description.as_ref().unwrap().contains("NACK"));

        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["limit"].is_object());
        assert!(schema["properties"]["dataplane_name"].is_object());
        assert!(schema["properties"]["type_url"].is_object());
        assert!(schema["properties"]["since"].is_object());

        // No required params
        assert!(
            schema["required"].is_null()
                || schema["required"].as_array().map(|a| a.is_empty()).unwrap_or(true)
        );
    }

    // ========================================================================
    // Execute: ops_xds_delivery_status
    // ========================================================================

    #[tokio::test]
    async fn test_execute_ops_xds_delivery_status_empty() {
        let db = test_db("ops_delivery_empty").await;

        let result = execute_ops_xds_delivery_status(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");
        assert!(output["success"].as_bool().unwrap_or(false));
        assert!(output["dataplanes"].as_array().unwrap_or(&vec![]).is_empty());
    }

    #[tokio::test]
    async fn test_execute_ops_xds_delivery_status_with_nacks() {
        let db = test_db("ops_delivery_nacks").await;

        // Create a dataplane for team-a
        sqlx::query(
            "INSERT INTO dataplanes (id, name, team, created_at, updated_at) \
             VALUES ($1, $2, $3, NOW(), NOW())",
        )
        .bind("dp-status-id")
        .bind("dp-status")
        .bind(TEAM_A_ID)
        .execute(&db.pool)
        .await
        .expect("insert dataplane");

        // Insert a NACK event for that dataplane
        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-status",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "threshold missing",
        )
        .await;

        let result = execute_ops_xds_delivery_status(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");
        assert!(output["success"].as_bool().unwrap_or(false));

        let dataplanes = output["dataplanes"].as_array().expect("dataplanes array");
        assert!(!dataplanes.is_empty(), "should have at least one dataplane");

        // Find dp-status in the results
        let dp =
            dataplanes.iter().find(|d| d["name"] == "dp-status").expect("should find dp-status");
        let resource_types = &dp["resource_types"];
        assert!(resource_types["CDS"].is_object(), "should have CDS entry");
        assert_eq!(resource_types["CDS"]["status"], "NACK");
    }

    #[tokio::test]
    async fn test_execute_ops_xds_delivery_status_team_isolation() {
        let db = test_db("ops_delivery_iso").await;

        // Create dataplanes for both teams
        sqlx::query(
            "INSERT INTO dataplanes (id, name, team, created_at, updated_at) \
             VALUES ($1, $2, $3, NOW(), NOW())",
        )
        .bind("dp-iso-a-id")
        .bind("dp-iso-a")
        .bind(TEAM_A_ID)
        .execute(&db.pool)
        .await
        .expect("insert dp-a");

        sqlx::query(
            "INSERT INTO dataplanes (id, name, team, created_at, updated_at) \
             VALUES ($1, $2, $3, NOW(), NOW())",
        )
        .bind("dp-iso-b-id")
        .bind("dp-iso-b")
        .bind(TEAM_B_ID)
        .execute(&db.pool)
        .await
        .expect("insert dp-b");

        // Insert NACKs for team-b
        insert_nack_event(
            &db.pool,
            TEAM_B_ID,
            "dp-iso-b",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "secret error",
        )
        .await;

        // Team-a should only see its own dataplane, not team-b's NACKs
        let result = execute_ops_xds_delivery_status(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");

        let dataplanes = output["dataplanes"].as_array().expect("dataplanes array");
        // Team-a should see dp-iso-a but NOT dp-iso-b
        assert!(
            dataplanes.iter().all(|d| d["name"] != "dp-iso-b"),
            "should not see team-b dataplanes"
        );
    }

    #[tokio::test]
    async fn test_execute_ops_xds_delivery_status_filter_by_dataplane() {
        let db = test_db("ops_delivery_filter").await;

        // Create two dataplanes for team-a
        for (id, name) in [("dp-f1-id", "dp-filter-1"), ("dp-f2-id", "dp-filter-2")] {
            sqlx::query(
                "INSERT INTO dataplanes (id, name, team, created_at, updated_at) \
                 VALUES ($1, $2, $3, NOW(), NOW())",
            )
            .bind(id)
            .bind(name)
            .bind(TEAM_A_ID)
            .execute(&db.pool)
            .await
            .expect("insert dataplane");
        }

        let result = execute_ops_xds_delivery_status(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"dataplane_name": "dp-filter-1"}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("valid JSON");

        let dataplanes = output["dataplanes"].as_array().expect("dataplanes array");
        assert_eq!(dataplanes.len(), 1, "should only show filtered dataplane");
        assert_eq!(dataplanes[0]["name"], "dp-filter-1");
    }

    // ========================================================================
    // Execute: ops_nack_history
    // ========================================================================

    /// Helper: insert a NACK event for testing.
    async fn insert_nack_event(
        pool: &crate::storage::DbPool,
        team: &str,
        dataplane_name: &str,
        type_url: &str,
        error_message: &str,
    ) {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO xds_nack_events (id, team, dataplane_name, type_url, version_rejected, nonce, error_code, error_message, node_id, resource_names, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())",
        )
        .bind(&id)
        .bind(team)
        .bind(dataplane_name)
        .bind(type_url)
        .bind("v1")
        .bind("nonce-1")
        .bind(13_i64) // INTERNAL error code
        .bind(error_message)
        .bind(None::<&str>)
        .bind(Some(r#"["my-cluster"]"#))
        .execute(pool)
        .await
        .expect("insert nack event");
    }

    #[tokio::test]
    async fn test_execute_ops_nack_history_empty() {
        let db = TestDatabase::new("ops_nack_hist_empty").await;

        let result = execute_ops_nack_history(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed on empty");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert_eq!(output["count"], 0);
        assert!(output["events"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_execute_ops_nack_history_returns_events() {
        let db = test_db("ops_nack_hist_data").await;

        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "missing threshold",
        )
        .await;

        let result = execute_ops_nack_history(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        assert_eq!(output["success"], true);
        assert!(output["count"].as_i64().unwrap() >= 1);

        let event = &output["events"][0];
        assert_eq!(event["dataplane_name"], "dp-1");
        assert_eq!(event["resource_type"], "CDS");
        assert_eq!(event["error_message"], "missing threshold");
        assert!(event["resource_names"].is_array());
    }

    #[tokio::test]
    async fn test_execute_ops_nack_history_filter_by_dataplane() {
        let db = test_db("ops_nack_hist_dp").await;

        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-alpha",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "error alpha",
        )
        .await;
        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-beta",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "error beta",
        )
        .await;

        let result = execute_ops_nack_history(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"dataplane_name": "dp-alpha"}),
        )
        .await
        .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        let events = output["events"].as_array().unwrap();
        for event in events {
            assert_eq!(event["dataplane_name"], "dp-alpha");
        }
    }

    #[tokio::test]
    async fn test_execute_ops_nack_history_filter_by_type_url_short() {
        let db = test_db("ops_nack_hist_type").await;

        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "cds error",
        )
        .await;
        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.listener.v3.Listener",
            "lds error",
        )
        .await;

        // Filter by "CDS" short form
        let result =
            execute_ops_nack_history(&db.pool, TEAM_A_ID, None, json!({"type_url": "CDS"}))
                .await
                .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        let events = output["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["resource_type"], "CDS");
    }

    #[tokio::test]
    async fn test_execute_ops_nack_history_team_isolation() {
        let db = test_db("ops_nack_hist_iso").await;

        // team-a NACK
        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-a",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "team-a error",
        )
        .await;

        // team-b NACK
        insert_nack_event(
            &db.pool,
            TEAM_B_ID,
            "dp-b",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "team-b error",
        )
        .await;

        // team-a should only see its own events
        let result = execute_ops_nack_history(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");
        let events = output["events"].as_array().unwrap();
        for event in events {
            assert_ne!(event["dataplane_name"], "dp-b", "team-a must NOT see team-b's NACK events");
        }
    }

    #[tokio::test]
    async fn test_execute_ops_nack_history_invalid_since() {
        let db = TestDatabase::new("ops_nack_hist_bad_since").await;

        let result = execute_ops_nack_history(
            &db.pool,
            TEAM_A_ID,
            None,
            json!({"since": "not-a-timestamp"}),
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            McpError::InvalidParams(msg) => assert!(msg.contains("Invalid 'since' timestamp")),
            other => panic!("expected InvalidParams, got: {:?}", other),
        }
    }

    // ========================================================================
    // expand_type_url unit tests
    // ========================================================================

    #[test]
    fn test_expand_type_url_short_forms() {
        assert!(expand_type_url("CDS").contains("Cluster"));
        assert!(expand_type_url("cds").contains("Cluster")); // case insensitive
        assert!(expand_type_url("RDS").contains("RouteConfiguration"));
        assert!(expand_type_url("LDS").contains("Listener"));
        assert!(expand_type_url("EDS").contains("ClusterLoadAssignment"));
    }

    #[test]
    fn test_expand_type_url_passthrough() {
        let full = "type.googleapis.com/envoy.config.cluster.v3.Cluster";
        assert_eq!(expand_type_url(full), full);
    }

    // ========================================================================
    // check_health_check_thresholds unit tests
    // ========================================================================

    #[test]
    fn test_check_health_check_thresholds_valid() {
        let config = r#"{"endpoints": [{"host": "127.0.0.1", "port": 8080}], "healthChecks": [{"type": "http", "path": "/health", "healthy_threshold": 2, "unhealthy_threshold": 3}]}"#;
        let issues = check_health_check_thresholds("test-cluster", config);
        assert!(issues.is_empty(), "valid thresholds should produce no issues");
    }

    #[test]
    fn test_check_health_check_thresholds_missing() {
        // Health check with no thresholds — should produce errors
        let config = r#"{"endpoints": [{"host": "127.0.0.1", "port": 8080}], "healthChecks": [{"type": "http", "path": "/health"}]}"#;
        let issues = check_health_check_thresholds("test-cluster", config);
        assert_eq!(issues.len(), 2, "missing both thresholds should produce 2 errors");
        assert!(issues.iter().all(|i| i["category"] == "proto_violation"));
        assert!(issues.iter().all(|i| i["severity"] == "error"));
    }

    #[test]
    fn test_check_health_check_thresholds_zero() {
        let config = r#"{"endpoints": [{"host": "127.0.0.1", "port": 8080}], "healthChecks": [{"type": "http", "path": "/health", "healthy_threshold": 0, "unhealthy_threshold": 0}]}"#;
        let issues = check_health_check_thresholds("test-cluster", config);
        assert_eq!(issues.len(), 2, "zero thresholds should produce 2 errors");
    }

    #[test]
    fn test_check_health_check_thresholds_no_health_checks() {
        let config = r#"{"endpoints": [{"host": "127.0.0.1", "port": 8080}]}"#;
        let issues = check_health_check_thresholds("test-cluster", config);
        assert!(issues.is_empty(), "no health checks = no issues");
    }

    #[test]
    fn test_check_health_check_thresholds_invalid_json() {
        let issues = check_health_check_thresholds("test-cluster", "not json");
        assert!(issues.is_empty(), "unparseable config should be skipped gracefully");
    }

    // ========================================================================
    // check_cluster_required_fields tests
    // ========================================================================

    #[test]
    fn test_check_cluster_required_fields_valid() {
        let config = r#"{"endpoints": [{"host": "127.0.0.1", "port": 8080}]}"#;
        let issues = check_cluster_required_fields("test-cluster", config);
        assert!(issues.is_empty(), "valid cluster should produce no issues");
    }

    #[test]
    fn test_check_cluster_required_fields_empty_endpoints() {
        let config = r#"{"endpoints": []}"#;
        let issues = check_cluster_required_fields("test-cluster", config);
        assert_eq!(issues.len(), 1, "empty endpoints should produce 1 error");
        assert_eq!(issues[0]["category"], "proto_violation");
        assert_eq!(issues[0]["severity"], "error");
        assert!(issues[0]["message"].as_str().unwrap_or("").contains("no endpoints"));
    }

    #[test]
    fn test_check_cluster_required_fields_invalid_endpoint() {
        let config = r#"{"endpoints": ["not-a-valid-endpoint"]}"#;
        let issues = check_cluster_required_fields("test-cluster", config);
        assert!(
            issues.iter().any(|i| i["message"].as_str().unwrap_or("").contains("invalid format")),
            "invalid endpoint format should be detected"
        );
    }

    #[test]
    fn test_check_cluster_required_fields_empty_http_hc_path() {
        let config = r#"{"endpoints": [{"host": "127.0.0.1", "port": 8080}], "healthChecks": [{"type": "http", "path": ""}]}"#;
        let issues = check_cluster_required_fields("test-cluster", config);
        assert!(
            issues.iter().any(|i| i["message"].as_str().unwrap_or("").contains("empty path")),
            "empty HTTP health check path should be detected"
        );
    }

    #[test]
    fn test_check_cluster_required_fields_timeout_exceeds_interval() {
        let config = r#"{"endpoints": [{"host": "127.0.0.1", "port": 8080}], "healthChecks": [{"type": "http", "path": "/health", "timeout_seconds": 10, "interval_seconds": 5}]}"#;
        let issues = check_cluster_required_fields("test-cluster", config);
        assert!(
            issues.iter().any(|i| i["message"].as_str().unwrap_or("").contains("timeout")),
            "timeout > interval should be detected"
        );
    }

    #[test]
    fn test_check_cluster_required_fields_malformed_json() {
        let issues = check_cluster_required_fields("test-cluster", "not json");
        assert_eq!(issues.len(), 1, "malformed JSON should produce 1 error");
        assert_eq!(issues[0]["category"], "proto_violation");
        assert!(issues[0]["message"].as_str().unwrap_or("").contains("malformed"));
    }

    // ========================================================================
    // Enhanced ops_config_validate: proto violations + xds_delivery
    // ========================================================================

    #[tokio::test]
    async fn test_execute_ops_config_validate_detects_health_check_violations() {
        let db = test_db("ops_validate_hc").await;

        // Insert a cluster with missing thresholds into team-a
        let config_missing_thresholds = r#"{"endpoints": [{"host": "127.0.0.1", "port": 8080}], "healthChecks": [{"type": "http", "path": "/health"}]}"#;
        sqlx::query(
            "INSERT INTO clusters (id, name, service_name, configuration, version, team, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, 1, $5, NOW(), NOW())",
        )
        .bind("hc-bad-cluster-id")
        .bind("hc-bad-cluster")
        .bind("hc-svc")
        .bind(config_missing_thresholds)
        .bind(TEAM_A_ID)
        .execute(&db.pool)
        .await
        .expect("insert cluster");

        let result = execute_ops_config_validate(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");

        let issues = output["issues"].as_array().unwrap();
        let proto_issues: Vec<&Value> =
            issues.iter().filter(|i| i["category"] == "proto_violation").collect();
        assert!(
            proto_issues.len() >= 2,
            "should detect missing healthy_threshold and unhealthy_threshold"
        );
        assert!(proto_issues.iter().all(|i| i["severity"] == "error"));
        assert!(proto_issues.iter().any(|i| i["resource"] == "hc-bad-cluster"));
    }

    #[tokio::test]
    async fn test_execute_ops_config_validate_includes_recent_nacks() {
        let db = test_db("ops_validate_nacks").await;

        // Insert a NACK event for team-a
        insert_nack_event(
            &db.pool,
            TEAM_A_ID,
            "dp-1",
            "type.googleapis.com/envoy.config.cluster.v3.Cluster",
            "test nack error",
        )
        .await;

        let result = execute_ops_config_validate(&db.pool, TEAM_A_ID, None, json!({}))
            .await
            .expect("should succeed");

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
            _ => panic!("expected text content"),
        };
        let output: Value = serde_json::from_str(text).expect("should be valid JSON");

        let issues = output["issues"].as_array().unwrap();
        let delivery_issues: Vec<&Value> =
            issues.iter().filter(|i| i["category"] == "xds_delivery").collect();
        assert!(!delivery_issues.is_empty(), "should include recent NACK as xds_delivery warning");
        assert!(delivery_issues
            .iter()
            .any(|i| i["message"].as_str().unwrap_or("").contains("test nack error")));
    }
}
