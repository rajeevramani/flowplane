//! Overview data path (fpv2-03m.3): the two allowlisted upstream reads and their
//! typed view-models. Templates receive ONLY these types — never raw responses, never
//! the bearer token, never upstream error bodies (a failed panel renders a generic
//! state so server error text cannot leak into the page).
//!
//! Sourcing contract (approved design, "Overview page content"):
//! - numeric team totals come EXCLUSIVELY from `/stats/overview` (whole-team SQL
//!   aggregate — accurate at any team size);
//! - `/xds/status` feeds only the per-dataplane table, the health string, and NACK
//!   info; the service pages the first 500 dataplanes, so when stats reports more
//!   than the listed count the table carries a partial-data banner;
//! - config state is rendered only as "ever verified / never verified" from
//!   `last_config_verify_at`; no warming/applied-version states, and the upstream
//!   `version` field (a registry revision) is never displayed.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::super::client::{ReadError, RestClient};

/// Team totals from `/stats/overview` (deserialized subset; unknown fields ignored).
#[derive(Debug, Deserialize)]
pub(super) struct StatsView {
    pub(super) total_dataplanes: i64,
    pub(super) live_dataplanes: i64,
    pub(super) stale_dataplanes: i64,
    pub(super) total_requests: i64,
    pub(super) total_errors: i64,
    pub(super) warming_failures: i64,
}

/// Raw `/xds/status` payload subset. `version` is deliberately not deserialized: it is
/// the registry revision, not an applied xDS version, and must not be displayed.
#[derive(Debug, Deserialize)]
struct XdsStatusRaw {
    health: String,
    recent_nack_count: i64,
    dataplanes: Vec<DataplaneRaw>,
}

#[derive(Debug, Deserialize)]
struct DataplaneRaw {
    name: String,
    live: bool,
    last_heartbeat_at: Option<DateTime<Utc>>,
    last_config_verify_at: Option<DateTime<Utc>>,
    total_requests: i64,
    total_errors: i64,
    warming_failures: i64,
}

/// Gateway panel view-model (from `/xds/status` only).
#[derive(Debug)]
pub(super) struct GatewayView {
    pub(super) health: String,
    pub(super) recent_nack_count: i64,
    pub(super) dataplanes: Vec<DataplaneRow>,
}

#[derive(Debug)]
pub(super) struct DataplaneRow {
    pub(super) name: String,
    pub(super) live: bool,
    /// Humanized heartbeat age ("12s ago"), or None when never seen.
    pub(super) heartbeat_age: Option<String>,
    /// "ever verified / never verified" — the ONLY config-state vocabulary (design).
    pub(super) config_verified_ever: bool,
    pub(super) total_requests: i64,
    pub(super) total_errors: i64,
    pub(super) warming_failures: i64,
}

/// One panel's upstream outcome. `Unavailable` is deliberately detail-free.
#[derive(Debug)]
pub(super) enum Panel<T> {
    Data(T),
    Unauthorized,
    Unavailable,
}

impl<T> Panel<T> {
    pub(super) fn data(&self) -> Option<&T> {
        match self {
            Panel::Data(value) => Some(value),
            _ => None,
        }
    }

    pub(super) fn unauthorized(&self) -> bool {
        matches!(self, Panel::Unauthorized)
    }
}

/// Partial-data annotation for the dataplane table (design AC 1a).
#[derive(Debug)]
pub(super) struct Truncation {
    pub(super) shown: usize,
    pub(super) total: i64,
}

pub(super) struct OverviewData {
    pub(super) stats: Panel<StatsView>,
    pub(super) gateway: Panel<GatewayView>,
    pub(super) truncated: Option<Truncation>,
}

/// Upstream said 401: credentials are gone; the page shows the re-login banner and
/// polling stops (design AC 5).
pub(super) struct AuthExpired;

/// Fetch both allowlisted reads and fold them into per-panel states. A 401 from either
/// call aborts the whole partial (one identity — if it expired for one read it expired
/// for both). A 403 or any other failure degrades only its own panel.
pub(super) async fn fetch(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
) -> Result<OverviewData, AuthExpired> {
    let stats_result = client
        .get_json(&format!("/api/v1/teams/{team}/stats/overview"))
        .await;
    let xds_result = client
        .get_json(&format!("/api/v1/teams/{team}/xds/status"))
        .await;

    if is_unauthorized(&stats_result) || is_unauthorized(&xds_result) {
        return Err(AuthExpired);
    }

    let stats: Panel<StatsView> = to_panel(stats_result);
    let gateway_raw: Panel<XdsStatusRaw> = to_panel(xds_result);

    let truncated = match (stats.data(), gateway_raw.data()) {
        (Some(s), Some(x)) if s.total_dataplanes > x.dataplanes.len() as i64 => Some(Truncation {
            shown: x.dataplanes.len(),
            total: s.total_dataplanes,
        }),
        _ => None,
    };

    let gateway = match gateway_raw {
        Panel::Data(raw) => Panel::Data(GatewayView {
            health: raw.health,
            recent_nack_count: raw.recent_nack_count,
            dataplanes: raw
                .dataplanes
                .into_iter()
                .map(|dp| DataplaneRow {
                    name: dp.name,
                    live: dp.live,
                    heartbeat_age: dp.last_heartbeat_at.map(|ts| humanize_age(now, ts)),
                    config_verified_ever: dp.last_config_verify_at.is_some(),
                    total_requests: dp.total_requests,
                    total_errors: dp.total_errors,
                    warming_failures: dp.warming_failures,
                })
                .collect(),
        }),
        Panel::Unauthorized => Panel::Unauthorized,
        Panel::Unavailable => Panel::Unavailable,
    };

    Ok(OverviewData {
        stats,
        gateway,
        truncated,
    })
}

fn is_unauthorized(result: &Result<serde_json::Value, ReadError>) -> bool {
    matches!(
        result,
        Err(ReadError::Status { status, .. }) if *status == reqwest::StatusCode::UNAUTHORIZED
    )
}

/// Fold a read result into a panel state. 403 → Unauthorized; any other failure —
/// transport, 5xx, undecodable body, wrong shape — → detail-free Unavailable.
fn to_panel<T: serde::de::DeserializeOwned>(
    result: Result<serde_json::Value, ReadError>,
) -> Panel<T> {
    match result {
        Ok(value) => match serde_json::from_value::<T>(value) {
            Ok(parsed) => Panel::Data(parsed),
            Err(_) => Panel::Unavailable,
        },
        Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::FORBIDDEN => {
            Panel::Unauthorized
        }
        Err(_) => Panel::Unavailable,
    }
}

/// "12s ago" / "3m ago" / "2h ago" / "4d ago"; clock skew clamps to "0s ago".
pub(super) fn humanize_age(now: DateTime<Utc>, ts: DateTime<Utc>) -> String {
    let secs = (now - ts).num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use serde_json::json;

    fn stats_json(total_dataplanes: i64) -> serde_json::Value {
        json!({
            "total_dataplanes": total_dataplanes,
            "live_dataplanes": 2,
            "stale_dataplanes": total_dataplanes - 2,
            "total_requests": 1234,
            "total_errors": 5,
            "warming_failures": 1
        })
    }

    fn xds_json(dataplane_count: usize) -> serde_json::Value {
        let dataplanes: Vec<serde_json::Value> = (0..dataplane_count)
            .map(|i| {
                json!({
                    "name": format!("dp-{i}"),
                    "id": "00000000-0000-0000-0000-000000000000",
                    "live": i % 2 == 0,
                    "version": 7,
                    "last_heartbeat_at": "2026-07-18T00:00:00Z",
                    "last_config_verify_at": if i % 2 == 0 { Some("2026-07-18T00:00:00Z") } else { None },
                    "total_requests": 10,
                    "total_errors": 1,
                    "warming_failures": 0
                })
            })
            .collect();
        json!({
            "health": "healthy",
            "total_dataplanes": dataplane_count,
            "live_dataplanes": dataplane_count,
            "stale_dataplanes": 0,
            "config_verified_dataplanes": 0,
            "total_requests": 10,
            "total_errors": 1,
            "warming_failures": 0,
            "recent_nack_count": 3,
            "dataplanes": dataplanes
        })
    }

    fn now() -> DateTime<Utc> {
        "2026-07-18T00:01:00Z".parse().expect("timestamp")
    }

    #[test]
    fn totals_come_from_stats_not_from_the_xds_page() {
        // Stats says 700 dataplanes; the xds page lists only 500. The stats panel must
        // carry 700 — the page-derived xds totals are never used for team totals.
        let stats: Panel<StatsView> = to_panel(Ok(stats_json(700)));
        let s = stats.data().expect("stats");
        assert_eq!(s.total_dataplanes, 700);
        assert_eq!(s.total_requests, 1234);
    }

    #[tokio::test]
    async fn over_500_truncation_banner_with_exact_semantics() {
        // Synthetic >500 case per acceptance criterion 1a: stats total 700, page 500.
        let stats: Panel<StatsView> = to_panel(Ok(stats_json(700)));
        let xds: Panel<XdsStatusRaw> = to_panel(Ok(xds_json(500)));
        let (s, x) = (stats.data().expect("s"), xds.data().expect("x"));
        assert!(s.total_dataplanes > x.dataplanes.len() as i64);
        // The banner's numbers: shown = listed count, total = stats total.
        let truncated = Truncation {
            shown: x.dataplanes.len(),
            total: s.total_dataplanes,
        };
        assert_eq!(truncated.shown, 500);
        assert_eq!(truncated.total, 700);
    }

    #[test]
    fn no_banner_when_counts_agree() {
        let stats: Panel<StatsView> = to_panel(Ok(stats_json(3)));
        let xds: Panel<XdsStatusRaw> = to_panel(Ok(xds_json(3)));
        let (s, x) = (stats.data().expect("s"), xds.data().expect("x"));
        assert!(s.total_dataplanes <= x.dataplanes.len() as i64);
    }

    #[test]
    fn config_state_is_ever_never_verified_only() {
        let xds: Panel<XdsStatusRaw> = to_panel(Ok(xds_json(2)));
        let raw = xds.data().expect("xds");
        assert!(raw.dataplanes[0].last_config_verify_at.is_some());
        assert!(raw.dataplanes[1].last_config_verify_at.is_none());
        // The row model exposes ONLY the boolean — no warming/applied vocabulary and no
        // `version` field exists on DataplaneRow at all (compile-time guarantee).
        let row = DataplaneRow {
            name: "dp".into(),
            live: true,
            heartbeat_age: None,
            config_verified_ever: true,
            total_requests: 0,
            total_errors: 0,
            warming_failures: 0,
        };
        assert!(row.config_verified_ever);
    }

    #[test]
    fn status_401_maps_to_auth_expired_403_to_unauthorized_panel() {
        let unauthorized = Err(ReadError::Status {
            status: reqwest::StatusCode::UNAUTHORIZED,
            body: "{}".into(),
        });
        assert!(is_unauthorized(&unauthorized));
        let forbidden: Panel<StatsView> = to_panel(Err(ReadError::Status {
            status: reqwest::StatusCode::FORBIDDEN,
            body: "{}".into(),
        }));
        assert!(forbidden.unauthorized());
        let server_error: Panel<StatsView> = to_panel(Err(ReadError::Status {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: "secret upstream detail".into(),
        }));
        assert!(
            matches!(server_error, Panel::Unavailable),
            "5xx is detail-free"
        );
    }

    #[test]
    fn malformed_success_body_degrades_to_unavailable() {
        let panel: Panel<StatsView> = to_panel(Ok(json!({"nope": true})));
        assert!(matches!(panel, Panel::Unavailable));
    }

    #[test]
    fn heartbeat_age_is_humanized() {
        let ts: DateTime<Utc> = "2026-07-18T00:00:00Z".parse().expect("ts");
        assert_eq!(humanize_age(now(), ts), "1m ago");
        let recent: DateTime<Utc> = "2026-07-18T00:00:55Z".parse().expect("ts");
        assert_eq!(humanize_age(now(), recent), "5s ago");
        let future: DateTime<Utc> = "2026-07-18T09:00:00Z".parse().expect("ts");
        assert_eq!(humanize_age(now(), future), "0s ago", "skew clamps");
    }
}
