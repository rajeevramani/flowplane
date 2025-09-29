// Contract tests for Platform API Abstraction
// These tests validate the API contract and will initially fail until implementation is complete

use serde_json::json;
use hyper::{StatusCode, Method};
use uuid::Uuid;
use crate::test_utils::{TestApiClient, assert_json_schema};

// Helper to create test client - will be implemented with actual API
async fn platform_api_test_client() -> TestApiClient {
    todo!("Initialize test API client with Platform API support")
}

#[tokio::test]
async fn post_api_definition_success() {
    let client = platform_api_test_client().await;

    let api_definition = json!({
        "team": "test-team",
        "domain": "test.flowplane.dev",
        "routes": [
            {
                "path_config": {
                    "match_type": "prefix",
                    "pattern": "/api/v1/"
                },
                "upstream_config": {
                    "targets": [
                        {
                            "name": "test-backend",
                            "endpoint": "test.svc.cluster.local:8080",
                            "weight": 100
                        }
                    ]
                },
                "timeout_seconds": 30
            }
        ],
        "listener_isolation": false
    });

    let response = client
        .request(Method::POST, "/v1/api-definitions")
        .json(&api_definition)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::CREATED);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_json_schema(&body, &json!({
        "type": "object",
        "required": ["id", "team", "domain", "listener", "routes", "clusters", "bootstrap_uri"],
        "properties": {
            "id": {"type": "string", "format": "uuid"},
            "team": {"type": "string"},
            "domain": {"type": "string"},
            "listener": {"type": "string"},
            "routes": {"type": "array", "items": {"type": "string"}},
            "clusters": {"type": "array", "items": {"type": "string"}},
            "bootstrap_uri": {"type": "string", "format": "uri"}
        }
    }));
    assert_eq!(body["team"], "test-team");
    assert_eq!(body["domain"], "test.flowplane.dev");
}

#[tokio::test]
async fn post_api_definition_validation_error() {
    let client = platform_api_test_client().await;

    // Invalid config: missing required fields
    let invalid_config = json!({
        "team": "",  // Invalid empty team
        "domain": "invalid-domain-format",
        "routes": []  // Invalid empty routes
    });

    let response = client
        .request(Method::POST, "/v1/api-definitions")
        .json(&invalid_config)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_json_schema(&body, &json!({
        "type": "object",
        "required": ["error", "validation_errors"],
        "properties": {
            "error": {"type": "string"},
            "validation_errors": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["field", "message"],
                    "properties": {
                        "field": {"type": "string"},
                        "message": {"type": "string"},
                        "code": {"type": "string"}
                    }
                }
            }
        }
    }));

    let validation_errors = body["validation_errors"].as_array().expect("validation_errors array");
    assert!(validation_errors.iter().any(|error|
        error["field"].as_str() == Some("team")
    ));
    assert!(validation_errors.iter().any(|error|
        error["field"].as_str() == Some("routes")
    ));
}

#[tokio::test]
async fn post_api_definition_collision_error() {
    let client = platform_api_test_client().await;

    // Create first API definition
    let first_api = json!({
        "team": "team-a",
        "domain": "shared.flowplane.dev",
        "routes": [
            {
                "path_config": {
                    "match_type": "prefix",
                    "pattern": "/api/"
                },
                "upstream_config": {
                    "targets": [
                        {
                            "name": "service-a",
                            "endpoint": "service-a.svc.cluster.local:8080",
                            "weight": 100
                        }
                    ]
                }
            }
        ],
        "listener_isolation": false
    });

    client
        .request(Method::POST, "/v1/api-definitions")
        .json(&first_api)
        .send()
        .await
        .expect("First API creation failed");

    // Attempt to create conflicting API definition
    let conflicting_api = json!({
        "team": "team-b",
        "domain": "shared.flowplane.dev",  // Same domain
        "routes": [
            {
                "path_config": {
                    "match_type": "prefix",
                    "pattern": "/api/"  // Same path prefix
                },
                "upstream_config": {
                    "targets": [
                        {
                            "name": "service-b",
                            "endpoint": "service-b.svc.cluster.local:8080",
                            "weight": 100
                        }
                    ]
                }
            }
        ],
        "listener_isolation": false
    });

    let response = client
        .request(Method::POST, "/v1/api-definitions")
        .json(&conflicting_api)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::CONFLICT);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_json_schema(&body, &json!({
        "type": "object",
        "required": ["error", "collision_type", "conflicting_resource"],
        "properties": {
            "error": {"type": "string"},
            "collision_type": {"type": "string"},
            "conflicting_resource": {
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "team": {"type": "string"},
                    "domain": {"type": "string"},
                    "path": {"type": "string"}
                }
            },
            "suggestions": {"type": "array", "items": {"type": "string"}}
        }
    }));

    assert_eq!(body["collision_type"], "path_conflict");
    assert_eq!(body["conflicting_resource"]["team"], "team-a");
}

#[tokio::test]
async fn post_api_definition_multi_upstream_success() {
    let client = platform_api_test_client().await;

    let multi_upstream_api = json!({
        "team": "canary-team",
        "domain": "canary.flowplane.dev",
        "routes": [
            {
                "path_config": {
                    "match_type": "prefix",
                    "pattern": "/api/",
                    "rewrite": {
                        "prefix": "/internal/"
                    }
                },
                "upstream_config": {
                    "targets": [
                        {
                            "name": "stable",
                            "endpoint": "stable.svc.cluster.local:8080",
                            "weight": 80
                        },
                        {
                            "name": "canary",
                            "endpoint": "canary.svc.cluster.local:8080",
                            "weight": 20
                        }
                    ],
                    "load_balancing": "weighted_round_robin"
                },
                "timeout_seconds": 15
            }
        ],
        "listener_isolation": true,
        "tls": {
            "mode": "terminate",
            "cert_source": {
                "type": "secret_manager",
                "reference": "arn:aws:secretsmanager:us-east-1:123456789012:secret:canary-cert"
            }
        }
    });

    let response = client
        .request(Method::POST, "/v1/api-definitions")
        .json(&multi_upstream_api)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::CREATED);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_eq!(body["team"], "canary-team");
    assert!(body["clusters"].as_array().unwrap().len() >= 2); // Multiple clusters for targets
}

#[tokio::test]
async fn get_api_definition_success() {
    let client = platform_api_test_client().await;

    // First create an API definition
    let api_definition = json!({
        "team": "get-test-team",
        "domain": "get-test.flowplane.dev",
        "routes": [
            {
                "path_config": {
                    "match_type": "exact",
                    "pattern": "/healthz"
                },
                "upstream_config": {
                    "targets": [
                        {
                            "name": "health-service",
                            "endpoint": "health.svc.cluster.local:8080",
                            "weight": 100
                        }
                    ]
                },
                "timeout_seconds": 5
            }
        ],
        "listener_isolation": false
    });

    let create_response = client
        .request(Method::POST, "/v1/api-definitions")
        .json(&api_definition)
        .send()
        .await
        .expect("Create API request failed");

    let create_body: serde_json::Value = create_response.json().await.expect("Parse create response");
    let api_id = create_body["id"].as_str().expect("API ID");

    // Now retrieve it
    let response = client
        .request(Method::GET, &format!("/v1/api-definitions/{}", api_id))
        .send()
        .await
        .expect("GET request failed");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_eq!(body["id"], api_id);
    assert_eq!(body["team"], "get-test-team");
    assert_eq!(body["domain"], "get-test.flowplane.dev");
}

#[tokio::test]
async fn get_api_definition_not_found() {
    let client = platform_api_test_client().await;

    let non_existent_id = Uuid::new_v4();
    let response = client
        .request(Method::GET, &format!("/v1/api-definitions/{}", non_existent_id))
        .send()
        .await
        .expect("GET request failed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_json_schema(&body, &json!({
        "type": "object",
        "required": ["error", "code"],
        "properties": {
            "error": {"type": "string"},
            "code": {"type": "string"}
        }
    }));
}

#[tokio::test]
async fn put_api_definition_success() {
    let client = platform_api_test_client().await;

    // Create API definition first
    let api_definition = json!({
        "team": "update-team",
        "domain": "update.flowplane.dev",
        "routes": [
            {
                "path_config": {
                    "match_type": "prefix",
                    "pattern": "/api/"
                },
                "upstream_config": {
                    "targets": [
                        {
                            "name": "update-service",
                            "endpoint": "update.svc.cluster.local:8080",
                            "weight": 100
                        }
                    ]
                }
            }
        ],
        "listener_isolation": false
    });

    let create_response = client
        .request(Method::POST, "/v1/api-definitions")
        .json(&api_definition)
        .send()
        .await
        .expect("Create API request failed");

    let create_body: serde_json::Value = create_response.json().await.expect("Parse create response");
    let api_id = create_body["id"].as_str().expect("API ID");

    // Update the API definition
    let update_request = json!({
        "listener_isolation": true,
        "metadata": {
            "environment": "production",
            "team_contact": "update-team@example.com"
        }
    });

    let response = client
        .request(Method::PUT, &format!("/v1/api-definitions/{}", api_id))
        .json(&update_request)
        .send()
        .await
        .expect("PUT request failed");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_eq!(body["id"], api_id);
    assert!(body["version"].as_i64().unwrap() > create_body["version"].as_i64().unwrap());
}

#[tokio::test]
async fn delete_api_definition_success() {
    let client = platform_api_test_client().await;

    // Create API definition first
    let api_definition = json!({
        "team": "delete-team",
        "domain": "delete.flowplane.dev",
        "routes": [
            {
                "path_config": {
                    "match_type": "prefix",
                    "pattern": "/api/"
                },
                "upstream_config": {
                    "targets": [
                        {
                            "name": "delete-service",
                            "endpoint": "delete.svc.cluster.local:8080",
                            "weight": 100
                        }
                    ]
                }
            }
        ],
        "listener_isolation": false
    });

    let create_response = client
        .request(Method::POST, "/v1/api-definitions")
        .json(&api_definition)
        .send()
        .await
        .expect("Create API request failed");

    let create_body: serde_json::Value = create_response.json().await.expect("Parse create response");
    let api_id = create_body["id"].as_str().expect("API ID");

    // Delete the API definition
    let response = client
        .request(Method::DELETE, &format!("/v1/api-definitions/{}", api_id))
        .send()
        .await
        .expect("DELETE request failed");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify it's actually deleted
    let get_response = client
        .request(Method::GET, &format!("/v1/api-definitions/{}", api_id))
        .send()
        .await
        .expect("Verification GET request failed");

    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn post_api_definition_route_success() {
    let client = platform_api_test_client().await;

    // Create API definition first
    let api_definition = json!({
        "team": "route-team",
        "domain": "route.flowplane.dev",
        "routes": [
            {
                "path_config": {
                    "match_type": "prefix",
                    "pattern": "/api/"
                },
                "upstream_config": {
                    "targets": [
                        {
                            "name": "main-service",
                            "endpoint": "main.svc.cluster.local:8080",
                            "weight": 100
                        }
                    ]
                }
            }
        ],
        "listener_isolation": false
    });

    let create_response = client
        .request(Method::POST, "/v1/api-definitions")
        .json(&api_definition)
        .send()
        .await
        .expect("Create API request failed");

    let create_body: serde_json::Value = create_response.json().await.expect("Parse create response");
    let api_id = create_body["id"].as_str().expect("API ID");

    // Add a new route
    let route_request = json!({
        "route": {
            "path_config": {
                "match_type": "exact",
                "pattern": "/healthz"
            },
            "upstream_config": {
                "targets": [
                    {
                        "name": "health-service",
                        "endpoint": "health.svc.cluster.local:8080",
                        "weight": 100
                    }
                ]
            },
            "timeout_seconds": 5
        },
        "deployment_note": "Add health check endpoint"
    });

    let response = client
        .request(Method::POST, &format!("/v1/api-definitions/{}/routes", api_id))
        .json(&route_request)
        .send()
        .await
        .expect("Add route request failed");

    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_json_schema(&body, &json!({
        "type": "object",
        "required": ["api_id", "route_id", "revision", "bootstrap_uri"],
        "properties": {
            "api_id": {"type": "string", "format": "uuid"},
            "route_id": {"type": "string", "format": "uuid"},
            "revision": {"type": "integer"},
            "bootstrap_uri": {"type": "string", "format": "uri"}
        }
    }));

    assert_eq!(body["api_id"], api_id);
    assert!(body["revision"].as_i64().unwrap() > create_body["version"].as_i64().unwrap());
}

#[tokio::test]
async fn post_api_definition_route_collision() {
    let client = platform_api_test_client().await;

    // Create API definition with existing route
    let api_definition = json!({
        "team": "collision-team",
        "domain": "collision.flowplane.dev",
        "routes": [
            {
                "path_config": {
                    "match_type": "prefix",
                    "pattern": "/api/"
                },
                "upstream_config": {
                    "targets": [
                        {
                            "name": "existing-service",
                            "endpoint": "existing.svc.cluster.local:8080",
                            "weight": 100
                        }
                    ]
                }
            }
        ],
        "listener_isolation": false
    });

    let create_response = client
        .request(Method::POST, "/v1/api-definitions")
        .json(&api_definition)
        .send()
        .await
        .expect("Create API request failed");

    let create_body: serde_json::Value = create_response.json().await.expect("Parse create response");
    let api_id = create_body["id"].as_str().expect("API ID");

    // Attempt to add conflicting route
    let conflicting_route = json!({
        "route": {
            "path_config": {
                "match_type": "prefix",
                "pattern": "/api/"  // Conflicts with existing route
            },
            "upstream_config": {
                "targets": [
                    {
                        "name": "conflicting-service",
                        "endpoint": "conflicting.svc.cluster.local:8080",
                        "weight": 100
                    }
                ]
            }
        },
        "deployment_note": "This should fail due to collision"
    });

    let response = client
        .request(Method::POST, &format!("/v1/api-definitions/{}/routes", api_id))
        .json(&conflicting_route)
        .send()
        .await
        .expect("Add route request failed");

    assert_eq!(response.status(), StatusCode::CONFLICT);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_eq!(body["collision_type"], "path_conflict");
}

#[tokio::test]
async fn get_api_definitions_list_success() {
    let client = platform_api_test_client().await;

    // Create multiple API definitions for listing
    for i in 1..=3 {
        let api_definition = json!({
            "team": "list-team",
            "domain": format!("list-{}.flowplane.dev", i),
            "routes": [
                {
                    "path_config": {
                        "match_type": "prefix",
                        "pattern": "/api/"
                    },
                    "upstream_config": {
                        "targets": [
                            {
                                "name": format!("service-{}", i),
                                "endpoint": format!("service-{}.svc.cluster.local:8080", i),
                                "weight": 100
                            }
                        ]
                    }
                }
            ],
            "listener_isolation": false
        });

        client
            .request(Method::POST, "/v1/api-definitions")
            .json(&api_definition)
            .send()
            .await
            .expect("Create API request failed");
    }

    // List API definitions
    let response = client
        .request(Method::GET, "/v1/api-definitions?team=list-team&limit=10")
        .send()
        .await
        .expect("List request failed");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_json_schema(&body, &json!({
        "type": "object",
        "required": ["items", "total", "limit", "offset"],
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "team", "domain"]
                }
            },
            "total": {"type": "integer"},
            "limit": {"type": "integer"},
            "offset": {"type": "integer"}
        }
    }));

    let items = body["items"].as_array().expect("items array");
    assert!(items.len() >= 3);
    assert!(items.iter().all(|item| item["team"] == "list-team"));
}

#[tokio::test]
async fn get_bootstrap_yaml_success() {
    let client = platform_api_test_client().await;

    // Create API definition first
    let api_definition = json!({
        "team": "bootstrap-team",
        "domain": "bootstrap.flowplane.dev",
        "routes": [
            {
                "path_config": {
                    "match_type": "prefix",
                    "pattern": "/api/"
                },
                "upstream_config": {
                    "targets": [
                        {
                            "name": "bootstrap-service",
                            "endpoint": "bootstrap.svc.cluster.local:8080",
                            "weight": 100
                        }
                    ]
                }
            }
        ],
        "listener_isolation": false
    });

    let create_response = client
        .request(Method::POST, "/v1/api-definitions")
        .json(&api_definition)
        .send()
        .await
        .expect("Create API request failed");

    let create_body: serde_json::Value = create_response.json().await.expect("Parse create response");
    let api_id = create_body["id"].as_str().expect("API ID");

    // Download bootstrap
    let response = client
        .request(Method::GET, &format!("/v1/api-definitions/{}/bootstrap?format=yaml", api_id))
        .send()
        .await
        .expect("Bootstrap request failed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("content-type").unwrap(), "application/yaml");

    let bootstrap_content = response.text().await.expect("Read bootstrap content");
    assert!(bootstrap_content.contains("listeners:"));
    assert!(bootstrap_content.contains("clusters:"));
}

#[tokio::test]
async fn authorization_error_insufficient_permissions() {
    let client = platform_api_test_client().await;

    let api_definition = json!({
        "team": "unauthorized-team",  // User doesn't have access to this team
        "domain": "unauthorized.flowplane.dev",
        "routes": [
            {
                "path_config": {
                    "match_type": "prefix",
                    "pattern": "/api/"
                },
                "upstream_config": {
                    "targets": [
                        {
                            "name": "unauthorized-service",
                            "endpoint": "unauthorized.svc.cluster.local:8080",
                            "weight": 100
                        }
                    ]
                }
            }
        ],
        "listener_isolation": false
    });

    let response = client
        .request(Method::POST, "/v1/api-definitions")
        .json(&api_definition)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_json_schema(&body, &json!({
        "type": "object",
        "required": ["error", "required_scope"],
        "properties": {
            "error": {"type": "string"},
            "required_scope": {"type": "string"},
            "user_scopes": {"type": "array", "items": {"type": "string"}}
        }
    }));
}