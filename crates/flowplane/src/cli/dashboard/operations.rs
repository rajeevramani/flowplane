//! Operations tab (fpv2-55x.4): a per-team NACK-history view backed by the windowed
//! `GET /api/v1/teams/{team}/xds/nacks` read. ONE windowed call drives both the honest
//! "N in last 24h" header (the response's `window_total`) and the paged table (its `items`);
//! cursor paging feeds the response's `next_cursor` back as `before`. No Envoy admin, no
//! stream involvement — the operator workflow stays on this persisted CP surface.

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use super::super::client::{ReadError, RestClient};
use super::data::{AuthExpired, Panel};
use super::resources::encode_segment;

/// The header window: "N in last 24h". A computed instant serialized as RFC 3339 (`…Z`), never
/// the shorthand "24h" — the endpoint's `since` contract is an RFC-3339 timestamp.
const WINDOW_HOURS: i64 = 24;
/// Rows per page.
const PAGE_LIMIT: i64 = 50;

// =============================================================================================
// Upstream item shapes (typed decode; a decode failure surfaces as Unavailable, never a
// silently empty panel).
// =============================================================================================

#[derive(Debug, Deserialize)]
struct NackItem {
    node_id: String,
    type_url: String,
    version_rejected: String,
    error_message: String,
    #[serde(default)]
    quarantined_resources: Vec<String>,
    created_at: DateTime<Utc>,
}

/// The `{ items, window_total, next_cursor }` envelope. `items` and `window_total` are required
/// (a response missing either is malformed → the panel degrades to Unavailable, never a
/// deceptively-empty "No NACKs"); `next_cursor` is genuinely absent on the last page.
#[derive(Debug, Deserialize)]
struct NackPageItem {
    items: Vec<NackItem>,
    window_total: i64,
    #[serde(default)]
    next_cursor: Option<String>,
}

// =============================================================================================
// Rendered rows.
// =============================================================================================

#[derive(Debug)]
pub(super) struct NackRow {
    pub(super) created_at: String,
    pub(super) node_id: String,
    pub(super) type_url: String,
    pub(super) version_rejected: String,
    pub(super) error_message: String,
    /// Quarantined resource names; each links into the F2 Resources view in the template.
    pub(super) quarantined: Vec<String>,
}

pub(super) struct OperationsPanel {
    pub(super) window_hours: i64,
    /// Honest windowed count for the header (`window_total` of the `since=now-24h` read).
    pub(super) window_total: i64,
    pub(super) rows: Vec<NackRow>,
    /// Already percent-encoded cursor for the "older" link's `?before=` value; `None` on the
    /// last page.
    pub(super) next_cursor: Option<String>,
}

fn is_unauthorized(result: &Result<serde_json::Value, ReadError>) -> bool {
    matches!(
        result,
        Err(ReadError::Status { status, .. }) if *status == reqwest::StatusCode::UNAUTHORIZED
    )
}

/// Fetch one page of the 24h NACK window. `before` is the (raw) cursor from a prior page.
/// 401 → AuthExpired (stop polling / re-login); 403 → Unauthorized panel; decode/5xx → Unavailable.
pub(super) async fn fetch(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
    before: Option<&str>,
) -> Result<Panel<OperationsPanel>, AuthExpired> {
    // Computed instant, serialized RFC 3339 with a `Z` suffix (no `+00:00`) so the query value
    // needs no special handling; still percent-encoded defensively.
    let since =
        (now - Duration::hours(WINDOW_HOURS)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut path = format!(
        "/api/v1/teams/{team}/xds/nacks?since={}&limit={PAGE_LIMIT}",
        encode_segment(&since)
    );
    if let Some(cursor) = before {
        path.push_str(&format!("&before={}", encode_segment(cursor)));
    }

    let result = client.get_json(&path).await;
    if is_unauthorized(&result) {
        return Err(AuthExpired);
    }
    let page: NackPageItem = match result {
        Ok(value) => match serde_json::from_value(value) {
            Ok(parsed) => parsed,
            Err(_) => return Ok(Panel::Unavailable),
        },
        Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::FORBIDDEN => {
            return Ok(Panel::Unauthorized)
        }
        Err(_) => return Ok(Panel::Unavailable),
    };

    let rows = page
        .items
        .into_iter()
        .map(|item| NackRow {
            created_at: item
                .created_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            node_id: item.node_id,
            type_url: item.type_url,
            version_rejected: item.version_rejected,
            error_message: item.error_message,
            quarantined: item.quarantined_resources,
        })
        .collect();

    Ok(Panel::Data(OperationsPanel {
        window_hours: WINDOW_HOURS,
        window_total: page.window_total,
        rows,
        next_cursor: page.next_cursor.as_deref().map(encode_segment),
    }))
}
