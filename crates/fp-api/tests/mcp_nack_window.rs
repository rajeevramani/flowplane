#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

//! Spec-driven black-box integration tests for the MCP `ops_xds_nacks` tool.
//!
//! The tool is driven purely over JSON-RPC (`tools/call`) and its `structuredContent`
//! result asserted against the documented NACK-window contract:
//!   `{ items: [...], window_total: <int>, next_cursor: <string|null> }`
//! with half-open `[since, until)` windowing, `window_total` independent of `limit`,
//! and `<created_at>,<id>` cursor paging fed back as `before`.
//!
//! Rows are seeded via raw SQL into `xds_nack_events` with whole-second timestamps so
//! exact-boundary assertions round-trip. All assertions are scoped to a freshly-seeded
//! team (constitution inv. 18) — never global counts.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Duration;
use fp_core::dev::DevIssuer;
use fp_domain::authz::TeamRef;
use fp_domain::OrgRole;
use fp_storage::repos::identity;
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use tower::ServiceExt;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

struct Fixture {
    app: axum::Router,
    pool: PgPool,
    admin_token: String,
    #[allow(dead_code)]
    member_token: String,
    team_name: String,
    #[allow(dead_code)]
    team_id: uuid::Uuid,
    team: TeamRef,
    other_team_name: String,
}

async fn fixture() -> Option<Fixture> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let issuer = DevIssuer::generate().expect("issuer");
    let validator = fp_core::OidcValidator::new(issuer.oidc_config());
    validator
        .load_jwks_json(issuer.jwks_json())
        .await
        .expect("jwks");

    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let other_team = identity::create_team(&pool, org.id, &unique("other"), "")
        .await
        .expect("other team");
    let admin_subject = unique("admin-sub");
    let member_subject = unique("member-sub");
    let admin = identity::upsert_user_by_subject(&pool, &admin_subject, "admin@test", "Admin")
        .await
        .expect("admin");
    let member = identity::upsert_user_by_subject(&pool, &member_subject, "member@test", "Member")
        .await
        .expect("member");
    identity::add_org_membership(&pool, admin, org.id, OrgRole::Admin)
        .await
        .expect("admin membership");
    identity::add_org_membership(&pool, member, org.id, OrgRole::Member)
        .await
        .expect("member membership");

    let admin_token = issuer
        .mint(&admin_subject, "admin@test", "Admin", 600)
        .expect("admin token");
    let member_token = issuer
        .mint(&member_subject, "member@test", "Member", 600)
        .expect("member token");

    let app = fp_api::build_router(fp_api::AppState {
        pool: pool.clone(),
        prometheus: PrometheusBuilder::new().build_recorder().handle(),
        version: "test",
        validator: Some(std::sync::Arc::new(validator)),
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        xds_degraded: None,
        discovery_forwarding_policy: Default::default(),
        egress_advisory: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    });

    Some(Fixture {
        app,
        pool,
        admin_token,
        member_token,
        team_name: team.name,
        team_id: team.id.as_uuid(),
        team: TeamRef {
            id: team.id,
            org_id: org.id,
        },
        other_team_name: other_team.name,
    })
}

fn mcp_request(token: &str, session: Option<&str>, body: serde_json::Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/mcp")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    if let Some(session) = session {
        builder = builder.header("mcp-session-id", session);
    }
    builder.body(Body::from(body.to_string())).expect("request")
}

async fn initialize(app: axum::Router, token: &str) -> String {
    let response = app
        .oneshot(mcp_request(
            token,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-11-25" }
            }),
        ))
        .await
        .expect("initialize");
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .expect("session")
        .to_string()
}

/// Raw `tools/list` returning the full JSON-RPC body so we can inspect a tool's
/// `inputSchema` (the name-only helper in `mcp_static_tools.rs` discards it).
async fn tools_list_raw(
    app: axum::Router,
    token: &str,
    session: &str,
    team: &str,
) -> serde_json::Value {
    let response = app
        .oneshot(mcp_request(
            token,
            Some(session),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": { "team": team }
            }),
        ))
        .await
        .expect("tools/list");
    assert_eq!(response.status(), StatusCode::OK);
    json_of(response).await
}

async fn tools_call(
    app: axum::Router,
    token: &str,
    session: &str,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let response = app
        .oneshot(mcp_request(
            token,
            Some(session),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": name,
                    "arguments": arguments
                }
            }),
        ))
        .await
        .expect("tools/call");
    assert_eq!(response.status(), StatusCode::OK);
    json_of(response).await
}

/// Insert one NACK row with a caller-controlled `created_at` (whole seconds) and
/// return its id. Timestamps are seeded exactly so half-open boundary assertions
/// round-trip through Postgres `TIMESTAMPTZ` (microsecond) precision.
async fn seed_nack(
    pool: &PgPool,
    team: TeamRef,
    node_id: &str,
    created_at: DateTime<Utc>,
) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO xds_nack_events \
           (id, team_id, org_id, node_id, type_url, version_rejected, error_message, \
            quarantined_resources, created_at) \
         VALUES ($1, $2, $3, $4, \
            'type.googleapis.com/envoy.config.listener.v3.Listener', \
            '1', $5, '[\"lds-listener\"]'::jsonb, $6)",
    )
    .bind(id)
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(node_id)
    .bind(format!("rejected by {node_id}"))
    .bind(created_at)
    .execute(pool)
    .await
    .expect("seed nack");
    id
}

/// A fixed whole-second base instant so exact-boundary windows round-trip.
fn base_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-07-20T10:00:00Z")
        .expect("base time")
        .with_timezone(&Utc)
}

fn structured(result: &serde_json::Value) -> &serde_json::Value {
    assert_eq!(
        result["result"]["isError"], false,
        "tool call unexpectedly errored: {result}"
    );
    &result["result"]["structuredContent"]
}

fn item_ids(sc: &serde_json::Value) -> Vec<String> {
    sc["items"]
        .as_array()
        .expect("items array")
        .iter()
        .map(|i| i["id"].as_str().expect("item id").to_string())
        .collect()
}

/// AC5 shape: no window args → `items` array, integer `window_total`, `next_cursor`
/// present-or-null. Seed >1 row; items non-empty and window_total == rows seeded.
#[tokio::test]
async fn ac5_shape_reports_items_and_window_total() {
    let Some(fx) = fixture().await else {
        return;
    };
    let t = base_time();
    let mut seeded = Vec::new();
    for k in 0..3 {
        seeded.push(seed_nack(&fx.pool, fx.team, "node-a", t + Duration::seconds(k)).await);
    }

    let session = initialize(fx.app.clone(), &fx.admin_token).await;
    let result = tools_call(
        fx.app.clone(),
        &fx.admin_token,
        &session,
        "ops_xds_nacks",
        serde_json::json!({ "team": fx.team_name }),
    )
    .await;
    let sc = structured(&result);

    assert!(sc["items"].is_array(), "items must be an array");
    assert!(
        sc["window_total"].is_i64(),
        "window_total must be an integer"
    );
    assert!(
        sc.get("next_cursor").is_some(),
        "next_cursor key must be present (null or string)"
    );

    assert_eq!(sc["window_total"], serde_json::json!(3));
    assert_eq!(item_ids(sc).len(), 3, "all 3 seeded rows returned");
    // 3 rows < default limit 50 → no further page.
    assert!(sc["next_cursor"].is_null(), "no next page for 3 rows");
}

/// AC5 window_total independence + cursor paging: seed 5, page with limit=2. Each page
/// reports the FULL window_total (5), pages are disjoint, and their union is exactly the
/// 5 seeded ids in newest-first order with no dups/gaps.
#[tokio::test]
async fn ac5_paging_covers_window_newest_first_without_gaps() {
    let Some(fx) = fixture().await else {
        return;
    };
    let t = base_time();
    // Seed 5 rows at distinct whole-second times so newest-first order is unambiguous.
    let mut seeded_by_time: Vec<(i64, uuid::Uuid)> = Vec::new();
    for k in 0..5 {
        let id = seed_nack(&fx.pool, fx.team, "node-a", t + Duration::seconds(k)).await;
        seeded_by_time.push((k, id));
    }
    // Newest-first = descending created_at.
    let expected_order: Vec<String> = seeded_by_time
        .iter()
        .rev()
        .map(|(_, id)| id.to_string())
        .collect();

    let session = initialize(fx.app.clone(), &fx.admin_token).await;

    let mut collected: Vec<String> = Vec::new();
    let mut before: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut args = serde_json::json!({ "team": fx.team_name, "limit": 2 });
        if let Some(cursor) = &before {
            args["before"] = serde_json::json!(cursor);
        }
        let result = tools_call(
            fx.app.clone(),
            &fx.admin_token,
            &session,
            "ops_xds_nacks",
            args,
        )
        .await;
        let sc = structured(&result);

        // window_total is the full window every page, independent of limit.
        assert_eq!(
            sc["window_total"],
            serde_json::json!(5),
            "window_total must be the full window, not the page size"
        );

        let page = item_ids(sc);
        assert!(page.len() <= 2, "limit=2 must cap the page");
        if pages == 0 {
            assert_eq!(page.len(), 2, "first page of 5 with limit 2 is full");
            assert!(
                sc["next_cursor"].as_str().is_some(),
                "a further page exists → next_cursor is a non-null string"
            );
        }

        // Disjointness: no id from an earlier page reappears.
        for id in &page {
            assert!(
                !collected.contains(id),
                "pages must be disjoint; {id} seen twice"
            );
        }
        collected.extend(page);

        match sc["next_cursor"].as_str() {
            Some(cursor) => before = Some(cursor.to_string()),
            None => break,
        }
        pages += 1;
        assert!(pages < 10, "paging did not terminate");
    }

    assert_eq!(
        collected, expected_order,
        "union of all pages must equal the 5 seeded ids, newest-first, no dups/gaps"
    );
}

/// AC1/AC2 ties: N rows sharing ONE `created_at` must page through the total order
/// `(created_at, id)` with no duplicates or gaps — a timestamp-only cursor would skip/repeat here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac2_paging_over_equal_created_at_covers_every_row_once() {
    let Some(fx) = fixture().await else {
        return;
    };
    let t = base_time();
    // Five rows sharing the SAME created_at (the tie case the cursor must survive).
    let mut seeded: Vec<String> = Vec::new();
    for _ in 0..5 {
        seeded.push(
            seed_nack(&fx.pool, fx.team, "node-tie", t)
                .await
                .to_string(),
        );
    }
    seeded.sort();

    let session = initialize(fx.app.clone(), &fx.admin_token).await;

    let mut collected: Vec<String> = Vec::new();
    let mut before: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut args = serde_json::json!({ "team": fx.team_name, "limit": 2 });
        if let Some(cursor) = &before {
            args["before"] = serde_json::json!(cursor);
        }
        let result = tools_call(
            fx.app.clone(),
            &fx.admin_token,
            &session,
            "ops_xds_nacks",
            args,
        )
        .await;
        let sc = structured(&result);
        assert_eq!(
            sc["window_total"],
            serde_json::json!(5),
            "window_total counts all tied rows"
        );
        let page = item_ids(sc);
        assert!(page.len() <= 2, "limit=2 caps the page even under ties");
        for id in &page {
            assert!(
                !collected.contains(id),
                "tie paging must be disjoint; {id} seen twice"
            );
        }
        collected.extend(page);
        match sc["next_cursor"].as_str() {
            Some(cursor) => before = Some(cursor.to_string()),
            None => break,
        }
        pages += 1;
        assert!(pages < 10, "tie paging did not terminate");
    }
    collected.sort();
    assert_eq!(
        collected, seeded,
        "paging equal-timestamp rows must cover every id exactly once (no gaps/dups)"
    );
}

/// A present non-string `before` is a client error, not a silent "no cursor" (parity with
/// REST + the stricter since/until handling).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_string_before_is_rejected() {
    let Some(fx) = fixture().await else {
        return;
    };
    let session = initialize(fx.app.clone(), &fx.admin_token).await;
    let result = tools_call(
        fx.app.clone(),
        &fx.admin_token,
        &session,
        "ops_xds_nacks",
        serde_json::json!({ "team": fx.team_name, "before": 123 }),
    )
    .await;
    // Surfaces as a JSON-RPC error or an isError tool result — never a clean success.
    let is_error =
        result.get("error").is_some() || result["result"]["isError"] == serde_json::json!(true);
    assert!(
        is_error,
        "a non-string `before` must be rejected, got: {result}"
    );
}

/// AC1 window filter (half-open `[since, until)`): `since=T` returns only rows with
/// created_at >= T; a row exactly at `until` is excluded.
#[tokio::test]
async fn ac1_half_open_window_since_inclusive_until_exclusive() {
    let Some(fx) = fixture().await else {
        return;
    };
    let t = base_time();
    // Rows at t+0 .. t+4.
    let mut ids = Vec::new();
    for k in 0..5 {
        ids.push(seed_nack(&fx.pool, fx.team, "node-a", t + Duration::seconds(k)).await);
    }
    let session = initialize(fx.app.clone(), &fx.admin_token).await;

    // since = t+2 → rows at t+2, t+3, t+4 (inclusive lower bound).
    let boundary = (t + Duration::seconds(2)).to_rfc3339();
    let result = tools_call(
        fx.app.clone(),
        &fx.admin_token,
        &session,
        "ops_xds_nacks",
        serde_json::json!({ "team": fx.team_name, "since": boundary }),
    )
    .await;
    let sc = structured(&result);
    assert_eq!(sc["window_total"], serde_json::json!(3), "since inclusive");
    let returned = item_ids(sc);
    assert!(!returned.contains(&ids[0].to_string()), "t+0 excluded");
    assert!(!returned.contains(&ids[1].to_string()), "t+1 excluded");
    assert!(returned.contains(&ids[2].to_string()), "t+2 included (>=)");

    // [t+1, t+3): rows at t+1 and t+2 only; the row exactly at until (t+3) is excluded.
    let since = (t + Duration::seconds(1)).to_rfc3339();
    let until = (t + Duration::seconds(3)).to_rfc3339();
    let result = tools_call(
        fx.app.clone(),
        &fx.admin_token,
        &session,
        "ops_xds_nacks",
        serde_json::json!({ "team": fx.team_name, "since": since, "until": until }),
    )
    .await;
    let sc = structured(&result);
    assert_eq!(sc["window_total"], serde_json::json!(2), "half-open count");
    let returned = item_ids(sc);
    assert!(returned.contains(&ids[1].to_string()), "t+1 included");
    assert!(returned.contains(&ids[2].to_string()), "t+2 included");
    assert!(
        !returned.contains(&ids[3].to_string()),
        "row exactly at until (t+3) must be excluded"
    );
}

/// AC6: `since > until` is an empty (not error) window — items == [], window_total == 0,
/// next_cursor null, isError false.
#[tokio::test]
async fn ac6_since_after_until_is_empty_not_error() {
    let Some(fx) = fixture().await else {
        return;
    };
    let t = base_time();
    for k in 0..3 {
        seed_nack(&fx.pool, fx.team, "node-a", t + Duration::seconds(k)).await;
    }
    let session = initialize(fx.app.clone(), &fx.admin_token).await;

    let since = (t + Duration::seconds(10)).to_rfc3339();
    let until = (t + Duration::seconds(1)).to_rfc3339();
    let result = tools_call(
        fx.app.clone(),
        &fx.admin_token,
        &session,
        "ops_xds_nacks",
        serde_json::json!({ "team": fx.team_name, "since": since, "until": until }),
    )
    .await;
    // Explicitly not an error.
    assert_eq!(
        result["result"]["isError"], false,
        "empty window is not an error"
    );
    let sc = &result["result"]["structuredContent"];
    assert_eq!(sc["items"], serde_json::json!([]), "no items");
    assert_eq!(
        sc["window_total"],
        serde_json::json!(0),
        "empty window_total"
    );
    assert!(sc["next_cursor"].is_null(), "no next page for empty window");
}

/// AC5 schema parity/no-drift: the advertised `ops_xds_nacks` inputSchema exposes the
/// NACK-specific window params (team/since/until/limit/before) and NOT `offset` (which
/// would indicate it fell back to the shared list schema).
#[tokio::test]
async fn ac5_schema_advertises_window_params_not_offset() {
    let Some(fx) = fixture().await else {
        return;
    };
    let session = initialize(fx.app.clone(), &fx.admin_token).await;
    let body = tools_list_raw(fx.app.clone(), &fx.admin_token, &session, &fx.team_name).await;
    let tools = body["result"]["tools"].as_array().expect("tools array");
    let tool = tools
        .iter()
        .find(|t| t["name"] == "ops_xds_nacks")
        .expect("ops_xds_nacks must be listed for an org admin");
    let props = tool["inputSchema"]["properties"]
        .as_object()
        .expect("inputSchema.properties object");

    for key in ["team", "since", "until", "limit", "before"] {
        assert!(
            props.contains_key(key),
            "ops_xds_nacks inputSchema must expose `{key}`; got {:?}",
            props.keys().collect::<Vec<_>>()
        );
    }
    assert!(
        !props.contains_key("offset"),
        "ops_xds_nacks must use the cursor window schema, not the shared offset list schema"
    );
}

/// Team isolation: rows exist for BOTH fx.team and the second team; a query for
/// fx.team_name returns none of the other team's rows and counts only fx.team's.
#[tokio::test]
async fn team_isolation_excludes_other_team_rows() {
    let Some(fx) = fixture().await else {
        return;
    };
    let t = base_time();

    // Look up the other team's (id, org_id) — same org as fx.team.
    let row = sqlx::query("SELECT id, org_id FROM teams WHERE name = $1")
        .bind(&fx.other_team_name)
        .fetch_one(&fx.pool)
        .await
        .expect("other team lookup");
    let other_team = TeamRef {
        id: fp_domain::TeamId::from(row.get::<uuid::Uuid, _>("id")),
        org_id: fp_domain::OrgId::from(row.get::<uuid::Uuid, _>("org_id")),
    };

    // Seed 2 rows for fx.team and 3 for the other team, interleaved in time.
    let mut mine = Vec::new();
    for k in 0..2 {
        mine.push(seed_nack(&fx.pool, fx.team, "node-mine", t + Duration::seconds(k * 2)).await);
    }
    let mut theirs = Vec::new();
    for k in 0..3 {
        theirs.push(
            seed_nack(
                &fx.pool,
                other_team,
                "node-theirs",
                t + Duration::seconds(k * 2 + 1),
            )
            .await,
        );
    }

    let session = initialize(fx.app.clone(), &fx.admin_token).await;
    let result = tools_call(
        fx.app.clone(),
        &fx.admin_token,
        &session,
        "ops_xds_nacks",
        serde_json::json!({ "team": fx.team_name }),
    )
    .await;
    let sc = structured(&result);

    assert_eq!(
        sc["window_total"],
        serde_json::json!(2),
        "window_total counts only fx.team rows"
    );
    let returned = item_ids(sc);
    for id in &mine {
        assert!(
            returned.contains(&id.to_string()),
            "own row {id} must appear"
        );
    }
    for id in &theirs {
        assert!(
            !returned.contains(&id.to_string()),
            "other team's row {id} must not leak"
        );
    }
}
