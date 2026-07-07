//! Slice s4 (ai-gateway-e2e-trace): CLI parity for `flowplane ai trace` against a REAL
//! control plane — the actual fp-api router over a loopback TCP listener backed by the
//! shared test PostgreSQL. The command is a pure REST client (no direct DB access from the
//! CLI process: it receives only FLOWPLANE_SERVER/FLOWPLANE_TOKEN), and its rendered
//! envelope `data` must equal the REST response body — same hops, same miss payload.
//!
//! Parallel-safety (invariant 18): the CP binds 127.0.0.1:0, all org/team/agent names are
//! unique per run, and the child process gets an isolated HOME.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod common;

use fp_domain::AgentKind;
use fp_storage::repos::identity;
use serde_json::Value;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_trace_cli_renders_the_same_hops_as_rest() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    // Tenancy fixture: one org, one team, one agent whose bearer token authenticates via
    // the fpat_ path (no OIDC validator needed) and holds exactly (ai-usage, read).
    let org = identity::create_org(&pool, &unique("org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("team"), "")
        .await
        .expect("team");
    let token = format!(
        "fpat_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut tx = pool.begin().await.expect("begin");
    let agent = identity::create_agent_tx(
        &mut tx,
        org.id,
        &unique("agent"),
        AgentKind::CpTool,
        &identity::hash_agent_token(&token),
        None,
    )
    .await
    .expect("agent");
    identity::add_agent_grant_in_tx(
        &mut tx,
        agent.id,
        org.id,
        team.id,
        fp_domain::authz::Resource::AiUsage,
        fp_domain::authz::Action::Read,
        None,
    )
    .await
    .expect("agent grant");
    tx.commit().await.expect("commit");

    // Seed one trace row (the write path is slice s2's; this test owns retrieval parity).
    let request_id = uuid::Uuid::now_v7().to_string();
    fp_storage::repos::ai_trace::upsert_trace_event(
        &pool,
        &fp_storage::repos::ai_trace::AiTraceEventUpsert {
            team_id: team.id,
            request_id: request_id.clone(),
            trace_id: None,
            route_config_id: fp_domain::RouteConfigId::from(uuid::Uuid::now_v7()),
            listener_id: None,
            provider_id: None,
            model: Some("gpt-5".into()),
            status_code: Some(200),
            hops: serde_json::json!([
                {"hop": "route_match", "started_at": "2026-07-04T00:00:00.100Z",
                 "ended_at": "2026-07-04T00:00:00.200Z", "outcome": "matched",
                 "origin": "listener", "failed": false, "detail": {}},
                {"hop": "budget", "started_at": "2026-07-04T00:00:00.210Z",
                 "ended_at": "2026-07-04T00:00:00.230Z", "outcome": "allowed",
                 "origin": "listener", "failed": false, "detail": {}},
                {"hop": "upstream", "started_at": "2026-07-04T00:00:00.300Z",
                 "ended_at": "2026-07-04T00:00:00.900Z", "outcome": "ok",
                 "origin": "upstream", "failed": false, "detail": {"status": 200}}
            ]),
        },
    )
    .await
    .expect("seed trace row");

    // A real CP: the production router served over loopback on an ephemeral port.
    let app = fp_api::build_router(fp_api::AppState {
        pool,
        prometheus: metrics_exporter_prometheus::PrometheusBuilder::new()
            .build_recorder()
            .handle(),
        version: "test",
        validator: None,
        write_throttle: std::sync::Arc::new(fp_api::throttle::WriteThrottle::new(1000)),
        xds_readiness: None,
        egress_policy: Default::default(),
        rls_repush: None,
        rls_grpc_configured: false,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind CP to an ephemeral port");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // REST reference response, fetched directly.
    let http = reqwest::Client::new();
    let rest: Value = http
        .get(format!(
            "{base_url}/api/v1/teams/{}/ai/trace?request_id={request_id}&limit=50",
            team.name
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("rest call")
        .json()
        .await
        .expect("rest json");
    assert_eq!(
        rest["traces"].as_array().map(Vec::len),
        Some(1),
        "seeded row must be visible over REST: {rest}"
    );

    // The CLI, black-box: REST-only client (only server URL + token are provided).
    let home = common::unique_tempdir();
    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", &base_url)
        .env("FLOWPLANE_TOKEN", &token)
        .args([
            "ai",
            "trace",
            "--team",
            &team.name,
            "--request-id",
            &request_id,
            "-o",
            "json",
        ])
        .output()
        .expect("run flowplane ai trace");
    assert_eq!(
        out.status.code(),
        Some(0),
        "ai trace must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let envelope: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not a JSON envelope ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    assert_eq!(envelope["kind"], "aiTrace");

    // Parity: the CLI envelope's data IS the REST body — identical hop timeline included.
    assert_eq!(
        envelope["data"], rest,
        "CLI must render the same payload as REST"
    );
    assert_eq!(
        envelope["data"]["traces"][0]["hops"], rest["traces"][0]["hops"],
        "hop timelines must match"
    );

    // Miss parity: an unknown id renders the same distinguishable miss payload as REST.
    let missing_id = uuid::Uuid::now_v7().to_string();
    let rest_miss: Value = http
        .get(format!(
            "{base_url}/api/v1/teams/{}/ai/trace?request_id={missing_id}&limit=50",
            team.name
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("rest miss call")
        .json()
        .await
        .expect("rest miss json");
    assert_eq!(rest_miss["miss"]["message"], "no trace row found");
    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_SERVER", &base_url)
        .env("FLOWPLANE_TOKEN", &token)
        .args([
            "ai",
            "trace",
            "--team",
            &team.name,
            "--request-id",
            &missing_id,
            "-o",
            "json",
        ])
        .output()
        .expect("run flowplane ai trace (miss)");
    assert_eq!(
        out.status.code(),
        Some(0),
        "a miss is a 200 and must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let envelope: Value = serde_json::from_slice(&out.stdout).expect("miss envelope");
    assert_eq!(envelope["data"], rest_miss, "miss payload must match REST");

    server.abort();
}
