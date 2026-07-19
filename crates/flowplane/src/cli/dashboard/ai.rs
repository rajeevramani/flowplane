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
    total_tokens: u64,
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
}
