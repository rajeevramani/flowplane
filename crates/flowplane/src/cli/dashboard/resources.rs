//! Resources explorer data path (fpv2-cxw.1): the paged sweep engine and the tables
//! view-models for the three gateway collections (clusters, route-configs, listeners).
//!
//! Acquisition contract (approved design, "Bounded acquisition contract"):
//! - a sweep fetches EVERY page of one team-scoped list GET (limit cap 500, offset
//!   paging), sequentially — [`MAX_IN_FLIGHT_PAGES`] pins the in-flight bound;
//! - the byte budget is enforced DURING the sweep: fetching stops the moment the
//!   running total crosses it, and what was fetched renders with an explicit
//!   "partial — first N of M" state;
//! - failure classes: upstream 401 → dashboard-global re-login; 403 → that panel's
//!   unauthorized state; 404/5xx/transport on the FIRST page → unavailable panel;
//!   the same after earlier pages succeeded → partial data, never silently dropped;
//! - interleaved writes are tolerated: a short page ends the sweep (totals may have
//!   drifted); the result is a best-effort snapshot.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use super::super::client::{ReadError, RestClient};
use super::data::{humanize_age, AuthExpired, Panel};

/// Upstream collections the resources explorer may list. Every sweep path is
/// `/api/v1/teams/{team}/<segment>` plus paging query — this table is the dashboard's
/// upstream allowlist for the resources explorer, and no entry may ever name a secret
/// VALUE route (the CP has none; the design pins the assertion). Per-domain rate-limit
/// policy sweeps use [`policies_sub_path`], the only non-flat path shape.
pub(super) const COLLECTIONS: &[&str] = &[
    "clusters",
    "route-configs",
    "listeners",
    "rate-limit-domains",
    // Metadata-only list endpoint: every response redacts the value
    // (`value_redacted: true`); the CP has NO secret-value read route at all.
    "secrets",
    // Needed for secret used-by resolution (`credential_secret_id`).
    "ai/providers",
    // AI Gateway tab (ui-f5 S4): routes, budgets (state included in the list payload),
    // all read-only paginated GETs.
    "ai/routes",
    "ai/budgets",
    // API lifecycle read model (ui-f4 S7): definitions plus their per-API sub-lists
    // (specs / events / route-bindings / tools), all read-only paginated GETs.
    "api-definitions",
    // Learning tab (ui-f4 S8): capture-session metadata list (no observation bodies).
    "learning-sessions",
];

/// Percent-encode one path segment (RFC 3986 unreserved kept verbatim). Rate-limit
/// domain names are free text (1–253 chars, control chars excluded) and may contain
/// `/`, `?`, `#`, `%` — they must never alter which upstream path is requested.
pub(super) fn encode_segment(raw: &str) -> String {
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

/// The one allowlisted non-flat sweep target: a domain's policies list.
pub(super) fn policies_sub_path(domain: &str) -> String {
    format!("rate-limit-domains/{}/policies", encode_segment(domain))
}

/// Release-safe sweep allowlist: the flat COLLECTIONS, per-domain policy sub-lists, and
/// the API-lifecycle metadata sub-lists. Spec CONTENT paths are structurally excluded —
/// the dashboard never fetches a spec document body.
pub(super) fn allowed_sweep_segment(segment: &str) -> bool {
    if COLLECTIONS.contains(&segment) || segment.ends_with("/policies") {
        return true;
    }
    let Some(rest) = segment.strip_prefix("api-definitions/") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        [_api, "specs"] | [_api, "route-bindings"] | [_api, "tools"] => true,
        [_api, "specs", version, "events"] => version.parse::<i64>().is_ok(),
        _ => false,
    }
}

/// The API's list-page cap (`fp-api` `ListQuery`, cap 500) — a cap, not a guarantee.
pub(super) const PAGE_LIMIT: i64 = 500;

/// Per-sweep byte budget (design: ~10 MiB starting threshold, adjustable). Bytes are
/// the ACTUAL response-body bytes received for each fetched page.
pub(super) const SWEEP_BYTE_BUDGET: usize = 10 * 1024 * 1024;

/// Bounded concurrency for page GETs within one sweep. Pages are fetched sequentially
/// (offsets depend on prior pages), so the bound is 1; the constant names the contract
/// and the test asserts it is never exceeded.
pub(super) const MAX_IN_FLIGHT_PAGES: usize = 1;

// The sweep loop below awaits each page before requesting the next; a bound other than
// 1 would require restructuring it. Keep the constant honest.
const _: () = assert!(MAX_IN_FLIGHT_PAGES == 1);

/// Why a sweep's data is incomplete. Rendered as an explicit per-panel notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PartialReason {
    /// The byte budget was crossed mid-pagination; remaining pages were not requested.
    Budget,
    /// A page failed (404/5xx/transport/decode) after earlier pages succeeded.
    UpstreamFailure,
}

/// A sweep that could not produce renderable data at all.
pub(super) enum SweepFailure {
    /// Upstream 401 — credentials are gone for the whole dashboard.
    AuthExpired,
    /// Upstream 403 — this collection is not readable by the principal.
    Unauthorized,
    /// First page failed; there is nothing to render.
    Unavailable,
}

/// All fetched items of one collection plus completeness metadata.
pub(super) struct Sweep {
    pub(super) items: Vec<Value>,
    /// Envelope `total` from the last successful page (the M in "first N of M").
    pub(super) total: i64,
    pub(super) partial: Option<PartialReason>,
}

#[derive(Debug, Deserialize)]
struct PageEnvelope {
    items: Vec<Value>,
    total: i64,
}

/// Fetch every page of `/api/v1/teams/{team}/{segment}` under `byte_budget`, per the
/// acquisition contract in the module docs.
pub(super) async fn sweep(
    client: &RestClient,
    team: &str,
    segment: &str,
    byte_budget: usize,
) -> Result<Sweep, SweepFailure> {
    // Fail closed at runtime, not just in debug builds: an unlisted segment (notably any
    // spec CONTENT path) is never fetched — it resolves to Unavailable instead.
    if !allowed_sweep_segment(segment) {
        debug_assert!(
            false,
            "sweep segment {segment:?} is not in the resources allowlist"
        );
        return Err(SweepFailure::Unavailable);
    }
    let mut items: Vec<Value> = Vec::new();
    let mut total: i64 = 0;
    let mut bytes: usize = 0;
    let mut offset: i64 = 0;
    loop {
        let path = format!("/api/v1/teams/{team}/{segment}?limit={PAGE_LIMIT}&offset={offset}");
        let failed = |items: Vec<Value>, total: i64| {
            if items.is_empty() {
                Err(SweepFailure::Unavailable)
            } else {
                Ok(Sweep {
                    items,
                    total,
                    partial: Some(PartialReason::UpstreamFailure),
                })
            }
        };
        let (page, page_bytes) = match client.get_json_sized(&path).await {
            Ok(sized) => sized,
            Err(ReadError::Status { status, .. })
                if status == reqwest::StatusCode::UNAUTHORIZED =>
            {
                return Err(SweepFailure::AuthExpired)
            }
            Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::FORBIDDEN => {
                return Err(SweepFailure::Unauthorized)
            }
            Err(_) => return failed(items, total),
        };
        bytes += page_bytes;
        let envelope: PageEnvelope = match serde_json::from_value(page) {
            Ok(envelope) => envelope,
            // A 2xx body that is not a list envelope is an upstream failure, not data.
            Err(_) => return failed(items, total),
        };
        total = envelope.total;
        let fetched = envelope.items.len() as i64;
        items.extend(envelope.items);
        offset += fetched;
        // Natural end first: a short page means the server has no more rows, even when
        // the envelope total drifted under interleaved writes (tolerated snapshot).
        // Completing the collection beats the budget — the budget only cancels
        // *remaining* pages.
        if fetched < PAGE_LIMIT || offset >= total {
            return Ok(Sweep {
                items,
                total,
                partial: None,
            });
        }
        if bytes > byte_budget {
            return Ok(Sweep {
                items,
                total,
                partial: Some(PartialReason::Budget),
            });
        }
    }
}

// =============================================================================================
// Typed table rows. Items that fail typed decode (version skew) render a fallback row
// rather than disappearing — nothing is silently dropped.
// =============================================================================================

#[derive(Debug, Deserialize)]
struct ResourceItem<S> {
    name: String,
    revision: i64,
    updated_at: DateTime<Utc>,
    spec: S,
}

#[derive(Debug)]
pub(super) struct ClusterRow {
    pub(super) name: String,
    pub(super) endpoints: usize,
    pub(super) aggregate: usize,
    pub(super) tls: bool,
    pub(super) lb_policy: String,
    pub(super) revision: i64,
    pub(super) updated: String,
    /// True when the item's spec did not decode as the domain type (version skew).
    pub(super) unparsed: bool,
}

#[derive(Debug)]
pub(super) struct RouteConfigRow {
    pub(super) name: String,
    pub(super) vhosts: usize,
    pub(super) routes: usize,
    pub(super) revision: i64,
    pub(super) updated: String,
    pub(super) unparsed: bool,
}

#[derive(Debug)]
pub(super) struct ListenerRow {
    pub(super) name: String,
    pub(super) bind: String,
    pub(super) protocol: String,
    pub(super) route_config: Option<String>,
    pub(super) filters: usize,
    pub(super) tls: bool,
    pub(super) revision: i64,
    pub(super) updated: String,
    pub(super) unparsed: bool,
}

/// One rendered table: rows + the "first N of M" partial notice when incomplete.
pub(super) struct Table<R> {
    pub(super) rows: Vec<R>,
    pub(super) total: i64,
    pub(super) partial: Option<PartialNotice>,
}

pub(super) struct PartialNotice {
    pub(super) shown: usize,
    pub(super) total: i64,
    pub(super) reason: PartialReason,
    pub(super) collection: &'static str,
}

impl PartialNotice {
    pub(super) fn budget(&self) -> bool {
        self.reason == PartialReason::Budget
    }
}

fn notice(sweep: &Sweep, collection: &'static str) -> Option<PartialNotice> {
    sweep.partial.map(|reason| PartialNotice {
        shown: sweep.items.len(),
        total: sweep.total,
        reason,
        collection,
    })
}

/// Fallback for an item whose spec/shape did not decode: keep the name if present.
fn item_name(item: &Value) -> String {
    item.get("name")
        .and_then(Value::as_str)
        .unwrap_or("(unknown)")
        .to_string()
}

fn enum_str<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(_) => "?".into(),
    }
}

fn cluster_row(item: Value, now: DateTime<Utc>) -> ClusterRow {
    match serde_json::from_value::<ResourceItem<fp_domain::gateway::ClusterSpec>>(item.clone()) {
        Ok(it) => ClusterRow {
            name: it.name,
            endpoints: it.spec.endpoints.len(),
            aggregate: it.spec.aggregate_clusters.len(),
            tls: it.spec.use_tls || it.spec.upstream_tls.is_some(),
            lb_policy: enum_str(&it.spec.lb_policy),
            revision: it.revision,
            updated: humanize_age(now, it.updated_at),
            unparsed: false,
        },
        Err(_) => ClusterRow {
            name: item_name(&item),
            endpoints: 0,
            aggregate: 0,
            tls: false,
            lb_policy: "?".into(),
            revision: 0,
            updated: "?".into(),
            unparsed: true,
        },
    }
}

fn route_config_row(item: Value, now: DateTime<Utc>) -> RouteConfigRow {
    match serde_json::from_value::<ResourceItem<fp_domain::gateway::route_config::RouteConfigSpec>>(
        item.clone(),
    ) {
        Ok(it) => RouteConfigRow {
            name: it.name,
            vhosts: it.spec.virtual_hosts.len(),
            routes: it.spec.virtual_hosts.iter().map(|v| v.routes.len()).sum(),
            revision: it.revision,
            updated: humanize_age(now, it.updated_at),
            unparsed: false,
        },
        Err(_) => RouteConfigRow {
            name: item_name(&item),
            vhosts: 0,
            routes: 0,
            revision: 0,
            updated: "?".into(),
            unparsed: true,
        },
    }
}

fn listener_row(item: Value, now: DateTime<Utc>) -> ListenerRow {
    match serde_json::from_value::<ResourceItem<fp_domain::gateway::listener::ListenerSpec>>(
        item.clone(),
    ) {
        Ok(it) => ListenerRow {
            bind: format!("{}:{}", it.spec.address, it.spec.port),
            protocol: enum_str(&it.spec.protocol),
            route_config: it.spec.route_config.clone(),
            filters: it.spec.http_filters.len(),
            tls: it.spec.tls_context.is_some(),
            name: it.name,
            revision: it.revision,
            updated: humanize_age(now, it.updated_at),
            unparsed: false,
        },
        Err(_) => ListenerRow {
            name: item_name(&item),
            bind: "?".into(),
            protocol: "?".into(),
            route_config: None,
            filters: 0,
            tls: false,
            revision: 0,
            updated: "?".into(),
            unparsed: true,
        },
    }
}

pub(super) fn to_panel<R>(
    result: Result<Sweep, SweepFailure>,
    collection: &'static str,
    row: impl Fn(Value) -> R,
) -> Result<Panel<Table<R>>, AuthExpired> {
    match result {
        Ok(sweep) => {
            let partial = notice(&sweep, collection);
            let total = sweep.total;
            Ok(Panel::Data(Table {
                rows: sweep.items.into_iter().map(row).collect(),
                total,
                partial,
            }))
        }
        Err(SweepFailure::AuthExpired) => Err(AuthExpired),
        Err(SweepFailure::Unauthorized) => Ok(Panel::Unauthorized),
        Err(SweepFailure::Unavailable) => Ok(Panel::Unavailable),
    }
}

pub(super) async fn fetch_clusters(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
) -> Result<Panel<Table<ClusterRow>>, AuthExpired> {
    let result = sweep(client, team, "clusters", SWEEP_BYTE_BUDGET).await;
    to_panel(result, "clusters", |item| cluster_row(item, now))
}

pub(super) async fn fetch_route_configs(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
) -> Result<Panel<Table<RouteConfigRow>>, AuthExpired> {
    let result = sweep(client, team, "route-configs", SWEEP_BYTE_BUDGET).await;
    to_panel(result, "route configs", |item| route_config_row(item, now))
}

pub(super) async fn fetch_listeners(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
) -> Result<Panel<Table<ListenerRow>>, AuthExpired> {
    let result = sweep(client, team, "listeners", SWEEP_BYTE_BUDGET).await;
    to_panel(result, "listeners", |item| listener_row(item, now))
}

// =============================================================================================
// Topology (fpv2-cxw.2): three sweeps folded into one derived panel.
// =============================================================================================

/// The topology panel's states. A derived panel needs all three collections, so a 403
/// or first-page failure on ANY collection resolves the whole panel to that state,
/// naming the collection.
pub(super) enum TopologyPanel {
    Topology {
        topo: super::joins::Topology,
        notices: Vec<PartialNotice>,
    },
    /// Node budget crossed: the tables render instead, with an explicit notice.
    DegradedTables {
        nodes: usize,
        clusters: Table<ClusterRow>,
        route_configs: Table<RouteConfigRow>,
        listeners: Table<ListenerRow>,
        notices: Vec<PartialNotice>,
    },
    Unauthorized {
        collection: &'static str,
    },
    Unavailable {
        collection: &'static str,
    },
}

pub(super) async fn fetch_topology(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
) -> Result<TopologyPanel, AuthExpired> {
    enum Failed {
        Auth,
        Panel(Box<TopologyPanel>),
    }
    let one = |result: Result<Sweep, SweepFailure>, collection: &'static str| match result {
        Ok(s) => Ok(s),
        Err(SweepFailure::AuthExpired) => Err(Failed::Auth),
        Err(SweepFailure::Unauthorized) => {
            Err(Failed::Panel(Box::new(TopologyPanel::Unauthorized {
                collection,
            })))
        }
        Err(SweepFailure::Unavailable) => {
            Err(Failed::Panel(Box::new(TopologyPanel::Unavailable {
                collection,
            })))
        }
    };
    let run = async {
        let listeners = one(
            sweep(client, team, "listeners", SWEEP_BYTE_BUDGET).await,
            "listeners",
        )?;
        let route_configs = one(
            sweep(client, team, "route-configs", SWEEP_BYTE_BUDGET).await,
            "route-configs",
        )?;
        let clusters = one(
            sweep(client, team, "clusters", SWEEP_BYTE_BUDGET).await,
            "clusters",
        )?;
        Ok((listeners, route_configs, clusters))
    };
    let (listeners, route_configs, clusters) = match run.await {
        Ok(sweeps) => sweeps,
        Err(Failed::Auth) => return Err(AuthExpired),
        Err(Failed::Panel(panel)) => return Ok(*panel),
    };

    let notices: Vec<PartialNotice> = [
        notice(&listeners, "listeners"),
        notice(&route_configs, "route configs"),
        notice(&clusters, "clusters"),
    ]
    .into_iter()
    .flatten()
    .collect();

    let topo =
        super::joins::build_topology(&listeners.items, &route_configs.items, &clusters.items);
    if topo.degraded {
        let nodes = topo.nodes;
        let clusters_table = Table {
            total: clusters.total,
            partial: notice(&clusters, "clusters"),
            rows: clusters
                .items
                .into_iter()
                .map(|item| cluster_row(item, now))
                .collect(),
        };
        let route_configs_table = Table {
            total: route_configs.total,
            partial: notice(&route_configs, "route configs"),
            rows: route_configs
                .items
                .into_iter()
                .map(|item| route_config_row(item, now))
                .collect(),
        };
        let listeners_table = Table {
            total: listeners.total,
            partial: notice(&listeners, "listeners"),
            rows: listeners
                .items
                .into_iter()
                .map(|item| listener_row(item, now))
                .collect(),
        };
        return Ok(TopologyPanel::DegradedTables {
            nodes,
            clusters: clusters_table,
            route_configs: route_configs_table,
            listeners: listeners_table,
            notices,
        });
    }
    Ok(TopologyPanel::Topology { topo, notices })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::cli::config::EffectiveConfig;
    use axum::extract::{Query, State};
    use axum::response::IntoResponse;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Mock upstream: serves `total_items` cluster rows in `PAGE_LIMIT` pages, records
    /// every offset requested, tracks the max number of concurrent in-flight requests,
    /// and can fail a specific page with a canned status.
    struct Upstream {
        total_items: usize,
        fail_page: Option<(usize, u16)>,
        offsets: Mutex<Vec<i64>>,
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
        page_bytes: Mutex<Vec<usize>>,
    }

    #[derive(serde::Deserialize)]
    struct Paging {
        #[serde(default)]
        offset: i64,
        #[serde(default)]
        limit: i64,
    }

    async fn upstream_handler(
        State(state): State<Arc<Upstream>>,
        Query(paging): Query<Paging>,
    ) -> axum::response::Response {
        let current = state.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        state.max_in_flight.fetch_max(current, Ordering::SeqCst);
        // Hold briefly so overlapping requests would be observed as concurrency.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let page_index = state.offsets.lock().unwrap().len();
        state.offsets.lock().unwrap().push(paging.offset);

        let response = if let Some((fail_at, status)) = state.fail_page {
            if page_index == fail_at {
                state.in_flight.fetch_sub(1, Ordering::SeqCst);
                return (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    axum::Json(json!({"code": "err", "message": "canned failure"})),
                )
                    .into_response();
            } else {
                page_body(&state, paging)
            }
        } else {
            page_body(&state, paging)
        };
        state.in_flight.fetch_sub(1, Ordering::SeqCst);
        response
    }

    fn page_body(state: &Upstream, paging: Paging) -> axum::response::Response {
        let start = paging.offset.max(0) as usize;
        let end = (start + paging.limit.max(0) as usize).min(state.total_items);
        let items: Vec<serde_json::Value> = (start..end)
            .map(|i| {
                json!({
                    "id": uuid::Uuid::now_v7().to_string(),
                    "name": format!("c-{i}"),
                    "revision": 1,
                    "created_at": "2026-07-18T00:00:00Z",
                    "updated_at": "2026-07-18T00:00:00Z",
                    "spec": { "endpoints": [{"host": "10.0.0.1", "port": 8080}] }
                })
            })
            .collect();
        let body = json!({
            "items": items,
            "total": state.total_items as i64,
            "limit": paging.limit,
            "offset": paging.offset
        });
        // axum's Json writes exactly serde_json::to_vec bytes, so this records the same
        // count the client's text().len() sees.
        let wire_bytes = serde_json::to_vec(&body).map(|v| v.len()).unwrap_or(0);
        state.page_bytes.lock().unwrap().push(wire_bytes);
        axum::Json(body).into_response()
    }

    async fn start_upstream(
        total_items: usize,
        fail_page: Option<(usize, u16)>,
    ) -> (String, Arc<Upstream>) {
        let state = Arc::new(Upstream {
            total_items,
            fail_page,
            offsets: Mutex::new(Vec::new()),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
            page_bytes: Mutex::new(Vec::new()),
        });
        let app = axum::Router::new()
            .fallback(upstream_handler)
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock upstream");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), state)
    }

    fn test_client(server: &str) -> RestClient {
        RestClient::for_tests(EffectiveConfig {
            server: server.to_string(),
            org: Some("test-org".into()),
            team: Some("test-team".into()),
            token: Some("test-bearer-token".into()),
            token_source: None,
            dev_fallback_available: false,
            timeout: 5,
            oidc_issuer: None,
            oidc_client_id: None,
            oidc_scope: None,
            callback_url: None,
        })
    }

    #[tokio::test]
    async fn sweep_traverses_every_page() {
        let (url, state) = start_upstream(1200, None).await;
        let client = test_client(&url);
        let out = sweep(&client, "team-x", "clusters", SWEEP_BYTE_BUDGET)
            .await
            .ok()
            .expect("sweep ok");
        assert_eq!(out.items.len(), 1200, "all pages collected");
        assert_eq!(out.total, 1200);
        assert!(out.partial.is_none());
        assert_eq!(
            state.offsets.lock().unwrap().clone(),
            vec![0, 500, 1000],
            "offset paging must walk 0/500/1000"
        );
    }

    #[tokio::test]
    async fn budget_cancels_sweep_mid_pagination() {
        // 4 full-ish pages (2000 items). Budget = page1 bytes + 1 → page 2 crosses it;
        // pages 3 and 4 must never be requested.
        let (url, state) = start_upstream(2000, None).await;
        let client = test_client(&url);
        // Probe page 1's encoded size with a throwaway sweep of budget 0 (fetches page 1,
        // then stops on budget).
        let probe = sweep(&client, "team-x", "clusters", 0).await.ok().unwrap();
        assert_eq!(probe.items.len(), 500);
        assert_eq!(probe.partial, Some(PartialReason::Budget));
        let page1_bytes = state.page_bytes.lock().unwrap()[0];
        state.offsets.lock().unwrap().clear();
        state.page_bytes.lock().unwrap().clear();

        let out = sweep(&client, "team-x", "clusters", page1_bytes + 1)
            .await
            .ok()
            .unwrap();
        assert_eq!(out.items.len(), 1000, "pages 1+2 kept");
        assert_eq!(out.total, 2000, "M comes from the envelope total");
        assert_eq!(out.partial, Some(PartialReason::Budget));
        assert_eq!(
            state.offsets.lock().unwrap().clone(),
            vec![0, 500],
            "pages 3 and 4 must never be requested after the budget is crossed"
        );
    }

    #[tokio::test]
    async fn failure_classes_map_per_design() {
        // 401 anywhere → AuthExpired.
        let (url, _) = start_upstream(10, Some((0, 401))).await;
        assert!(matches!(
            sweep(&test_client(&url), "t", "clusters", SWEEP_BYTE_BUDGET).await,
            Err(SweepFailure::AuthExpired)
        ));
        // 403 → Unauthorized.
        let (url, _) = start_upstream(10, Some((0, 403))).await;
        assert!(matches!(
            sweep(&test_client(&url), "t", "clusters", SWEEP_BYTE_BUDGET).await,
            Err(SweepFailure::Unauthorized)
        ));
        // First-page 500 → Unavailable (nothing to render).
        let (url, _) = start_upstream(10, Some((0, 500))).await;
        assert!(matches!(
            sweep(&test_client(&url), "t", "clusters", SWEEP_BYTE_BUDGET).await,
            Err(SweepFailure::Unavailable)
        ));
        // First-page 404 → Unavailable.
        let (url, _) = start_upstream(10, Some((0, 404))).await;
        assert!(matches!(
            sweep(&test_client(&url), "t", "clusters", SWEEP_BYTE_BUDGET).await,
            Err(SweepFailure::Unavailable)
        ));
        // Mid-sweep 500 (page 2 of 3) → partial data, earlier pages kept.
        let (url, _) = start_upstream(1200, Some((1, 500))).await;
        let out = sweep(&test_client(&url), "t", "clusters", SWEEP_BYTE_BUDGET)
            .await
            .ok()
            .unwrap();
        assert_eq!(out.items.len(), 500, "page 1 kept");
        assert_eq!(out.partial, Some(PartialReason::UpstreamFailure));
        // Mid-sweep 404 → same partial class.
        let (url, _) = start_upstream(1200, Some((1, 404))).await;
        let out = sweep(&test_client(&url), "t", "clusters", SWEEP_BYTE_BUDGET)
            .await
            .ok()
            .unwrap();
        assert_eq!(out.items.len(), 500);
        assert_eq!(out.partial, Some(PartialReason::UpstreamFailure));
    }

    #[tokio::test]
    async fn in_flight_page_gets_never_exceed_the_bound() {
        let (url, state) = start_upstream(1700, None).await;
        let out = sweep(&test_client(&url), "t", "clusters", SWEEP_BYTE_BUDGET)
            .await
            .ok()
            .unwrap();
        assert_eq!(out.items.len(), 1700);
        assert!(
            state.max_in_flight.load(Ordering::SeqCst) <= MAX_IN_FLIGHT_PAGES,
            "in-flight page GETs must never exceed MAX_IN_FLIGHT_PAGES"
        );
    }

    #[tokio::test]
    async fn short_page_under_drifted_total_is_tolerated_as_complete() {
        // The envelope claims 5000 items but the server only has 650: page 2 comes back
        // short (150 rows). The sweep ends complete — a tolerated snapshot, not partial.
        struct Drift;
        async fn drift_handler(Query(paging): Query<Paging>) -> axum::response::Response {
            let start = paging.offset.max(0) as usize;
            let end = (start + paging.limit.max(0) as usize).min(650);
            let items: Vec<serde_json::Value> = (start..end)
                .map(|i| {
                    json!({"name": format!("c-{i}"), "revision": 1,
                    "updated_at": "2026-07-18T00:00:00Z",
                    "spec": {"endpoints": []}})
                })
                .collect();
            axum::Json(json!({"items": items, "total": 5000, "limit": paging.limit,
                "offset": paging.offset}))
            .into_response()
        }
        let _ = Drift;
        let app = axum::Router::new().fallback(drift_handler);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let out = sweep(
            &test_client(&format!("http://{addr}")),
            "t",
            "clusters",
            SWEEP_BYTE_BUDGET,
        )
        .await
        .ok()
        .unwrap();
        assert_eq!(out.items.len(), 650);
        assert!(out.partial.is_none(), "short page ends the sweep cleanly");
    }

    /// Codex S7 review finding 2: the sweep allowlist must structurally exclude spec
    /// CONTENT paths in release builds, not only via debug_assert. Enumerates the
    /// allowed api-definitions subpath shapes and pins the content rejection.
    #[test]
    fn sweep_allowlist_rejects_spec_content_paths_at_runtime() {
        assert!(allowed_sweep_segment("api-definitions"));
        assert!(allowed_sweep_segment("api-definitions/catalog/specs"));
        assert!(allowed_sweep_segment(
            "api-definitions/catalog/route-bindings"
        ));
        assert!(allowed_sweep_segment("api-definitions/catalog/tools"));
        assert!(allowed_sweep_segment(
            "api-definitions/catalog/specs/3/events"
        ));
        assert!(!allowed_sweep_segment(
            "api-definitions/catalog/specs/3/content"
        ));
        assert!(!allowed_sweep_segment(
            "api-definitions/catalog/specs/abc/events"
        ));
        assert!(!allowed_sweep_segment("api-definitions/catalog/specs/3"));
        assert!(!allowed_sweep_segment("api-definitions/catalog"));
        assert!(!allowed_sweep_segment("secrets/value"));
        assert!(allowed_sweep_segment("rate-limit-domains/foo/policies"));
    }

    #[test]
    fn upstream_allowlist_has_no_secret_value_route() {
        // The exact allowlist: team-scoped LIST endpoints only. `secrets` is the
        // metadata list (values are write-only upstream); design AC 4 pins that no
        // secret-VALUE route can ever enter this table.
        assert_eq!(
            COLLECTIONS,
            &[
                "clusters",
                "route-configs",
                "listeners",
                "rate-limit-domains",
                "secrets",
                "ai/providers",
                // ui-f5 S4 (reviewed addition): AI Gateway tab. Routes and budgets are
                // read-only team-scoped LIST endpoints; budget state arrives inside the
                // list payload — no secret-value or content route enters the table.
                "ai/routes",
                "ai/budgets",
                // ui-f4 S7 (reviewed addition): API-lifecycle read model. Definitions
                // plus per-API metadata sub-lists (specs / events / route-bindings /
                // tools) — all read-only metadata; the spec CONTENT endpoint is
                // deliberately NOT swept by the APIs tab.
                "api-definitions",
                // ui-f4 S8 (reviewed addition): capture-session metadata list for the
                // Learning tab. Session bodies/observations are never fetched.
                "learning-sessions"
            ],
            "the upstream allowlist is closed; review any change against design AC 4"
        );
        for segment in COLLECTIONS {
            assert!(
                !segment.contains("value"),
                "no secret-value route may enter the resources allowlist: {segment:?}"
            );
        }
        // The only non-flat sweep shape stays inside rate-limit-domains and encodes
        // the tenant-chosen segment so it cannot rewrite the requested path.
        let sub = policies_sub_path("a/../secrets?x=1#f");
        assert!(sub.starts_with("rate-limit-domains/"));
        assert!(sub.ends_with("/policies"));
        assert!(
            !sub.contains("secrets?"),
            "hostile domain must be encoded: {sub}"
        );
        assert!(!sub.contains('#') && !sub.contains('?'));
        assert_eq!(sub.matches('/').count(), 2, "exactly one encoded segment");
    }

    #[test]
    fn encode_segment_round_trips_hostile_bytes() {
        assert_eq!(encode_segment("checkout"), "checkout");
        assert_eq!(encode_segment("a/b"), "a%2Fb");
        assert_eq!(encode_segment("a|b c%"), "a%7Cb%20c%25");
        assert_eq!(encode_segment("dom?x=1#f"), "dom%3Fx%3D1%23f");
    }

    fn now() -> DateTime<Utc> {
        "2026-07-18T00:01:00Z".parse().expect("ts")
    }

    #[test]
    fn cluster_row_maps_spec_fields() {
        let item = json!({
            "name": "checkout",
            "revision": 7,
            "updated_at": "2026-07-18T00:00:00Z",
            "spec": {
                "endpoints": [{"host": "10.0.0.1", "port": 8080}, {"host": "10.0.0.2", "port": 8080}],
                "aggregate_clusters": ["a", "b", "c"],
                "lb_policy": "least-request",
                "use_tls": true
            }
        });
        let row = cluster_row(item, now());
        assert!(!row.unparsed);
        assert_eq!(row.name, "checkout");
        assert_eq!(row.endpoints, 2);
        assert_eq!(row.aggregate, 3);
        assert!(row.tls);
        assert_eq!(row.lb_policy, "least-request");
        assert_eq!(row.revision, 7);
        assert_eq!(row.updated, "1m ago");
    }

    #[test]
    fn listener_and_route_config_rows_map_spec_fields() {
        let listener = json!({
            "name": "edge",
            "revision": 3,
            "updated_at": "2026-07-18T00:00:00Z",
            "spec": {
                "address": "0.0.0.0",
                "port": 8443,
                "protocol": "https",
                "route_config": "edge-routes",
                "http_filters": [
                    {"filter": {"type": "cors",
                        "allow_origin": [{"match": "exact", "value": "https://a.example.com"}],
                        "allow_methods": ["GET"]}}
                ],
                "tls_context": {"cert_chain_file": "/etc/tls/cert.pem", "private_key_file": "/etc/tls/key.pem"}
            }
        });
        let row = listener_row(listener, now());
        assert!(!row.unparsed, "listener fixture must decode");
        assert_eq!(row.bind, "0.0.0.0:8443");
        assert_eq!(row.protocol, "https");
        assert_eq!(row.route_config.as_deref(), Some("edge-routes"));
        assert_eq!(row.filters, 1);
        assert!(row.tls);

        let rc = json!({
            "name": "edge-routes",
            "revision": 2,
            "updated_at": "2026-07-18T00:00:00Z",
            "spec": {
                "virtual_hosts": [
                    {"name": "vh-a", "domains": ["a.example.com"], "routes": [
                        {"name": "r1", "match": {"prefix": {"prefix": "/"}},
                         "action": {"cluster": "checkout"}}
                    ]},
                    {"name": "vh-b", "domains": ["b.example.com"], "routes": []}
                ]
            }
        });
        let row = route_config_row(rc, now());
        assert!(!row.unparsed, "route-config fixture must decode");
        assert_eq!(row.vhosts, 2);
        assert_eq!(row.routes, 1);
    }

    #[test]
    fn undecodable_item_renders_a_fallback_row_not_nothing() {
        let junk = json!({"name": "mystery", "spec": {"totally": "unknown"}});
        let row = cluster_row(junk, now());
        assert!(row.unparsed);
        assert_eq!(
            row.name, "mystery",
            "the name survives for the fallback row"
        );
    }
}
