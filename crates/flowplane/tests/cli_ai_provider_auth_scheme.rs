//! Slice fpv2-crv.1: CLI round-trip of the OPTIONAL `auth_scheme` field on
//! `AiProviderSpec` — `flowplane ai providers create/get/update` against a REAL
//! control plane (the actual fp-api router over a loopback TCP listener backed by
//! the shared test PostgreSQL), mirroring the cli_ai_retention.rs harness.
//!
//! Spec under test (written spec-first from the acceptance criteria):
//! - `ai providers create -f <spec file>` with `"auth_scheme": "Bearer"` in the spec
//!   creates the provider, and `ai providers get` renders the field back losslessly
//!   (CLI `data` == REST response body).
//! - `ai providers update -f <spec file>` whose spec OMITS the field removes it: a
//!   subsequent get shows the spec WITHOUT an `auth_scheme` key (absent, not null).
//!
//! Parallel-safety (invariant 18): the CP binds 127.0.0.1:0, all org/team/resource
//! names are unique per run, and the child process gets an isolated HOME. Skips when
//! FLOWPLANE_TEST_DATABASE_URL is unset (shared-PG convention).

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
async fn ai_provider_auth_scheme_round_trips_through_the_cli() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    // Secret creation (the provider's credential prerequisite) needs the encryption key.
    std::env::set_var(
        "FLOWPLANE_SECRET_ENCRYPTION_KEY",
        "12345678901234567890123456789012",
    );
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    // Tenancy fixture: one org, one team, one agent whose bearer token authenticates
    // via the fpat_ path and holds the grants this journey needs — secrets:create for
    // the credential prerequisite, ai-providers read/create/update for the surface.
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
    let grants = [
        (
            fp_domain::authz::Resource::Secrets,
            fp_domain::authz::Action::Create,
        ),
        (
            fp_domain::authz::Resource::Secrets,
            fp_domain::authz::Action::Read,
        ),
        (
            fp_domain::authz::Resource::AiProviders,
            fp_domain::authz::Action::Read,
        ),
        (
            fp_domain::authz::Resource::AiProviders,
            fp_domain::authz::Action::Create,
        ),
        (
            fp_domain::authz::Resource::AiProviders,
            fp_domain::authz::Action::Update,
        ),
    ];
    for (resource, action) in grants {
        identity::add_agent_grant_in_tx(&mut tx, agent.id, org.id, team.id, resource, action, None)
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
    let home = common::unique_tempdir();
    let cli = |args: &[&str]| {
        common::flowplane_cmd(&home)
            .env("FLOWPLANE_SERVER", &base_url)
            .env("FLOWPLANE_TOKEN", &token)
            .args(args)
            .output()
            .expect("run flowplane")
    };
    let parse_success = |out: &std::process::Output, ctx: &str| -> Value {
        assert_eq!(
            out.status.code(),
            Some(0),
            "{ctx} must exit 0; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{ctx}: stdout is not a JSON envelope ({e}): {}",
                String::from_utf8_lossy(&out.stdout)
            )
        })
    };

    // Prerequisite over REST with the same token: a team secret the provider spec
    // can reference as credential_secret_id.
    let secret_body: Value = {
        let response = http
            .post(format!("{base_url}/api/v1/teams/{}/secrets", team.name))
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "name": unique("secret"),
                "description": "ai credential",
                "spec": {"type": "generic_secret", "secret": "aGVsbG8="}
            }))
            .send()
            .await
            .expect("create secret");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::CREATED,
            "prerequisite secret must be creatable"
        );
        response.json().await.expect("secret json")
    };
    let secret_id = secret_body["id"].as_str().expect("secret id").to_string();

    let provider_name = unique("prov");
    let spec_with = serde_json::json!({
        "kind": "openai-compatible",
        "base_url": "https://llm.example",
        "path_prefix": "/v1",
        "credential_secret_id": secret_id,
        "models": ["gpt-5-mini"],
        "auth_scheme": "Bearer"
    });
    let mut spec_without = spec_with.clone();
    spec_without
        .as_object_mut()
        .expect("spec object")
        .remove("auth_scheme");

    // CREATE through the CLI: file-driven mutator (`-f`), body = {name, spec} like the
    // REST POST body (the cluster create convention: the name comes from the file).
    let create_file = home.join("provider-create.json");
    std::fs::write(
        &create_file,
        serde_json::json!({ "name": provider_name, "spec": spec_with }).to_string(),
    )
    .expect("write create spec file");
    let out = cli(&[
        "ai",
        "providers",
        "create",
        "--team",
        &team.name,
        "-f",
        create_file.to_str().expect("utf-8 path"),
        "-o",
        "json",
    ]);
    let envelope = parse_success(&out, "ai providers create");
    assert_eq!(
        envelope["data"]["spec"]["auth_scheme"], "Bearer",
        "create output must carry the auth_scheme from the spec file: {envelope}"
    );
    assert_eq!(envelope["data"]["revision"], 1);

    // GET parity: the CLI renders the same payload REST returns, auth_scheme included.
    let provider_url = format!(
        "{base_url}/api/v1/teams/{}/ai/providers/{provider_name}",
        team.name
    );
    let rest_with: Value = http
        .get(&provider_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("rest get")
        .json()
        .await
        .expect("rest json");
    assert_eq!(rest_with["spec"]["auth_scheme"], "Bearer");
    let out = cli(&[
        "ai",
        "providers",
        "get",
        &provider_name,
        "--team",
        &team.name,
        "-o",
        "json",
    ]);
    let envelope = parse_success(&out, "ai providers get (set)");
    assert_eq!(
        envelope["data"], rest_with,
        "CLI must render the same provider payload as REST"
    );

    // UPDATE through the CLI with a spec file that OMITS auth_scheme: removal.
    let update_file = home.join("provider-update.json");
    std::fs::write(
        &update_file,
        serde_json::json!({ "spec": spec_without }).to_string(),
    )
    .expect("write update spec file");
    let out = cli(&[
        "ai",
        "providers",
        "update",
        &provider_name,
        "--team",
        &team.name,
        "-f",
        update_file.to_str().expect("utf-8 path"),
        "--revision",
        "1",
        "-o",
        "json",
    ]);
    let envelope = parse_success(&out, "ai providers update (remove auth_scheme)");
    assert_eq!(envelope["data"]["revision"], 2);
    assert!(
        !envelope["data"]["spec"]
            .as_object()
            .unwrap_or_else(|| panic!("update output must carry a spec object: {envelope}"))
            .contains_key("auth_scheme"),
        "after an update whose spec omits auth_scheme, the field must be ABSENT: {envelope}"
    );

    // GET after removal: absent for both REST and the CLI rendering (not null/empty).
    let rest_without: Value = http
        .get(&provider_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("rest get after removal")
        .json()
        .await
        .expect("rest json after removal");
    assert!(
        !rest_without["spec"]
            .as_object()
            .expect("spec object")
            .contains_key("auth_scheme"),
        "REST spec must omit auth_scheme after removal: {rest_without}"
    );
    let out = cli(&[
        "ai",
        "providers",
        "get",
        &provider_name,
        "--team",
        &team.name,
        "-o",
        "json",
    ]);
    let envelope = parse_success(&out, "ai providers get (removed)");
    assert_eq!(
        envelope["data"], rest_without,
        "CLI must render the same post-removal payload as REST"
    );

    server.abort();
}
