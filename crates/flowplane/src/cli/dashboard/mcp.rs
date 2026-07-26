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
use super::resources::encode_segment;

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

/// A `Page<T>` envelope from the org-scoped identity reads. `items` is the current page and
/// `total` the full row count — used to surface a truncation notice when the fetch cap hides
/// rows. The `limit`/`offset` fields are ignored. A decode failure surfaces as Unavailable.
#[derive(Debug, Deserialize)]
struct Page<T> {
    items: Vec<T>,
    total: i64,
}

/// The org-scoped agents panel is fetched with an explicit `?limit=` cap; the CP still bounds it
/// (max 500), so an org with more agents than the cap would silently show only the first page.
/// Request the max and, whenever `total` exceeds what is shown, carry a truncation notice so the
/// dashboard says so out loud instead of implying it listed everyone. Mirrors the dataplane
/// panel's `Truncation` annotation (see `super::data`).
const AGENTS_FETCH_LIMIT: usize = 500;

fn truncation(shown: usize, total: i64) -> Option<super::data::Truncation> {
    (total > shown as i64).then_some(super::data::Truncation { shown, total })
}

#[derive(Debug, Deserialize)]
struct AgentItem {
    id: String,
    name: String,
    kind: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct AgentGrantItem {
    team_name: String,
    resource: String,
    action: String,
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

/// One row of the org-scoped agents panel. `id` is kept only to build the lazy
/// grants-expand link (`?id=<agent_id>`); no credential material exists on this row.
#[derive(Debug)]
pub(super) struct AgentRow {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) kind: String,
    pub(super) status: String,
}

/// One row of an agent's grants (fetched lazily on row expand). No credential material.
#[derive(Debug)]
pub(super) struct AgentGrantRow {
    pub(super) team_name: String,
    pub(super) resource: String,
    pub(super) action: String,
}

/// The agents panel payload: the (capped) rows plus an optional truncation notice when the org
/// holds more agents than the fetch cap surfaced.
#[derive(Debug)]
pub(super) struct AgentsPanel {
    pub(super) rows: Vec<AgentRow>,
    pub(super) truncated: Option<super::data::Truncation>,
}

/// One agent's grants panel payload: the (capped) rows plus an optional truncation notice.
#[derive(Debug)]
pub(super) struct AgentGrantsPanel {
    pub(super) rows: Vec<AgentGrantRow>,
    pub(super) truncated: Option<super::data::Truncation>,
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

/// Fetch the org-scoped agents list. This is an org-wide read — a caller without org
/// authority gets a 403, which the panel renders as `Unauthorized` with an org-authority
/// note (never a silently empty table). Same 401 → AuthExpired / decode-fail → Unavailable
/// discipline as the sibling MCP panels.
pub(super) async fn fetch_agents(client: &RestClient) -> Result<Panel<AgentsPanel>, AuthExpired> {
    let result = client
        .get_json(&format!("/api/v1/agents?limit={AGENTS_FETCH_LIMIT}"))
        .await;
    if is_unauthorized(&result) {
        return Err(AuthExpired);
    }
    let page: Page<AgentItem> = match result {
        Ok(value) => match serde_json::from_value(value) {
            Ok(parsed) => parsed,
            Err(_) => return Ok(Panel::Unavailable),
        },
        Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::FORBIDDEN => {
            return Ok(Panel::Unauthorized)
        }
        Err(_) => return Ok(Panel::Unavailable),
    };
    let rows: Vec<AgentRow> = page
        .items
        .into_iter()
        .map(|a| AgentRow {
            id: a.id,
            name: a.name,
            kind: a.kind,
            status: a.status,
        })
        .collect();
    let truncated = truncation(rows.len(), page.total);
    Ok(Panel::Data(AgentsPanel { rows, truncated }))
}

/// Fetch one agent's grants (lazily, on row expand). A 403 → `Unauthorized`; a 404
/// (unknown or cross-org agent) or any other failure → `Unavailable`, matching the
/// catch-all convention of the sibling reads (only an explicit 403 is `Unauthorized`).
pub(super) async fn fetch_agent_grants(
    client: &RestClient,
    agent_id: &str,
) -> Result<Panel<AgentGrantsPanel>, AuthExpired> {
    // The agent id arrives as a user-supplied query value; percent-encode it so a hostile
    // value (`/`, `?`, `#`, `%`, dot-segments) cannot alter which upstream path is requested.
    let seg = encode_segment(agent_id);
    let result = client
        .get_json(&format!(
            "/api/v1/agents/{seg}/grants?limit={AGENTS_FETCH_LIMIT}"
        ))
        .await;
    if is_unauthorized(&result) {
        return Err(AuthExpired);
    }
    let page: Page<AgentGrantItem> = match result {
        Ok(value) => match serde_json::from_value(value) {
            Ok(parsed) => parsed,
            Err(_) => return Ok(Panel::Unavailable),
        },
        Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::FORBIDDEN => {
            return Ok(Panel::Unauthorized)
        }
        Err(_) => return Ok(Panel::Unavailable),
    };
    let rows: Vec<AgentGrantRow> = page
        .items
        .into_iter()
        .map(|g| AgentGrantRow {
            team_name: g.team_name,
            resource: g.resource,
            action: g.action,
        })
        .collect();
    let truncated = truncation(rows.len(), page.total);
    Ok(Panel::Data(AgentGrantsPanel { rows, truncated }))
}

fn is_unauthorized(result: &Result<serde_json::Value, ReadError>) -> bool {
    matches!(
        result,
        Err(ReadError::Status { status, .. }) if *status == reqwest::StatusCode::UNAUTHORIZED
    )
}
