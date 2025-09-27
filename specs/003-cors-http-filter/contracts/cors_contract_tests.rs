// Contract tests for CORS HTTP Filter API
// These tests validate the API contract and will initially fail until implementation is complete

use serde_json::json;
use hyper::{StatusCode, Method};
use crate::test_utils::{TestApiClient, assert_json_schema};

// Helper to create test client - will be implemented with actual API
async fn cors_test_client() -> TestApiClient {
    todo!("Initialize test API client with CORS filter support")
}

#[tokio::test]
async fn put_listener_cors_config_success() {
    let client = cors_test_client().await;

    let cors_config = json!({
        "allow_origins": [
            {
                "match_type": "exact",
                "pattern": "https://app.example.com",
                "case_sensitive": true
            }
        ],
        "allow_methods": ["GET", "POST", "PUT", "DELETE"],
        "allow_headers": ["content-type", "authorization"],
        "expose_headers": ["x-custom-header"],
        "allow_credentials": false,
        "max_age": 86400
    });

    let response = client
        .request(Method::PUT, "/listeners/test-listener/filters/cors")
        .json(&cors_config)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_json_schema(&body, &json!({
        "type": "object",
        "required": ["message", "filter_name", "config_checksum"],
        "properties": {
            "message": {"type": "string"},
            "filter_name": {"type": "string"},
            "config_checksum": {"type": "string"}
        }
    }));
    assert_eq!(body["filter_name"], "envoy.filters.http.cors");
}

#[tokio::test]
async fn put_listener_cors_config_validation_error() {
    let client = cors_test_client().await;

    // Invalid config: empty allow_origins
    let invalid_config = json!({
        "allow_origins": [],
        "allow_methods": ["GET"],
        "allow_credentials": false
    });

    let response = client
        .request(Method::PUT, "/listeners/test-listener/filters/cors")
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

    // Check that validation error mentions allow_origins
    let validation_errors = body["validation_errors"].as_array().expect("validation_errors array");
    assert!(validation_errors.iter().any(|error|
        error["field"].as_str() == Some("allow_origins")
    ));
}

#[tokio::test]
async fn put_listener_cors_config_invalid_max_age() {
    let client = cors_test_client().await;

    // Invalid config: max_age exceeds protobuf Duration limit
    let invalid_config = json!({
        "allow_origins": [
            {
                "match_type": "exact",
                "pattern": "https://example.com",
                "case_sensitive": true
            }
        ],
        "allow_methods": ["GET"],
        "max_age": 999999999999u64  // Exceeds 315,576,000,000
    });

    let response = client
        .request(Method::PUT, "/listeners/test-listener/filters/cors")
        .json(&invalid_config)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    let validation_errors = body["validation_errors"].as_array().expect("validation_errors array");
    assert!(validation_errors.iter().any(|error|
        error["field"].as_str() == Some("max_age")
    ));
}

#[tokio::test]
async fn get_listener_cors_config_success() {
    let client = cors_test_client().await;

    // First set a configuration
    let cors_config = json!({
        "allow_origins": [
            {
                "match_type": "exact",
                "pattern": "https://app.example.com",
                "case_sensitive": true
            }
        ],
        "allow_methods": ["GET", "POST"],
        "allow_credentials": false
    });

    client
        .request(Method::PUT, "/listeners/test-listener/filters/cors")
        .json(&cors_config)
        .send()
        .await
        .expect("Setup PUT request failed");

    // Now retrieve it
    let response = client
        .request(Method::GET, "/listeners/test-listener/filters/cors")
        .send()
        .await
        .expect("GET request failed");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_eq!(body["allow_methods"], json!(["GET", "POST"]));
    assert_eq!(body["allow_credentials"], false);
}

#[tokio::test]
async fn get_listener_cors_config_not_found() {
    let client = cors_test_client().await;

    let response = client
        .request(Method::GET, "/listeners/nonexistent-listener/filters/cors")
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
async fn delete_listener_cors_config_success() {
    let client = cors_test_client().await;

    // First set a configuration
    let cors_config = json!({
        "allow_origins": [
            {
                "match_type": "exact",
                "pattern": "https://app.example.com",
                "case_sensitive": true
            }
        ],
        "allow_methods": ["GET"]
    });

    client
        .request(Method::PUT, "/listeners/test-listener/filters/cors")
        .json(&cors_config)
        .send()
        .await
        .expect("Setup PUT request failed");

    // Now delete it
    let response = client
        .request(Method::DELETE, "/listeners/test-listener/filters/cors")
        .send()
        .await
        .expect("DELETE request failed");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify it's actually deleted
    let get_response = client
        .request(Method::GET, "/listeners/test-listener/filters/cors")
        .send()
        .await
        .expect("Verification GET request failed");

    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn put_route_cors_config_success() {
    let client = cors_test_client().await;

    let route_cors_config = json!({
        "enabled": true,
        "inherit_from_listener": false,
        "cors_policy": {
            "allow_origins": [
                {
                    "match_type": "exact",
                    "pattern": "https://admin.example.com",
                    "case_sensitive": true
                }
            ],
            "allow_methods": ["GET", "POST", "DELETE"],
            "allow_headers": ["content-type", "authorization", "x-admin-token"],
            "allow_credentials": true,
            "max_age": 600
        }
    });

    let response = client
        .request(Method::PUT, "/routes/admin-route/filters/cors")
        .json(&route_cors_config)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_eq!(body["filter_name"], "envoy.filters.http.cors");
}

#[tokio::test]
async fn put_route_cors_config_disabled() {
    let client = cors_test_client().await;

    let disabled_config = json!({
        "enabled": false,
        "inherit_from_listener": false
    });

    let response = client
        .request(Method::PUT, "/routes/public-route/filters/cors")
        .json(&disabled_config)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn put_route_cors_config_validation_error() {
    let client = cors_test_client().await;

    // Invalid: enabled but no policy and not inheriting
    let invalid_config = json!({
        "enabled": true,
        "inherit_from_listener": false
        // Missing cors_policy
    });

    let response = client
        .request(Method::PUT, "/routes/test-route/filters/cors")
        .json(&invalid_config)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_route_cors_config_success() {
    let client = cors_test_client().await;

    // Setup route configuration
    let route_config = json!({
        "enabled": true,
        "inherit_from_listener": true
    });

    client
        .request(Method::PUT, "/routes/test-route/filters/cors")
        .json(&route_config)
        .send()
        .await
        .expect("Setup PUT request failed");

    // Retrieve configuration
    let response = client
        .request(Method::GET, "/routes/test-route/filters/cors")
        .send()
        .await
        .expect("GET request failed");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["inherit_from_listener"], true);
}

#[tokio::test]
async fn delete_route_cors_config_success() {
    let client = cors_test_client().await;

    // Setup route configuration
    let route_config = json!({
        "enabled": true,
        "inherit_from_listener": false,
        "cors_policy": {
            "allow_origins": [
                {
                    "match_type": "exact",
                    "pattern": "https://test.example.com",
                    "case_sensitive": true
                }
            ],
            "allow_methods": ["GET"]
        }
    });

    client
        .request(Method::PUT, "/routes/test-route/filters/cors")
        .json(&route_config)
        .send()
        .await
        .expect("Setup PUT request failed");

    // Delete configuration
    let response = client
        .request(Method::DELETE, "/routes/test-route/filters/cors")
        .send()
        .await
        .expect("DELETE request failed");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn cors_config_with_regex_pattern_success() {
    let client = cors_test_client().await;

    let regex_config = json!({
        "allow_origins": [
            {
                "match_type": "safe_regex",
                "pattern": "https://[a-z0-9-]+\\.example\\.com",
                "case_sensitive": true
            }
        ],
        "allow_methods": ["GET", "POST"],
        "allow_credentials": false
    });

    let response = client
        .request(Method::PUT, "/listeners/test-listener/filters/cors")
        .json(&regex_config)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn cors_config_with_invalid_regex_pattern() {
    let client = cors_test_client().await;

    let invalid_regex_config = json!({
        "allow_origins": [
            {
                "match_type": "safe_regex",
                "pattern": "[invalid-regex-*",  // Invalid regex syntax
                "case_sensitive": true
            }
        ],
        "allow_methods": ["GET"]
    });

    let response = client
        .request(Method::PUT, "/listeners/test-listener/filters/cors")
        .json(&invalid_regex_config)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    let validation_errors = body["validation_errors"].as_array().expect("validation_errors array");
    assert!(validation_errors.iter().any(|error|
        error["field"].as_str().unwrap_or("").contains("allow_origins") &&
        error["message"].as_str().unwrap_or("").contains("regex")
    ));
}

#[tokio::test]
async fn cors_config_wildcard_with_credentials_rejected() {
    let client = cors_test_client().await;

    // Security violation: wildcard origin with credentials
    let unsafe_config = json!({
        "allow_origins": [
            {
                "match_type": "exact",
                "pattern": "*",
                "case_sensitive": true
            }
        ],
        "allow_methods": ["GET", "POST"],
        "allow_credentials": true  // This should be rejected with wildcard
    });

    let response = client
        .request(Method::PUT, "/listeners/test-listener/filters/cors")
        .json(&unsafe_config)
        .send()
        .await
        .expect("API request failed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body: serde_json::Value = response.json().await.expect("Parse response JSON");
    let validation_errors = body["validation_errors"].as_array().expect("validation_errors array");
    assert!(validation_errors.iter().any(|error|
        error["message"].as_str().unwrap_or("").to_lowercase().contains("wildcard") &&
        error["message"].as_str().unwrap_or("").to_lowercase().contains("credentials")
    ));
}