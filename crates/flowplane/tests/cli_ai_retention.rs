//! Slice s5 (ai-gateway-e2e-trace): CLI parity for `flowplane ai retention get/set` against
//! a REAL control plane — the actual fp-api router over a loopback TCP listener backed by
//! the shared test PostgreSQL. The commands are pure REST clients (the CLI process receives
//! only FLOWPLANE_SERVER/FLOWPLANE_TOKEN, no DB URL), and the rendered envelope `data` must
//! equal the REST response body for both the built-in default and a stored policy.
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
async fn ai_retention_cli_get_and_set_work_as_rest_only_clients() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    // Tenancy fixture: one org, one team, one agent whose bearer token authenticates via
    // the fpat_ path and holds (ai-usage, read) + (ai-usage, update) — the two grants the
    // retention surface enforces.
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
    for action in [
        fp_domain::authz::Action::Read,
        fp_domain::authz::Action::Update,
    ] {
        identity::add_agent_grant_in_tx(
            &mut tx,
            agent.id,
            org.id,
            team.id,
            fp_domain::authz::Resource::AiUsage,
            action,
            None,
        )
        .await
        .expect("agent grant");
    }
    tx.commit().await.expect("commit");

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
        discovery_forwarding_policy: Default::default(),
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
    let http = reqwest::Client::new();
    let retention_url = format!("{base_url}/api/v1/teams/{}/ai/retention", team.name);
    let home = common::unique_tempdir();
    let cli = |args: &[&str]| {
        common::flowplane_cmd(&home)
            .env("FLOWPLANE_SERVER", &base_url)
            .env("FLOWPLANE_TOKEN", &token)
            .args(args)
            .output()
            .expect("run flowplane")
    };

    // GET before any policy: the CLI renders the same built-in-default body as REST.
    let rest_default: Value = http
        .get(&retention_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("rest get")
        .json()
        .await
        .expect("rest json");
    assert_eq!(rest_default["trace_ttl_days"], 30);
    assert_eq!(rest_default["is_default"], true);
    let out = cli(&["ai", "retention", "get", "--team", &team.name, "-o", "json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "ai retention get must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let envelope: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not a JSON envelope ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    assert_eq!(envelope["kind"], "aiRetention");
    assert_eq!(
        envelope["data"], rest_default,
        "CLI must render the same payload as REST"
    );

    // SET through the CLI (a pure REST PUT): the stored policy is what REST then returns.
    let out = cli(&[
        "ai",
        "retention",
        "set",
        "--team",
        &team.name,
        "--days",
        "14",
        "-o",
        "json",
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "ai retention set must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let envelope: Value = serde_json::from_slice(&out.stdout).expect("set envelope");
    assert_eq!(envelope["kind"], "aiRetention");
    assert_eq!(envelope["data"]["trace_ttl_days"], 14);
    assert_eq!(envelope["data"]["is_default"], false);
    assert_eq!(envelope["data"]["revision"], 1);

    // GET parity after the set: CLI and REST agree on the stored policy.
    let rest_stored: Value = http
        .get(&retention_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("rest get stored")
        .json()
        .await
        .expect("rest stored json");
    assert_eq!(rest_stored["trace_ttl_days"], 14);
    let out = cli(&["ai", "retention", "get", "--team", &team.name, "-o", "json"]);
    assert_eq!(out.status.code(), Some(0));
    let envelope: Value = serde_json::from_slice(&out.stdout).expect("get envelope");
    assert_eq!(
        envelope["data"], rest_stored,
        "stored-policy payload must match REST"
    );

    server.abort();
}
