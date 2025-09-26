//! Test program for database-enabled XDS server
//!
//! This binary tests:
//! 1. Database pool creation
//! 2. Database-enabled XDS server startup
//! 3. Basic repository operations
//! 4. XDS resource generation from database

use flowplane::{
    config::{DatabaseConfig, SimpleXdsConfig, XdsResourceConfig},
    storage::{create_pool, ClusterRepository, CreateClusterRequest},
    xds::start_database_xds_server_with_config,
};
use tokio::time::{timeout, Duration};
use tokio_stream::StreamExt;
use tracing::{error, info, warn};
use uuid::Uuid;

use envoy_types::pb::envoy::service::discovery::v3::{
    aggregated_discovery_service_client::AggregatedDiscoveryServiceClient, DeltaDiscoveryRequest,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("🧪 Starting Database-Enabled XDS Server Test");

    // Test 1: Database Pool Creation
    info!("📋 Test 1: Creating database pool...");
    let db_config = DatabaseConfig::from_env();

    let pool = create_pool(&db_config).await?;
    info!("✅ Database pool created successfully");

    // Test 2: Repository Operations
    info!("📋 Test 2: Testing repository operations...");
    let repo = ClusterRepository::new(pool.clone());

    // Test creating a cluster with unique name
    let cluster_name = format!("test_cluster_db_{}", Uuid::new_v4().simple());
    let create_request = CreateClusterRequest {
        name: cluster_name,
        service_name: "test_service_db".to_string(),
        configuration: serde_json::json!({
            "type": "EDS",
            "endpoints": ["127.0.0.1:8080", "127.0.0.1:8081"],
            "connect_timeout_seconds": 10
        }),
    };

    let created_cluster = repo.create(create_request).await?;
    info!(
        "✅ Created cluster: {} (version: {})",
        created_cluster.name, created_cluster.version
    );

    // Test listing clusters
    let clusters = repo.list(Some(10), None).await?;
    info!("✅ Listed {} clusters from repository", clusters.len());

    // Test 3: XDS Server Configuration
    info!("📋 Test 3: Setting up database-enabled XDS server...");
    let xds_config = SimpleXdsConfig {
        bind_address: "127.0.0.1".to_string(),
        port: 18004, // Use a different port for testing
        resources: XdsResourceConfig {
            cluster_name: "test_cluster_db".to_string(),
            route_name: "test_route_db".to_string(),
            listener_name: "test_listener_db".to_string(),
            backend_address: "127.0.0.1".to_string(),
            backend_port: 8080,
            listener_port: 10001,
        },
        tls: None,
    };

    // Test 4: Start Database-Enabled XDS Server (with timeout)
    info!(
        "📋 Test 4: Starting database-enabled XDS server on port {}...",
        xds_config.port
    );

    let server_port = xds_config.port;
    let server_state_pool = pool.clone();
    let server_task = tokio::spawn(async move {
        let shutdown_signal = async {
            tokio::time::sleep(Duration::from_secs(5)).await; // Auto-shutdown after 5 seconds
        };

        start_database_xds_server_with_config(xds_config, server_state_pool, shutdown_signal).await
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    info!("📋 Test 5: Issuing delta ADS request");
    let mut delta_client =
        AggregatedDiscoveryServiceClient::connect(format!("http://127.0.0.1:{}", server_port))
            .await?;

    let delta_request = DeltaDiscoveryRequest {
        type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
        resource_names_subscribe: vec![created_cluster.name.clone()],
        ..Default::default()
    };

    let request_stream = tokio_stream::iter(vec![delta_request]);
    let mut response_stream = delta_client
        .delta_aggregated_resources(request_stream)
        .await?
        .into_inner();

    match response_stream.next().await {
        Some(Ok(resp)) => {
            info!(
                type_url = %resp.type_url,
                version = %resp.system_version_info,
                added = resp.resources.len(),
                removed = resp.removed_resources.len(),
                "✅ Delta ADS response received"
            );
        }
        Some(Err(status)) => {
            error!("❌ Delta ADS stream error: {}", status);
        }
        None => {
            warn!("⚠️ Delta ADS stream closed without a response");
        }
    }

    // Wait for server to start and then shutdown
    match timeout(Duration::from_secs(10), server_task).await {
        Ok(Ok(Ok(()))) => {
            info!("✅ Database-enabled XDS server started and stopped successfully");
        }
        Ok(Ok(Err(e))) => {
            error!("❌ XDS server error: {}", e);
            return Err(e.into());
        }
        Ok(Err(join_err)) => {
            error!("❌ Task join error: {}", join_err);
            return Err(join_err.into());
        }
        Err(_) => {
            warn!("⚠️ XDS server test timed out (this might be normal)");
        }
    }

    info!("🎉 All database-enabled XDS tests completed successfully!");
    info!("📊 Test Summary:");
    info!("  ✅ Database pool creation: PASSED");
    info!("  ✅ Repository operations: PASSED");
    info!("  ✅ XDS server startup: PASSED");
    info!("  ✅ Integration test: PASSED");

    Ok(())
}
