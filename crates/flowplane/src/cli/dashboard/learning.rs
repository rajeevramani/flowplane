//! Learning tab (ui-f4 S8): capture sessions with completed sessions linking to the
//! spec versions their API produced, plus the read-only OpenAPI content viewer.
//!
//! The viewer is the dashboard's ONLY spec-content fetch: a single non-sweep
//! conditional GET of the CP's `/content` endpoint (the sweep allowlist keeps content
//! paths structurally unreachable for every sweep). Responses to the browser carry the
//! dashboard-global `no-store` headers; the per-launch in-process cache revalidates by
//! ETag (`If-None-Match` → 304) so an unchanged document is not re-downloaded.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

use super::super::client::{ConditionalRead, ReadError, RestClient};
use super::data::{humanize_age, AuthExpired, Panel};
use super::resources::{encode_segment, sweep, PartialNotice, SweepFailure, SWEEP_BYTE_BUDGET};

// =============================================================================================
// Sessions panel.
// =============================================================================================

#[derive(Debug, Deserialize)]
struct SessionItem {
    id: String,
    name: String,
    status: String,
    #[serde(default)]
    api_definition_id: Option<String>,
    sample_count: i64,
    path_count: i64,
    #[serde(default)]
    completed_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct SpecMetaItem {
    version: i64,
    source_kind: String,
    #[serde(default)]
    capture_session_id: Option<String>,
}

/// One learned spec version a completed session's API produced — the viewer link target.
#[derive(Debug)]
pub(super) struct ProducedSpec {
    pub(super) api: String,
    pub(super) version: i64,
}

#[derive(Debug)]
pub(super) struct SessionRow {
    pub(super) name: String,
    pub(super) status: String,
    pub(super) api: String,
    pub(super) samples: i64,
    pub(super) paths: i64,
    pub(super) age: String,
    pub(super) completed: bool,
    /// Learned spec versions this session produced (completed sessions only), matched by
    /// the versions' capture-session provenance.
    pub(super) produced: Vec<ProducedSpec>,
    /// True when the upstream session row failed typed decode (version skew): rendered
    /// as an explicit fallback row, never silently dropped.
    pub(super) unparsed: bool,
}

pub(super) struct LearningPanel {
    pub(super) rows: Vec<SessionRow>,
    pub(super) total: i64,
    /// Spec-metadata rows that failed typed decode (version skew) — surfaced, not
    /// silently dropped.
    pub(super) unparsed_specs: usize,
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

pub(super) async fn fetch_learning(
    client: &RestClient,
    team: &str,
    now: DateTime<Utc>,
) -> Result<Panel<LearningPanel>, AuthExpired> {
    let mut notices = Vec::new();
    let sessions_result = sweep(client, team, "learning-sessions", SWEEP_BYTE_BUDGET).await;
    let sessions = match record_notice(sessions_result, "learning sessions", &mut notices) {
        Ok(items) => items,
        Err(SweepFailure::AuthExpired) => return Err(AuthExpired),
        Err(SweepFailure::Unauthorized) => return Ok(Panel::Unauthorized),
        Err(SweepFailure::Unavailable) => return Ok(Panel::Unavailable),
    };
    let total = sessions.len() as i64;
    // Typed-decode failures render fallback rows (name kept when present) — nothing is
    // silently dropped (dashboard acquisition convention).
    let mut unparsed_rows: Vec<SessionRow> = Vec::new();
    let sessions: Vec<SessionItem> = sessions
        .into_iter()
        .filter_map(|item| match serde_json::from_value(item.clone()) {
            Ok(session) => Some(session),
            Err(_) => {
                unparsed_rows.push(SessionRow {
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("(unknown)")
                        .to_string(),
                    status: String::new(),
                    api: String::new(),
                    samples: 0,
                    paths: 0,
                    age: String::new(),
                    completed: false,
                    produced: Vec::new(),
                    unparsed: true,
                });
                None
            }
        })
        .collect();

    // API id → name map for the session rows (and viewer links).
    let mut api_names: BTreeMap<String, String> = BTreeMap::new();
    if sessions.iter().any(|s| s.api_definition_id.is_some()) {
        let apis_result = sweep(client, team, "api-definitions", SWEEP_BYTE_BUDGET).await;
        match record_notice(apis_result, "api definitions", &mut notices) {
            Ok(items) => {
                for item in items {
                    if let (Some(id), Some(name)) = (
                        item.get("id").and_then(Value::as_str),
                        item.get("name").and_then(Value::as_str),
                    ) {
                        api_names.insert(id.to_string(), name.to_string());
                    }
                }
            }
            Err(SweepFailure::AuthExpired) => return Err(AuthExpired),
            // Without names there are no viewer links; surface it, keep the sessions.
            Err(_) => notices.push(PartialNotice {
                shown: 0,
                total: 0,
                reason: super::resources::PartialReason::UpstreamFailure,
                collection: "api definitions",
            }),
        }
    }

    // Learned spec versions keyed by their PRODUCING capture session (the versions'
    // provenance stamp), fetched once per distinct completed-session API — metadata
    // sweeps only, never content. A learned version with no/unknown provenance is not
    // attributed to any session.
    let mut produced_by_session: BTreeMap<String, Vec<ProducedSpec>> = BTreeMap::new();
    let mut unparsed_specs = 0_usize;
    let completed_apis: std::collections::BTreeSet<&String> = sessions
        .iter()
        .filter(|s| s.completed_at.is_some())
        .filter_map(|s| s.api_definition_id.as_ref())
        .collect();
    for api_id in completed_apis {
        let Some(api_name) = api_names.get(api_id) else {
            continue;
        };
        let seg = encode_segment(api_name);
        let specs_result = sweep(
            client,
            team,
            &format!("api-definitions/{seg}/specs"),
            SWEEP_BYTE_BUDGET,
        )
        .await;
        match record_notice(specs_result, "spec versions", &mut notices) {
            Ok(items) => {
                for item in items {
                    let Ok(meta) = serde_json::from_value::<SpecMetaItem>(item) else {
                        unparsed_specs += 1;
                        continue;
                    };
                    if meta.source_kind != "learned" {
                        continue;
                    }
                    let Some(session_id) = meta.capture_session_id else {
                        continue;
                    };
                    produced_by_session
                        .entry(session_id)
                        .or_default()
                        .push(ProducedSpec {
                            api: api_name.clone(),
                            version: meta.version,
                        });
                }
            }
            Err(SweepFailure::AuthExpired) => return Err(AuthExpired),
            Err(_) => notices.push(PartialNotice {
                shown: 0,
                total: 0,
                reason: super::resources::PartialReason::UpstreamFailure,
                collection: "spec versions",
            }),
        }
    }
    for produced in produced_by_session.values_mut() {
        produced.sort_by_key(|p| std::cmp::Reverse(p.version));
    }

    let mut rows: Vec<SessionRow> = sessions
        .into_iter()
        .map(|s| {
            let completed = s.completed_at.is_some();
            let api = s
                .api_definition_id
                .as_ref()
                .and_then(|id| api_names.get(id))
                .cloned()
                .unwrap_or_else(|| "—".into());
            let produced = if completed {
                produced_by_session
                    .get(&s.id)
                    .map(|specs| {
                        specs
                            .iter()
                            .map(|p| ProducedSpec {
                                api: p.api.clone(),
                                version: p.version,
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            SessionRow {
                name: s.name,
                status: s.status,
                api,
                samples: s.sample_count,
                paths: s.path_count,
                age: humanize_age(now, s.completed_at.unwrap_or(s.created_at)),
                completed,
                produced,
                unparsed: false,
            }
        })
        .collect();
    rows.extend(unparsed_rows);
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Panel::Data(LearningPanel {
        rows,
        total,
        unparsed_specs,
        notices,
    }))
}

// =============================================================================================
// Content viewer.
// =============================================================================================

pub(super) enum ContentView {
    /// Pretty-printed document plus the hash it revalidates under.
    Document {
        api: String,
        version: i64,
        etag: String,
        pretty: String,
        /// True when this render came from the per-launch cache after a 304.
        revalidated: bool,
    },
    Unauthorized,
    Unavailable,
}

/// Per-launch viewer cache: (api, version) → (etag, pretty-printed document).
pub(super) type ContentCache = std::sync::Mutex<BTreeMap<(String, i64), (String, String)>>;

pub(super) async fn fetch_content(
    client: &RestClient,
    team: &str,
    api: &str,
    version: i64,
    cache: &ContentCache,
) -> Result<ContentView, AuthExpired> {
    let key = (api.to_string(), version);
    // A poisoned lock only means a prior panic mid-insert; the cache stays usable.
    let cached = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .cloned();
    let path = format!(
        "/api/v1/teams/{team}/api-definitions/{}/specs/{version}/content",
        encode_segment(api)
    );
    let result = client
        .get_json_conditional(&path, cached.as_ref().map(|(etag, _)| etag.as_str()))
        .await;
    match result {
        Ok(ConditionalRead::NotModified) => {
            let Some((etag, pretty)) = cached else {
                // A 304 with no cache entry cannot happen (we only send If-None-Match
                // from the cache); treat defensively as unavailable.
                return Ok(ContentView::Unavailable);
            };
            Ok(ContentView::Document {
                api: api.to_string(),
                version,
                etag,
                pretty,
                revalidated: true,
            })
        }
        Ok(ConditionalRead::Fresh { value, etag }) => {
            let pretty = serde_json::to_string_pretty(&value).unwrap_or_default();
            let etag = etag.unwrap_or_default();
            if !etag.is_empty() {
                cache
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(key, (etag.clone(), pretty.clone()));
            }
            Ok(ContentView::Document {
                api: api.to_string(),
                version,
                etag,
                pretty,
                revalidated: false,
            })
        }
        Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::UNAUTHORIZED => {
            Err(AuthExpired)
        }
        Err(ReadError::Status { status, .. }) if status == reqwest::StatusCode::FORBIDDEN => {
            Ok(ContentView::Unauthorized)
        }
        Err(_) => Ok(ContentView::Unavailable),
    }
}
