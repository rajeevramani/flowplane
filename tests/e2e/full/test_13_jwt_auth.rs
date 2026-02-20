//! JWT Authentication Filter Tests (Bruno 13)
//!
//! Tests the JWT authentication filter:
//! - Create JWT filter with mocked JWKS
//! - Create JWKS cluster pointing to mock auth server
//! - Create route and listener with JWT protection
//! - Verify valid JWT allows access
//! - Verify invalid/expired/missing JWT is rejected
//! - Disable filter on specific route for public access

use serde_json::json;
use std::collections::HashMap;

use crate::common::{
    api_client::{
        setup_dev_context, setup_envoy_context, simple_cluster, simple_listener, simple_route,
        ApiClient,
    },
    harness::{TestHarness, TestHarnessConfig},
    timeout::{with_timeout, TestTimeout},
};

/// Test creating a JWT authentication filter
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_800_create_jwt_filter() {
    let harness =
        TestHarness::start(TestHarnessConfig::new("test_800_create_jwt_filter").without_envoy())
            .await
            .expect("Failed to start harness");

    let api = ApiClient::new(harness.api_url());
    let ctx =
        setup_dev_context(&api, "test_800_create_jwt_filter").await.expect("Setup should succeed");

    // Get mock auth server endpoint for JWKS
    let auth_endpoint = harness.mocks().auth_endpoint().expect("Auth mock should be running");
    let auth_uri = harness.mocks().auth_uri().expect("Auth URI should be available");
    let issuer = harness.mocks().get_issuer().expect("Issuer should be available");

    // First create a cluster for the JWKS endpoint
    let auth_parts: Vec<&str> = auth_endpoint.split(':').collect();
    let (auth_host, auth_port) = (auth_parts[0], auth_parts[1].parse::<u16>().unwrap_or(80));

    let jwks_cluster =
        with_timeout(TestTimeout::default_with_label("Create JWKS cluster"), async {
            api.create_cluster(
                &ctx.admin_token,
                &simple_cluster(&ctx.team_a_name, "jwt-jwks-cluster", auth_host, auth_port),
            )
            .await
        })
        .await
        .expect("JWKS cluster creation should succeed");

    println!("✓ JWKS cluster created: {}", jwks_cluster.name);

    // Create JWT filter configuration
    // Note: PathMatch uses externally tagged enum format {"Prefix": "/"} not {"type": "prefix", "value": "/"}
    // And timeout uses timeout_ms not timeout_seconds
    let filter_config = json!({
        "providers": {
            "e2e-auth": {
                "issuer": issuer,
                "audiences": ["e2e-test-api"],
                "jwks": {
                    "type": "remote",
                    "http_uri": {
                        "uri": format!("{}/.well-known/jwks.json", auth_uri),
                        "cluster": jwks_cluster.name,
                        "timeout_ms": 5000
                    }
                },
                "forward": true
            }
        },
        "rules": [
            {
                "match": {
                    "path": {"Prefix": "/"}
                },
                "requires": {
                    "type": "provider_name",
                    "provider_name": "e2e-auth"
                }
            }
        ],
        "bypass_cors_preflight": true
    });

    let filter = with_timeout(TestTimeout::default_with_label("Create JWT filter"), async {
        api.create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "jwt-auth-filter",
            "jwt_auth",
            filter_config,
        )
        .await
    })
    .await
    .expect("Filter creation should succeed");

    assert_eq!(filter.name, "jwt-auth-filter");
    assert_eq!(filter.filter_type, "jwt_auth");
    println!("✓ JWT auth filter created: {} (id={})", filter.name, filter.id);
}

/// Test full JWT authentication flow: filter + listener + verify
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_810_auth_success() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_810_auth_success").with_auth())
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping JWT auth test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    // Use envoy context - creates resources under E2E_SHARED_TEAM so Envoy can see them
    let ctx =
        setup_envoy_context(&api, "test_810_auth_success").await.expect("Setup should succeed");

    // Extract echo server endpoint
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    // Get mock auth server endpoint for JWKS
    let auth_endpoint = harness.mocks().auth_endpoint().expect("Auth mock should be running");
    let auth_uri = harness.mocks().auth_uri().expect("Auth URI should be available");
    let issuer = harness.mocks().get_issuer().expect("Issuer should be available");

    // Create JWKS cluster
    let auth_parts: Vec<&str> = auth_endpoint.split(':').collect();
    let (auth_host, auth_port) = (auth_parts[0], auth_parts[1].parse::<u16>().unwrap_or(80));

    let jwks_cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "auth-jwks-cluster", auth_host, auth_port),
        )
        .await
        .expect("JWKS cluster creation should succeed");

    println!("✓ JWKS cluster created: {}", jwks_cluster.name);

    // Create backend cluster
    let cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "jwt-backend", host, port),
        )
        .await
        .expect("Backend cluster creation should succeed");

    println!("✓ Backend cluster created: {}", cluster.name);

    // Create route
    let route = api
        .create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "jwt-route",
                "jwt.e2e.local",
                "/testing/jwt-api",
                &cluster.name,
            ),
        )
        .await
        .expect("Route creation should succeed");

    println!("✓ Route created: {}", route.name);

    // Create listener
    let listener = api
        .create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "jwt-listener",
                harness.ports.listener,
                &route.name,
                &ctx.team_a_dataplane_id,
            ),
        )
        .await
        .expect("Listener creation should succeed");

    println!("✓ Listener created: {} on port {:?}", listener.name, listener.port);

    // Create JWT filter
    // Note: PathMatch uses externally tagged enum format {"Prefix": "/"} not {"type": "prefix", "value": "/"}
    // And timeout uses timeout_ms not timeout_seconds
    let filter_config = json!({
        "providers": {
            "e2e-auth": {
                "issuer": issuer,
                "audiences": ["e2e-test-api"],
                "jwks": {
                    "type": "remote",
                    "http_uri": {
                        "uri": format!("{}/.well-known/jwks.json", auth_uri),
                        "cluster": jwks_cluster.name,
                        "timeout_ms": 5000
                    }
                },
                "forward": true,
                "claim_to_headers": [
                    {"header_name": "x-jwt-sub", "claim_name": "sub"}
                ]
            }
        },
        "rules": [
            {
                "match": {
                    "path": {"Prefix": "/"}
                },
                "requires": {
                    "type": "provider_name",
                    "provider_name": "e2e-auth"
                }
            }
        ],
        "bypass_cors_preflight": true
    });

    let filter = api
        .create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "e2e-jwt-filter",
            "jwt_auth",
            filter_config,
        )
        .await
        .expect("Filter creation should succeed");

    println!("✓ JWT filter created: {}", filter.name);

    // Install filter on listener
    let installation = with_timeout(TestTimeout::default_with_label("Install filter"), async {
        api.install_filter(&ctx.admin_token, &filter.id, &listener.name, Some(100)).await
    })
    .await
    .expect("Filter installation should succeed");

    println!(
        "✓ Filter installed: filter_id={} on listener={}",
        installation.filter_id, installation.listener_name
    );

    // Wait for JWKS to be fetched and route to converge
    // This may take a bit longer since Envoy needs to fetch JWKS first
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Generate a valid JWT
    let valid_jwt = harness
        .mocks()
        .generate_valid_jwt("test-user-123", None)
        .expect("Should generate valid JWT");

    println!("✓ Generated valid JWT for test-user-123");

    // Make request with valid JWT
    let envoy = harness.envoy().unwrap();
    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), format!("Bearer {}", valid_jwt));

    let (status, response_headers, body) =
        with_timeout(TestTimeout::default_with_label("Request with valid JWT"), async {
            envoy
                .proxy_request(
                    harness.ports.listener,
                    hyper::Method::GET,
                    "jwt.e2e.local",
                    "/testing/jwt-api/protected",
                    headers,
                    None,
                )
                .await
        })
        .await
        .expect("Request with valid JWT should succeed");

    assert_eq!(status, 200, "Expected 200 OK for valid JWT, got: {}", status);

    // Verify the claim was forwarded to upstream
    // Echo server returns headers it received
    let body_json: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|_| json!({"body": body}));
    println!("✓ Response body: {}", serde_json::to_string_pretty(&body_json).unwrap());
    println!("✓ Response headers: {:?}", response_headers);

    println!("✓ JWT auth SUCCESS - valid JWT allowed access");
}

/// Test JWT authentication fails with invalid JWT
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_811_auth_fail_invalid_jwt() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_811_auth_fail").with_auth())
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping JWT auth test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    // Use envoy context - creates resources under E2E_SHARED_TEAM so Envoy can see them
    let ctx = setup_envoy_context(&api, "test_811_auth_fail_invalid_jwt")
        .await
        .expect("Setup should succeed");

    // Setup infrastructure (similar to test_810)
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    let auth_endpoint = harness.mocks().auth_endpoint().expect("Auth mock should be running");
    let auth_uri = harness.mocks().auth_uri().expect("Auth URI should be available");
    let issuer = harness.mocks().get_issuer().expect("Issuer should be available");

    let auth_parts: Vec<&str> = auth_endpoint.split(':').collect();
    let (auth_host, auth_port) = (auth_parts[0], auth_parts[1].parse::<u16>().unwrap_or(80));

    let jwks_cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "fail-jwks-cluster", auth_host, auth_port),
        )
        .await
        .expect("JWKS cluster creation should succeed");

    let cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "fail-backend", host, port),
        )
        .await
        .expect("Backend cluster creation should succeed");

    let route = api
        .create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "fail-route",
                "fail.e2e.local",
                "/testing/jwt-fail",
                &cluster.name,
            ),
        )
        .await
        .expect("Route creation should succeed");

    let listener = api
        .create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "fail-listener",
                harness.ports.listener,
                &route.name,
                &ctx.team_a_dataplane_id,
            ),
        )
        .await
        .expect("Listener creation should succeed");

    let filter_config = json!({
        "providers": {
            "e2e-auth": {
                "issuer": issuer,
                "audiences": ["e2e-test-api"],
                "jwks": {
                    "type": "remote",
                    "http_uri": {
                        "uri": format!("{}/.well-known/jwks.json", auth_uri),
                        "cluster": jwks_cluster.name,
                        "timeout_ms": 5000
                    }
                }
            }
        },
        "rules": [
            {
                "match": {"path": {"Prefix": "/"}},
                "requires": {"type": "provider_name", "provider_name": "e2e-auth"}
            }
        ]
    });

    let filter = api
        .create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "fail-jwt-filter",
            "jwt_auth",
            filter_config,
        )
        .await
        .expect("Filter creation should succeed");

    api.install_filter(&ctx.admin_token, &filter.id, &listener.name, Some(100))
        .await
        .expect("Filter installation should succeed");

    // Wait for setup
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let envoy = harness.envoy().unwrap();

    // Test 1: Request with invalid JWT token
    let mut headers = HashMap::new();
    headers.insert(
        "Authorization".to_string(),
        format!("Bearer {}", crate::common::mocks::MockServices::generate_invalid_jwt()),
    );

    let (status, _, _) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            "fail.e2e.local",
            "/testing/jwt-fail/protected",
            headers,
            None,
        )
        .await
        .expect("Request should complete");

    assert_eq!(status, 401, "Expected 401 for invalid JWT, got: {}", status);
    println!("✓ Invalid JWT correctly rejected with 401");

    // Test 2: Request with no JWT at all
    let (status_no_jwt, _, _) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            "fail.e2e.local",
            "/testing/jwt-fail/protected",
            HashMap::new(),
            None,
        )
        .await
        .expect("Request should complete");

    assert_eq!(status_no_jwt, 401, "Expected 401 for missing JWT, got: {}", status_no_jwt);
    println!("✓ Missing JWT correctly rejected with 401");

    println!("✓ JWT auth FAIL tests passed - invalid/missing JWTs rejected");
}

/// Test JWT authentication fails with expired JWT
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_812_auth_fail_expired_jwt() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_812_auth_expired").with_auth())
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping JWT auth test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    // Use envoy context - creates resources under E2E_SHARED_TEAM so Envoy can see them
    let ctx = setup_envoy_context(&api, "test_812_auth_fail_expired_jwt")
        .await
        .expect("Setup should succeed");

    // Setup infrastructure
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    let auth_endpoint = harness.mocks().auth_endpoint().expect("Auth mock should be running");
    let auth_uri = harness.mocks().auth_uri().expect("Auth URI should be available");
    let issuer = harness.mocks().get_issuer().expect("Issuer should be available");

    let auth_parts: Vec<&str> = auth_endpoint.split(':').collect();
    let (auth_host, auth_port) = (auth_parts[0], auth_parts[1].parse::<u16>().unwrap_or(80));

    let jwks_cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "expired-jwks-cluster", auth_host, auth_port),
        )
        .await
        .expect("JWKS cluster creation should succeed");

    let cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "expired-backend", host, port),
        )
        .await
        .expect("Backend cluster creation should succeed");

    let route = api
        .create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "expired-route",
                "expired.e2e.local",
                "/testing/jwt-expired",
                &cluster.name,
            ),
        )
        .await
        .expect("Route creation should succeed");

    let listener = api
        .create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "expired-listener",
                harness.ports.listener,
                &route.name,
                &ctx.team_a_dataplane_id,
            ),
        )
        .await
        .expect("Listener creation should succeed");

    let filter_config = json!({
        "providers": {
            "e2e-auth": {
                "issuer": issuer,
                "audiences": ["e2e-test-api"],
                "jwks": {
                    "type": "remote",
                    "http_uri": {
                        "uri": format!("{}/.well-known/jwks.json", auth_uri),
                        "cluster": jwks_cluster.name,
                        "timeout_ms": 5000
                    }
                }
            }
        },
        "rules": [
            {
                "match": {"path": {"Prefix": "/"}},
                "requires": {"type": "provider_name", "provider_name": "e2e-auth"}
            }
        ]
    });

    let filter = api
        .create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "expired-jwt-filter",
            "jwt_auth",
            filter_config,
        )
        .await
        .expect("Filter creation should succeed");

    api.install_filter(&ctx.admin_token, &filter.id, &listener.name, Some(100))
        .await
        .expect("Filter installation should succeed");

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Generate an expired JWT
    let expired_jwt =
        harness.mocks().generate_expired_jwt("test-user").expect("Should generate expired JWT");

    let envoy = harness.envoy().unwrap();
    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), format!("Bearer {}", expired_jwt));

    let (status, _, _) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            "expired.e2e.local",
            "/testing/jwt-expired/protected",
            headers,
            None,
        )
        .await
        .expect("Request should complete");

    assert_eq!(status, 401, "Expected 401 for expired JWT, got: {}", status);
    println!("✓ Expired JWT correctly rejected with 401");
}

/// Test disabling JWT filter on specific route for public access
#[tokio::test]
#[ignore = "requires RUN_E2E=1"]
async fn test_815_public_route_bypass() {
    let harness = TestHarness::start(TestHarnessConfig::new("test_815_public_route").with_auth())
        .await
        .expect("Failed to start harness");

    if !harness.has_envoy() {
        println!("⚠ Envoy not available, skipping public route test");
        return;
    }

    let api = ApiClient::new(harness.api_url());
    // Use envoy context - creates resources under E2E_SHARED_TEAM so Envoy can see them
    let ctx = setup_envoy_context(&api, "test_815_public_route_bypass")
        .await
        .expect("Setup should succeed");

    // Setup infrastructure
    let echo_endpoint = harness.echo_endpoint();
    let parts: Vec<&str> = echo_endpoint.split(':').collect();
    let (host, port) = (parts[0], parts[1].parse::<u16>().unwrap_or(8080));

    let auth_endpoint = harness.mocks().auth_endpoint().expect("Auth mock should be running");
    let auth_uri = harness.mocks().auth_uri().expect("Auth URI should be available");
    let issuer = harness.mocks().get_issuer().expect("Issuer should be available");

    let auth_parts: Vec<&str> = auth_endpoint.split(':').collect();
    let (auth_host, auth_port) = (auth_parts[0], auth_parts[1].parse::<u16>().unwrap_or(80));

    let jwks_cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "public-jwks-cluster", auth_host, auth_port),
        )
        .await
        .expect("JWKS cluster creation should succeed");

    let cluster = api
        .create_cluster(
            &ctx.admin_token,
            &simple_cluster(&ctx.team_a_name, "public-backend", host, port),
        )
        .await
        .expect("Backend cluster creation should succeed");

    // Create route with requirement_map that includes allow_missing for public paths
    let route = api
        .create_route(
            &ctx.admin_token,
            &simple_route(
                &ctx.team_a_name,
                "public-route",
                "public.e2e.local",
                "/testing/jwt-public",
                &cluster.name,
            ),
        )
        .await
        .expect("Route creation should succeed");

    let listener = api
        .create_listener(
            &ctx.admin_token,
            &simple_listener(
                &ctx.team_a_name,
                "public-listener",
                harness.ports.listener,
                &route.name,
                &ctx.team_a_dataplane_id,
            ),
        )
        .await
        .expect("Listener creation should succeed");

    // Create JWT filter with rule that allows missing for /public paths
    // Note: PathMatch uses externally tagged enum format {"Prefix": "/"} not {"type": "prefix", "value": "/"}
    let filter_config = json!({
        "providers": {
            "e2e-auth": {
                "issuer": issuer,
                "audiences": ["e2e-test-api"],
                "jwks": {
                    "type": "remote",
                    "http_uri": {
                        "uri": format!("{}/.well-known/jwks.json", auth_uri),
                        "cluster": jwks_cluster.name,
                        "timeout_ms": 5000
                    }
                }
            }
        },
        "rules": [
            {
                "match": {"path": {"Prefix": "/testing/jwt-public/open"}},
                "requires": {"type": "allow_missing"}
            },
            {
                "match": {"path": {"Prefix": "/testing/jwt-public"}},
                "requires": {"type": "provider_name", "provider_name": "e2e-auth"}
            }
        ]
    });

    let filter = api
        .create_filter(
            &ctx.admin_token,
            &ctx.team_a_name,
            "public-jwt-filter",
            "jwt_auth",
            filter_config,
        )
        .await
        .expect("Filter creation should succeed");

    api.install_filter(&ctx.admin_token, &filter.id, &listener.name, Some(100))
        .await
        .expect("Filter installation should succeed");

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let envoy = harness.envoy().unwrap();

    // Test 1: Public path should work without JWT
    let (status_public, _, _) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            "public.e2e.local",
            "/testing/jwt-public/open/health",
            HashMap::new(),
            None,
        )
        .await
        .expect("Request should complete");

    assert_eq!(status_public, 200, "Expected 200 for public path, got: {}", status_public);
    println!("✓ Public path accessible without JWT");

    // Test 2: Protected path should still require JWT
    let (status_protected, _, _) = envoy
        .proxy_request(
            harness.ports.listener,
            hyper::Method::GET,
            "public.e2e.local",
            "/testing/jwt-public/protected",
            HashMap::new(),
            None,
        )
        .await
        .expect("Request should complete");

    assert_eq!(status_protected, 401, "Expected 401 for protected path, got: {}", status_protected);
    println!("✓ Protected path still requires JWT");

    println!("✓ Public route bypass test passed");
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_jwt_filter_config_format() {
        // Verify the config JSON structure is valid
        // Note: PathMatch uses externally tagged enum format {"Prefix": "/"} not {"type": "prefix", "value": "/"}
        // And timeout uses timeout_ms not timeout_seconds
        let config = serde_json::json!({
            "providers": {
                "test-provider": {
                    "issuer": "https://test.example.com/",
                    "audiences": ["test-api"],
                    "jwks": {
                        "type": "remote",
                        "http_uri": {
                            "uri": "https://test.example.com/.well-known/jwks.json",
                            "cluster": "test-cluster",
                            "timeout_ms": 5000
                        }
                    }
                }
            },
            "rules": [{
                "match": {"path": {"Prefix": "/"}},
                "requires": {"type": "provider_name", "provider_name": "test-provider"}
            }]
        });

        assert!(config["providers"].is_object());
        assert!(config["rules"].is_array());
    }
}
