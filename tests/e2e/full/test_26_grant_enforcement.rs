//! Grant Enforcement E2E Tests (test_26)
//!
//! Tests grant-based access control through the REST API and MCP tools:
//! - REST: read with matching grant, write without grant, cross-team isolation, grant lifecycle
//! - MCP: tools/list filtering for CP agents, tools/call enforcement, gateway-tool agent isolation
//!
//! All tests authenticate via real Zitadel JWT tokens (not mocked auth).

use serde_json::{json, Value};

use crate::common::{
    api_client::ApiClient,
    shared_infra::SharedInfrastructure,
    timeout::{with_timeout, TestTimeout},
    zitadel,
};

/// Shared setup: get infra, obtain superadmin token, create a tenant org + team,
/// create a human test user in Zitadel, add them to the org + team.
/// Returns (api, admin_token, org_name, org_id, team_name, team_id, user_id, user_email, user_password).
async fn setup_grant_test(
    suffix: &str,
) -> (ApiClient, String, String, String, String, String, String, String, String) {
    let infra = SharedInfrastructure::get_or_init()
        .await
        .expect("Failed to initialize shared infrastructure");

    let api = ApiClient::new(infra.api_url());

    let admin_token =
        with_timeout(TestTimeout::default_with_label("Obtain superadmin JWT"), async {
            zitadel::obtain_human_token(
                &infra.zitadel_config,
                zitadel::SUPERADMIN_EMAIL,
                zitadel::SUPERADMIN_PASSWORD,
            )
            .await
        })
        .await
        .expect("JWT acquisition should succeed");

    // Create tenant org
    let org_name = format!("grant-e2e-{}", suffix);
    let tenant_org = with_timeout(TestTimeout::default_with_label("Create tenant org"), async {
        api.create_organization_idempotent(
            &admin_token,
            &org_name,
            &format!("Grant E2E Org {}", suffix),
            Some("Tenant org for grant enforcement E2E tests"),
        )
        .await
    })
    .await
    .expect("Create tenant org should succeed");
    let org_id = tenant_org.id.clone();
    let org_name = tenant_org.name.clone();

    // Create team within the org
    let team_name = format!("grant-team-{}", suffix);
    let team = with_timeout(TestTimeout::default_with_label("Create team"), async {
        api.create_team_idempotent(
            &admin_token,
            &team_name,
            Some(&format!("Grant team {}", suffix)),
            &org_id,
        )
        .await
    })
    .await
    .expect("Create team should succeed");
    let team_id = team.id.clone();
    let team_name = team.name.clone();

    // Create human test user in Zitadel
    let user_email = format!("grant-user-{}@e2e.test", suffix);
    let user_password = "GrantTest123!";
    with_timeout(TestTimeout::default_with_label("Create Zitadel user"), async {
        zitadel::create_human_user(
            &infra.zitadel_config.base_url,
            &infra.zitadel_config.admin_pat,
            &user_email,
            "Grant",
            &format!("User {}", suffix),
            user_password,
        )
        .await
    })
    .await
    .expect("Create Zitadel user should succeed");

    // JIT-provision user in CP by authenticating them
    // (POST /api/v1/users no longer exists — users are provisioned via JWT auth)
    let user_token =
        with_timeout(TestTimeout::default_with_label("Authenticate test user"), async {
            zitadel::obtain_human_token(&infra.zitadel_config, &user_email, user_password).await
        })
        .await
        .expect("User authentication should succeed");

    let user_session =
        api.get_auth_session(&user_token).await.expect("User auth session should succeed");
    let user_id = user_session.user_id;

    // Add user to org as member
    with_timeout(TestTimeout::default_with_label("Add user to org"), async {
        api.add_org_member(&admin_token, &org_id, &user_id, "member").await
    })
    .await
    .expect("Add org member should succeed");

    // Add user to team (via admin API)
    let (add_status, _) =
        with_timeout(TestTimeout::default_with_label("Add user to team"), async {
            api.post(
                &admin_token,
                &format!("/api/v1/orgs/{}/teams/{}/members", org_name, team_name),
                json!({ "userId": user_id }),
            )
            .await
        })
        .await
        .expect("Add team member should succeed");

    assert!(
        add_status.is_success() || add_status.as_u16() == 409,
        "Add team member should return 2xx or 409, got {}",
        add_status,
    );

    (
        api,
        admin_token,
        org_name,
        org_id,
        team_name,
        team_id,
        user_id,
        user_email,
        user_password.to_string(),
    )
}

/// Helper: create a grant for a principal via the grants API
async fn create_grant(
    api: &ApiClient,
    admin_token: &str,
    org_name: &str,
    principal_id: &str,
    team: &str,
    resource_type: &str,
    action: &str,
) -> Value {
    let (status, body) = with_timeout(TestTimeout::default_with_label("Create grant"), async {
        api.post(
            admin_token,
            &format!("/api/v1/orgs/{}/principals/{}/grants", org_name, principal_id),
            json!({
                "grantType": "resource",
                "resourceType": resource_type,
                "action": action,
                "team": team,
            }),
        )
        .await
    })
    .await
    .expect("Create grant request should succeed");

    assert!(
        status.is_success() || status.as_u16() == 409,
        "Create grant should return 2xx or 409, got {} - {:?}",
        status,
        body,
    );
    body
}

/// Helper: list grants for a principal
async fn list_grants(
    api: &ApiClient,
    admin_token: &str,
    org_name: &str,
    principal_id: &str,
) -> Vec<Value> {
    let (status, body) = with_timeout(TestTimeout::default_with_label("List grants"), async {
        api.get(
            admin_token,
            &format!("/api/v1/orgs/{}/principals/{}/grants", org_name, principal_id),
        )
        .await
    })
    .await
    .expect("List grants request should succeed");

    assert!(status.is_success(), "List grants should return 2xx, got {}", status);
    body["grants"].as_array().cloned().unwrap_or_default()
}

/// Helper: delete a grant
async fn delete_grant(
    api: &ApiClient,
    admin_token: &str,
    org_name: &str,
    principal_id: &str,
    grant_id: &str,
) {
    let status = with_timeout(TestTimeout::default_with_label("Delete grant"), async {
        api.delete(
            admin_token,
            &format!("/api/v1/orgs/{}/principals/{}/grants/{}", org_name, principal_id, grant_id),
        )
        .await
    })
    .await
    .expect("Delete grant request should succeed");

    assert_eq!(status.as_u16(), 204, "Delete grant should return 204, got {}", status);
}

/// Helper: obtain a user JWT token
async fn get_user_token(user_email: &str, user_password: &str) -> String {
    let infra = SharedInfrastructure::get_or_init().await.expect("infra should be ready");
    with_timeout(TestTimeout::default_with_label("Obtain user JWT"), async {
        zitadel::obtain_human_token(&infra.zitadel_config, user_email, user_password).await
    })
    .await
    .expect("User JWT acquisition should succeed")
}

/// Helper: send an MCP JSON-RPC request with session initialization and return the parsed response
async fn mcp_request(api: &ApiClient, token: &str, method: &str, params: Value) -> Value {
    let (status, body) = with_timeout(TestTimeout::default_with_label("MCP request"), async {
        api.mcp_request(token, method, params).await
    })
    .await
    .expect("MCP request should succeed");

    assert!(
        status.is_success(),
        "MCP request '{}' should return 2xx, got {} - {:?}",
        method,
        status,
        body,
    );
    body
}

// =============================================================================
// REST API Scenarios
// =============================================================================

/// Scenario 1: Read with matching grant — user with clusters:read grant can GET /api/v1/clusters
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2600_read_with_matching_grant() {
    let (api, admin_token, org_name, _org_id, team_name, _team_id, user_id, user_email, password) =
        setup_grant_test("s1-read").await;

    // Grant clusters:read
    create_grant(&api, &admin_token, &org_name, &user_id, &team_name, "clusters", "read").await;

    // Allow cache to settle
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Obtain user token and try listing clusters
    let user_token = get_user_token(&user_email, &password).await;
    let clusters_path = format!("/api/v1/teams/{}/clusters", team_name);
    let (status, _body) = with_timeout(TestTimeout::default_with_label("GET clusters"), async {
        api.get(&user_token, &clusters_path).await
    })
    .await
    .expect("GET clusters should succeed");

    assert_eq!(
        status.as_u16(),
        200,
        "User with clusters:read grant should get 200, got {}",
        status
    );
    println!("ok Scenario 1: clusters:read grant → GET /api/v1/teams/{{team}}/clusters → 200");
}

/// Scenario 2: Write without grant — same user (no clusters:create) gets 403
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2601_write_without_grant() {
    let (api, admin_token, org_name, _org_id, team_name, _team_id, user_id, user_email, password) =
        setup_grant_test("s2-write").await;

    // Grant ONLY clusters:read (not clusters:create)
    create_grant(&api, &admin_token, &org_name, &user_id, &team_name, "clusters", "read").await;

    // Allow cache to settle
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let user_token = get_user_token(&user_email, &password).await;

    // Try creating a cluster — should be denied
    let clusters_path = format!("/api/v1/teams/{}/clusters", team_name);
    let (status, _body) = with_timeout(TestTimeout::default_with_label("POST clusters"), async {
        api.post(
            &user_token,
            &clusters_path,
            json!({
                "name": "unauthorized-cluster",
                "endpoints": [{"host": "127.0.0.1", "port": 8080}]
            }),
        )
        .await
    })
    .await
    .expect("POST clusters should return a response");

    assert_eq!(
        status.as_u16(),
        403,
        "User without clusters:create grant should get 403, got {}",
        status
    );
    println!(
        "ok Scenario 2: no clusters:create grant → POST /api/v1/teams/{{team}}/clusters → 403"
    );
}

/// Scenario 3: Cross-team isolation — user with team-A grant cannot access team-B resources
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2602_cross_team_isolation() {
    let infra = SharedInfrastructure::get_or_init().await.expect("infra");
    let api = ApiClient::new(infra.api_url());

    let admin_token =
        with_timeout(TestTimeout::default_with_label("Obtain superadmin JWT"), async {
            zitadel::obtain_human_token(
                &infra.zitadel_config,
                zitadel::SUPERADMIN_EMAIL,
                zitadel::SUPERADMIN_PASSWORD,
            )
            .await
        })
        .await
        .expect("JWT acquisition should succeed");

    // Create org with two teams
    let org = with_timeout(TestTimeout::default_with_label("Create org"), async {
        api.create_organization_idempotent(
            &admin_token,
            "grant-e2e-s3-iso",
            "Grant E2E Isolation",
            None,
        )
        .await
    })
    .await
    .expect("Create org should succeed");

    let team_a = with_timeout(TestTimeout::default_with_label("Create team A"), async {
        api.create_team_idempotent(&admin_token, "iso-team-a", Some("Isolation Team A"), &org.id)
            .await
    })
    .await
    .expect("Create team A should succeed");

    let team_b = with_timeout(TestTimeout::default_with_label("Create team B"), async {
        api.create_team_idempotent(&admin_token, "iso-team-b", Some("Isolation Team B"), &org.id)
            .await
    })
    .await
    .expect("Create team B should succeed");

    // Create user in Zitadel
    let user_email = "iso-user@e2e.test";
    let user_password = "IsoTest123!";
    let _ = with_timeout(TestTimeout::default_with_label("Create Zitadel user"), async {
        zitadel::create_human_user(
            &infra.zitadel_config.base_url,
            &infra.zitadel_config.admin_pat,
            user_email,
            "Isolation",
            "User",
            user_password,
        )
        .await
    })
    .await
    .expect("Create Zitadel user should succeed");

    // JIT-provision user in CP by authenticating them
    let iso_user_token =
        with_timeout(TestTimeout::default_with_label("Authenticate isolation user"), async {
            zitadel::obtain_human_token(&infra.zitadel_config, user_email, user_password).await
        })
        .await
        .expect("User authentication should succeed");

    let iso_session =
        api.get_auth_session(&iso_user_token).await.expect("User auth session should succeed");
    let user_id = iso_session.user_id;

    // Add user to org
    with_timeout(TestTimeout::default_with_label("Add to org"), async {
        api.add_org_member(&admin_token, &org.id, &user_id, "member").await
    })
    .await
    .expect("Add org member should succeed");

    // Add user to team A ONLY (not team B)
    let _ = with_timeout(TestTimeout::default_with_label("Add to team A"), async {
        api.post(
            &admin_token,
            &format!("/api/v1/orgs/{}/teams/{}/members", org.name, team_a.name),
            json!({ "userId": user_id }),
        )
        .await
    })
    .await
    .expect("Add to team A should succeed");

    // Grant clusters:read + clusters:create on team A
    create_grant(&api, &admin_token, &org.name, &user_id, &team_a.name, "clusters", "read").await;
    create_grant(&api, &admin_token, &org.name, &user_id, &team_a.name, "clusters", "create").await;

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let user_token = get_user_token(user_email, user_password).await;

    // Creating a cluster in team A should succeed
    let team_a_clusters_path = format!("/api/v1/teams/{}/clusters", team_a.name);
    let (status_a, _) =
        with_timeout(TestTimeout::default_with_label("POST cluster in team A"), async {
            api.post(
                &user_token,
                &team_a_clusters_path,
                json!({
                    "name": "iso-cluster-a",
                    "endpoints": [{"host": "127.0.0.1", "port": 9090}]
                }),
            )
            .await
        })
        .await
        .expect("POST cluster to team A should return a response");

    // Accept 201 (created) or 409 (already exists from re-run)
    assert!(
        status_a.as_u16() == 201 || status_a.as_u16() == 409,
        "Team A cluster create should succeed (201/409), got {}",
        status_a
    );

    // Creating a cluster in team B should fail — user is NOT a member of team B
    let team_b_clusters_path = format!("/api/v1/teams/{}/clusters", team_b.name);
    let (status_b, _) =
        with_timeout(TestTimeout::default_with_label("POST cluster in team B"), async {
            api.post(
                &user_token,
                &team_b_clusters_path,
                json!({
                    "name": "iso-cluster-b",
                    "endpoints": [{"host": "127.0.0.1", "port": 9091}]
                }),
            )
            .await
        })
        .await
        .expect("POST cluster to team B should return a response");

    assert_eq!(
        status_b.as_u16(),
        403,
        "User with team-A grants should get 403 when creating in team-B, got {}",
        status_b
    );
    println!("ok Scenario 3: cross-team isolation verified (team-A ok, team-B denied)");
}

/// Scenario 4: Grant lifecycle — create grant → access → revoke → deny
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2603_grant_lifecycle() {
    let (api, admin_token, org_name, _org_id, team_name, _team_id, user_id, user_email, password) =
        setup_grant_test("s4-lifecycle").await;

    // Step 1: User has default clusters:read grant from team membership (members get
    // read grants for all resources via grants_for_org_role). Listing returns 200 (empty
    // because no clusters exist yet in this team).
    let user_token = get_user_token(&user_email, &password).await;
    let clusters_path = format!("/api/v1/teams/{}/clusters", team_name);
    let (status, body) =
        with_timeout(TestTimeout::default_with_label("GET clusters (default grant)"), async {
            api.get(&user_token, &clusters_path).await
        })
        .await
        .expect("GET clusters should return a response");

    assert_eq!(
        status.as_u16(),
        200,
        "Team member with default clusters:read grant → 200 (empty list), got {}",
        status
    );
    let empty = vec![];
    let clusters = body.as_array().unwrap_or(&empty);
    assert!(
        clusters.is_empty(),
        "No clusters created yet, should see empty list, got {} items",
        clusters.len()
    );
    println!("ok Step 1: default grant → 200 (empty list)");

    // Step 2: Create clusters:read grant → should succeed
    create_grant(&api, &admin_token, &org_name, &user_id, &team_name, "clusters", "read").await;

    // Wait for cache eviction + re-obtain token (permissions may be cached in JWT)
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let user_token = get_user_token(&user_email, &password).await;

    let (status, _) =
        with_timeout(TestTimeout::default_with_label("GET clusters (with grant)"), async {
            api.get(&user_token, &clusters_path).await
        })
        .await
        .expect("GET clusters should return a response");

    assert_eq!(status.as_u16(), 200, "With clusters:read grant → should get 200, got {}", status);
    println!("ok Step 2: grant created → 200");

    // Verify grant appears in list and get its ID for deletion
    let grants = list_grants(&api, &admin_token, &org_name, &user_id).await;
    let grant = grants.iter().find(|g| {
        g["resourceType"].as_str() == Some("clusters") && g["action"].as_str() == Some("read")
    });
    assert!(grant.is_some(), "clusters:read grant should appear in list");
    let grant_id = grant.unwrap()["id"].as_str().expect("Grant should have id");

    // Step 3: Revoke grant → should be denied again (403, same as step 1)
    delete_grant(&api, &admin_token, &org_name, &user_id, grant_id).await;

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let user_token = get_user_token(&user_email, &password).await;

    let (status, _) =
        with_timeout(TestTimeout::default_with_label("GET clusters (revoked)"), async {
            api.get(&user_token, &clusters_path).await
        })
        .await
        .expect("GET clusters should return a response");

    assert_eq!(status.as_u16(), 403, "After revocation → 403 (same as step 1), got {}", status);
    println!("ok Step 3: grant revoked → 403 (access denied)");

    // Verify grant no longer in list
    let grants = list_grants(&api, &admin_token, &org_name, &user_id).await;
    let has_grant = grants.iter().any(|g| {
        g["resourceType"].as_str() == Some("clusters") && g["action"].as_str() == Some("read")
    });
    assert!(!has_grant, "clusters:read grant should be gone after revocation");

    println!("ok Scenario 4: grant lifecycle (create → access → revoke → deny) verified");
}

// =============================================================================
// MCP Tool Scenarios
// =============================================================================

/// Helper: create a CP-tool agent and return (agent_id, client_id, client_secret).
/// Deletes existing agent first if present (re-run safety), since client_secret
/// is only returned at creation time.
async fn create_cp_agent(
    api: &ApiClient,
    admin_token: &str,
    org_name: &str,
    agent_name: &str,
    team_name: &str,
) -> (String, String, String) {
    // Delete existing agent if present (idempotent re-run support)
    let _ = with_timeout(TestTimeout::default_with_label("Delete existing agent"), async {
        api.delete(admin_token, &format!("/api/v1/orgs/{}/agents/{}", org_name, agent_name)).await
    })
    .await;

    let (status, body) = with_timeout(TestTimeout::default_with_label("Create agent"), async {
        api.post(
            admin_token,
            &format!("/api/v1/orgs/{}/agents", org_name),
            json!({
                "name": agent_name,
                "teams": [team_name],
            }),
        )
        .await
    })
    .await
    .expect("Create agent should succeed");

    assert!(status.is_success(), "Create agent should return 2xx, got {} - {:?}", status, body,);

    let agent_id = body["agentId"].as_str().expect("agent should have agentId").to_string();
    let client_id = body["clientId"].as_str().expect("new agent should have clientId").to_string();
    let client_secret =
        body["clientSecret"].as_str().expect("new agent should have clientSecret").to_string();

    (agent_id, client_id, client_secret)
}

/// Helper: obtain an agent token via client_credentials grant
async fn get_agent_token(client_id: &str, client_secret: &str) -> String {
    let infra = SharedInfrastructure::get_or_init().await.expect("infra should be ready");
    with_timeout(TestTimeout::default_with_label("Obtain agent JWT"), async {
        zitadel::obtain_agent_token(&infra.zitadel_config, client_id, client_secret).await
    })
    .await
    .expect("Agent JWT acquisition should succeed")
}

/// Scenario 5: CP agent tools/list filtering — agent with clusters:read + routes:create
/// grants should only see matching tools
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2604_cp_agent_tools_list_filtering() {
    let (api, admin_token, org_name, _org_id, team_name, _team_id, _, _, _) =
        setup_grant_test("s5-list").await;

    // Create CP-tool agent
    let (agent_id, client_id, client_secret) =
        create_cp_agent(&api, &admin_token, &org_name, "list-filter-agent", &team_name).await;

    // Grant clusters:read + routes:create
    create_grant(&api, &admin_token, &org_name, &agent_id, &team_name, "clusters", "read").await;
    create_grant(&api, &admin_token, &org_name, &agent_id, &team_name, "routes", "create").await;

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Obtain agent token
    let agent_token = get_agent_token(&client_id, &client_secret).await;

    // Call tools/list
    let response = mcp_request(&api, &agent_token, "tools/list", json!({})).await;

    let tools =
        response["result"]["tools"].as_array().expect("tools/list should return tools array");

    // Should have tools matching clusters:read (cp_list_clusters, cp_get_cluster)
    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    assert!(
        tool_names.contains(&"cp_list_clusters"),
        "Should have cp_list_clusters for clusters:read grant. Tools: {:?}",
        tool_names
    );
    assert!(
        tool_names.contains(&"cp_create_route_config"),
        "Should have cp_create_route_config for routes:create grant. Tools: {:?}",
        tool_names
    );

    // Should NOT have tools for unganted resources
    assert!(
        !tool_names.contains(&"cp_create_cluster"),
        "Should NOT have cp_create_cluster (no clusters:create). Tools: {:?}",
        tool_names
    );
    assert!(
        !tool_names.contains(&"cp_list_listeners"),
        "Should NOT have cp_list_listeners (no listeners:read). Tools: {:?}",
        tool_names
    );

    // Should have no api_* (gateway) tools — CP agents never see those
    let has_api_tool = tool_names.iter().any(|n| n.starts_with("api_"));
    assert!(!has_api_tool, "CP agent should see no api_* tools. Tools: {:?}", tool_names);

    println!("ok Scenario 5: CP agent tools/list shows {} tools matching grants only", tools.len());
}

/// Scenario 6: CP agent tools/call enforcement — granted tool succeeds, non-granted tool denied
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2605_cp_agent_tools_call_enforcement() {
    let (api, admin_token, org_name, _org_id, team_name, _team_id, _, _, _) =
        setup_grant_test("s6-call").await;

    // Create CP-tool agent with clusters:read grant only
    let (agent_id, client_id, client_secret) =
        create_cp_agent(&api, &admin_token, &org_name, "call-enforce-agent", &team_name).await;

    create_grant(&api, &admin_token, &org_name, &agent_id, &team_name, "clusters", "read").await;

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let agent_token = get_agent_token(&client_id, &client_secret).await;

    // Granted tool: cp_list_clusters → should succeed
    let response = mcp_request(
        &api,
        &agent_token,
        "tools/call",
        json!({
            "name": "cp_list_clusters",
            "arguments": { "team": team_name }
        }),
    )
    .await;

    // Successful tool call returns result with content
    assert!(
        response.get("result").is_some(),
        "cp_list_clusters should succeed (granted). Response: {:?}",
        response
    );
    assert!(
        response.get("error").is_none(),
        "cp_list_clusters should not error (granted). Response: {:?}",
        response
    );
    println!("ok cp_list_clusters (granted) → success");

    // Non-granted tool: cp_create_cluster → should be denied
    let response = mcp_request(
        &api,
        &agent_token,
        "tools/call",
        json!({
            "name": "cp_create_cluster",
            "arguments": {
                "team": team_name,
                "name": "unauthorized-mcp-cluster",
                "endpoints": [{"host": "127.0.0.1", "port": 8080}]
            }
        }),
    )
    .await;

    // Should be an error response (Forbidden)
    let has_error = response.get("error").is_some();
    let result_text = response
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    let denied = has_error
        || result_text.to_lowercase().contains("denied")
        || result_text.to_lowercase().contains("forbidden")
        || result_text.to_lowercase().contains("permission");

    assert!(
        denied,
        "cp_create_cluster should be denied (no clusters:create grant). Response: {:?}",
        response
    );
    println!("ok cp_create_cluster (not granted) → denied");
    println!("ok Scenario 6: CP agent tools/call enforcement verified");
}

/// Scenario 7: Gateway-tool agent isolation — should see only api_* tools, zero cp_* tools
///
/// Note: The agent_context is hardcoded as CpTool in current code (TODO E.3).
/// This test verifies that an agent with NO resource grants sees no CP tools,
/// which effectively demonstrates the filtering boundary.
/// When agent types become configurable, this test can be enhanced.
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_2606_agent_no_grants_sees_no_tools() {
    let (api, admin_token, org_name, _org_id, team_name, _team_id, _, _, _) =
        setup_grant_test("s7-nogrant").await;

    // Create CP-tool agent with NO grants
    let (_agent_id, client_id, client_secret) =
        create_cp_agent(&api, &admin_token, &org_name, "no-grant-agent", &team_name).await;

    // Do NOT create any grants

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let agent_token = get_agent_token(&client_id, &client_secret).await;

    // Call tools/list — should return empty list
    let response = mcp_request(&api, &agent_token, "tools/list", json!({})).await;

    let tools =
        response["result"]["tools"].as_array().expect("tools/list should return tools array");

    // No grants → no CP tools visible
    let cp_tools: Vec<&str> =
        tools.iter().filter_map(|t| t["name"].as_str()).filter(|n| n.starts_with("cp_")).collect();

    assert!(
        cp_tools.is_empty(),
        "Agent with no grants should see zero cp_* tools, but saw: {:?}",
        cp_tools
    );

    // Also no api_* tools (CP-tool agents never see gateway tools)
    let api_tools: Vec<&str> =
        tools.iter().filter_map(|t| t["name"].as_str()).filter(|n| n.starts_with("api_")).collect();

    assert!(
        api_tools.is_empty(),
        "CP-tool agent should see zero api_* tools, but saw: {:?}",
        api_tools
    );

    println!("ok Scenario 7: agent with no grants sees zero tools (tools count: {})", tools.len());
}
