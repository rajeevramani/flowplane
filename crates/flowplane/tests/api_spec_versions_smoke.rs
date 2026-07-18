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
    // The content endpoint additionally requires learning-sessions:read (dual-grant rule).
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

    // ui-f4 S2 smoke: publish v1 over HTTP (reject is learned-only; this spec is imported),
    // then the events history shows the published event verbatim.
    let published = http
        .post(format!(
            "{base_url}/api/v1/teams/{}/api-definitions/{api_name}/specs/1/publish",
            team.name
        ))
        .bearer_auth(&token)
        .json(&json!({"reason": "smoke"}))
        .send()
        .await
        .expect("publish");
    assert_eq!(published.status(), 200, "publish v1 over HTTP");
    let events: Value = http
        .get(format!(
            "{base_url}/api/v1/teams/{}/api-definitions/{api_name}/specs/1/events",
            team.name
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("events")
        .error_for_status()
        .expect("200")
        .json()
        .await
        .expect("json");
    assert_eq!(events["total"], 1);
    assert_eq!(events["items"][0]["decision"], "published");
    assert_eq!(events["items"][0]["reason"], "smoke");
    let missing: Value = http
        .get(format!(
            "{base_url}/api/v1/teams/{}/api-definitions/{api_name}/specs/999/events",
            team.name
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("missing version")
        .status()
        .as_u16()
        .into();
    assert_eq!(missing, 404);

    // ui-f4 S3 smoke: content round-trips with ETag revalidation over HTTP.
    let content_url = format!(
        "{base_url}/api/v1/teams/{}/api-definitions/{api_name}/specs/1/content",
        team.name
    );
    let content_resp = http
        .get(&content_url)
        .bearer_auth(&token)
        .send()
        .await
        .expect("content");
    assert_eq!(content_resp.status(), 200);
    let etag = content_resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("etag header")
        .to_string();
    let cache_control = content_resp
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .expect("cache-control")
        .to_string();
    assert!(cache_control.contains("no-store") && cache_control.contains("private"));
    let doc: Value = content_resp.json().await.expect("content json");
    assert_eq!(doc["info"]["title"], "smoke");
    let not_modified = http
        .get(&content_url)
        .bearer_auth(&token)
        .header("If-None-Match", &etag)
        .send()
        .await
        .expect("revalidate");
    assert_eq!(not_modified.status(), 304);

    // ui-f4 S4 smoke: tools list over HTTP includes the generated tool with schemas.
    let tools: Value = http
        .get(format!(
            "{base_url}/api/v1/teams/{}/api-definitions/{api_name}/tools",
            team.name
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("tools")
        .error_for_status()
        .expect("200")
        .json()
        .await
        .expect("json");
    assert_eq!(tools["total"], 1);
    assert_eq!(tools["items"][0]["operation_id"], "listItems");
    assert_eq!(tools["items"][0]["enabled"], true);
    let bindings: Value = http
        .get(format!(
            "{base_url}/api/v1/teams/{}/api-definitions/{api_name}/route-bindings",
            team.name
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("bindings")
        .error_for_status()
        .expect("200")
        .json()
        .await
        .expect("json");
    assert_eq!(bindings["total"], 0, "API created without a route binding");

    server.abort();
}
