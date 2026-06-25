#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fp_core::dev::DevIssuer;
use fp_domain::api_lifecycle::{SpecFormat, SpecSourceKind, SpecVersionInput};
use fp_domain::authz::TeamRef;
use fp_domain::{ApiDefinitionId, ListenerId, OrgRole, RouteConfigId, SpecVersionId};
use fp_storage::repos::{api_lifecycle as storage_api_lifecycle, identity};
use http_body_util::BodyExt;
use metrics_exporter_prometheus::PrometheusBuilder;
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

async fn insert_route_config(pool: &PgPool, team: TeamRef, name: &str) -> RouteConfigId {
    let id = RouteConfigId::generate();
    sqlx::query(
        "INSERT INTO route_configs (id, team_id, org_id, name, spec) \
         VALUES ($1, $2, $3, $4, '{\"virtual_hosts\":[]}'::jsonb)",
    )
    .bind(id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .execute(pool)
    .await
    .expect("route config");
    id
}

async fn insert_listener(
    pool: &PgPool,
    team: TeamRef,
    name: &str,
    public_base_url: Option<&str>,
) -> ListenerId {
    let id = ListenerId::generate();
    let spec = match public_base_url {
        Some(public_base_url) => serde_json::json!({
            "address": "0.0.0.0",
            "port": 18080,
            "public_base_url": public_base_url
        }),
        None => serde_json::json!({
            "address": "0.0.0.0",
            "port": 18080
        }),
    };
    sqlx::query(
        "INSERT INTO listeners (id, team_id, org_id, name, spec) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id.as_uuid())
    .bind(team.id.as_uuid())
    .bind(team.org_id.as_uuid())
    .bind(name)
    .bind(spec)
    .execute(pool)
    .await
    .expect("listener");
    id
}

struct Fixture {
    app: axum::Router,
    pool: PgPool,
    admin_token: String,
    member_token: String,
    team_name: String,
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
        discovery_forwarding_policy: Default::default(),
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

async fn tools_list(app: axum::Router, token: &str, session: &str, team: &str) -> Vec<String> {
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
    let body = json_of(response).await;
    body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_string())
        .collect()
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

fn assert_dynamic_descriptor_metadata(
    content: &serde_json::Value,
    api_id: ApiDefinitionId,
    spec_id: SpecVersionId,
) {
    assert_eq!(content["type"], "gateway_invocation");
    uuid::Uuid::parse_str(content["apiToolId"].as_str().expect("descriptor apiToolId"))
        .expect("apiToolId uuid");
    assert_eq!(
        content["apiDefinitionId"],
        serde_json::json!(api_id.as_uuid())
    );
    assert_eq!(
        content["specVersionId"],
        serde_json::json!(spec_id.as_uuid())
    );
    let expires_at = chrono::DateTime::parse_from_rfc3339(
        content["expiresAt"].as_str().expect("descriptor expiresAt"),
    )
    .expect("expiresAt rfc3339");
    assert!(
        expires_at.timestamp() > chrono::Utc::now().timestamp(),
        "descriptor expiresAt should be a future staleness window"
    );
    uuid::Uuid::parse_str(
        content["correlationId"]
            .as_str()
            .expect("descriptor correlationId"),
    )
    .expect("correlationId uuid");
}

async fn create_agent(
    app: axum::Router,
    admin_token: &str,
    kind: &str,
    team_id: uuid::Uuid,
    grants: Vec<(&str, &str)>,
) -> String {
    let grants = grants
        .into_iter()
        .map(|(resource, action)| {
            serde_json::json!({
                "team_id": team_id,
                "resource": resource,
                "action": action,
            })
        })
        .collect::<Vec<_>>();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agents")
                .header("authorization", format!("Bearer {admin_token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": unique("agent"),
                        "kind": kind,
                        "grants": grants,
                    })
                    .to_string(),
                ))
                .expect("create agent"),
        )
        .await
        .expect("agent response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    body["token"].as_str().expect("token").to_string()
}

async fn publish_spec(
    app: axum::Router,
    admin_token: &str,
    team_name: &str,
    api_name: &str,
    version: i64,
) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/teams/{team_name}/api-definitions/{api_name}/specs/{version}/publish"
                ))
                .header("authorization", format!("Bearer {admin_token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "reason": "test" }).to_string(),
                ))
                .expect("publish request"),
        )
        .await
        .expect("publish response");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn tools_list_filters_by_principal_kind_grant_and_team() {
    let Some(fx) = fixture().await else {
        return;
    };

    let admin_session = initialize(fx.app.clone(), &fx.admin_token).await;
    let admin_tools = tools_list(
        fx.app.clone(),
        &fx.admin_token,
        &admin_session,
        &fx.team_name,
    )
    .await;
    assert!(admin_tools.contains(&"cp_clusters_create".to_string()));
    assert!(admin_tools.contains(&"ops_stats_overview".to_string()));

    let member_session = initialize(fx.app.clone(), &fx.member_token).await;
    let member_tools = tools_list(
        fx.app.clone(),
        &fx.member_token,
        &member_session,
        &fx.team_name,
    )
    .await;
    assert!(
        member_tools.is_empty(),
        "grantless member should see no CP tools"
    );

    let cp_token = create_agent(
        fx.app.clone(),
        &fx.admin_token,
        "cp-tool",
        fx.team_id,
        vec![("clusters", "read")],
    )
    .await;
    let cp_session = initialize(fx.app.clone(), &cp_token).await;
    let cp_tools = tools_list(fx.app.clone(), &cp_token, &cp_session, &fx.team_name).await;
    assert!(cp_tools.contains(&"cp_clusters_list".to_string()));
    assert!(cp_tools.contains(&"cp_clusters_get".to_string()));
    assert!(!cp_tools.contains(&"cp_clusters_create".to_string()));
    let cross_team_tools =
        tools_list(fx.app.clone(), &cp_token, &cp_session, &fx.other_team_name).await;
    assert!(
        cross_team_tools.is_empty(),
        "agent grant must not cross teams"
    );

    let gateway_token = create_agent(
        fx.app.clone(),
        &fx.admin_token,
        "gateway-tool",
        fx.team_id,
        vec![],
    )
    .await;
    let gateway_session = initialize(fx.app.clone(), &gateway_token).await;
    let gateway_tools = tools_list(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &fx.team_name,
    )
    .await;
    assert!(
        gateway_tools.is_empty(),
        "gateway agents do not list CP tools"
    );
}

#[tokio::test]
async fn static_tool_denial_records_shared_authz_denial_signal() {
    let Some(fx) = fixture().await else {
        return;
    };

    let member_session = initialize(fx.app.clone(), &fx.member_token).await;
    let denied = tools_call(
        fx.app.clone(),
        &fx.member_token,
        &member_session,
        "cp_clusters_list",
        serde_json::json!({ "team": fx.team_name }),
    )
    .await;
    assert_eq!(denied["error"]["data"]["kind"], "authz");
    assert!(denied["error"]["message"]
        .as_str()
        .expect("message")
        .contains("missing permission: clusters:read"));

    let denial_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log \
         WHERE team_id = $1 AND action = 'authz.denied' \
           AND detail->>'resource' = 'clusters' \
           AND detail->>'action' = 'read'",
    )
    .bind(fx.team_id)
    .fetch_one(&fx.pool)
    .await
    .expect("denial audit count");
    assert_eq!(denial_count, 1);
}

#[tokio::test]
async fn tools_call_uses_service_path_and_emits_mutation_audit() {
    let Some(fx) = fixture().await else {
        return;
    };
    let session = initialize(fx.app.clone(), &fx.admin_token).await;
    let cluster = unique("cluster");
    let response = fx
        .app
        .clone()
        .oneshot(mcp_request(
            &fx.admin_token,
            Some(&session),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "cp_clusters_create",
                    "arguments": {
                        "team": fx.team_name,
                        "name": cluster,
                        "spec": {
                            "endpoints": [{ "host": "10.0.0.10", "port": 8080 }]
                        }
                    }
                }
            }),
        ))
        .await
        .expect("tools/call");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_of(response).await;
    assert_eq!(body["result"]["isError"], false);
    assert_eq!(body["result"]["structuredContent"]["name"], cluster);

    let audit = sqlx::query(
        "SELECT action, resource FROM audit_log \
         WHERE team_id = $1 AND action = 'cluster.create' AND resource = $2 \
         ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(fx.team_id)
    .bind(format!("clusters/{cluster}"))
    .fetch_one(&fx.pool)
    .await
    .expect("audit row");
    assert_eq!(audit.get::<String, _>("action"), "cluster.create");
    assert_eq!(
        audit.get::<String, _>("resource"),
        format!("clusters/{cluster}")
    );
}

#[tokio::test]
async fn static_cp_workflow_chains_resources_ops_read_and_recovery_hints() {
    let Some(fx) = fixture().await else {
        return;
    };
    let session = initialize(fx.app.clone(), &fx.admin_token).await;
    let tools = tools_list(fx.app.clone(), &fx.admin_token, &session, &fx.team_name).await;
    for expected in [
        "cp_clusters_create",
        "cp_route_configs_create",
        "cp_listeners_create",
        "cp_listeners_get",
        "ops_xds_status",
    ] {
        assert!(tools.contains(&expected.to_string()), "missing {expected}");
    }

    let missing_route_config = unique("missing-routes");
    let bad_listener = tools_call(
        fx.app.clone(),
        &fx.admin_token,
        &session,
        "cp_listeners_create",
        serde_json::json!({
            "team": fx.team_name,
            "name": unique("bad-listener"),
            "spec": {
                "address": "0.0.0.0",
                "port": 19080,
                "route_config": missing_route_config
            }
        }),
    )
    .await;
    assert_eq!(bad_listener["result"]["isError"], true);
    assert_eq!(
        bad_listener["result"]["error"]["hint"],
        "create the route config first, then the listener"
    );
    assert!(bad_listener["result"]["content"][0]["text"]
        .as_str()
        .expect("error text")
        .contains("Hint: create the route config first, then the listener"));

    let cluster = unique("cluster");
    let created_cluster = tools_call(
        fx.app.clone(),
        &fx.admin_token,
        &session,
        "cp_clusters_create",
        serde_json::json!({
            "team": fx.team_name,
            "name": cluster,
            "spec": {
                "endpoints": [{ "host": "10.0.0.10", "port": 8080 }]
            }
        }),
    )
    .await;
    assert_eq!(created_cluster["result"]["isError"], false);
    assert_eq!(
        created_cluster["result"]["structuredContent"]["name"],
        cluster
    );

    let route_config = unique("routes");
    let route_config_spec = serde_json::json!({
        "virtual_hosts": [{
            "name": "default",
            "domains": ["api.example.test"],
            "routes": [{
                "name": "items",
                "match": { "prefix": { "prefix": "/" } },
                "action": {
                    "cluster": cluster,
                    "timeout_secs": 15
                }
            }]
        }]
    });
    let created_route_config = tools_call(
        fx.app.clone(),
        &fx.admin_token,
        &session,
        "cp_route_configs_create",
        serde_json::json!({
            "team": fx.team_name,
            "name": route_config,
            "spec": route_config_spec
        }),
    )
    .await;
    assert_eq!(created_route_config["result"]["isError"], false);
    assert_eq!(
        created_route_config["result"]["structuredContent"]["name"],
        route_config
    );

    let listener = unique("listener");
    let created_listener = tools_call(
        fx.app.clone(),
        &fx.admin_token,
        &session,
        "cp_listeners_create",
        serde_json::json!({
            "team": fx.team_name,
            "name": listener,
            "spec": {
                "address": "0.0.0.0",
                "port": 19081,
                "route_config": route_config
            }
        }),
    )
    .await;
    assert_eq!(created_listener["result"]["isError"], false);
    assert_eq!(
        created_listener["result"]["structuredContent"]["spec"]["route_config"],
        route_config
    );

    let inspected_listener = tools_call(
        fx.app.clone(),
        &fx.admin_token,
        &session,
        "cp_listeners_get",
        serde_json::json!({
            "team": fx.team_name,
            "name": listener
        }),
    )
    .await;
    assert_eq!(inspected_listener["result"]["isError"], false);
    assert_eq!(
        inspected_listener["result"]["structuredContent"]["name"],
        listener
    );
    assert_eq!(
        inspected_listener["result"]["structuredContent"]["spec"]["route_config"],
        route_config
    );

    let xds_status = tools_call(
        fx.app.clone(),
        &fx.admin_token,
        &session,
        "ops_xds_status",
        serde_json::json!({ "team": fx.team_name }),
    )
    .await;
    assert_eq!(xds_status["result"]["isError"], false);
    assert!(xds_status["result"]["structuredContent"]["total_dataplanes"].is_number());
    assert!(xds_status["result"]["structuredContent"]["recent_nack_count"].is_number());
}

#[tokio::test]
async fn dynamic_api_tools_are_live_enabled_and_team_scoped() {
    let Some(fx) = fixture().await else {
        return;
    };
    let api_name = unique("catalog");
    let response = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/teams/{}/api-definitions", fx.team_name))
                .header("authorization", format!("Bearer {}", fx.admin_token))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": api_name,
                        "openapi": {
                            "openapi": "3.0.3",
                            "info": { "title": "Catalog", "version": "1" },
                            "paths": {
                                "/items/{id}": {
                                    "get": { "operationId": "getItem" }
                                }
                            }
                        }
                    })
                    .to_string(),
                ))
                .expect("create api"),
        )
        .await
        .expect("api response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    let api_id = uuid::Uuid::parse_str(body["api"]["id"].as_str().expect("api id")).expect("uuid");
    // Publish v1 through the product publish surface (not a raw UPDATE): imported specs
    // become servable only through the explicit publish gate (acceptance #6 / inv 16).
    publish_spec(fx.app.clone(), &fx.admin_token, &fx.team_name, &api_name, 1).await;
    let tool_name: String =
        sqlx::query_scalar("SELECT name FROM api_tools WHERE api_definition_id = $1")
            .bind(api_id)
            .fetch_one(&fx.pool)
            .await
            .expect("tool name");
    let mcp_name = format!("api_{tool_name}");

    let gateway_token = create_agent(
        fx.app.clone(),
        &fx.admin_token,
        "gateway-tool",
        fx.team_id,
        vec![("mcp-tools", "execute")],
    )
    .await;
    let gateway_session = initialize(fx.app.clone(), &gateway_token).await;
    let tools = tools_list(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &fx.team_name,
    )
    .await;
    assert!(tools.contains(&mcp_name));

    let cross_team_tools = tools_list(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &fx.other_team_name,
    )
    .await;
    assert!(!cross_team_tools.contains(&mcp_name));

    let call = tools_call(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &mcp_name,
        serde_json::json!({
            "team": fx.team_name,
            "pathParams": { "id": "123" }
        }),
    )
    .await;
    assert_eq!(call["result"]["isError"], true);
    assert!(call["result"]["error"]["message"]
        .as_str()
        .expect("message")
        .contains("has no listener/dataplane route"));
    let audit = sqlx::query(
        "SELECT action, resource, outcome FROM audit_log \
         WHERE team_id = $1 AND action = 'api_tool.execute' AND resource = $2 \
         ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(fx.team_id)
    .bind(format!("api-tools/{tool_name}"))
    .fetch_one(&fx.pool)
    .await
    .expect("dynamic audit row");
    assert_eq!(audit.get::<String, _>("action"), "api_tool.execute");
    assert_eq!(audit.get::<String, _>("outcome"), "failure");

    let route_config_id = insert_route_config(&fx.pool, fx.team, &unique("catalog-routes")).await;
    let listener_id = insert_listener(
        &fx.pool,
        fx.team,
        &unique("catalog-listener"),
        Some("https://gateway.example"),
    )
    .await;
    let mut tx = fx.pool.begin().await.expect("route binding tx");
    storage_api_lifecycle::create_route_binding(
        &mut tx,
        fx.team,
        ApiDefinitionId::from(api_id),
        &unique("catalog-binding"),
        &fp_domain::api_lifecycle::ApiRouteBindingSpec {
            route_config_id,
            listener_id: Some(listener_id),
            virtual_host: Some("api.example".into()),
            route: None,
        },
    )
    .await
    .expect("route binding");
    tx.commit().await.expect("route binding commit");

    let descriptor = tools_call(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &mcp_name,
        serde_json::json!({
            "team": fx.team_name,
            "pathParams": { "id": "123" },
            "query": { "debug": true },
            "headers": { "x-client": "mcp" },
            "body": { "sample": true }
        }),
    )
    .await;
    assert_eq!(descriptor["result"]["isError"], false);
    let content = &descriptor["result"]["structuredContent"];
    assert_eq!(content["type"], "gateway_invocation");
    assert_eq!(content["tool"].as_str(), Some(mcp_name.as_str()));
    assert_eq!(
        content["url"].as_str(),
        Some("https://gateway.example/items/123?debug=true")
    );
    assert_eq!(content["headers"]["host"], "api.example");
    assert_eq!(content["headers"]["x-client"], "mcp");
    assert_eq!(content["auth"]["mode"], "caller_gateway_credentials");
    assert_eq!(content["body"], serde_json::json!({ "sample": true }));

    let host_override = tools_call(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &mcp_name,
        serde_json::json!({
            "team": fx.team_name,
            "headers": { "Host": "other.example" }
        }),
    )
    .await;
    assert_eq!(host_override["result"]["isError"], true);
    assert!(host_override["result"]["error"]["message"]
        .as_str()
        .expect("host override message")
        .contains("controlled by the api route binding"));

    let member_session = initialize(fx.app.clone(), &fx.member_token).await;
    let denied = tools_call(
        fx.app.clone(),
        &fx.member_token,
        &member_session,
        &mcp_name,
        serde_json::json!({ "team": fx.team_name }),
    )
    .await;
    assert_eq!(denied["result"]["isError"], true);
    let denial_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log \
         WHERE team_id = $1 AND action = 'authz.denied' \
           AND detail->>'resource' = 'mcp-tools' \
           AND detail->>'action' = 'execute'",
    )
    .bind(fx.team_id)
    .fetch_one(&fx.pool)
    .await
    .expect("denial audit count");
    assert!(denial_count > 0);

    let response = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/teams/{}/mcp/tools/{tool_name}",
                    fx.team_name
                ))
                .header("authorization", format!("Bearer {}", fx.admin_token))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "enabled": false }).to_string(),
                ))
                .expect("disable tool"),
        )
        .await
        .expect("disable response");
    assert_eq!(response.status(), StatusCode::OK);

    let tools = tools_list(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &fx.team_name,
    )
    .await;
    assert!(!tools.contains(&mcp_name));
}

#[tokio::test]
async fn dynamic_api_tools_reflect_republished_specs() {
    let Some(fx) = fixture().await else {
        return;
    };
    let api_name = unique("catalog");
    let response = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/teams/{}/api-definitions", fx.team_name))
                .header("authorization", format!("Bearer {}", fx.admin_token))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "name": api_name }).to_string(),
                ))
                .expect("create api"),
        )
        .await
        .expect("api response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    let api_id = ApiDefinitionId::from(
        uuid::Uuid::parse_str(body["api"]["id"].as_str().expect("api id")).expect("uuid"),
    );

    let mut tx = fx.pool.begin().await.expect("spec tx");
    let first = storage_api_lifecycle::create_spec_version(
        &mut tx,
        fx.team,
        api_id,
        &SpecVersionInput {
            source_kind: SpecSourceKind::Learned,
            format: SpecFormat::OpenApi3,
            spec: serde_json::json!({
                "openapi": "3.0.3",
                "info": { "title": "Catalog", "version": "1" },
                "paths": { "/items": { "get": { "operationId": "listItems" } } }
            }),
        },
    )
    .await
    .expect("first spec");
    let second = storage_api_lifecycle::create_spec_version(
        &mut tx,
        fx.team,
        api_id,
        &SpecVersionInput {
            source_kind: SpecSourceKind::Learned,
            format: SpecFormat::OpenApi3,
            spec: serde_json::json!({
                "openapi": "3.0.3",
                "info": { "title": "Catalog", "version": "2" },
                "paths": { "/orders": { "post": { "operationId": "createOrder" } } }
            }),
        },
    )
    .await
    .expect("second spec");
    tx.commit().await.expect("spec commit");

    let gateway_token = create_agent(
        fx.app.clone(),
        &fx.admin_token,
        "gateway-tool",
        fx.team_id,
        vec![("mcp-tools", "execute")],
    )
    .await;
    let gateway_session = initialize(fx.app.clone(), &gateway_token).await;
    let first_tool = format!("api_{api_name}-listitems");
    let second_tool = format!("api_{api_name}-createorder");
    let route_config_id = insert_route_config(&fx.pool, fx.team, &unique("catalog-routes")).await;
    let listener_id = insert_listener(
        &fx.pool,
        fx.team,
        &unique("catalog-listener"),
        Some("https://gateway.example"),
    )
    .await;
    let mut tx = fx.pool.begin().await.expect("route binding tx");
    storage_api_lifecycle::create_route_binding(
        &mut tx,
        fx.team,
        api_id,
        &unique("catalog-binding"),
        &fp_domain::api_lifecycle::ApiRouteBindingSpec {
            route_config_id,
            listener_id: Some(listener_id),
            virtual_host: Some("api.example".into()),
            route: None,
        },
    )
    .await
    .expect("route binding");
    tx.commit().await.expect("route binding commit");

    publish_spec(
        fx.app.clone(),
        &fx.admin_token,
        &fx.team_name,
        &api_name,
        first.version,
    )
    .await;
    let tools = tools_list(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &fx.team_name,
    )
    .await;
    assert!(tools.contains(&first_tool));
    let first_descriptor = tools_call(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &first_tool,
        serde_json::json!({ "team": fx.team_name }),
    )
    .await;
    assert_eq!(first_descriptor["result"]["isError"], false);
    let first_content = &first_descriptor["result"]["structuredContent"];
    assert_eq!(first_content["tool"].as_str(), Some(first_tool.as_str()));
    assert_eq!(
        first_content["url"].as_str(),
        Some("https://gateway.example/items")
    );
    assert_dynamic_descriptor_metadata(first_content, api_id, first.id);

    publish_spec(
        fx.app.clone(),
        &fx.admin_token,
        &fx.team_name,
        &api_name,
        second.version,
    )
    .await;
    let tools = tools_list(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &fx.team_name,
    )
    .await;
    assert!(!tools.contains(&first_tool));
    assert!(tools.contains(&second_tool));
    let second_descriptor = tools_call(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &second_tool,
        serde_json::json!({ "team": fx.team_name }),
    )
    .await;
    assert_eq!(second_descriptor["result"]["isError"], false);
    let second_content = &second_descriptor["result"]["structuredContent"];
    assert_eq!(second_content["tool"].as_str(), Some(second_tool.as_str()));
    assert_eq!(
        second_content["url"].as_str(),
        Some("https://gateway.example/orders")
    );
    assert_dynamic_descriptor_metadata(second_content, api_id, second.id);
    assert_ne!(
        first_content["specVersionId"],
        second_content["specVersionId"]
    );
}

/// Acceptance #1: an API created with `--from-openapi` (imported) is INERT by default —
/// it does NOT auto-publish, so its `api_<tool>` is absent from `tools/list` until an
/// explicit publish.
#[tokio::test]
async fn imported_api_is_inert_before_publish() {
    let Some(fx) = fixture().await else {
        return;
    };
    let api_name = unique("imported");
    let response = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/teams/{}/api-definitions", fx.team_name))
                .header("authorization", format!("Bearer {}", fx.admin_token))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": api_name,
                        "openapi": {
                            "openapi": "3.0.3",
                            "info": { "title": "Imported", "version": "1" },
                            "paths": {
                                "/items/{id}": {
                                    "get": { "operationId": "getItem" }
                                }
                            }
                        }
                    })
                    .to_string(),
                ))
                .expect("create api"),
        )
        .await
        .expect("api response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    let api_id = uuid::Uuid::parse_str(body["api"]["id"].as_str().expect("api id")).expect("uuid");

    // The tool row is generated on import, but the API is inert (no published pointer),
    // so the MCP tool must NOT be exposed.
    let tool_name: String =
        sqlx::query_scalar("SELECT name FROM api_tools WHERE api_definition_id = $1")
            .bind(api_id)
            .fetch_one(&fx.pool)
            .await
            .expect("tool name");
    let mcp_name = format!("api_{tool_name}");

    let gateway_token = create_agent(
        fx.app.clone(),
        &fx.admin_token,
        "gateway-tool",
        fx.team_id,
        vec![("mcp-tools", "execute")],
    )
    .await;
    let gateway_session = initialize(fx.app.clone(), &gateway_token).await;
    let tools = tools_list(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &fx.team_name,
    )
    .await;
    assert!(
        !tools.contains(&mcp_name),
        "imported API must be inert before publish: {mcp_name} should be absent"
    );
}

/// Acceptance #2: after publishing an imported spec, its `api_<tool>` is listed for the
/// owning team and absent for any other team.
#[tokio::test]
async fn publishing_imported_api_serves_tools_team_scoped() {
    let Some(fx) = fixture().await else {
        return;
    };
    let api_name = unique("imported");
    let response = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/teams/{}/api-definitions", fx.team_name))
                .header("authorization", format!("Bearer {}", fx.admin_token))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": api_name,
                        "openapi": {
                            "openapi": "3.0.3",
                            "info": { "title": "Imported", "version": "1" },
                            "paths": {
                                "/items/{id}": {
                                    "get": { "operationId": "getItem" }
                                }
                            }
                        }
                    })
                    .to_string(),
                ))
                .expect("create api"),
        )
        .await
        .expect("api response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_of(response).await;
    let api_id = uuid::Uuid::parse_str(body["api"]["id"].as_str().expect("api id")).expect("uuid");
    let tool_name: String =
        sqlx::query_scalar("SELECT name FROM api_tools WHERE api_definition_id = $1")
            .bind(api_id)
            .fetch_one(&fx.pool)
            .await
            .expect("tool name");
    let mcp_name = format!("api_{tool_name}");

    // Publish v1 through the product publish surface.
    publish_spec(fx.app.clone(), &fx.admin_token, &fx.team_name, &api_name, 1).await;

    let gateway_token = create_agent(
        fx.app.clone(),
        &fx.admin_token,
        "gateway-tool",
        fx.team_id,
        vec![("mcp-tools", "execute")],
    )
    .await;
    let gateway_session = initialize(fx.app.clone(), &gateway_token).await;
    let tools = tools_list(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &fx.team_name,
    )
    .await;
    assert!(
        tools.contains(&mcp_name),
        "published imported API must expose {mcp_name} to the owning team"
    );

    let cross_team_tools = tools_list(
        fx.app.clone(),
        &gateway_token,
        &gateway_session,
        &fx.other_team_name,
    )
    .await;
    assert!(
        !cross_team_tools.contains(&mcp_name),
        "published imported API must be team-scoped: {mcp_name} absent for other team"
    );
}

/// Acceptance #4: publishing an imported spec emits an `api.spec.publish` audit row for the
/// team and at least one outbox event (the tool-regeneration / config-change event).
#[tokio::test]
async fn publishing_imported_api_emits_audit_and_outbox() {
    let Some(fx) = fixture().await else {
        return;
    };
    let api_name = unique("imported");
    let response = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/teams/{}/api-definitions", fx.team_name))
                .header("authorization", format!("Bearer {}", fx.admin_token))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": api_name,
                        "openapi": {
                            "openapi": "3.0.3",
                            "info": { "title": "Imported", "version": "1" },
                            "paths": {
                                "/items/{id}": {
                                    "get": { "operationId": "getItem" }
                                }
                            }
                        }
                    })
                    .to_string(),
                ))
                .expect("create api"),
        )
        .await
        .expect("api response");
    assert_eq!(response.status(), StatusCode::CREATED);

    // Count team outbox events before publish so we can assert a positive delta. The exact
    // tool-regeneration `event_type` is an implementation detail (not exposed black-box), so
    // we assert that publish appended at least one new event for the team. The `events`
    // table shape (event_type, team_id, payload) is from migration 0003_outbox.sql, and the
    // query mirrors the count pattern used by crates/fp-core/tests/rate_limit.rs.
    let events_before: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE team_id = $1")
        .bind(fx.team_id)
        .fetch_one(&fx.pool)
        .await
        .expect("events before");

    // Publish v1 through the product publish surface.
    publish_spec(fx.app.clone(), &fx.admin_token, &fx.team_name, &api_name, 1).await;

    // (a) audit row for the publish action.
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log \
         WHERE team_id = $1 AND action = 'api.spec.publish'",
    )
    .bind(fx.team_id)
    .fetch_one(&fx.pool)
    .await
    .expect("publish audit count");
    assert!(
        audit_count >= 1,
        "publish must emit an api.spec.publish audit row for the team"
    );

    // (b) outbox row for the tool-regeneration / config-change event.
    let events_after: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE team_id = $1")
        .bind(fx.team_id)
        .fetch_one(&fx.pool)
        .await
        .expect("events after");
    assert!(
        events_after > events_before,
        "publish must append at least one outbox event for the team \
         (tool-regeneration / config-change event)"
    );
}

/// Acceptance #5: an imported spec cannot be rejected via the REST reject endpoint — reject
/// stays learned-only. The error message must say so.
#[tokio::test]
async fn imported_spec_reject_is_learned_only_over_rest() {
    let Some(fx) = fixture().await else {
        return;
    };
    let api_name = unique("imported");
    let response = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/teams/{}/api-definitions", fx.team_name))
                .header("authorization", format!("Bearer {}", fx.admin_token))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": api_name,
                        "openapi": {
                            "openapi": "3.0.3",
                            "info": { "title": "Imported", "version": "1" },
                            "paths": {
                                "/items/{id}": {
                                    "get": { "operationId": "getItem" }
                                }
                            }
                        }
                    })
                    .to_string(),
                ))
                .expect("create api"),
        )
        .await
        .expect("api response");
    assert_eq!(response.status(), StatusCode::CREATED);

    // Attempt to reject v1 over REST. Path confirmed from existing tests
    // (crates/fp-api/tests/api_crud.rs registers
    //  /api/v1/teams/{team}/api-definitions/{name}/specs/{version}/reject).
    let reject = fx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/teams/{}/api-definitions/{api_name}/specs/1/reject",
                    fx.team_name
                ))
                .header("authorization", format!("Bearer {}", fx.admin_token))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "reason": "nope" }).to_string(),
                ))
                .expect("reject request"),
        )
        .await
        .expect("reject response");
    assert_ne!(
        reject.status(),
        StatusCode::OK,
        "rejecting an imported spec must fail"
    );
    let body = json_of(reject).await;
    let message = body.to_string();
    assert!(
        message.contains("only learned spec versions can be rejected"),
        "expected learned-only reject error, got: {message}"
    );
}
