//! Ops service layer — typed business logic for gateway diagnostics.
//!
//! These functions encapsulate the read-only diagnostic operations used by both
//! MCP tool handlers and REST API handlers. They accept typed parameters (no
//! serde_json::Value parsing) and return structured results.
//!
//! ## Team parameter
//!
//! The `team` parameter in all functions is a **team ID** (UUID string), not a
//! team name. Resource tables (clusters, listeners, route_configs, dataplanes)
//! store team IDs since migration 20260207000002. The `xds_nack_events` table
//! also uses the same value passed through from the MCP/REST layer.
//!
//! Callers (MCP handlers, REST handlers) are responsible for resolving team
//! names to IDs before calling these functions.

use crate::storage::repositories::{
    AuditLogFilters, AuditLogRepository, ClusterRepository, DataplaneRepository,
    NackEventRepository, ReportingRepository,
};
use crate::storage::DbPool;
use crate::xds::{ClusterSpec, HealthCheckSpec};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use tracing::instrument;

// =============================================================================
// Result types
// =============================================================================

/// Result of tracing a request path through the gateway.
#[derive(Debug, Clone, Serialize)]
pub struct TraceRequestResult {
    pub path: String,
    pub port: Option<i64>,
    pub match_count: usize,
    pub matches: Vec<Value>,
    pub endpoints: Vec<Value>,
    pub unmatched_reason: Option<String>,
    pub message: String,
}

/// Result of querying gateway topology.
#[derive(Debug, Clone, Serialize)]
pub struct TopologyServiceResult {
    pub scope: String,
    pub name: Option<String>,
    pub rows: Option<Vec<Value>>,
    pub orphan_clusters: Vec<Value>,
    pub orphan_route_configs: Vec<Value>,
    pub summary: Value,
    pub truncated: bool,
    pub message: String,
}

/// A single validation issue found during config validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub severity: String,
    pub category: String,
    pub message: String,
    pub resource: String,
}

/// Result of validating gateway configuration.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigValidationResult {
    pub valid: bool,
    pub issues: Vec<ValidationIssue>,
    pub summary: ValidationSummary,
    pub next_step: String,
}

/// Summary counts for config validation.
#[derive(Debug, Clone, Serialize)]
pub struct ValidationSummary {
    pub total_issues: usize,
    pub warnings: usize,
    pub errors: usize,
    pub proto_violations: usize,
    pub recent_nacks: usize,
    pub listeners: i64,
    pub route_configs: i64,
    pub clusters: i64,
    pub routes: i64,
}

/// Result of querying xDS delivery status.
#[derive(Debug, Clone, Serialize)]
pub struct XdsDeliveryStatusResult {
    pub dataplanes: Vec<Value>,
    pub summary: XdsDeliverySummary,
    pub message: String,
    pub next_step: String,
}

/// Summary of xDS delivery health.
#[derive(Debug, Clone, Serialize)]
pub struct XdsDeliverySummary {
    pub total_dataplanes: usize,
    pub healthy: usize,
    pub nacked: usize,
}

/// A single formatted NACK event.
///
/// `source` distinguishes a stream-side ADS NACK (`"stream"`) from a warming
/// failure scraped by the dataplane agent (`"warming_report"`). For warming
/// reports `nonce` and `version_rejected` are typically `None` — render them
/// as `-` in human output and `null` in JSON.
#[derive(Debug, Clone, Serialize)]
pub struct NackEventFormatted {
    pub timestamp: String,
    pub dataplane_name: String,
    pub resource_type: String,
    pub error_message: String,
    pub error_code: i64,
    pub resource_names: Option<Vec<String>>,
    pub version_rejected: Option<String>,
    pub nonce: Option<String>,
    pub source: String,
}

/// Result of querying NACK history.
#[derive(Debug, Clone, Serialize)]
pub struct NackHistoryResult {
    pub events: Vec<NackEventFormatted>,
    pub count: usize,
    pub message: String,
}

/// A single PII-stripped audit log summary.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntrySummary {
    pub id: i64,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub resource_name: Option<String>,
    pub action: String,
    pub created_at: String,
}

/// Result of querying audit logs.
#[derive(Debug, Clone, Serialize)]
pub struct AuditQueryResult {
    pub entries: Vec<AuditEntrySummary>,
    pub count: usize,
    pub message: String,
}

/// Ops service error type.
#[derive(Debug, thiserror::Error)]
pub enum OpsServiceError {
    #[error("Invalid parameter: {0}")]
    InvalidParam(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

// =============================================================================
// Helper functions
// =============================================================================

/// Default flowplane-agent diagnostics poll interval (seconds). Reports older
/// than `2 * AGENT_POLL_INTERVAL_SECS` are considered stale.
pub(crate) const AGENT_POLL_INTERVAL_SECS: i64 = 30;

/// Classify the agent liveness signal for a dataplane based on its most recent
/// `last_config_verify` timestamp.
///
/// - `NOT_MONITORED` — no agent has ever reported (`last_config_verify` is `None`).
/// - `STALE`         — most recent report is older than `2 * poll_interval_secs`.
/// - `OK`            — most recent report is within `2 * poll_interval_secs`.
///
/// `now` is passed in so callers can drive the function deterministically from
/// tests; production callers pass `chrono::Utc::now()`.
pub(crate) fn classify_agent_status(
    last_config_verify: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    poll_interval_secs: i64,
) -> &'static str {
    match last_config_verify {
        None => "NOT_MONITORED",
        Some(ts) => {
            let age = now.signed_duration_since(ts);
            if age <= chrono::Duration::seconds(poll_interval_secs.saturating_mul(2)) {
                "OK"
            } else {
                "STALE"
            }
        }
    }
}

/// Map a full xDS type URL to a short resource type label.
fn type_url_to_label(type_url: &str) -> &str {
    if type_url.contains("Cluster") && !type_url.contains("ClusterLoadAssignment") {
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

/// Expand short-form type labels to full xDS type URLs.
fn expand_type_url(input: &str) -> String {
    match input.to_uppercase().as_str() {
        "CDS" => "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
        "RDS" => "type.googleapis.com/envoy.config.route.v3.RouteConfiguration".to_string(),
        "LDS" => "type.googleapis.com/envoy.config.listener.v3.Listener".to_string(),
        "EDS" => "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment".to_string(),
        _ => input.to_string(),
    }
}

/// Check health check thresholds in a cluster's configuration JSON.
fn check_health_check_thresholds(cluster_name: &str, configuration: &str) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    let spec: ClusterSpec = match serde_json::from_str(configuration) {
        Ok(s) => s,
        Err(_) => return issues,
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
            issues.push(ValidationIssue {
                severity: "error".to_string(),
                category: "proto_violation".to_string(),
                message: format!(
                    "Cluster '{}' health check #{}: missing or zero healthy_threshold — Envoy will NACK this config",
                    cluster_name, i + 1
                ),
                resource: cluster_name.to_string(),
            });
        }

        if unhealthy_threshold.is_none() || *unhealthy_threshold == Some(0) {
            issues.push(ValidationIssue {
                severity: "error".to_string(),
                category: "proto_violation".to_string(),
                message: format!(
                    "Cluster '{}' health check #{}: missing or zero unhealthy_threshold — Envoy will NACK this config",
                    cluster_name, i + 1
                ),
                resource: cluster_name.to_string(),
            });
        }
    }

    issues
}

/// Check required fields that Envoy enforces at the proto level.
fn check_cluster_required_fields(cluster_name: &str, configuration: &str) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    let spec: ClusterSpec = match serde_json::from_str(configuration) {
        Ok(s) => s,
        Err(_) => {
            issues.push(ValidationIssue {
                severity: "error".to_string(),
                category: "proto_violation".to_string(),
                message: format!(
                    "Cluster '{}': configuration JSON is malformed — Envoy cannot parse this",
                    cluster_name
                ),
                resource: cluster_name.to_string(),
            });
            return issues;
        }
    };

    if spec.endpoints.is_empty() {
        issues.push(ValidationIssue {
            severity: "error".to_string(),
            category: "proto_violation".to_string(),
            message: format!(
                "Cluster '{}': no endpoints defined — Envoy will have no backends to route to",
                cluster_name
            ),
            resource: cluster_name.to_string(),
        });
    }

    for (i, ep) in spec.endpoints.iter().enumerate() {
        if ep.to_host_port().is_none() {
            issues.push(ValidationIssue {
                severity: "error".to_string(),
                category: "proto_violation".to_string(),
                message: format!(
                    "Cluster '{}' endpoint #{} '{}': invalid format — must be 'host:port'",
                    cluster_name,
                    i + 1,
                    ep
                ),
                resource: cluster_name.to_string(),
            });
        }
    }

    for (i, hc) in spec.health_checks.iter().enumerate() {
        if let HealthCheckSpec::Http { path, interval_seconds, timeout_seconds, .. } = hc {
            if path.is_empty() {
                issues.push(ValidationIssue {
                    severity: "error".to_string(),
                    category: "proto_violation".to_string(),
                    message: format!(
                        "Cluster '{}' health check #{}: HTTP health check has empty path — Envoy requires a path (e.g., '/healthz')",
                        cluster_name, i + 1
                    ),
                    resource: cluster_name.to_string(),
                });
            }

            if let (Some(timeout), Some(interval)) = (timeout_seconds, interval_seconds) {
                if timeout > interval {
                    issues.push(ValidationIssue {
                        severity: "warning".to_string(),
                        category: "proto_violation".to_string(),
                        message: format!(
                            "Cluster '{}' health check #{}: timeout ({}s) exceeds interval ({}s) — health checks may overlap or Envoy may reject",
                            cluster_name, i + 1, timeout, interval
                        ),
                        resource: cluster_name.to_string(),
                    });
                }
            }
        }
    }

    issues
}

// =============================================================================
// Service functions
// =============================================================================

/// Trace a request path through the gateway chain.
///
/// Returns the full listener → route_config → virtual_host → route → cluster chain
/// for every matching route, plus the backend endpoints.
#[instrument(skip(pool), fields(team = %team, path = %path), name = "ops_svc_trace_request")]
pub async fn trace_request(
    pool: &DbPool,
    team: &str,
    path: &str,
    port: Option<i64>,
) -> Result<TraceRequestResult, OpsServiceError> {
    // Input validation
    if path.len() > 2048 {
        return Err(OpsServiceError::InvalidParam(
            "'path' exceeds maximum length of 2048 characters".to_string(),
        ));
    }
    if !path.starts_with('/') {
        return Err(OpsServiceError::InvalidParam("'path' must start with '/'".to_string()));
    }
    if path.contains('\0') {
        return Err(OpsServiceError::InvalidParam(
            "'path' must not contain null bytes".to_string(),
        ));
    }

    let repo = ReportingRepository::new(pool.clone());
    let result = repo
        .trace_request_path(team, path, port)
        .await
        .map_err(|e| OpsServiceError::Internal(format!("Failed to trace request path: {}", e)))?;

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

    // Serialize matches and endpoints to Value for the result
    let matches: Vec<Value> = result
        .matches
        .iter()
        .map(|m| {
            serde_json::to_value(m)
                .map_err(|e| OpsServiceError::Internal(format!("Serialization error: {}", e)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let endpoints: Vec<Value> = result
        .endpoints
        .iter()
        .map(|e| {
            serde_json::to_value(e)
                .map_err(|e| OpsServiceError::Internal(format!("Serialization error: {}", e)))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(TraceRequestResult {
        path: path.to_string(),
        port,
        match_count: result.matches.len(),
        matches,
        endpoints,
        unmatched_reason,
        message,
    })
}

/// View the full gateway topology.
///
/// Returns resource counts, orphan detection, and optionally the full topology rows.
#[instrument(skip(pool), fields(team = %team), name = "ops_svc_topology")]
pub async fn topology(
    pool: &DbPool,
    team: &str,
    scope: Option<&str>,
    name: Option<&str>,
    limit: Option<i64>,
    include_details: bool,
) -> Result<TopologyServiceResult, OpsServiceError> {
    let repo = ReportingRepository::new(pool.clone());
    let result = repo
        .full_topology(team, scope, name, limit)
        .await
        .map_err(|e| OpsServiceError::Internal(format!("Failed to get topology: {}", e)))?;

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

    let rows = if include_details {
        Some(
            result
                .rows
                .iter()
                .map(|r| {
                    serde_json::to_value(r).map_err(|e| {
                        OpsServiceError::Internal(format!("Serialization error: {}", e))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        )
    } else {
        None
    };

    let orphan_clusters: Vec<Value> = result
        .orphan_clusters
        .iter()
        .map(|o| {
            serde_json::to_value(o)
                .map_err(|e| OpsServiceError::Internal(format!("Serialization error: {}", e)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let orphan_route_configs: Vec<Value> = result
        .orphan_route_configs
        .iter()
        .map(|o| {
            serde_json::to_value(o)
                .map_err(|e| OpsServiceError::Internal(format!("Serialization error: {}", e)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let summary = serde_json::to_value(&result.summary)
        .map_err(|e| OpsServiceError::Internal(format!("Serialization error: {}", e)))?;

    Ok(TopologyServiceResult {
        scope: scope.unwrap_or("full").to_string(),
        name: name.map(|n| n.to_string()),
        rows,
        orphan_clusters,
        orphan_route_configs,
        summary,
        truncated: result.truncated,
        message,
    })
}

/// Validate gateway configuration and detect problems.
///
/// Checks for orphan resources, proto violations, and recent xDS NACKs.
#[instrument(skip(pool), fields(team = %team), name = "ops_svc_config_validate")]
pub async fn config_validate(
    pool: &DbPool,
    team: &str,
) -> Result<ConfigValidationResult, OpsServiceError> {
    let repo = ReportingRepository::new(pool.clone());
    let cluster_repo = ClusterRepository::new(pool.clone());
    let nack_repo = NackEventRepository::new(pool.clone());

    // Run independent queries concurrently
    let team_filter = [team.to_string()];
    let (topology_result, clusters, recent_nacks) = tokio::join!(
        repo.full_topology(team, None, None, Some(200)),
        cluster_repo.list_by_teams(&team_filter, true, Some(500), Some(0)),
        nack_repo.list_recent(team, None, Some(5)),
    );

    let topology_result = topology_result
        .map_err(|e| OpsServiceError::Internal(format!("Failed to validate config: {}", e)))?;
    let clusters = clusters.map_err(|e| {
        OpsServiceError::Internal(format!("Failed to list clusters for validation: {}", e))
    })?;
    let recent_nacks = recent_nacks
        .map_err(|e| OpsServiceError::Internal(format!("Failed to query recent NACKs: {}", e)))?;

    let mut issues: Vec<ValidationIssue> = Vec::new();

    // Check for orphan clusters
    for orphan in &topology_result.orphan_clusters {
        issues.push(ValidationIssue {
            severity: "warning".to_string(),
            category: "orphan_cluster".to_string(),
            message: format!(
                "Cluster '{}' (service: {}) is not referenced by any route_config — it receives no traffic",
                orphan.name, orphan.service_name
            ),
            resource: orphan.name.clone(),
        });
    }

    // Check for orphan route configs
    for orphan in &topology_result.orphan_route_configs {
        issues.push(ValidationIssue {
            severity: "warning".to_string(),
            category: "orphan_route_config".to_string(),
            message: format!(
                "Route config '{}' (path: {}, cluster: {}) is not bound to any listener — its routes are unreachable",
                orphan.name, orphan.path_prefix, orphan.cluster_name
            ),
            resource: orphan.name.clone(),
        });
    }

    // Check for listeners with no route configs (O(1) dedup via HashSet)
    let mut seen_empty_listeners: HashSet<&str> = HashSet::new();
    for row in &topology_result.rows {
        if row.route_config_name.is_none() && seen_empty_listeners.insert(&row.listener_name) {
            issues.push(ValidationIssue {
                severity: "warning".to_string(),
                category: "empty_listener".to_string(),
                message: format!(
                    "Listener '{}' ({}:{}) has no route configs bound — it cannot route traffic",
                    row.listener_name,
                    row.listener_address,
                    row.listener_port.map(|p| p.to_string()).unwrap_or_else(|| "?".to_string())
                ),
                resource: row.listener_name.clone(),
            });
        }
    }

    // Proto violation checks
    for cluster in &clusters {
        let mut hc_issues = check_health_check_thresholds(&cluster.name, &cluster.configuration);
        issues.append(&mut hc_issues);

        let mut rf_issues = check_cluster_required_fields(&cluster.name, &cluster.configuration);
        issues.append(&mut rf_issues);
    }

    for nack in &recent_nacks {
        let resource_names: Option<Vec<String>> =
            nack.resource_names.as_ref().and_then(|r| serde_json::from_str(r).ok());

        issues.push(ValidationIssue {
            severity: "warning".to_string(),
            category: "xds_delivery".to_string(),
            message: format!(
                "NACK from '{}' on {} at {}: {}",
                nack.dataplane_name,
                type_url_to_label(&nack.type_url),
                nack.created_at.to_rfc3339(),
                nack.error_message
            ),
            resource: resource_names
                .as_ref()
                .and_then(|r| r.first().cloned())
                .unwrap_or_else(|| nack.dataplane_name.clone()),
        });
    }

    let warning_count = issues.iter().filter(|i| i.severity == "warning").count();
    let error_count = issues.iter().filter(|i| i.severity == "error").count();
    let proto_violation_count = issues.iter().filter(|i| i.category == "proto_violation").count();
    let nack_count = recent_nacks.len();
    let valid = issues.is_empty();
    let total_issues = issues.len();

    let next_step = if error_count > 0 {
        "Fix proto_violation errors first — these will cause Envoy to NACK the config. Use cp_get_cluster to inspect affected clusters, then cp_update_cluster to fix.".to_string()
    } else if nack_count > 0 {
        "Recent NACKs detected. Use ops_nack_history to see full error details, then ops_xds_delivery_status to check current delivery state.".to_string()
    } else if warning_count > 0 {
        "Warnings found (orphan resources or connectivity gaps). Use ops_topology to see the full resource graph and identify unused resources.".to_string()
    } else {
        "Configuration looks good. No issues detected.".to_string()
    };

    Ok(ConfigValidationResult {
        valid,
        issues,
        summary: ValidationSummary {
            total_issues,
            warnings: warning_count,
            errors: error_count,
            proto_violations: proto_violation_count,
            recent_nacks: nack_count,
            listeners: topology_result.summary.listener_count,
            route_configs: topology_result.summary.route_config_count,
            clusters: topology_result.summary.cluster_count,
            routes: topology_result.summary.route_count,
        },
        next_step,
    })
}

/// Query xDS delivery status per dataplane.
#[instrument(skip(pool), fields(team = %team), name = "ops_svc_xds_delivery_status")]
pub async fn xds_delivery_status(
    pool: &DbPool,
    team: &str,
    dataplane_name: Option<&str>,
) -> Result<XdsDeliveryStatusResult, OpsServiceError> {
    let dataplane_repo = DataplaneRepository::new(pool.clone());
    let nack_repo = NackEventRepository::new(pool.clone());

    let dataplanes = match dataplane_name {
        Some(name) => match dataplane_repo.get_by_name(team, name).await {
            Ok(Some(dp)) => vec![dp],
            Ok(None) => vec![],
            Err(e) => {
                return Err(OpsServiceError::Internal(format!("Failed to get dataplane: {}", e)));
            }
        },
        None => dataplane_repo
            .list_by_team(team, Some(100), Some(0))
            .await
            .map_err(|e| OpsServiceError::Internal(format!("Failed to list dataplanes: {}", e)))?,
    };

    let known_types = ["CDS", "RDS", "LDS"];
    let mut dataplane_statuses = Vec::new();
    let mut nack_count = 0;

    for dp in &dataplanes {
        let latest_nacks = nack_repo.latest_per_type_url(team, &dp.name).await.map_err(|e| {
            OpsServiceError::Internal(format!(
                "Failed to get NACK status for dataplane '{}': {}",
                dp.name, e
            ))
        })?;

        let mut resource_types = json!({});
        let mut has_nack = false;

        let mut nack_map = std::collections::HashMap::new();
        for nack in &latest_nacks {
            let label = type_url_to_label(&nack.type_url);
            nack_map.insert(label.to_string(), nack);
        }

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

        let agent_status = classify_agent_status(
            dp.last_config_verify,
            chrono::Utc::now(),
            AGENT_POLL_INTERVAL_SECS,
        );
        let last_config_verify_str = dp.last_config_verify.map(|ts| ts.to_rfc3339());

        dataplane_statuses.push(json!({
            "name": dp.name,
            "resource_types": resource_types,
            "last_config_verify": last_config_verify_str,
            "agent_status": agent_status,
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
        "Create a dataplane first, then bootstrap an Envoy instance to connect to it.".to_string()
    } else if nack_count > 0 {
        "Use ops_nack_history to see full NACK details, then ops_config_validate to find the root cause.".to_string()
    } else {
        "All dataplanes are healthy. No action needed.".to_string()
    };

    Ok(XdsDeliveryStatusResult {
        dataplanes: dataplane_statuses,
        summary: XdsDeliverySummary { total_dataplanes: total, healthy, nacked: nack_count },
        message,
        next_step,
    })
}

/// Query recent NACK event history.
#[instrument(skip(pool), fields(team = %team), name = "ops_svc_nack_history")]
pub async fn nack_history(
    pool: &DbPool,
    team: &str,
    dataplane_name: Option<&str>,
    type_url: Option<&str>,
    since: Option<&str>,
    limit: Option<i64>,
) -> Result<NackHistoryResult, OpsServiceError> {
    let limit_i32 = limit.map(|v| (v as i32).min(100)).unwrap_or(10);

    // Parse `since` timestamp if provided
    let since_parsed: Option<DateTime<Utc>> = match since {
        Some(s) => {
            let parsed: DateTime<Utc> = s.parse().map_err(|_| {
                OpsServiceError::InvalidParam(format!(
                    "Invalid 'since' timestamp '{}' — expected ISO 8601 format (e.g., 2026-02-25T00:00:00Z)",
                    s
                ))
            })?;
            Some(parsed)
        }
        None => None,
    };

    let nack_repo = NackEventRepository::new(pool.clone());

    let events = if let Some(dp_name) = dataplane_name {
        nack_repo
            .list_by_dataplane(team, dp_name, since_parsed, Some(limit_i32))
            .await
            .map_err(|e| OpsServiceError::Internal(format!("Failed to query NACK events: {}", e)))?
    } else if let Some(tu) = type_url {
        let full_url = expand_type_url(tu);
        nack_repo
            .list_by_type_url(team, &full_url, since_parsed, Some(limit_i32))
            .await
            .map_err(|e| OpsServiceError::Internal(format!("Failed to query NACK events: {}", e)))?
    } else {
        nack_repo
            .list_recent(team, since_parsed, Some(limit_i32))
            .await
            .map_err(|e| OpsServiceError::Internal(format!("Failed to query NACK events: {}", e)))?
    };

    let formatted: Vec<NackEventFormatted> = events
        .iter()
        .map(|e| {
            let resource_names: Option<Vec<String>> =
                e.resource_names.as_ref().and_then(|r| serde_json::from_str(r).ok());

            NackEventFormatted {
                timestamp: e.created_at.to_rfc3339(),
                dataplane_name: e.dataplane_name.clone(),
                resource_type: type_url_to_label(&e.type_url).to_string(),
                error_message: e.error_message.clone(),
                error_code: e.error_code,
                resource_names,
                version_rejected: e.version_rejected.clone(),
                nonce: e.nonce.clone(),
                source: e.source.as_str().to_string(),
            }
        })
        .collect();

    let count = formatted.len();
    let message = if count == 0 {
        "No NACK events found matching the filters".to_string()
    } else {
        format!("Found {} NACK event(s)", count)
    };

    Ok(NackHistoryResult { events: formatted, count, message })
}

/// Query recent audit log entries (PII-stripped summaries).
#[instrument(skip(pool), fields(team = %team), name = "ops_svc_audit_query")]
pub async fn audit_query(
    pool: &DbPool,
    team: &str,
    org_id: Option<&str>,
    resource_type: Option<&str>,
    action: Option<&str>,
    limit: Option<i64>,
) -> Result<AuditQueryResult, OpsServiceError> {
    let limit_i32 = limit.map(|v| (v as i32).min(100)).or(Some(20));

    let filters = AuditLogFilters {
        resource_type: resource_type.map(|s| s.to_string()),
        action: action.map(|s| s.to_string()),
        user_id: None,
        org_id: org_id.map(|o| o.to_string()),
        team_id: Some(team.to_string()),
        start_date: None,
        end_date: None,
    };

    let repo = AuditLogRepository::new(pool.clone());
    let entries = repo
        .query_logs(Some(filters), limit_i32, Some(0))
        .await
        .map_err(|e| OpsServiceError::Internal(format!("Failed to query audit logs: {}", e)))?;

    let summaries: Vec<AuditEntrySummary> = entries
        .into_iter()
        .map(|entry| AuditEntrySummary {
            id: entry.id,
            resource_type: entry.resource_type,
            resource_id: entry.resource_id,
            resource_name: entry.resource_name,
            action: entry.action,
            created_at: entry.created_at.to_rfc3339(),
        })
        .collect();

    let count = summaries.len();
    Ok(AuditQueryResult {
        entries: summaries,
        count,
        message: format!("Found {} audit log entries", count),
    })
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // classify_agent_status unit tests
    // ========================================================================

    #[test]
    fn classify_agent_status_returns_not_monitored_when_never_reported() {
        let now = Utc::now();
        assert_eq!(classify_agent_status(None, now, 30), "NOT_MONITORED");
    }

    #[test]
    fn classify_agent_status_returns_ok_when_recent() {
        let now = Utc::now();
        let recent = now - chrono::Duration::seconds(15);
        assert_eq!(classify_agent_status(Some(recent), now, 30), "OK");
    }

    #[test]
    fn classify_agent_status_returns_ok_at_exactly_two_intervals() {
        let now = Utc::now();
        let edge = now - chrono::Duration::seconds(60);
        assert_eq!(classify_agent_status(Some(edge), now, 30), "OK");
    }

    #[test]
    fn classify_agent_status_returns_stale_just_past_two_intervals() {
        let now = Utc::now();
        let stale = now - chrono::Duration::seconds(61);
        assert_eq!(classify_agent_status(Some(stale), now, 30), "STALE");
    }

    #[test]
    fn classify_agent_status_returns_stale_when_far_in_the_past() {
        let now = Utc::now();
        let ancient = now - chrono::Duration::days(7);
        assert_eq!(classify_agent_status(Some(ancient), now, 30), "STALE");
    }

    #[test]
    fn classify_agent_status_handles_clock_skew_future_timestamp_as_ok() {
        // A future timestamp (clock skew between agent and CP) should not be
        // misreported as STALE — the report is, if anything, fresher than now.
        let now = Utc::now();
        let future = now + chrono::Duration::seconds(5);
        assert_eq!(classify_agent_status(Some(future), now, 30), "OK");
    }

    #[test]
    fn classify_agent_status_respects_custom_poll_interval() {
        let now = Utc::now();
        let ts = now - chrono::Duration::seconds(150);
        // poll=10 → window=20 → 150s old is STALE
        assert_eq!(classify_agent_status(Some(ts), now, 10), "STALE");
        // poll=120 → window=240 → 150s old is OK
        assert_eq!(classify_agent_status(Some(ts), now, 120), "OK");
    }

    // ========================================================================
    // expand_type_url unit tests
    // ========================================================================

    #[test]
    fn test_expand_type_url_short_forms() {
        assert!(expand_type_url("CDS").contains("Cluster"));
        assert!(expand_type_url("cds").contains("Cluster"));
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
    // type_url_to_label unit tests
    // ========================================================================

    #[test]
    fn test_type_url_to_label() {
        assert_eq!(type_url_to_label("type.googleapis.com/envoy.config.cluster.v3.Cluster"), "CDS");
        assert_eq!(
            type_url_to_label("type.googleapis.com/envoy.config.route.v3.RouteConfiguration"),
            "RDS"
        );
        assert_eq!(
            type_url_to_label("type.googleapis.com/envoy.config.listener.v3.Listener"),
            "LDS"
        );
        assert_eq!(
            type_url_to_label("type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment"),
            "EDS"
        );
        assert_eq!(type_url_to_label("unknown.type"), "unknown.type");
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
        let config = r#"{"endpoints": [{"host": "127.0.0.1", "port": 8080}], "healthChecks": [{"type": "http", "path": "/health"}]}"#;
        let issues = check_health_check_thresholds("test-cluster", config);
        assert_eq!(issues.len(), 2, "missing both thresholds should produce 2 errors");
        assert!(issues.iter().all(|i| i.category == "proto_violation"));
        assert!(issues.iter().all(|i| i.severity == "error"));
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
        assert_eq!(issues[0].category, "proto_violation");
        assert_eq!(issues[0].severity, "error");
        assert!(issues[0].message.contains("no endpoints"));
    }

    #[test]
    fn test_check_cluster_required_fields_invalid_endpoint() {
        let config = r#"{"endpoints": ["not-a-valid-endpoint"]}"#;
        let issues = check_cluster_required_fields("test-cluster", config);
        assert!(
            issues.iter().any(|i| i.message.contains("invalid format")),
            "invalid endpoint format should be detected"
        );
    }

    #[test]
    fn test_check_cluster_required_fields_empty_http_hc_path() {
        let config = r#"{"endpoints": [{"host": "127.0.0.1", "port": 8080}], "healthChecks": [{"type": "http", "path": ""}]}"#;
        let issues = check_cluster_required_fields("test-cluster", config);
        assert!(
            issues.iter().any(|i| i.message.contains("empty path")),
            "empty HTTP health check path should be detected"
        );
    }

    #[test]
    fn test_check_cluster_required_fields_timeout_exceeds_interval() {
        let config = r#"{"endpoints": [{"host": "127.0.0.1", "port": 8080}], "healthChecks": [{"type": "http", "path": "/health", "timeout_seconds": 10, "interval_seconds": 5}]}"#;
        let issues = check_cluster_required_fields("test-cluster", config);
        assert!(
            issues.iter().any(|i| i.message.contains("timeout")),
            "timeout > interval should be detected"
        );
    }

    #[test]
    fn test_check_cluster_required_fields_malformed_json() {
        let issues = check_cluster_required_fields("test-cluster", "not json");
        assert_eq!(issues.len(), 1, "malformed JSON should produce 1 error");
        assert_eq!(issues[0].category, "proto_violation");
        assert!(issues[0].message.contains("malformed"));
    }
}
