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
    #[serde(default)]
    description: String,
    revision: i64,
    tool_count: i64,
    enabled_tool_count: i64,
    route_binding_count: i64,
    #[serde(default)]
    latest_version: Option<i64>,
    #[serde(default)]
    published_version: Option<i64>,
    latest_decision: Option<String>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, PartialEq, Eq)]
struct ApiLifecyclePresentation {
    label: String,
    tone: &'static str,
    published: bool,
    attention: bool,
    latest_unpublished: bool,
}

fn api_lifecycle_presentation(
    published_version: Option<i64>,
    latest_version: Option<i64>,
    latest_decision: Option<&str>,
) -> ApiLifecyclePresentation {
    // A published pointer without any latest version is an inconsistent future payload;
    // keep the truthful "no spec" label and do not count it as published.
    let published = published_version.is_some() && latest_version.is_some();
    let latest_unpublished = latest_version.is_some() && latest_version != published_version;
    let attention = latest_unpublished && matches!(latest_decision, Some("submitted" | "reviewed"));
    let tone = if latest_version.is_some() && latest_version == published_version {
        "good"
    } else if attention {
        "warn"
    } else {
        "neutral"
    };
    ApiLifecyclePresentation {
        label: state_pill(published_version, latest_version, latest_decision),
        tone,
        published,
        attention,
        latest_unpublished,
    }
}

#[derive(Debug)]
pub(super) struct ApiRow {
    pub(super) name: String,
    pub(super) display_name: String,
    pub(super) description: String,
    pub(super) revision: i64,
    pub(super) tool_count: i64,
    pub(super) enabled_tool_count: i64,
    pub(super) route_binding_count: i64,
    pub(super) lifecycle: String,
    pub(super) lifecycle_tone: &'static str,
    pub(super) published: bool,
    pub(super) attention: bool,
    pub(super) latest_unpublished: bool,
    pub(super) updated: String,
    pub(super) unparsed: bool,
}

fn api_row(item: Value, now: DateTime<Utc>) -> ApiRow {
    // Both additive fields are required wire keys. `latest_decision: null` is valid,
    // but an absent key means producer/consumer version skew and must fail closed.
    if item.get("enabled_tool_count").is_none() || item.get("latest_decision").is_none() {
        return unparsed_api_row(&item);
    }
    match serde_json::from_value::<ApiListItem>(item.clone()) {
        Ok(item) => {
            let lifecycle = api_lifecycle_presentation(
                item.published_version,
                item.latest_version,
                item.latest_decision.as_deref(),
            );
            ApiRow {
                lifecycle: lifecycle.label,
                lifecycle_tone: lifecycle.tone,
                published: lifecycle.published,
                attention: lifecycle.attention,
                latest_unpublished: lifecycle.latest_unpublished,
                display_name: if item.display_name.is_empty() {
                    "—".into()
                } else {
                    item.display_name
                },
                description: if item.description.is_empty() {
                    "—".into()
                } else {
                    item.description
                },
                name: item.name,
                revision: item.revision,
                tool_count: item.tool_count,
                enabled_tool_count: item.enabled_tool_count,
                route_binding_count: item.route_binding_count,
                updated: humanize_age(now, item.updated_at),
                unparsed: false,
            }
        }
        Err(_) => unparsed_api_row(&item),
    }
}

fn unparsed_api_row(item: &Value) -> ApiRow {
    ApiRow {
        name: item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("(unknown)")
            .to_string(),
        display_name: String::new(),
        description: String::new(),
        revision: 0,
        tool_count: 0,
        enabled_tool_count: 0,
        route_binding_count: 0,
        lifecycle: String::new(),
        lifecycle_tone: "neutral",
        published: false,
        attention: false,
        latest_unpublished: false,
        updated: String::new(),
        unparsed: true,
    }
}

pub(super) struct ApisOverview {
    pub(super) table: Table<ApiRow>,
    pub(super) api_count: String,
    pub(super) published_count: String,
    pub(super) attention_count: String,
    pub(super) tool_count: String,
    pub(super) enabled_tool_count: String,
    pub(super) disabled_tool_count: String,
    pub(super) binding_count: String,
    pub(super) apis_with_bindings: String,
    pub(super) latest_unpublished_count: String,
    pub(super) unparsed_count: usize,
}

fn lower_bound(value: impl std::fmt::Display, partial: bool) -> String {
    if partial {
        format!("≥{value}")
    } else {
        value.to_string()
    }
}

fn overview_from_table(table: Table<ApiRow>) -> ApisOverview {
    let partial = table.partial.is_some();
    let unparsed_count = table.rows.iter().filter(|row| row.unparsed).count();
    let parsed = table.rows.iter().filter(|row| !row.unparsed);
    let published_count = parsed.clone().filter(|row| row.published).count();
    let attention_count = parsed.clone().filter(|row| row.attention).count();
    let tool_count: i64 = parsed.clone().map(|row| row.tool_count).sum();
    let enabled_tool_count: i64 = parsed.clone().map(|row| row.enabled_tool_count).sum();
    let binding_count: i64 = parsed.clone().map(|row| row.route_binding_count).sum();
    let apis_with_bindings = parsed
        .clone()
        .filter(|row| row.route_binding_count > 0)
        .count();
    let latest_unpublished_count = parsed.filter(|row| row.latest_unpublished).count();
    ApisOverview {
        // Card 1 is the visible definition count. Unparseable rows remain visible API
        // definitions; only lifecycle/tool/binding category sums exclude them.
        api_count: lower_bound(table.rows.len(), partial),
        published_count: lower_bound(published_count, partial),
        attention_count: lower_bound(attention_count, partial),
        tool_count: lower_bound(tool_count, partial),
        enabled_tool_count: lower_bound(enabled_tool_count, partial),
        disabled_tool_count: lower_bound(tool_count.saturating_sub(enabled_tool_count), partial),
        binding_count: lower_bound(binding_count, partial),
        apis_with_bindings: lower_bound(apis_with_bindings, partial),
        latest_unpublished_count: lower_bound(latest_unpublished_count, partial),
        unparsed_count,
        table,
    }
}

pub(super) async fn fetch_apis(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
) -> Result<Panel<ApisOverview>, AuthExpired> {
    let result = sweep(client, team, "api-definitions", SWEEP_BYTE_BUDGET).await;
    Ok(
        match to_panel(result, "api definitions", |item| api_row(item, now))? {
            Panel::Data(table) => Panel::Data(overview_from_table(table)),
            Panel::Unauthorized => Panel::Unauthorized,
            Panel::Unavailable => Panel::Unavailable,
        },
    )
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
    #[serde(default = "unknown_format")]
    format: String,
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
    #[serde(default)]
    id: String,
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

fn unknown_format() -> String {
    "—".into()
}

fn event_rows(now: DateTime<Utc>, mut events: Vec<EventItem>) -> Vec<EventRow> {
    events.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    events
        .into_iter()
        .map(|event| EventRow {
            decision: event.decision,
            actor_type: event.actor_type,
            reason: event.reason,
            created: humanize_age(now, event.created_at),
        })
        .collect()
}

#[derive(Debug)]
pub(super) struct PipelineStage {
    pub(super) label: String,
    pub(super) kind: &'static str,
}

fn compose_pipeline(
    source_kind: Option<&str>,
    events: &[EventRow],
    events_available: bool,
    published_version: Option<i64>,
    enabled_tools: usize,
    total_tools: usize,
    tools_partial: bool,
) -> Vec<PipelineStage> {
    let mut stages = Vec::new();
    if let Some(source) = source_kind {
        stages.push(PipelineStage {
            label: format!("source: {source}"),
            kind: "source",
        });
    }
    stages.extend(events.iter().map(|event| PipelineStage {
        label: event.decision.clone(),
        kind: "event",
    }));
    if !events_available {
        stages.push(PipelineStage {
            label: "review events unavailable".into(),
            kind: "unavailable",
        });
    }
    if let Some(version) = published_version {
        stages.push(PipelineStage {
            label: format!("published v{version}"),
            kind: "published",
        });
    }
    let tool_label = if tools_partial {
        format!("≥{enabled_tools}/≥{total_tools} tools enabled (visible)")
    } else {
        format!("{enabled_tools}/{total_tools} tools enabled")
    };
    stages.push(PipelineStage {
        label: tool_label,
        kind: "tools",
    });
    stages
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
    pub(super) description: String,
    pub(super) revision: String,
    pub(super) updated: String,
    pub(super) latest_format: String,
    pub(super) latest_hash: String,
    pub(super) published_version: Option<i64>,
    pub(super) tool_summary: String,
    pub(super) events_available: bool,
    pub(super) pipeline: Vec<PipelineStage>,
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

    // The definition row supplies persisted metadata and the published-spec pointer.
    let (published_spec_id, description, revision, updated): (
        Option<String>,
        String,
        String,
        String,
    ) = match client
        .get_json_sized(&format!("/api/v1/teams/{team}/api-definitions/{seg}"))
        .await
    {
        Ok((value, _)) => {
            let published = value
                .get("published_spec_version_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            let description = value
                .get("description")
                .and_then(Value::as_str)
                .filter(|description| !description.is_empty())
                .unwrap_or("—")
                .to_string();
            let revision = value
                .get("revision")
                .and_then(Value::as_i64)
                .map(|revision| revision.to_string())
                .unwrap_or_else(|| "—".into());
            let updated = value
                .get("updated_at")
                .and_then(Value::as_str)
                .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
                .map(|timestamp| humanize_age(now, timestamp.with_timezone(&Utc)))
                .unwrap_or_else(|| "—".into());
            (published, description, revision, updated)
        }
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
            Ok(items) => {
                let decoded = items
                    .into_iter()
                    .filter_map(|item| serde_json::from_value::<EventItem>(item).ok())
                    .collect();
                Some(event_rows(now, decoded))
            }
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
    let notices_before_tools = notices.len();
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
    let tools_partial = notices.len() > notices_before_tools;

    let enabled_tool_count = tools.iter().filter(|tool| tool.enabled).count();
    let latest_format = latest
        .map(|spec| spec.format.clone())
        .unwrap_or_else(|| "—".into());
    let latest_hash = latest
        .map(|spec| spec.spec_hash.clone())
        .unwrap_or_else(|| "—".into());
    let pipeline = compose_pipeline(
        latest.map(|spec| spec.source_kind.as_str()),
        events.as_deref().unwrap_or(&[]),
        events.is_some(),
        published_version,
        enabled_tool_count,
        tools.len(),
        tools_partial,
    );
    let tool_summary = if tools_partial {
        format!("≥{enabled_tool_count} / ≥{} enabled (visible)", tools.len())
    } else {
        format!("{enabled_tool_count} / {} enabled", tools.len())
    };

    Ok(Panel::Data(ApiDetail {
        api: api.to_string(),
        pill,
        description,
        revision,
        updated,
        latest_format,
        latest_hash,
        published_version,
        tool_summary,
        events_available: events.is_some(),
        pipeline,
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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{
        api_lifecycle_presentation, api_row, compose_pipeline, event_rows, overview_from_table,
        state_pill, ApiRow, EventItem,
    };
    use crate::cli::dashboard::resources::{PartialNotice, PartialReason, Table};
    use chrono::{TimeZone, Utc};
    use serde_json::json;

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

    #[test]
    fn list_lifecycle_maps_every_truthful_tuple_and_tone() {
        type Case = (
            Option<i64>,
            Option<i64>,
            Option<&'static str>,
            &'static str,
            &'static str,
            bool,
            bool,
        );
        let cases: &[Case] = &[
            (None, None, None, "no spec", "neutral", false, false),
            (Some(1), Some(1), None, "published v1", "good", false, false),
            (
                None,
                Some(1),
                Some("submitted"),
                "v1 submitted",
                "warn",
                true,
                true,
            ),
            (
                None,
                Some(1),
                Some("reviewed"),
                "v1 reviewed",
                "warn",
                true,
                true,
            ),
            (
                None,
                Some(1),
                Some("published"),
                "v1 published",
                "neutral",
                false,
                true,
            ),
            (
                None,
                Some(1),
                Some("rejected"),
                "v1 rejected",
                "neutral",
                false,
                true,
            ),
            (
                None,
                Some(1),
                Some("unpublished"),
                "v1 unpublished",
                "neutral",
                false,
                true,
            ),
            (
                None,
                Some(1),
                None,
                "v1 (no events)",
                "neutral",
                false,
                true,
            ),
            (
                Some(1),
                Some(2),
                Some("submitted"),
                "published v1 · v2 submitted",
                "warn",
                true,
                true,
            ),
            (
                Some(1),
                Some(2),
                Some("reviewed"),
                "published v1 · v2 reviewed",
                "warn",
                true,
                true,
            ),
            (
                Some(1),
                Some(2),
                Some("published"),
                "published v1 · v2 published",
                "neutral",
                false,
                true,
            ),
            (
                Some(1),
                Some(2),
                Some("rejected"),
                "published v1 · v2 rejected",
                "neutral",
                false,
                true,
            ),
            (
                Some(1),
                Some(2),
                Some("unpublished"),
                "published v1 · v2 unpublished",
                "neutral",
                false,
                true,
            ),
            (
                Some(1),
                Some(2),
                Some("archived"),
                "published v1 · v2 archived",
                "neutral",
                false,
                true,
            ),
        ];
        for (published, latest, decision, label, tone, attention, latest_unpublished) in cases {
            let got = api_lifecycle_presentation(*published, *latest, *decision);
            assert_eq!(got.label, *label);
            assert_eq!(got.tone, *tone);
            assert_eq!(got.attention, *attention);
            assert_eq!(got.latest_unpublished, *latest_unpublished);
        }
        assert!(!api_lifecycle_presentation(Some(1), None, None).published);
    }

    #[test]
    fn additive_wire_keys_are_required_but_latest_decision_accepts_null() {
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();
        let base = json!({
            "name": "orders",
            "display_name": "Orders",
            "description": "Order lifecycle",
            "revision": 7,
            "tool_count": 3,
            "enabled_tool_count": 2,
            "route_binding_count": 1,
            "latest_version": 2,
            "published_version": 1,
            "latest_decision": null,
            "updated_at": "2026-01-01T00:00:00Z"
        });
        let parsed = api_row(base.clone(), now);
        assert!(!parsed.unparsed);
        assert_eq!(parsed.enabled_tool_count, 2);
        assert_eq!(parsed.lifecycle, "published v1 · v2 (no events)");

        for missing in ["enabled_tool_count", "latest_decision"] {
            let mut value = base.clone();
            value.as_object_mut().unwrap().remove(missing);
            assert!(api_row(value, now).unparsed, "missing {missing}");
        }
    }

    fn row(
        name: &str,
        tools: i64,
        enabled: i64,
        bindings: i64,
        current_published: bool,
        attention: bool,
        latest_unpublished: bool,
    ) -> ApiRow {
        ApiRow {
            name: name.into(),
            display_name: name.into(),
            description: format!("{name} description"),
            revision: 1,
            tool_count: tools,
            enabled_tool_count: enabled,
            route_binding_count: bindings,
            lifecycle: "fixture".into(),
            lifecycle_tone: "neutral",
            published: current_published,
            attention,
            latest_unpublished,
            updated: "now".into(),
            unparsed: false,
        }
    }

    #[test]
    fn overview_is_exact_when_complete_and_lower_bound_when_partial() {
        let mut rows = vec![
            row("a", 3, 2, 1, true, false, false),
            row("b", 1, 1, 0, true, false, false),
            row("c", 2, 1, 2, false, true, true),
            row("d", 0, 0, 0, false, false, true),
            row("e", 1, 0, 0, false, false, false),
        ];
        let mut malformed = row("malformed", 99, 99, 99, false, true, true);
        malformed.unparsed = true;
        rows.push(malformed);

        let complete = overview_from_table(Table {
            rows,
            total: 6,
            partial: None,
        });
        assert_eq!(complete.api_count, "6");
        assert_eq!(complete.published_count, "2");
        assert_eq!(complete.attention_count, "1");
        assert_eq!(complete.tool_count, "7");
        assert_eq!(complete.enabled_tool_count, "4");
        assert_eq!(complete.disabled_tool_count, "3");
        assert_eq!(complete.binding_count, "3");
        assert_eq!(complete.apis_with_bindings, "2");
        assert_eq!(complete.latest_unpublished_count, "2");
        assert_eq!(complete.unparsed_count, 1);

        let partial_rows = complete.table.rows;
        let partial = overview_from_table(Table {
            rows: partial_rows,
            total: 40,
            partial: Some(PartialNotice {
                shown: 6,
                total: 40,
                reason: PartialReason::Budget,
                collection: "api definitions",
            }),
        });
        assert_eq!(partial.api_count, "≥6");
        assert_eq!(partial.published_count, "≥2");
        assert_eq!(partial.attention_count, "≥1");
        assert_eq!(partial.tool_count, "≥7");
        assert_eq!(partial.enabled_tool_count, "≥4");
        assert_eq!(partial.disabled_tool_count, "≥3");
        assert_eq!(partial.binding_count, "≥3");
        assert_eq!(partial.apis_with_bindings, "≥2");
        assert_eq!(partial.latest_unpublished_count, "≥2");
    }

    #[test]
    fn detail_pipeline_orders_persisted_events_and_never_invents_stages() {
        let now = Utc.with_ymd_and_hms(2026, 7, 27, 0, 0, 0).unwrap();
        let events = event_rows(
            now,
            vec![
                EventItem {
                    id: "event-3".into(),
                    decision: "future-decision".into(),
                    actor_type: "human".into(),
                    reason: "kept verbatim".into(),
                    created_at: Utc.with_ymd_and_hms(2026, 7, 27, 0, 3, 0).unwrap(),
                },
                EventItem {
                    id: "event-1".into(),
                    decision: "submitted".into(),
                    actor_type: "system".into(),
                    reason: String::new(),
                    created_at: Utc.with_ymd_and_hms(2026, 7, 27, 0, 1, 0).unwrap(),
                },
                EventItem {
                    id: "event-2".into(),
                    decision: "reviewed".into(),
                    actor_type: "human".into(),
                    reason: "looks good".into(),
                    created_at: Utc.with_ymd_and_hms(2026, 7, 27, 0, 2, 0).unwrap(),
                },
            ],
        );
        assert_eq!(
            events
                .iter()
                .map(|event| event.decision.as_str())
                .collect::<Vec<_>>(),
            vec!["submitted", "reviewed", "future-decision"]
        );

        let pipeline = compose_pipeline(Some("openapi"), &events, true, Some(2), 1, 3, false);
        assert_eq!(
            pipeline
                .iter()
                .map(|stage| stage.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "source: openapi",
                "submitted",
                "reviewed",
                "future-decision",
                "published v2",
                "1/3 tools enabled",
            ]
        );
        let rendered = pipeline
            .iter()
            .map(|stage| stage.label.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!rendered.contains("captured"));
        assert!(!rendered.contains("cluster"));

        let degraded = compose_pipeline(Some("openapi"), &[], false, None, 2, 3, true);
        assert_eq!(
            degraded
                .iter()
                .map(|stage| stage.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "source: openapi",
                "review events unavailable",
                "≥2/≥3 tools enabled (visible)",
            ]
        );
    }
}
