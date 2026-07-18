//! ui-f4 S1 E2E smoke: `GET .../api-definitions/{name}/specs` black-box against a REAL
//! control plane — the production fp-api router served over loopback TCP backed by the
//! shared test PostgreSQL. Creates an API with an imported OpenAPI document over HTTP,
//! then lists its spec versions over HTTP and checks the page shape.
//!
//! Parallel-safety (invariant 18): the CP binds 127.0.0.1:0 and all org/team/agent/api
//! names are unique per run.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::AgentKind;
use fp_storage::repos::identity;
use serde_json::{json, Value};

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spec_versions_list_smoke_over_real_cp() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 4).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

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
    tx.commit().await.expect("commit");

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

    let api_name = unique("catalog");
    let created = http
        .post(format!(
            "{base_url}/api/v1/teams/{}/api-definitions",
            team.name
        ))
        .bearer_auth(&token)
        .json(&json!({
            "name": api_name,
            "openapi": {
                "openapi": "3.0.3",
                "info": {"title": "smoke", "version": "1.0.0"},
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

    let page: Value = http
        .get(format!(
            "{base_url}/api/v1/teams/{}/api-definitions/{api_name}/specs",
            team.name
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("list specs")
        .error_for_status()
        .expect("200")
        .json()
        .await
        .expect("json");
    assert_eq!(page["total"], 1);
    let items = page["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["version"], 1);
    assert_eq!(items[0]["source_kind"], "imported");
    assert_eq!(items[0]["format"], "openapi3");
    assert!(items[0]["spec_hash"]
        .as_str()
        .is_some_and(|h| !h.is_empty()));
    assert!(
        items[0].get("latest_decision").is_none(),
        "no review events yet: field omitted"
    );
    assert!(
        items[0].get("spec").is_none(),
        "list never inlines spec content"
    );

    server.abort();
}
