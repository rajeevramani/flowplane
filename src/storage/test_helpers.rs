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
async fn seed_test_data(pool: &DbPool) {
    // Create common test teams with predictable UUIDs
    let teams = [
        ("test-team", "00000000-0000-0000-0000-000000000001"),
        ("team-a", "00000000-0000-0000-0000-000000000002"),
        ("team-b", "00000000-0000-0000-0000-000000000003"),
    ];

    for (team_name, team_id) in &teams {
        sqlx::query(
            "INSERT INTO teams (id, name, display_name, status) \
             VALUES ($1, $2, $3, 'active') \
             ON CONFLICT (name) DO NOTHING",
        )
        .bind(team_id)
        .bind(team_name)
        .bind(format!("Test Team {}", team_name))
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
