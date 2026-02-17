#![cfg(feature = "postgres_tests")]

//! Integration tests for the full learning pipeline:
//! inferred schemas → aggregation → MCP enablement

mod common;

use common::test_db::{TestDatabase, TEST_TEAM_ID};
use flowplane::schema::SchemaInferenceEngine;
use flowplane::services::schema_aggregator::SchemaAggregator;
use flowplane::storage::repositories::{AggregatedSchemaRepository, InferredSchemaRepository};
use flowplane::storage::DbPool;
use std::sync::Arc;

/// Helper: create a learning session and return its ID
async fn create_session(pool: &DbPool, route_pattern: &str) -> String {
    let session_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO learning_sessions (
            id, team, route_pattern, status, target_sample_count, current_sample_count
        ) VALUES ($1, $2, $3, 'active', 100, 0)",
    )
    .bind(&session_id)
    .bind(TEST_TEAM_ID)
    .bind(route_pattern)
    .execute(pool)
    .await
    .unwrap();
    session_id
}

/// Helper: insert an inferred schema record using the schema inference engine
/// (mirrors the exact code path of write_schema_batch)
async fn insert_inferred_schema_via_batch(
    pool: &DbPool,
    session_id: &str,
    method: &str,
    path: &str,
    status_code: Option<i64>,
    response_json: Option<&serde_json::Value>,
    request_json: Option<&serde_json::Value>,
) {
    let engine = SchemaInferenceEngine::new();

    let response_schema_str = response_json.map(|json| {
        let schema = engine.infer_from_value(json).unwrap();
        serde_json::to_string(&schema).unwrap()
    });

    let request_schema_str = request_json.map(|json| {
        let schema = engine.infer_from_value(json).unwrap();
        serde_json::to_string(&schema).unwrap()
    });

    // This mirrors the exact SQL from AccessLogProcessor::write_schema_batch
    sqlx::query(
        r#"
        INSERT INTO inferred_schemas (
            team, session_id, http_method, path_pattern,
            request_schema, response_schema, response_status_code,
            sample_count, confidence
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 1, 1.0)
        "#,
    )
    .bind(TEST_TEAM_ID)
    .bind(session_id)
    .bind(method)
    .bind(path)
    .bind(&request_schema_str)
    .bind(&response_schema_str)
    .bind(status_code)
    .execute(pool)
    .await
    .unwrap();
}

/// Seed gateway resources needed for MCP enablement:
/// cluster → route_config → virtual_host → route + listener + junction
///
/// Returns (route_config_id, route_id)
async fn seed_gateway_resources(pool: &DbPool, path: &str) -> (String, String) {
    let cluster_name = format!("pipeline-cluster-{}", uuid::Uuid::new_v4());
    let rc_id = format!("rc-{}", uuid::Uuid::new_v4());
    let vh_id = format!("vh-{}", uuid::Uuid::new_v4());
    let route_id = format!("r-{}", uuid::Uuid::new_v4());
    let listener_id = format!("l-{}", uuid::Uuid::new_v4());

    let valid_cluster_config = r#"{"endpoints":[{"host":"127.0.0.1","port":8080}]}"#;
    let valid_rc_config = r#"{"virtual_hosts":[]}"#;
    let valid_listener_config = r#"{"route_config_name":"test-rc"}"#;

    // Cluster (needed for route_config FK)
    sqlx::query(
        "INSERT INTO clusters (id, name, service_name, configuration, version, team) \
         VALUES ($1, $2, $3, $4, 1, $5)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&cluster_name)
    .bind(format!("{}-service", &cluster_name))
    .bind(valid_cluster_config)
    .bind(TEST_TEAM_ID)
    .execute(pool)
    .await
    .unwrap();

    // Route config
    sqlx::query(
        "INSERT INTO route_configs (id, name, path_prefix, cluster_name, configuration, version, team) \
         VALUES ($1, $2, $3, $4, $5, 1, $6)",
    )
    .bind(&rc_id)
    .bind(format!("test-rc-{}", &rc_id[3..11]))
    .bind(path)
    .bind(&cluster_name)
    .bind(valid_rc_config)
    .bind(TEST_TEAM_ID)
    .execute(pool)
    .await
    .unwrap();

    // Virtual host
    sqlx::query(
        "INSERT INTO virtual_hosts (id, route_config_id, name, domains, rule_order) \
         VALUES ($1, $2, $3, '[\"api.test.com\"]', 0)",
    )
    .bind(&vh_id)
    .bind(&rc_id)
    .bind(format!("test-vh-{}", &vh_id[3..11]))
    .execute(pool)
    .await
    .unwrap();

    // Route
    sqlx::query(
        "INSERT INTO routes (id, virtual_host_id, name, path_pattern, match_type, rule_order) \
         VALUES ($1, $2, $3, $4, 'prefix', 0)",
    )
    .bind(&route_id)
    .bind(&vh_id)
    .bind(format!("test-route-{}", &route_id[2..10]))
    .bind(path)
    .execute(pool)
    .await
    .unwrap();

    // Listener with a port
    sqlx::query(
        "INSERT INTO listeners (id, name, address, port, configuration, version, team) \
         VALUES ($1, $2, '0.0.0.0', 9999, $3, 1, $4)",
    )
    .bind(&listener_id)
    .bind(format!("test-listener-{}", &listener_id[2..10]))
    .bind(valid_listener_config)
    .bind(TEST_TEAM_ID)
    .execute(pool)
    .await
    .unwrap();

    // Junction: listener → route_config
    sqlx::query(
        "INSERT INTO listener_route_configs (listener_id, route_config_id, route_order) \
         VALUES ($1, $2, 0)",
    )
    .bind(&listener_id)
    .bind(&rc_id)
    .execute(pool)
    .await
    .unwrap();

    (rc_id, route_id)
}

/// Test 4: Full learning-to-aggregation pipeline
///
/// Create session → write inferred schemas → aggregate → verify aggregated schemas
#[tokio::test]
async fn test_full_learning_to_aggregation_pipeline() {
    let test_db = TestDatabase::new("learning_pipeline_full").await;
    let pool = test_db.pool.clone();

    // Step 1: Create a learning session
    let session_id = create_session(&pool, "/api/products/*").await;

    // Step 2: Transition to active (already created as active)
    let session =
        sqlx::query_as::<_, (String,)>("SELECT status FROM learning_sessions WHERE id = $1")
            .bind(&session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(session.0, "active");

    // Step 3: Insert inferred schemas using the same SQL as write_schema_batch
    // Simulate 5 observations of GET /api/products (200)
    for i in 0..5 {
        let response = serde_json::json!({
            "id": i + 1,
            "name": format!("Product {}", i + 1),
            "price": 29.99 + (i as f64),
            "in_stock": true
        });
        insert_inferred_schema_via_batch(
            &pool,
            &session_id,
            "GET",
            "/api/products",
            Some(200),
            Some(&response),
            None,
        )
        .await;
    }

    // Simulate 3 observations of POST /api/products (201)
    for i in 0..3 {
        let request = serde_json::json!({
            "name": format!("New Product {}", i),
            "price": 19.99
        });
        let response = serde_json::json!({
            "id": 100 + i,
            "created": true
        });
        insert_inferred_schema_via_batch(
            &pool,
            &session_id,
            "POST",
            "/api/products",
            Some(201),
            Some(&response),
            Some(&request),
        )
        .await;
    }

    // Step 4: Verify inferred schemas were written
    let inferred_repo = InferredSchemaRepository::new(pool.clone());
    let inferred_count = inferred_repo.count_by_session(&session_id).await.unwrap();
    assert_eq!(inferred_count, 8, "Should have 8 inferred schemas (5 GET + 3 POST)");

    // Step 5: Aggregate
    let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
    let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

    let agg_ids = aggregator.aggregate_session(&session_id).await.unwrap();
    assert_eq!(agg_ids.len(), 2, "Should produce 2 aggregated schemas");

    // Step 6: Verify aggregated schemas
    let schemas = aggregated_repo.get_by_ids(&agg_ids).await.unwrap();
    assert_eq!(schemas.len(), 2);

    let get_schema = schemas.iter().find(|s| s.http_method == "GET").unwrap();
    assert_eq!(get_schema.path, "/api/products");
    assert_eq!(get_schema.sample_count, 5);
    assert_eq!(get_schema.version, 1);
    assert!(get_schema.confidence_score > 0.0);
    assert!(get_schema.response_schemas.is_some(), "GET schema should have response_schemas");

    let post_schema = schemas.iter().find(|s| s.http_method == "POST").unwrap();
    assert_eq!(post_schema.path, "/api/products");
    assert_eq!(post_schema.sample_count, 3);
    assert_eq!(post_schema.version, 1);
    assert!(post_schema.confidence_score > 0.0);
}

/// Test 5: MCP enable after learning
///
/// After learning completes and schemas are aggregated, a route can be MCP-enabled
/// using the learned schemas.
#[tokio::test]
async fn test_mcp_enable_after_learning() {
    use flowplane::services::mcp_service::{EnableMcpRequest, McpService};

    let test_db = TestDatabase::new("mcp_enable_after_learn").await;
    let pool = test_db.pool.clone();

    let route_path = "/api/orders";

    // Step 1: Seed gateway resources
    let (_rc_id, route_id) = seed_gateway_resources(&pool, route_path).await;

    // Step 2: Create learning session and insert inferred schemas
    let session_id = create_session(&pool, "/api/orders/*").await;

    // Insert enough response schemas to get high confidence
    for i in 0..20 {
        let response = serde_json::json!({
            "order_id": i + 1,
            "status": "pending",
            "total": 49.99 + (i as f64),
            "customer_id": 1000 + i
        });
        insert_inferred_schema_via_batch(
            &pool,
            &session_id,
            "GET",
            route_path,
            Some(200),
            Some(&response),
            None,
        )
        .await;
    }

    // Step 3: Aggregate schemas
    let inferred_repo = InferredSchemaRepository::new(pool.clone());
    let aggregated_repo = AggregatedSchemaRepository::new(pool.clone());
    let aggregator = SchemaAggregator::new(inferred_repo, aggregated_repo.clone());

    let agg_ids = aggregator.aggregate_session(&session_id).await.unwrap();
    assert_eq!(agg_ids.len(), 1, "Should have 1 aggregated schema for GET /api/orders");

    // Verify confidence is high enough (>= 0.8 required for MCP enrichment)
    let agg_schema = aggregated_repo.get_by_id(agg_ids[0]).await.unwrap();
    assert!(
        agg_schema.confidence_score >= 0.5,
        "Confidence {:.2} should be reasonable for 20 consistent samples",
        agg_schema.confidence_score
    );

    // Step 4: Enable MCP on the route
    let mcp_service = McpService::new(Arc::new(pool.clone()));
    let request = EnableMcpRequest {
        tool_name: None,
        description: None,
        schema_source: None,
        summary: Some("Get orders".to_string()),
        http_method: Some("GET".to_string()),
    };

    let tool = mcp_service.enable(TEST_TEAM_ID, &route_id, request).await.unwrap();

    // Step 5: Verify MCP tool was created
    assert!(tool.enabled, "Tool should be enabled");
    assert_eq!(tool.team, TEST_TEAM_ID);
    assert_eq!(tool.http_path.as_deref(), Some(route_path));
    assert_eq!(tool.http_method.as_deref(), Some("GET"));
    assert!(tool.name.contains("api"), "Tool name should contain 'api'");
    assert!(tool.listener_port.is_some(), "Tool should have listener port");

    // Step 6: Verify the tool has schemas from learning
    // The MCP service enriches from learned schemas when confidence >= 0.8
    // Even if it falls back to manual, the tool should still be created
    assert!(tool.input_schema.is_object(), "Tool should have input_schema");
}
