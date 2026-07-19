//! ui-f4 CLI read-model parity: `flowplane api spec list/events/show[--content]` and
//! `flowplane api bindings/tools` against a REAL control plane — the production fp-api
//! router served over loopback TCP backed by the shared test PostgreSQL. The commands are
//! pure REST clients (the CLI process receives only FLOWPLANE_SERVER/FLOWPLANE_TOKEN, no
//! DB URL), and each rendered envelope's `data` must equal the corresponding REST response
//! body byte-for-byte as JSON values.
//!
//! Parallel-safety (invariant 18): the CP binds 127.0.0.1:0, all org/team/agent/api names
//! are unique per run, and every child process gets an isolated HOME.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod common;

use fp_domain::AgentKind;
use fp_storage::repos::identity;
use serde_json::{json, Value};

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

/// Parse a CLI success envelope from stdout, asserting exit 0 first.
fn success_envelope(out: &std::process::Output, what: &str) -> Value {
    assert_eq!(
        out.status.code(),
        Some(0),
        "{what} must exit 0; stdout: {} stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{what}: stdout is not a JSON envelope ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_spec_cli_read_model_matches_rest() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    // Tenancy fixture: one org, one team, one fpat_ agent holding api-definitions
    // read+create+update plus learning-sessions read (the content endpoint's dual grant).
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
        fp_domain::authz::Action::Create,
        fp_domain::authz::Action::Update,
    ] {
        identity::add_agent_grant_in_tx(
            &mut tx,
            agent.id,
            org.id,
            team.id,
            fp_domain::authz::Resource::ApiDefinitions,
            action,
            None,
        )
        .await
        .expect("agent grant");
    }
    identity::add_agent_grant_in_tx(
        &mut tx,
        agent.id,
        org.id,
        team.id,
        fp_domain::authz::Resource::LearningSessions,
        fp_domain::authz::Action::Read,
        None,
    )
    .await
    .expect("learning grant");
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
        egress_advisory: Default::default(),
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
    let api_base = format!("{base_url}/api/v1/teams/{}/api-definitions", team.name);

    // Fixture over HTTP: create one API with a real path operation (empty paths is a 400),
    // then publish v1 so events history is non-empty and latest_decision is set.
    let api_name = unique("catalog");
    let created = http
        .post(&api_base)
        .bearer_auth(&token)
        .json(&json!({
            "name": api_name,
            "openapi": {
                "openapi": "3.0.3",
                "info": {"title": "cli-parity", "version": "1.0.0"},
                "paths": {"/items": {"get": {"operationId": "listItems"}}}
            }
        }))
        .send()
        .await
        .expect("create api");
    let created_status = created.status();
    if created_status != 201 {
        panic!(
            "create api over HTTP: {} — {}",
            created_status,
            created.text().await.unwrap_or_default()
        );
    }
    let published = http
        .post(format!("{api_base}/{api_name}/specs/1/publish"))
        .bearer_auth(&token)
        .json(&json!({"reason": "cli-parity"}))
        .send()
        .await
        .expect("publish");
    assert_eq!(published.status(), 200, "publish v1 over HTTP");

    let home = common::unique_tempdir();
    let cli = |args: &[&str]| {
        common::flowplane_cmd(&home)
            .env("FLOWPLANE_SERVER", &base_url)
            .env("FLOWPLANE_TOKEN", &token)
            .args(args)
            .output()
            .expect("run flowplane")
    };
    let rest_get = |path: String| {
        let http = http.clone();
        let token = token.clone();
        async move {
            http.get(path)
                .bearer_auth(&token)
                .send()
                .await
                .expect("rest get")
                .error_for_status()
                .expect("2xx")
                .json::<Value>()
                .await
                .expect("rest json")
        }
    };

    // Scenario 1 — `api spec list`: envelope data equals the REST Page byte-for-byte, and
    // the single row is version 1 with latest_decision "published".
    let rest_specs = rest_get(format!("{api_base}/{api_name}/specs")).await;
    let out = cli(&[
        "api", "spec", "list", &api_name, "--team", &team.name, "-o", "json",
    ]);
    let envelope = success_envelope(&out, "api spec list");
    assert_eq!(
        envelope["data"], rest_specs,
        "spec list data must equal the REST page"
    );
    let items = envelope["data"]["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    let row = &items[0];
    assert_eq!(row["version"], 1);
    assert_eq!(row["source_kind"], "imported");
    assert_eq!(row["format"], "openapi3");
    assert!(row["spec_hash"].as_str().is_some_and(|h| !h.is_empty()));
    assert_eq!(row["latest_decision"], "published");
    assert!(row["created_at"].is_string(), "created_at present");

    // Scenario 2 — `api spec events`: data equals the REST body; one published event with
    // the reason echoed.
    let rest_events = rest_get(format!("{api_base}/{api_name}/specs/1/events")).await;
    let out = cli(&[
        "api", "spec", "events", &api_name, "1", "--team", &team.name, "-o", "json",
    ]);
    let envelope = success_envelope(&out, "api spec events");
    assert_eq!(
        envelope["data"], rest_events,
        "events data must equal the REST page"
    );
    assert_eq!(envelope["data"]["total"], 1);
    assert_eq!(envelope["data"]["items"][0]["decision"], "published");
    assert_eq!(envelope["data"]["items"][0]["reason"], "cli-parity");

    // Scenario 3a — `api spec show` (metadata, no --content): the rendered data equals the
    // single version-1 item from the specs list.
    let out = cli(&[
        "api", "spec", "show", &api_name, "1", "--team", &team.name, "-o", "json",
    ]);
    let envelope = success_envelope(&out, "api spec show");
    assert_eq!(
        envelope["data"], rest_specs["items"][0],
        "show metadata must equal the list's version-1 row"
    );

    // Scenario 3b — `api spec show` for an unknown version: nonzero exit with a not_found
    // error envelope on stderr or stdout.
    let out = cli(&[
        "api", "spec", "show", &api_name, "999", "--team", &team.name, "-o", "json",
    ]);
    assert_ne!(
        out.status.code(),
        Some(0),
        "show of an unknown version must exit nonzero; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("not_found"),
        "error output must carry the not_found code; got: {combined}"
    );

    // Scenario 4 — `api spec show --content`: the rendered data is the stored OpenAPI
    // document itself (the JSON body of the content endpoint).
    let rest_content = rest_get(format!("{api_base}/{api_name}/specs/1/content")).await;
    let out = cli(&[
        "api",
        "spec",
        "show",
        &api_name,
        "1",
        "--content",
        "--team",
        &team.name,
        "-o",
        "json",
    ]);
    let envelope = success_envelope(&out, "api spec show --content");
    assert_eq!(
        envelope["data"], rest_content,
        "show --content data must equal the REST content body"
    );
    assert_eq!(envelope["data"]["info"]["title"], "cli-parity");

    // Scenario 5 — `api bindings` and `api tools`: data equals the REST pages; the tools
    // row carries enabled=true. (The fixture API has no route binding, so the bindings
    // page is empty — parity is still byte-exact.)
    let rest_bindings = rest_get(format!("{api_base}/{api_name}/route-bindings")).await;
    let out = cli(&[
        "api", "bindings", &api_name, "--team", &team.name, "-o", "json",
    ]);
    let envelope = success_envelope(&out, "api bindings");
    assert_eq!(
        envelope["data"], rest_bindings,
        "bindings data must equal the REST page"
    );

    let rest_tools = rest_get(format!("{api_base}/{api_name}/tools")).await;
    let out = cli(&[
        "api", "tools", &api_name, "--team", &team.name, "-o", "json",
    ]);
    let envelope = success_envelope(&out, "api tools");
    assert_eq!(
        envelope["data"], rest_tools,
        "tools data must equal the REST page"
    );
    assert_eq!(envelope["data"]["total"], 1);
    assert_eq!(envelope["data"]["items"][0]["enabled"], true);
    assert_eq!(envelope["data"]["items"][0]["operation_id"], "listItems");

    server.abort();
}
