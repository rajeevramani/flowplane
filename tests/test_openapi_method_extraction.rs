use flowplane::platform_api::openapi_adapter::openapi_to_api_definition_spec;
use serde_json::json;

#[tokio::test]
async fn test_openapi_creates_separate_routes_per_http_method() {
    let openapi_json = json!({
        "openapi": "3.0.0",
        "info": {
            "title": "Test API",
            "version": "1.0.0"
        },
        "servers": [{
            "url": "https://api.example.com"
        }],
        "paths": {
            "/users": {
                "get": {
                    "summary": "List users",
                    "responses": {
                        "200": {"description": "Success"}
                    }
                },
                "post": {
                    "summary": "Create user",
                    "responses": {
                        "201": {"description": "Created"}
                    }
                }
            },
            "/users/{id}": {
                "get": {
                    "summary": "Get user",
                    "parameters": [{
                        "name": "id",
                        "in": "path",
                        "required": true,
                        "schema": {"type": "string"}
                    }],
                    "responses": {
                        "200": {"description": "Success"}
                    }
                },
                "put": {
                    "summary": "Update user",
                    "parameters": [{
                        "name": "id",
                        "in": "path",
                        "required": true,
                        "schema": {"type": "string"}
                    }],
                    "responses": {
                        "200": {"description": "Success"}
                    }
                },
                "delete": {
                    "summary": "Delete user",
                    "parameters": [{
                        "name": "id",
                        "in": "path",
                        "required": true,
                        "schema": {"type": "string"}
                    }],
                    "responses": {
                        "204": {"description": "Deleted"}
                    }
                }
            }
        }
    });

    let openapi: openapiv3::OpenAPI = serde_json::from_value(openapi_json).expect("parse openapi");

    let spec = openapi_to_api_definition_spec(openapi, "test-team".to_string(), false, None)
        .expect("convert openapi to spec");

    // Should create 5 routes total: GET /users, POST /users, GET /users/{id}, PUT /users/{id}, DELETE /users/{id}
    assert_eq!(spec.routes.len(), 5, "Should create one route per HTTP method");

    // Verify each route has a header matcher for the HTTP method
    for route in &spec.routes {
        assert!(
            route.headers.is_some(),
            "Route {:?} should have header matchers",
            route.match_value
        );

        let headers = route.headers.as_ref().unwrap();
        assert_eq!(headers.len(), 1, "Should have exactly one header matcher (for :method)");

        let method_header = &headers[0];
        assert_eq!(
            method_header.name, ":method",
            "Header matcher should be for :method pseudo-header"
        );
        assert!(
            method_header.value.is_some(),
            "Method header should have a value (GET, POST, etc.)"
        );
    }

    // Verify paths with parameters use template matching
    let param_routes: Vec<_> =
        spec.routes.iter().filter(|r| r.match_value.contains("{id}")).collect();

    assert_eq!(param_routes.len(), 3, "Should have 3 routes with path parameter");

    for route in param_routes {
        assert_eq!(
            route.match_type, "template",
            "Routes with path parameters should use 'template' match type"
        );
    }

    // Verify routes without parameters use prefix matching
    let prefix_routes: Vec<_> =
        spec.routes.iter().filter(|r| !r.match_value.contains("{")).collect();

    for route in prefix_routes {
        assert_eq!(
            route.match_type, "prefix",
            "Routes without parameters should use 'prefix' match type"
        );
    }

    // Print routes for debugging
    println!("\nGenerated routes:");
    for (i, route) in spec.routes.iter().enumerate() {
        println!(
            "  {}: {} {} (match_type: {}, headers: {:?})",
            i,
            route
                .headers
                .as_ref()
                .and_then(|h| h.first())
                .and_then(|h| h.value.as_ref())
                .unwrap_or(&"NONE".to_string()),
            route.match_value,
            route.match_type,
            route.headers
        );
    }
}
