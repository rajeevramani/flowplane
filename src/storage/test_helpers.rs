//! Test database utilities for in-library tests.
//!
//! Provides PostgreSQL test database infrastructure using Testcontainers.
//! Each `TestDatabase` instance starts a fresh PostgreSQL container with
//! all migrations applied, providing full isolation between tests.
//!
//! This module is only available in test builds (`#[cfg(test)]`).

use crate::config::DatabaseConfig;
use crate::storage::{create_pool, DbPool};
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;

/// Predictable team IDs for seed data (UUIDs).
/// Tests can reference these IDs when working with team-scoped resources.
pub const TEST_TEAM_ID: &str = "00000000-0000-0000-0000-000000000001";
pub const TEAM_A_ID: &str = "00000000-0000-0000-0000-000000000002";
pub const TEAM_B_ID: &str = "00000000-0000-0000-0000-000000000003";
pub const PLATFORM_TEAM_ID: &str = "00000000-0000-0000-0000-000000000004";

/// Predictable organization ID for seed data.
/// All seeded teams belong to this organization.
pub const TEST_ORG_ID: &str = "00000000-0000-0000-0000-0000000000a1";

/// Stop and remove stale testcontainer PostgreSQL containers from previous runs.
///
/// Testcontainers-rs 0.26 has a known issue where `ContainerAsync::Drop` is async
/// but Rust's `Drop` is sync, so cleanup tasks get cancelled when the tokio runtime
/// shuts down. This function finds and removes any orphaned containers.
///
/// Runs at most once per test binary via `std::sync::Once`.
fn cleanup_stale_testcontainers() {
    use std::process::Command;

    static CLEANUP_ONCE: std::sync::Once = std::sync::Once::new();
    CLEANUP_ONCE.call_once(|| {
        let output = Command::new("docker")
            .args([
                "ps",
                "-q",
                "--filter",
                "label=org.testcontainers.managed-by=testcontainers",
                "--filter",
                "ancestor=postgres",
            ])
            .output();

        let container_ids = match output {
            Ok(out) if out.status.success() => {
                let ids = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if ids.is_empty() {
                    return;
                }
                ids
            }
            _ => return,
        };

        let ids: Vec<&str> = container_ids.lines().collect();
        if ids.is_empty() {
            return;
        }

        eprintln!(
            "[test_helpers] Cleaning up {} stale testcontainer(s) from previous runs",
            ids.len()
        );

        // Stop containers (best-effort)
        let mut stop_args = vec!["stop", "--time", "5"];
        stop_args.extend(&ids);
        let _ = Command::new("docker").args(&stop_args).output();

        // Remove containers (best-effort)
        let mut rm_args = vec!["rm", "-f"];
        rm_args.extend(&ids);
        let _ = Command::new("docker").args(&rm_args).output();
    });
}

/// A test database backed by a Testcontainers PostgreSQL instance.
///
/// The container is automatically stopped and removed when this struct is dropped.
/// Keep this struct alive for the duration of your test to maintain the database connection.
pub struct TestDatabase {
    pub pool: DbPool,
    _container: ContainerAsync<Postgres>,
}

impl TestDatabase {
    /// Create a new test database with all migrations applied and common seed data.
    ///
    /// Starts a fresh PostgreSQL container, connects to it, runs all migrations,
    /// and seeds standard test entities (teams, clusters) to satisfy FK constraints.
    /// The `prefix` parameter is used for logging/debugging purposes.
    pub async fn new(prefix: &str) -> Self {
        cleanup_stale_testcontainers();

        let container = Postgres::default().start().await.unwrap_or_else(|e| {
            panic!("Failed to start PostgreSQL container for {}: {}", prefix, e)
        });

        let host = container
            .get_host()
            .await
            .unwrap_or_else(|e| panic!("Failed to get container host for {}: {}", prefix, e));

        let port = container
            .get_host_port_ipv4(5432)
            .await
            .unwrap_or_else(|e| panic!("Failed to get container port for {}: {}", prefix, e));

        let url = format!("postgresql://postgres:postgres@{}:{}/postgres", host, port);

        let config = DatabaseConfig {
            url,
            auto_migrate: true,
            max_connections: 5,
            min_connections: 1,
            ..Default::default()
        };

        let pool = create_pool(&config)
            .await
            .unwrap_or_else(|e| panic!("Failed to create test pool for {}: {}", prefix, e));

        // Seed common test data to satisfy FK constraints
        seed_test_data(&pool).await;

        Self { pool, _container: container }
    }
}

/// Seed common test entities that many tests depend on for FK constraints.
///
/// PostgreSQL enforces FKs (unlike SQLite), so tests that insert dataplanes,
/// route_configs, etc. need the parent team/cluster rows to exist.
///
/// Teams are seeded with predictable UUIDs that tests can reference:
/// - test-team: 00000000-0000-0000-0000-000000000001
/// - team-a:    00000000-0000-0000-0000-000000000002
/// - team-b:    00000000-0000-0000-0000-000000000003
/// - platform:  00000000-0000-0000-0000-000000000004
async fn seed_test_data(pool: &DbPool) {
    // Create test organization first (teams require org_id NOT NULL)
    sqlx::query(
        "INSERT INTO organizations (id, name, display_name, status, created_at, updated_at) \
         VALUES ($1, 'test-org', 'Test Organization', 'active', NOW(), NOW()) \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(TEST_ORG_ID)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed test organization: {}", e));

    // Create common test teams with predictable UUIDs
    let teams = [
        ("test-team", "00000000-0000-0000-0000-000000000001"),
        ("team-a", "00000000-0000-0000-0000-000000000002"),
        ("team-b", "00000000-0000-0000-0000-000000000003"),
        ("platform", "00000000-0000-0000-0000-000000000004"),
    ];

    for (team_name, team_id) in &teams {
        sqlx::query(
            "INSERT INTO teams (id, name, display_name, org_id, status) \
             VALUES ($1, $2, $3, $4, 'active') \
             ON CONFLICT (org_id, name) DO NOTHING",
        )
        .bind(team_id)
        .bind(team_name)
        .bind(format!("Test Team {}", team_name))
        .bind(TEST_ORG_ID)
        .execute(pool)
        .await
        .unwrap_or_else(|e| panic!("Failed to seed team '{}': {}", team_name, e));
    }

    // Create common test clusters (needed for route_config FK on cluster_name).
    // Configuration must be a valid ClusterSpec JSON with at least one endpoint,
    // because xDS refresh parses and validates all stored cluster configs.
    let valid_cluster_config = r#"{"endpoints":[{"host":"127.0.0.1","port":8080}]}"#;
    for cluster_name in &["test-cluster", "cluster-a", "cluster-b"] {
        sqlx::query(
            "INSERT INTO clusters (id, name, service_name, configuration, version) \
             VALUES ($1, $2, $3, $4, 1) \
             ON CONFLICT (name) DO NOTHING",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(cluster_name)
        .bind(format!("{}-service", cluster_name))
        .bind(valid_cluster_config)
        .execute(pool)
        .await
        .unwrap_or_else(|e| panic!("Failed to seed cluster '{}': {}", cluster_name, e));
    }
}

/// Seed extended resources for reporting, topology, and trace tests.
///
/// Creates a full request flow for both team-a and team-b:
///   cluster → route_config → virtual_host → route → listener (via junction table)
///   + cluster_endpoints (healthy/unhealthy mix)
///   + 1 orphan cluster (team-a, no route_configs reference it)
///   + 1 orphan route_config (team-a, no listener bound)
pub async fn seed_reporting_data(pool: &DbPool) {
    let valid_cluster_config = r#"{"endpoints":[{"host":"10.0.1.5","port":8080}]}"#;
    let valid_listener_config = r#"{"route_config_name":"placeholder"}"#;
    let valid_rc_config = r#"{"virtual_hosts":[]}"#;

    // ====================================================================
    // Team-A: orders-svc stack
    // ====================================================================

    // Cluster: orders-svc (team-a)
    let orders_cluster_id = "c-orders-svc";
    sqlx::query(
        "INSERT INTO clusters (id, name, service_name, configuration, version, team) \
         VALUES ($1, 'orders-svc', 'orders-service', $2, 1, $3) \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(orders_cluster_id)
    .bind(valid_cluster_config)
    .bind(TEAM_A_ID)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed orders-svc cluster: {}", e));

    // Cluster endpoints for orders-svc: 1 healthy, 1 unhealthy
    sqlx::query(
        "INSERT INTO cluster_endpoints (id, cluster_id, address, port, health_status) \
         VALUES ($1, $2, '10.0.1.5', 8080, 'healthy') \
         ON CONFLICT (cluster_id, address, port) DO NOTHING",
    )
    .bind("ce-orders-healthy")
    .bind(orders_cluster_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed orders endpoint (healthy): {}", e));

    sqlx::query(
        "INSERT INTO cluster_endpoints (id, cluster_id, address, port, health_status) \
         VALUES ($1, $2, '10.0.1.6', 8080, 'unhealthy') \
         ON CONFLICT (cluster_id, address, port) DO NOTHING",
    )
    .bind("ce-orders-unhealthy")
    .bind(orders_cluster_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed orders endpoint (unhealthy): {}", e));

    // Route config: orders-routes (team-a, cluster=orders-svc)
    let orders_rc_id = "rc-orders-routes";
    sqlx::query(
        "INSERT INTO route_configs (id, name, path_prefix, cluster_name, configuration, version, team) \
         VALUES ($1, 'orders-routes', '/api/orders', 'orders-svc', $2, 1, $3) \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(orders_rc_id)
    .bind(valid_rc_config)
    .bind(TEAM_A_ID)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed orders-routes route config: {}", e));

    // Virtual host: orders-vhost (on orders-routes)
    let orders_vh_id = "vh-orders-vhost";
    sqlx::query(
        "INSERT INTO virtual_hosts (id, route_config_id, name, domains, rule_order) \
         VALUES ($1, $2, 'orders-vhost', '[\"*\"]', 0) \
         ON CONFLICT (route_config_id, name) DO NOTHING",
    )
    .bind(orders_vh_id)
    .bind(orders_rc_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed orders-vhost: {}", e));

    // Route: /api/orders (prefix match, on orders-vhost)
    sqlx::query(
        "INSERT INTO routes (id, virtual_host_id, name, path_pattern, match_type, rule_order) \
         VALUES ($1, $2, 'orders-route', '/api/orders', 'prefix', 0) \
         ON CONFLICT (virtual_host_id, name) DO NOTHING",
    )
    .bind("r-orders-route")
    .bind(orders_vh_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed orders route: {}", e));

    // Listener: http-8080 (team-a)
    let listener_8080_id = "l-http-8080";
    sqlx::query(
        "INSERT INTO listeners (id, name, address, port, configuration, version, team) \
         VALUES ($1, 'http-8080', '0.0.0.0', 8080, $2, 1, $3) \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(listener_8080_id)
    .bind(valid_listener_config)
    .bind(TEAM_A_ID)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed http-8080 listener: {}", e));

    // Junction: http-8080 → orders-routes
    sqlx::query(
        "INSERT INTO listener_route_configs (listener_id, route_config_id, route_order) \
         VALUES ($1, $2, 0) \
         ON CONFLICT (listener_id, route_config_id) DO NOTHING",
    )
    .bind(listener_8080_id)
    .bind(orders_rc_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed listener-route-config (8080→orders): {}", e));

    // ====================================================================
    // Team-B: payments-svc stack
    // ====================================================================

    // Cluster: payments-svc (team-b)
    let payments_cluster_id = "c-payments-svc";
    sqlx::query(
        "INSERT INTO clusters (id, name, service_name, configuration, version, team) \
         VALUES ($1, 'payments-svc', 'payments-service', $2, 1, $3) \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(payments_cluster_id)
    .bind(valid_cluster_config)
    .bind(TEAM_B_ID)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed payments-svc cluster: {}", e));

    // 1 healthy endpoint for payments-svc
    sqlx::query(
        "INSERT INTO cluster_endpoints (id, cluster_id, address, port, health_status) \
         VALUES ($1, $2, '10.0.2.5', 9090, 'healthy') \
         ON CONFLICT (cluster_id, address, port) DO NOTHING",
    )
    .bind("ce-payments-healthy")
    .bind(payments_cluster_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed payments endpoint: {}", e));

    // Route config: payments-routes (team-b, cluster=payments-svc)
    let payments_rc_id = "rc-payments-routes";
    sqlx::query(
        "INSERT INTO route_configs (id, name, path_prefix, cluster_name, configuration, version, team) \
         VALUES ($1, 'payments-routes', '/api/payments', 'payments-svc', $2, 1, $3) \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(payments_rc_id)
    .bind(valid_rc_config)
    .bind(TEAM_B_ID)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed payments-routes route config: {}", e));

    // Virtual host: payments-vhost (on payments-routes)
    let payments_vh_id = "vh-payments-vhost";
    sqlx::query(
        "INSERT INTO virtual_hosts (id, route_config_id, name, domains, rule_order) \
         VALUES ($1, $2, 'payments-vhost', '[\"*\"]', 0) \
         ON CONFLICT (route_config_id, name) DO NOTHING",
    )
    .bind(payments_vh_id)
    .bind(payments_rc_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed payments-vhost: {}", e));

    // Route: /api/payments (prefix match, on payments-vhost)
    sqlx::query(
        "INSERT INTO routes (id, virtual_host_id, name, path_pattern, match_type, rule_order) \
         VALUES ($1, $2, 'payments-route', '/api/payments', 'prefix', 0) \
         ON CONFLICT (virtual_host_id, name) DO NOTHING",
    )
    .bind("r-payments-route")
    .bind(payments_vh_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed payments route: {}", e));

    // Listener: http-9090 (team-b)
    let listener_9090_id = "l-http-9090";
    sqlx::query(
        "INSERT INTO listeners (id, name, address, port, configuration, version, team) \
         VALUES ($1, 'http-9090', '0.0.0.0', 9090, $2, 1, $3) \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(listener_9090_id)
    .bind(valid_listener_config)
    .bind(TEAM_B_ID)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed http-9090 listener: {}", e));

    // Junction: http-9090 → payments-routes
    sqlx::query(
        "INSERT INTO listener_route_configs (listener_id, route_config_id, route_order) \
         VALUES ($1, $2, 0) \
         ON CONFLICT (listener_id, route_config_id) DO NOTHING",
    )
    .bind(listener_9090_id)
    .bind(payments_rc_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed listener-route-config (9090→payments): {}", e));

    // ====================================================================
    // Orphans (team-a)
    // ====================================================================

    // Orphan cluster: no route_configs reference it
    sqlx::query(
        "INSERT INTO clusters (id, name, service_name, configuration, version, team) \
         VALUES ($1, 'orphan-cluster', 'orphan-service', $2, 1, $3) \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind("c-orphan-cluster")
    .bind(valid_cluster_config)
    .bind(TEAM_A_ID)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed orphan-cluster: {}", e));

    // Orphan route config: no listener bound to it
    // Note: references orders-svc which exists, so FK is satisfied
    sqlx::query(
        "INSERT INTO route_configs (id, name, path_prefix, cluster_name, configuration, version, team) \
         VALUES ($1, 'orphan-rc', '/api/orphan', 'orders-svc', $2, 1, $3) \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind("rc-orphan-rc")
    .bind(valid_rc_config)
    .bind(TEAM_A_ID)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to seed orphan-rc route config: {}", e));
}
