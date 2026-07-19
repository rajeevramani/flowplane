//! AI Gateway tab (ui-f5 S4/S5): providers, route→backend chains, budget meters with
//! real current-window state, windowed usage, and the paged trace drill-down.
//!
//! Window discipline (design fpv2-0t4): ONE `until = now` instant is captured per
//! refresh — the whole tab renders from a single partial fetch, so every query in that
//! render (usage card, usage table) shares the identical `[until − 24h, until)` pair;
//! there is no per-panel `now` resampling. Traces are user-paged by cursor and carry no
//! window.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

use super::super::client::{ReadError, RestClient};
use super::data::{humanize_age, AuthExpired, Panel};
use super::resources::{sweep, PartialNotice, PartialReason, SweepFailure, SWEEP_BYTE_BUDGET};

// =============================================================================================
// Upstream item shapes (typed decode; failures surface, never silently drop).
// =============================================================================================

#[derive(Debug, Deserialize)]
struct ProviderItem {
    id: String,
    name: String,
    spec: ProviderSpecItem,
}

#[derive(Debug, Deserialize)]
struct ProviderSpecItem {
    kind: String,
    base_url: String,
    #[serde(default)]
    models: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RouteItem {
    name: String,
    status: String,
    spec: RouteSpecItem,
}

#[derive(Debug, Deserialize)]
struct RouteSpecItem {
    #[serde(default)]
    backends: Vec<BackendItem>,
}

#[derive(Debug, Deserialize)]
struct BackendItem {
    provider_id: String,
    #[serde(default)]
    priority: u32,
}

#[derive(Debug, Deserialize)]
struct BudgetItem {
    name: String,
    spec: BudgetSpecItem,
    state: BudgetStateItem,
}

#[derive(Debug, Deserialize)]
struct BudgetSpecItem {
    mode: String,
}

#[derive(Debug, Deserialize)]
struct BudgetStateItem {
    used_units: u64,
    limit_units: u64,
    window_seconds: u32,
    window_start: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct UsagePage {
    #[serde(default)]
    items: Vec<UsageItem>,
    #[serde(default)]
    total: i64,
}

#[derive(Debug, Deserialize)]
struct UsageItem {
    #[serde(default)]
    route_config_id: Option<String>,
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    total_tokens: u64,
    #[serde(default)]
    event_count: u64,
}

// =============================================================================================
// Rendered rows.
// =============================================================================================

/// The provider kinds the tab renders (design: `openai` / `openai-compatible` only).
const RENDERED_PROVIDER_KINDS: &[&str] = &["openai", "openai-compatible"];

#[derive(Debug)]
pub(super) struct ProviderRow {
    pub(super) name: String,
    pub(super) kind: String,
    pub(super) base_url: String,
    pub(super) models: String,
}

#[derive(Debug)]
pub(super) struct RouteRow {
    pub(super) name: String,
    pub(super) status: String,
    /// Backend chain in priority order, provider names joined with " → ".
    pub(super) chain: String,
}

#[derive(Debug)]
pub(super) struct BudgetRow {
    pub(super) name: String,
    /// `shadow` / `enforcing`, verbatim from the spec.
    pub(super) mode: String,
    pub(super) used_units: u64,
    pub(super) limit_units: u64,
    /// Meter fill 0–100 (used/limit, capped at 100 for overrun display).
    pub(super) pct: u64,
    /// True at ≥ 80% of the limit — drives the near-limit warning card.
    pub(super) warn: bool,
    pub(super) window: String,
    pub(super) window_started: String,
}

#[derive(Debug)]
pub(super) struct UsageRow {
    pub(super) route: String,
    pub(super) provider: String,
    pub(super) prompt_tokens: u64,
    pub(super) completion_tokens: u64,
    pub(super) total_tokens: u64,
    pub(super) events: u64,
}

pub(super) struct AiPanel {
    // Summary cards.
    pub(super) provider_count: usize,
    pub(super) routes_active: usize,
    pub(super) routes_stale: usize,
    /// Tokens in the captured 24h window (sum of windowed usage `total_tokens`).
    pub(super) tokens_window: u64,
    /// The captured window rendered for the operator (`[until − 24h, until)`).
    pub(super) window_label: String,
    // Panels.
    pub(super) providers: Vec<ProviderRow>,
    pub(super) routes: Vec<RouteRow>,
    pub(super) budgets: Vec<BudgetRow>,
    /// Windowed usage table rows (same captured window pair as the Tokens card).
    pub(super) usage: Vec<UsageRow>,
    /// Budgets at ≥ 80% — the warning card lists them by name.
    pub(super) near_limit: Vec<String>,
    /// Typed-decode failures per collection (version skew) — surfaced, never dropped.
    pub(super) unparsed: Vec<(&'static str, usize)>,
    /// Providers hidden because their kind is outside the rendered set (defensive;
    /// no such kind exists today).
    pub(super) hidden_provider_kinds: usize,
    pub(super) notices: Vec<PartialNotice>,
}

fn record_notice(
    result: Result<super::resources::Sweep, SweepFailure>,
    collection: &'static str,
    notices: &mut Vec<PartialNotice>,
) -> Result<Vec<Value>, SweepFailure> {
    let sweep = result?;
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

fn fmt_ts(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Fetch everything the AI tab renders under ONE captured `until = now` instant.
pub(super) async fn fetch_ai(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
) -> Result<Panel<AiPanel>, AuthExpired> {
    // The single captured window pair for this refresh (design: no per-panel resampling).
    let until = now;
    let since = until - Duration::hours(24);

    let mut notices = Vec::new();
    let mut unparsed: Vec<(&'static str, usize)> = Vec::new();

    let providers_raw = match record_notice(
        sweep(client, team, "ai/providers", SWEEP_BYTE_BUDGET).await,
        "AI providers",
        &mut notices,
    ) {
        Ok(items) => items,
        Err(SweepFailure::AuthExpired) => return Err(AuthExpired),
        Err(SweepFailure::Unauthorized) => return Ok(Panel::Unauthorized),
        Err(SweepFailure::Unavailable) => return Ok(Panel::Unavailable),
    };
    let mut unparsed_providers = 0_usize;
    let providers: Vec<ProviderItem> = providers_raw
        .into_iter()
        .filter_map(|item| {
            serde_json::from_value(item)
                .map_err(|_| unparsed_providers += 1)
                .ok()
        })
        .collect();
    if unparsed_providers > 0 {
        unparsed.push(("AI providers", unparsed_providers));
    }

    // Remaining collections degrade to per-panel notices rather than killing the tab.
    let routes_raw = match record_notice(
        sweep(client, team, "ai/routes", SWEEP_BYTE_BUDGET).await,
        "AI routes",
        &mut notices,
    ) {
        Ok(items) => items,
        Err(SweepFailure::AuthExpired) => return Err(AuthExpired),
        Err(_) => {
            notices.push(PartialNotice {
                shown: 0,
                total: 0,
                reason: PartialReason::UpstreamFailure,
                collection: "AI routes",
            });
            Vec::new()
        }
    };
    let mut unparsed_routes = 0_usize;
    let routes: Vec<RouteItem> = routes_raw
        .into_iter()
        .filter_map(|item| {
            serde_json::from_value(item)
                .map_err(|_| unparsed_routes += 1)
                .ok()
        })
        .collect();
    if unparsed_routes > 0 {
        unparsed.push(("AI routes", unparsed_routes));
    }

    let budgets_raw = match record_notice(
        sweep(client, team, "ai/budgets", SWEEP_BYTE_BUDGET).await,
        "AI budgets",
        &mut notices,
    ) {
        Ok(items) => items,
        Err(SweepFailure::AuthExpired) => return Err(AuthExpired),
        Err(_) => {
            notices.push(PartialNotice {
                shown: 0,
                total: 0,
                reason: PartialReason::UpstreamFailure,
                collection: "AI budgets",
            });
            Vec::new()
        }
    };
    let mut unparsed_budgets = 0_usize;
    let budgets: Vec<BudgetItem> = budgets_raw
        .into_iter()
        .filter_map(|item| {
            serde_json::from_value(item)
                .map_err(|_| unparsed_budgets += 1)
                .ok()
        })
        .collect();
    if unparsed_budgets > 0 {
        unparsed.push(("AI budgets", unparsed_budgets));
    }

    // Windowed usage: paged GETs, every page under the SAME captured pair (one instant,
    // not one request — a team with > 500 grouped rows must still sum completely so the
    // Tokens card equals the direct windowed query; design AC 4).
    const USAGE_PAGE_LIMIT: i64 = 500;
    /// Runaway guard: 20 pages = 10k grouped (route, provider) pairs, far above any
    /// plausible team; crossing it renders an explicit partial notice.
    const USAGE_MAX_OFFSET: i64 = 10_000;
    let mut usage_rows: Vec<UsageItem> = Vec::new();
    let mut usage_offset: i64 = 0;
    loop {
        let usage_path = format!(
            "/api/v1/teams/{team}/ai/usage?since={}&until={}&limit={USAGE_PAGE_LIMIT}&offset={usage_offset}",
            percent_encode(&fmt_ts(since)),
            percent_encode(&fmt_ts(until)),
        );
        match client.get_json(&usage_path).await {
            Ok(value) => match serde_json::from_value::<UsagePage>(value) {
                Ok(page) => {
                    let n = page.items.len();
                    usage_rows.extend(page.items);
                    if (n as i64) < USAGE_PAGE_LIMIT {
                        break;
                    }
                    usage_offset += USAGE_PAGE_LIMIT;
                    if usage_offset >= USAGE_MAX_OFFSET {
                        notices.push(PartialNotice {
                            shown: usage_rows.len(),
                            total: page.total,
                            reason: PartialReason::Budget,
                            collection: "AI usage",
                        });
                        break;
                    }
                }
                Err(_) => {
                    unparsed.push(("AI usage", 1));
                    break;
                }
            },
            Err(ReadError::Status { status, .. })
                if status == reqwest::StatusCode::UNAUTHORIZED =>
            {
                return Err(AuthExpired)
            }
            Err(_) => {
                notices.push(PartialNotice {
                    shown: usage_rows.len(),
                    total: 0,
                    reason: PartialReason::UpstreamFailure,
                    collection: "AI usage",
                });
                break;
            }
        }
    }

    // Route-config id → name for the usage table (usage rows carry route_config_id).
    let mut route_config_names: BTreeMap<String, String> = BTreeMap::new();
    if usage_rows.iter().any(|u| u.route_config_id.is_some()) {
        match record_notice(
            sweep(client, team, "route-configs", SWEEP_BYTE_BUDGET).await,
            "route configs",
            &mut notices,
        ) {
            Ok(items) => {
                for item in items {
                    if let (Some(id), Some(name)) = (
                        item.get("id").and_then(Value::as_str),
                        item.get("name").and_then(Value::as_str),
                    ) {
                        route_config_names.insert(id.to_string(), name.to_string());
                    }
                }
            }
            Err(SweepFailure::AuthExpired) => return Err(AuthExpired),
            // Without names the table falls back to raw ids; surface it, keep the rows.
            Err(_) => notices.push(PartialNotice {
                shown: 0,
                total: 0,
                reason: PartialReason::UpstreamFailure,
                collection: "route configs",
            }),
        }
    }

    // Provider id → name for chains and usage attribution.
    let provider_names: BTreeMap<&str, &str> = providers
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();

    let hidden_provider_kinds = providers
        .iter()
        .filter(|p| !RENDERED_PROVIDER_KINDS.contains(&p.spec.kind.as_str()))
        .count();
    let provider_rows: Vec<ProviderRow> = providers
        .iter()
        .filter(|p| RENDERED_PROVIDER_KINDS.contains(&p.spec.kind.as_str()))
        .map(|p| ProviderRow {
            name: p.name.clone(),
            kind: p.spec.kind.clone(),
            base_url: p.spec.base_url.clone(),
            models: if p.spec.models.is_empty() {
                "—".into()
            } else {
                p.spec.models.join(", ")
            },
        })
        .collect();

    let route_rows: Vec<RouteRow> = routes
        .iter()
        .map(|r| {
            let mut backends: Vec<&BackendItem> = r.spec.backends.iter().collect();
            backends.sort_by_key(|b| b.priority);
            let chain = if backends.is_empty() {
                "—".to_string()
            } else {
                backends
                    .iter()
                    .map(|b| {
                        provider_names
                            .get(b.provider_id.as_str())
                            .copied()
                            .unwrap_or("(unknown provider)")
                    })
                    .collect::<Vec<_>>()
                    .join(" → ")
            };
            RouteRow {
                name: r.name.clone(),
                status: r.status.clone(),
                chain,
            }
        })
        .collect();

    let budget_rows: Vec<BudgetRow> = budgets
        .iter()
        .map(|b| {
            let pct_exact = if b.state.limit_units == 0 {
                100
            } else {
                b.state.used_units.saturating_mul(100) / b.state.limit_units
            };
            BudgetRow {
                name: b.name.clone(),
                mode: b.spec.mode.clone(),
                used_units: b.state.used_units,
                limit_units: b.state.limit_units,
                pct: pct_exact.min(100),
                warn: pct_exact >= 80,
                window: humanize_window(b.state.window_seconds),
                window_started: humanize_age(now, b.state.window_start),
            }
        })
        .collect();
    let near_limit: Vec<String> = budget_rows
        .iter()
        .filter(|b| b.warn)
        .map(|b| format!("{} ({}%, {})", b.name, b.pct, b.mode))
        .collect();

    let usage: Vec<UsageRow> = usage_rows
        .iter()
        .map(|u| UsageRow {
            route: u
                .route_config_id
                .as_deref()
                .map(|id| {
                    route_config_names
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| id.to_string())
                })
                .unwrap_or_else(|| "—".into()),
            provider: u
                .provider_id
                .as_deref()
                .map(|id| {
                    provider_names
                        .get(id)
                        .copied()
                        .map(str::to_string)
                        .unwrap_or_else(|| id.to_string())
                })
                .unwrap_or_else(|| "—".into()),
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
            events: u.event_count,
        })
        .collect();
    let tokens_window: u64 = usage_rows.iter().map(|u| u.total_tokens).sum();

    Ok(Panel::Data(AiPanel {
        provider_count: provider_rows.len(),
        routes_active: routes.iter().filter(|r| r.status == "active").count(),
        routes_stale: routes.iter().filter(|r| r.status == "stale").count(),
        tokens_window,
        window_label: format!("{} → {}", fmt_ts(since), fmt_ts(until)),
        providers: provider_rows,
        routes: route_rows,
        budgets: budget_rows,
        usage,
        near_limit,
        unparsed,
        hidden_provider_kinds,
        notices,
    }))
}

/// Human window length (the spec values are free-form seconds).
fn humanize_window(seconds: u32) -> String {
    match seconds {
        s if s % 86_400 == 0 && s >= 86_400 => format!("{}d", s / 86_400),
        s if s % 3_600 == 0 && s >= 3_600 => format!("{}h", s / 3_600),
        s if s % 60 == 0 && s >= 60 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// Percent-encode a query VALUE (RFC 3339 timestamps carry `:` and `+`).
fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// =============================================================================================
// Traces (S5): cursor-paged list with the per-request hop-timeline drill-down. Hops arrive
// inside the list response — the drill-down needs no second fetch. Paging is user-driven
// ("Load older"), strictly-before `(created_at, id)` cursors; no window applies.
// =============================================================================================

/// The trace list page size (server cap is 500; 50 keeps drill-downs scannable).
const TRACE_PAGE_LIMIT: i64 = 50;

/// Envelope decode used only for the `miss` payload; rows are decoded per-item so one
/// skewed row surfaces as a count instead of dropping the whole page.
#[derive(Debug, Deserialize)]
struct TraceResponse {
    #[serde(default)]
    miss: Option<TraceMiss>,
}

#[derive(Debug, Deserialize)]
struct TraceMiss {
    message: String,
    hint: String,
}

#[derive(Debug, Deserialize)]
struct TraceItem {
    id: String,
    request_id: String,
    #[serde(default)]
    trace_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    status_code: Option<i32>,
    #[serde(default)]
    failure_hop: Option<String>,
    #[serde(default)]
    hops: Value,
    created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub(super) struct HopRow {
    pub(super) hop: String,
    /// Operator-facing outcome label (design hop semantics: budget exhaustion =
    /// "rejected (429)", `no_upstream_connection` = 503, auth / not_configured named).
    pub(super) label: String,
    pub(super) failed: bool,
    /// Hop start/end timestamps, verbatim from the capture (the design's DIRECT
    /// hop-timeline data); empty when the entry lacks them.
    pub(super) started_at: String,
    pub(super) ended_at: String,
}

#[derive(Debug)]
pub(super) struct TraceRow {
    pub(super) request_id: String,
    pub(super) trace_id: String,
    pub(super) model: String,
    pub(super) status: String,
    pub(super) failure_hop: String,
    pub(super) age: String,
    pub(super) hops: Vec<HopRow>,
}

pub(super) struct TracePanel {
    pub(super) rows: Vec<TraceRow>,
    /// Cursor for the NEXT (older) page — the last row's `(created_at, id)`; present only
    /// when this page was full (more rows may exist).
    pub(super) older_cursor: Option<String>,
    /// Miss payload (id-filtered query matched nothing) — rendered distinctly.
    pub(super) miss: Option<(String, String)>,
    /// Rows that failed typed decode — surfaced, never dropped.
    pub(super) unparsed: usize,
}

/// Design hop semantics: the operator-facing label for one hop entry.
fn hop_label(hop: &str, outcome: &str, detail: Option<&Value>) -> String {
    match (hop, outcome) {
        ("budget", "rejected") => "rejected (429 budget exhausted)".to_string(),
        (_, "no_upstream_connection") => "no upstream connection (503)".to_string(),
        (_, "auth") => "auth failure".to_string(),
        (_, "not_configured") => "not configured".to_string(),
        ("budget", "enforcing") | ("budget", "ok") => outcome.to_string(),
        _ => {
            // `would_reject` shadow annotations carry detail worth surfacing.
            if let Some(d) = detail.and_then(|d| d.get("would_reject")) {
                if d == &Value::Bool(true) {
                    return format!("{outcome} (shadow would_reject)");
                }
            }
            outcome.to_string()
        }
    }
}

fn trace_rows(items: Vec<TraceItem>, now: DateTime<Utc>) -> Vec<TraceRow> {
    items
        .into_iter()
        .map(|t| {
            let hops = t
                .hops
                .as_array()
                .map(|entries| {
                    entries
                        .iter()
                        .map(|e| {
                            let hop = e
                                .get("hop")
                                .and_then(Value::as_str)
                                .unwrap_or("(unknown)")
                                .to_string();
                            let outcome = e.get("outcome").and_then(Value::as_str).unwrap_or("—");
                            HopRow {
                                label: hop_label(&hop, outcome, e.get("detail")),
                                failed: e.get("failed").and_then(Value::as_bool).unwrap_or(false),
                                started_at: e
                                    .get("started_at")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                ended_at: e
                                    .get("ended_at")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                hop,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            TraceRow {
                request_id: t.request_id,
                trace_id: t.trace_id.unwrap_or_else(|| "—".into()),
                model: t.model.unwrap_or_else(|| "—".into()),
                status: t
                    .status_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "—".into()),
                failure_hop: t.failure_hop.unwrap_or_default(),
                age: humanize_age(now, t.created_at),
                hops,
            }
        })
        .collect()
}

/// Fetch one trace page; `before` is the raw `<created_at>,<id>` cursor from the previous
/// page (validated server-side — a malformed value is the server's 400, surfaced as
/// unavailable rather than guessed at here).
pub(super) async fn fetch_traces(
    client: &RestClient,
    team: &str,
    before: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Panel<TracePanel>, AuthExpired> {
    let mut path = format!("/api/v1/teams/{team}/ai/trace?limit={TRACE_PAGE_LIMIT}");
    if let Some(before) = before {
        path.push_str(&format!("&before={}", percent_encode(before)));
    }
    let value = match client.get_json(&path).await {
        Ok(value) => value,
        Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::UNAUTHORIZED => {
            return Err(AuthExpired)
        }
        Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::FORBIDDEN => {
            return Ok(Panel::Unauthorized)
        }
        Err(_) => return Ok(Panel::Unavailable),
    };
    // Track decode failures per row: decode the envelope loosely first.
    let raw_traces = value
        .get("traces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let raw_count = raw_traces.len() as i64;
    let last_raw_cursor = raw_traces.last().and_then(|r| {
        let ts = r.get("created_at").and_then(Value::as_str)?;
        let id = r.get("id").and_then(Value::as_str)?;
        Some(format!("{ts},{id}"))
    });
    let mut unparsed = 0_usize;
    let items: Vec<TraceItem> = raw_traces
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).map_err(|_| unparsed += 1).ok())
        .collect();
    // Full-page detection uses the RAW row count: one skewed row must not strand every
    // older page (review pass 1). The cursor prefers the last RAW row's verbatim
    // created_at/id strings — a lossless round-trip of whatever the server serialized —
    // falling back to the last decoded row only if the raw fields are unreadable.
    let full_page = raw_count == TRACE_PAGE_LIMIT;
    let older_cursor = if full_page {
        last_raw_cursor.or_else(|| {
            items.last().map(|t| {
                format!(
                    "{},{}",
                    t.created_at.to_rfc3339_opts(SecondsFormat::Micros, true),
                    t.id
                )
            })
        })
    } else {
        None
    };
    let miss = serde_json::from_value::<TraceResponse>(value)
        .ok()
        .and_then(|r| r.miss)
        .map(|m| (m.message, m.hint));
    Ok(Panel::Data(TracePanel {
        rows: trace_rows(items, now),
        older_cursor,
        miss,
        unparsed,
    }))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn humanize_window_picks_largest_exact_unit() {
        assert_eq!(humanize_window(3600), "1h");
        assert_eq!(humanize_window(86_400), "1d");
        assert_eq!(humanize_window(90), "90s");
        assert_eq!(humanize_window(120), "2m");
    }

    #[test]
    fn percent_encode_escapes_rfc3339() {
        assert_eq!(
            percent_encode("2026-07-19T12:00:00Z"),
            "2026-07-19T12%3A00%3A00Z"
        );
        assert_eq!(percent_encode("a+b"), "a%2Bb");
    }

    #[test]
    fn rendered_kinds_are_the_design_set() {
        assert_eq!(RENDERED_PROVIDER_KINDS, &["openai", "openai-compatible"]);
    }

    #[test]
    fn hop_labels_follow_design_semantics() {
        assert_eq!(
            hop_label("budget", "rejected", None),
            "rejected (429 budget exhausted)"
        );
        assert_eq!(
            hop_label("upstream", "no_upstream_connection", None),
            "no upstream connection (503)"
        );
        assert_eq!(
            hop_label("credential_injection", "auth", None),
            "auth failure"
        );
        assert_eq!(
            hop_label("credential_injection", "not_configured", None),
            "not configured"
        );
        assert_eq!(hop_label("route_match", "matched", None), "matched");
        let detail = serde_json::json!({ "would_reject": true });
        assert_eq!(
            hop_label("budget", "shadow", Some(&detail)),
            "shadow (shadow would_reject)"
        );
    }

    #[test]
    fn trace_cursor_only_present_on_full_pages() {
        let now = "2026-07-19T12:00:00Z".parse().unwrap();
        let item = |i: u32| TraceItem {
            id: format!("00000000-0000-7000-8000-{i:012}"),
            request_id: format!("req-{i}"),
            trace_id: None,
            model: None,
            status_code: Some(200),
            failure_hop: None,
            hops: serde_json::json!([]),
            created_at: now,
        };
        let short: Vec<TraceItem> = (0..3).map(item).collect();
        assert_eq!(short.len(), 3);
        // trace_rows shapes without panicking on empty hops.
        let rows = trace_rows(short, now);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, "200");
    }
}
