//! APIs tab (ui-f4 S7): the API-lifecycle read model rendered from the CP's read
//! endpoints — enriched definition list, per-API detail with state pill, verbatim
//! review-event pipeline, spec lineage, Envoy chain (typed binding IDs joined to
//! route-config/listener names), and generated tools including disabled rows.
//!
//! Everything here is a read-only projection of CP responses: the pill and pipeline
//! render what the history says (all five persisted decisions defensively), never a
//! synthesized state machine.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use super::super::client::RestClient;
use super::data::{humanize_age, AuthExpired, Panel};
use super::resources::{
    encode_segment, sweep, to_panel, PartialNotice, Sweep, SweepFailure, Table, SWEEP_BYTE_BUDGET,
};

// =============================================================================================
// Definition list panel.
// =============================================================================================

#[derive(Debug, Deserialize)]
struct ApiListItem {
    name: String,
    #[serde(default)]
    display_name: String,
    tool_count: i64,
    route_binding_count: i64,
    #[serde(default)]
    latest_version: Option<i64>,
    #[serde(default)]
    published_version: Option<i64>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug)]
pub(super) struct ApiRow {
    pub(super) name: String,
    pub(super) display_name: String,
    pub(super) tool_count: i64,
    pub(super) route_binding_count: i64,
    pub(super) versions: String,
    pub(super) updated: String,
    pub(super) unparsed: bool,
}

fn api_row(item: Value, now: DateTime<Utc>) -> ApiRow {
    match serde_json::from_value::<ApiListItem>(item.clone()) {
        Ok(item) => ApiRow {
            versions: match (item.published_version, item.latest_version) {
                (Some(p), Some(l)) if p == l => format!("published v{p}"),
                (Some(p), Some(l)) => format!("published v{p} · latest v{l}"),
                (None, Some(l)) => format!("latest v{l} (unpublished)"),
                _ => "no spec".into(),
            },
            display_name: if item.display_name.is_empty() {
                "—".into()
            } else {
                item.display_name
            },
            name: item.name,
            tool_count: item.tool_count,
            route_binding_count: item.route_binding_count,
            updated: humanize_age(now, item.updated_at),
            unparsed: false,
        },
        Err(_) => ApiRow {
            name: item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unknown)")
                .to_string(),
            display_name: String::new(),
            tool_count: 0,
            route_binding_count: 0,
            versions: String::new(),
            updated: String::new(),
            unparsed: true,
        },
    }
}

pub(super) async fn fetch_apis(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
) -> Result<Panel<Table<ApiRow>>, AuthExpired> {
    let result = sweep(client, team, "api-definitions", SWEEP_BYTE_BUDGET).await;
    to_panel(result, "api definitions", |item| api_row(item, now))
}

// =============================================================================================
// Per-API detail panel.
// =============================================================================================

/// State pill derived 1:1 from `(published_version, latest_version, latest event of the
/// latest version)` — the tuple space is enumerated in the unit tests below. Decisions
/// render verbatim; a version with no review events renders "(no events)", which is a
/// fact about the history, not an invented lifecycle state.
pub(super) fn state_pill(
    published_version: Option<i64>,
    latest_version: Option<i64>,
    latest_decision: Option<&str>,
) -> String {
    match (published_version, latest_version) {
        (_, None) => "no spec".into(),
        (Some(p), Some(l)) if p == l => format!("published v{p}"),
        (Some(p), Some(l)) => match latest_decision {
            Some(d) => format!("published v{p} · v{l} {d}"),
            None => format!("published v{p} · v{l} (no events)"),
        },
        (None, Some(l)) => match latest_decision {
            Some(d) => format!("v{l} {d}"),
            None => format!("v{l} (no events)"),
        },
    }
}

#[derive(Debug, Deserialize)]
struct SpecListItem {
    id: String,
    version: i64,
    source_kind: String,
    spec_hash: String,
    #[serde(default)]
    latest_decision: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub(super) struct LineageRow {
    pub(super) version: i64,
    pub(super) source_kind: String,
    pub(super) hash_short: String,
    pub(super) decision: String,
    pub(super) published: bool,
    pub(super) created: String,
}

#[derive(Debug, Deserialize)]
struct EventItem {
    decision: String,
    actor_type: String,
    #[serde(default)]
    reason: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub(super) struct EventRow {
    pub(super) decision: String,
    pub(super) actor_type: String,
    pub(super) reason: String,
    pub(super) created: String,
}

#[derive(Debug, Deserialize)]
struct BindingItem {
    name: String,
    route_config_id: String,
    #[serde(default)]
    listener_id: Option<String>,
    #[serde(default)]
    virtual_host: Option<String>,
    #[serde(default)]
    route: Option<String>,
}

#[derive(Debug)]
pub(super) struct ChainRow {
    pub(super) binding: String,
    pub(super) route_config: String,
    pub(super) listener: String,
    pub(super) scope: String,
}

#[derive(Debug, Deserialize)]
struct ToolItem {
    name: String,
    operation_id: String,
    method: String,
    path: String,
    enabled: bool,
    input_schema: Value,
    output_schema: Value,
}

#[derive(Debug)]
pub(super) struct ToolRow {
    pub(super) name: String,
    pub(super) operation_id: String,
    pub(super) method: String,
    pub(super) path: String,
    pub(super) enabled: bool,
    pub(super) input_schema: String,
    pub(super) output_schema: String,
}

pub(super) struct ApiDetail {
    pub(super) api: String,
    pub(super) pill: String,
    /// Event history of the latest version, oldest first, verbatim. `None` means the
    /// events fetch FAILED — rendered as an explicit unavailable notice, never as an
    /// empty history (fail closed, no silent partial).
    pub(super) events: Option<Vec<EventRow>>,
    pub(super) latest_version: Option<i64>,
    pub(super) lineage: Vec<LineageRow>,
    pub(super) chain: Vec<ChainRow>,
    /// True when a route-config/listener join sweep failed: chain rows show raw IDs
    /// under an explicit notice instead of resolved names.
    pub(super) chain_names_unresolved: bool,
    pub(super) tools: Vec<ToolRow>,
    pub(super) notices: Vec<PartialNotice>,
}

fn short_id(id: &str) -> String {
    id.get(..8).unwrap_or(id).to_string()
}

/// Terminal panel state for a failed sweep (small, so `Result` stays cheap; the caller
/// maps it onto `Panel<ApiDetail>`).
enum PanelState {
    Unauthorized,
    Unavailable,
}

impl PanelState {
    fn into_panel(self) -> Panel<ApiDetail> {
        match self {
            Self::Unauthorized => Panel::Unauthorized,
            Self::Unavailable => Panel::Unavailable,
        }
    }
}

fn sweep_or_empty(
    result: Result<Sweep, SweepFailure>,
    collection: &'static str,
    notices: &mut Vec<PartialNotice>,
) -> Result<Vec<Value>, PanelState> {
    match result {
        Ok(sweep) => {
            if let Some(reason) = sweep.partial {
                notices.push(PartialNotice {
                    shown: sweep.items.len(),
                    total: sweep.total,
                    reason,
                    collection,
                });
            }
            Ok(sweep.items)
        }
        // AuthExpired is intercepted by every caller BEFORE this helper runs.
        Err(SweepFailure::AuthExpired) => Err(PanelState::Unavailable),
        Err(SweepFailure::Unauthorized) => Err(PanelState::Unauthorized),
        Err(SweepFailure::Unavailable) => Err(PanelState::Unavailable),
    }
}

pub(super) async fn fetch_api_detail(
    client: &RestClient,
    team: &str,
    api: &str,
    now: DateTime<Utc>,
) -> Result<Panel<ApiDetail>, AuthExpired> {
    let seg = encode_segment(api);
    let mut notices = Vec::new();

    // The definition row: only needed for published_spec_version_id.
    let published_spec_id: Option<String> = match client
        .get_json_sized(&format!("/api/v1/teams/{team}/api-definitions/{seg}"))
        .await
    {
        Ok((value, _)) => value
            .get("published_spec_version_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        Err(super::super::client::ReadError::Status { status, .. })
            if status == reqwest::StatusCode::UNAUTHORIZED =>
        {
            return Err(AuthExpired)
        }
        Err(super::super::client::ReadError::Status { status, .. })
            if status == reqwest::StatusCode::FORBIDDEN =>
        {
            return Ok(Panel::Unauthorized)
        }
        Err(_) => return Ok(Panel::Unavailable),
    };

    let specs_result = sweep(
        client,
        team,
        &format!("api-definitions/{seg}/specs"),
        SWEEP_BYTE_BUDGET,
    )
    .await;
    if matches!(specs_result, Err(SweepFailure::AuthExpired)) {
        return Err(AuthExpired);
    }
    let specs = match sweep_or_empty(specs_result, "spec versions", &mut notices) {
        Ok(items) => items,
        Err(state) => return Ok(state.into_panel()),
    };
    let specs: Vec<SpecListItem> = specs
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect();

    let latest = specs.iter().max_by_key(|s| s.version);
    let latest_version = latest.map(|s| s.version);
    let published_version = published_spec_id
        .as_deref()
        .and_then(|id| specs.iter().find(|s| s.id == id))
        .map(|s| s.version);
    let pill = state_pill(
        published_version,
        latest_version,
        latest.and_then(|s| s.latest_decision.as_deref()),
    );

    // Verbatim event history of the latest version (oldest first from the endpoint).
    // A failed fetch is surfaced as `None` — never rendered as an empty history.
    let mut events = Some(Vec::new());
    if let Some(v) = latest_version {
        let events_result = sweep(
            client,
            team,
            &format!("api-definitions/{seg}/specs/{v}/events"),
            SWEEP_BYTE_BUDGET,
        )
        .await;
        if matches!(events_result, Err(SweepFailure::AuthExpired)) {
            return Err(AuthExpired);
        }
        events = match sweep_or_empty(events_result, "review events", &mut notices) {
            Ok(items) => Some(
                items
                    .into_iter()
                    .filter_map(|item| serde_json::from_value::<EventItem>(item).ok())
                    .map(|e| EventRow {
                        decision: e.decision,
                        actor_type: e.actor_type,
                        reason: e.reason,
                        created: humanize_age(now, e.created_at),
                    })
                    .collect(),
            ),
            Err(_) => None,
        };
    }

    let lineage: Vec<LineageRow> = {
        let mut rows: Vec<&SpecListItem> = specs.iter().collect();
        rows.sort_by_key(|s| std::cmp::Reverse(s.version));
        rows.into_iter()
            .map(|s| LineageRow {
                version: s.version,
                source_kind: s.source_kind.clone(),
                hash_short: short_id(&s.spec_hash),
                decision: s
                    .latest_decision
                    .clone()
                    .unwrap_or_else(|| "(no events)".into()),
                published: published_spec_id.as_deref() == Some(s.id.as_str()),
                created: humanize_age(now, s.created_at),
            })
            .collect()
    };

    // Envoy chain: typed binding IDs joined to route-config/listener names (F2 data).
    let bindings_result = sweep(
        client,
        team,
        &format!("api-definitions/{seg}/route-bindings"),
        SWEEP_BYTE_BUDGET,
    )
    .await;
    if matches!(bindings_result, Err(SweepFailure::AuthExpired)) {
        return Err(AuthExpired);
    }
    let bindings = match sweep_or_empty(bindings_result, "route bindings", &mut notices) {
        Ok(items) => items,
        Err(state) => return Ok(state.into_panel()),
    };
    let mut chain = Vec::new();
    let mut chain_names_unresolved = false;
    if !bindings.is_empty() {
        let mut rc_names = std::collections::BTreeMap::new();
        let mut listener_names = std::collections::BTreeMap::new();
        for (segment, map) in [
            ("route-configs", &mut rc_names),
            ("listeners", &mut listener_names),
        ] {
            let result = sweep(client, team, segment, SWEEP_BYTE_BUDGET).await;
            if matches!(result, Err(SweepFailure::AuthExpired)) {
                return Err(AuthExpired);
            }
            match sweep_or_empty(result, "gateway resources", &mut notices) {
                Ok(items) => {
                    for item in items {
                        if let (Some(id), Some(name)) = (
                            item.get("id").and_then(Value::as_str),
                            item.get("name").and_then(Value::as_str),
                        ) {
                            map.insert(id.to_string(), name.to_string());
                        }
                    }
                }
                // A failed join sweep is surfaced explicitly; the chain then shows
                // raw IDs under a visible notice, never silently.
                Err(_) => chain_names_unresolved = true,
            }
        }
        chain = bindings
            .into_iter()
            .filter_map(|item| serde_json::from_value::<BindingItem>(item).ok())
            .map(|b| ChainRow {
                binding: b.name,
                route_config: rc_names
                    .get(&b.route_config_id)
                    .cloned()
                    .unwrap_or_else(|| short_id(&b.route_config_id)),
                listener: b
                    .listener_id
                    .as_deref()
                    .map(|id| {
                        listener_names
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| short_id(id))
                    })
                    .unwrap_or_else(|| "—".into()),
                scope: match (b.virtual_host.as_deref(), b.route.as_deref()) {
                    (Some(vh), Some(r)) => format!("{vh} / {r}"),
                    (Some(vh), None) => vh.to_string(),
                    (None, Some(r)) => r.to_string(),
                    (None, None) => "whole route config".into(),
                },
            })
            .collect();
    }

    let tools_result = sweep(
        client,
        team,
        &format!("api-definitions/{seg}/tools"),
        SWEEP_BYTE_BUDGET,
    )
    .await;
    if matches!(tools_result, Err(SweepFailure::AuthExpired)) {
        return Err(AuthExpired);
    }
    let tools = match sweep_or_empty(tools_result, "api tools", &mut notices) {
        Ok(items) => items,
        Err(state) => return Ok(state.into_panel()),
    };
    let tools: Vec<ToolRow> = tools
        .into_iter()
        .filter_map(|item| serde_json::from_value::<ToolItem>(item).ok())
        .map(|t| ToolRow {
            name: t.name,
            operation_id: t.operation_id,
            method: t.method,
            path: t.path,
            enabled: t.enabled,
            input_schema: serde_json::to_string_pretty(&t.input_schema).unwrap_or_default(),
            output_schema: serde_json::to_string_pretty(&t.output_schema).unwrap_or_default(),
        })
        .collect();

    Ok(Panel::Data(ApiDetail {
        api: api.to_string(),
        pill,
        events,
        latest_version,
        lineage,
        chain,
        chain_names_unresolved,
        tools,
        notices,
    }))
}

#[cfg(test)]
mod tests {
    use super::state_pill;

    /// Design acceptance 1: every derived pill maps 1:1 to a
    /// `(published_version, latest_version, latest event)` tuple — enumerated here,
    /// including all five persisted decisions and the no-events case. No invented
    /// "awaiting"/"draft" states exist in the mapping.
    #[test]
    fn pill_maps_one_to_one_from_tuples() {
        type Case = (Option<i64>, Option<i64>, Option<&'static str>, &'static str);
        let cases: &[Case] = &[
            (None, None, None, "no spec"),
            (Some(1), None, None, "no spec"),
            (None, Some(1), None, "v1 (no events)"),
            (None, Some(1), Some("submitted"), "v1 submitted"),
            (None, Some(1), Some("reviewed"), "v1 reviewed"),
            (None, Some(3), Some("rejected"), "v3 rejected"),
            (None, Some(2), Some("unpublished"), "v2 unpublished"),
            (Some(2), Some(2), Some("published"), "published v2"),
            (Some(2), Some(2), None, "published v2"),
            (
                Some(2),
                Some(3),
                Some("rejected"),
                "published v2 · v3 rejected",
            ),
            (
                Some(2),
                Some(3),
                Some("submitted"),
                "published v2 · v3 submitted",
            ),
            (
                Some(2),
                Some(3),
                Some("reviewed"),
                "published v2 · v3 reviewed",
            ),
            (
                Some(2),
                Some(3),
                Some("unpublished"),
                "published v2 · v3 unpublished",
            ),
            (Some(2), Some(3), None, "published v2 · v3 (no events)"),
        ];
        for (published, latest, decision, want) in cases {
            assert_eq!(
                state_pill(*published, *latest, *decision),
                *want,
                "tuple ({published:?}, {latest:?}, {decision:?})"
            );
        }
    }

    /// An unknown decision string (future enum growth) renders verbatim — never a panic
    /// or a blank cell.
    #[test]
    fn unknown_decisions_render_verbatim() {
        assert_eq!(
            state_pill(Some(1), Some(2), Some("archived")),
            "published v1 · v2 archived"
        );
    }
}
