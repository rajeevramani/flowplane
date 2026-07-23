//! MCP tab (ui-f6a S5): the server status card, the node-local connections table, and the
//! tool catalog split into a CP-tools panel (static `cp_*`/`ops_*`) and an API-tools panel
//! (generated `api_*`), each row annotated with per-caller executability.
//!
//! Catalog fetch discipline (design "Dashboard MCP tab"): the tab requests
//! `include_disabled=true` so an operator with the `mcp-tools:update` grant sees disabled
//! generated tools marked as such; a caller lacking that grant gets a 403, which the tab
//! degrades to the enabled-only catalog with the disabled affordance hidden — a read-only
//! principal keeps the full enabled view rather than losing the panel.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::super::client::{ReadError, RestClient};
use super::data::{humanize_age, AuthExpired, Panel};

// =============================================================================================
// Upstream item shapes (typed decode; a decode failure surfaces as Unavailable, never a
// silently empty panel).
// =============================================================================================

#[derive(Debug, Deserialize)]
struct StatusItem {
    transport: String,
    preferred_protocol_version: String,
    #[serde(default)]
    supported_protocol_versions: Vec<String>,
    session_ttl_seconds: u64,
    active_sessions: usize,
    static_tool_count: usize,
    dynamic_enabled_tool_count: usize,
    sse_enabled: bool,
    resources_enabled: bool,
    prompts_enabled: bool,
    api_invocation_mode: String,
}

#[derive(Debug, Deserialize)]
struct ConnectionItem {
    connection_id: String,
    principal_kind: String,
    transport: String,
    sse: bool,
    age_seconds: u64,
    idle_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct CatalogItem {
    name: String,
    description: String,
    resource: String,
    action: String,
    risk: String,
    kind: String,
    enabled: bool,
    executable_by_caller: bool,
}

// =============================================================================================
// Rendered rows.
// =============================================================================================

#[derive(Debug)]
pub(super) struct ConnectionRow {
    pub(super) connection_id: String,
    pub(super) principal_kind: String,
    pub(super) transport: String,
    pub(super) sse: bool,
    pub(super) age: String,
    pub(super) idle: String,
}

#[derive(Debug)]
pub(super) struct ToolRow {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) resource: String,
    pub(super) action: String,
    pub(super) risk: String,
    pub(super) enabled: bool,
    pub(super) executable_by_caller: bool,
}

pub(super) struct StatusPanel {
    pub(super) transport: String,
    pub(super) preferred_protocol_version: String,
    pub(super) supported_protocol_versions: String,
    pub(super) session_ttl_seconds: u64,
    pub(super) active_sessions: usize,
    pub(super) static_tool_count: usize,
    pub(super) dynamic_enabled_tool_count: usize,
    pub(super) sse_enabled: bool,
    pub(super) resources_enabled: bool,
    pub(super) prompts_enabled: bool,
    pub(super) api_invocation_mode: String,
    /// Connections attributed to this control-plane node only (design: node-local label).
    pub(super) connections: Panel<Vec<ConnectionRow>>,
}

pub(super) struct ToolsPanel {
    /// Static `cp_*`/`ops_*` control-plane tools.
    pub(super) cp_tools: Vec<ToolRow>,
    /// Generated `api_*` tools.
    pub(super) api_tools: Vec<ToolRow>,
    /// True when the catalog was fetched with `include_disabled=true` (the caller holds
    /// `mcp-tools:update`); false when we fell back to the enabled-only view after a 403,
    /// in which case the disabled-tools affordance is hidden.
    pub(super) include_disabled: bool,
    /// Count of disabled generated tools (only ever > 0 when `include_disabled`).
    pub(super) disabled_count: usize,
}

/// Fetch the status card + node-local connections. A 401 aborts the whole tab (identity
/// gone); a 403 or other failure on either read degrades only its own sub-panel.
pub(super) async fn fetch_status(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
) -> Result<Panel<StatusPanel>, AuthExpired> {
    let status_result = client
        .get_json(&format!("/api/v1/teams/{team}/mcp/status"))
        .await;
    let conn_result = client
        .get_json(&format!("/api/v1/teams/{team}/mcp/connections"))
        .await;

    if is_unauthorized(&status_result) || is_unauthorized(&conn_result) {
        return Err(AuthExpired);
    }

    let status: StatusItem = match status_result {
        Ok(value) => match serde_json::from_value(value) {
            Ok(parsed) => parsed,
            Err(_) => return Ok(Panel::Unavailable),
        },
        Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::FORBIDDEN => {
            return Ok(Panel::Unauthorized)
        }
        Err(_) => return Ok(Panel::Unavailable),
    };

    let connections = match conn_result {
        Ok(value) => match serde_json::from_value::<Vec<ConnectionItem>>(value) {
            Ok(items) => Panel::Data(
                items
                    .into_iter()
                    .map(|c| ConnectionRow {
                        connection_id: c.connection_id,
                        principal_kind: c.principal_kind,
                        transport: c.transport,
                        sse: c.sse,
                        age: humanize_age(
                            now,
                            now - chrono::Duration::seconds(c.age_seconds as i64),
                        ),
                        idle: humanize_age(
                            now,
                            now - chrono::Duration::seconds(c.idle_seconds as i64),
                        ),
                    })
                    .collect(),
            ),
            Err(_) => Panel::Unavailable,
        },
        Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::FORBIDDEN => {
            Panel::Unauthorized
        }
        Err(_) => Panel::Unavailable,
    };

    Ok(Panel::Data(StatusPanel {
        transport: status.transport,
        preferred_protocol_version: status.preferred_protocol_version,
        supported_protocol_versions: status.supported_protocol_versions.join(", "),
        session_ttl_seconds: status.session_ttl_seconds,
        active_sessions: status.active_sessions,
        static_tool_count: status.static_tool_count,
        dynamic_enabled_tool_count: status.dynamic_enabled_tool_count,
        sse_enabled: status.sse_enabled,
        resources_enabled: status.resources_enabled,
        prompts_enabled: status.prompts_enabled,
        api_invocation_mode: status.api_invocation_mode,
        connections,
    }))
}

/// Fetch the tool catalog, preferring the `include_disabled=true` view and degrading to the
/// enabled-only view on a 403 (caller lacks `mcp-tools:update`).
pub(super) async fn fetch_tools(
    client: &RestClient,
    team: &str,
) -> Result<Panel<ToolsPanel>, AuthExpired> {
    let with_disabled = client
        .get_json(&format!(
            "/api/v1/teams/{team}/mcp/tools?include_disabled=true"
        ))
        .await;

    // Fall back to the enabled-only catalog when include_disabled is refused (403 on the
    // mcp-tools:update gate) — a read-only principal keeps the full enabled view.
    let (result, include_disabled) = match with_disabled {
        Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::FORBIDDEN => (
            client
                .get_json(&format!("/api/v1/teams/{team}/mcp/tools"))
                .await,
            false,
        ),
        other => (other, true),
    };

    if is_unauthorized(&result) {
        return Err(AuthExpired);
    }

    let items: Vec<CatalogItem> = match result {
        Ok(value) => match serde_json::from_value(value) {
            Ok(parsed) => parsed,
            Err(_) => return Ok(Panel::Unavailable),
        },
        // A 403 here (with include_disabled already false) means no mcp-tools:read at all.
        Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::FORBIDDEN => {
            return Ok(Panel::Unauthorized)
        }
        Err(_) => return Ok(Panel::Unavailable),
    };

    let mut cp_tools = Vec::new();
    let mut api_tools = Vec::new();
    let mut disabled_count = 0_usize;
    for item in items {
        if !item.enabled {
            disabled_count += 1;
        }
        let row = ToolRow {
            name: item.name,
            description: item.description,
            resource: item.resource,
            action: item.action,
            risk: item.risk,
            enabled: item.enabled,
            executable_by_caller: item.executable_by_caller,
        };
        if item.kind == "dynamic" {
            api_tools.push(row);
        } else {
            cp_tools.push(row);
        }
    }

    Ok(Panel::Data(ToolsPanel {
        cp_tools,
        api_tools,
        include_disabled,
        disabled_count,
    }))
}

fn is_unauthorized(result: &Result<serde_json::Value, ReadError>) -> bool {
    matches!(
        result,
        Err(ReadError::Status { status, .. }) if *status == reqwest::StatusCode::UNAUTHORIZED
    )
}
