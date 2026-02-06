//! Test database utilities for integration tests.
//!
//! Provides PostgreSQL test database infrastructure using Testcontainers.
//! Each `TestDatabase` instance starts a fresh PostgreSQL container with
//! all migrations applied, providing full isolation between tests.

#![allow(clippy::duplicate_mod)]
#![allow(dead_code)]

use flowplane::config::DatabaseConfig;
use flowplane::storage::{create_pool, DbPool};
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;

/// A test database backed by a Testcontainers PostgreSQL instance.
///
/// The container is automatically stopped and removed when this struct is dropped.
/// Keep this struct alive for the duration of your test to maintain the database connection.
pub struct TestDatabase {
    pub pool: DbPool,
    _container: ContainerAsync<Postgres>,
}

impl TestDatabase {
    /// Create a new test database with all migrations applied.
    ///
    /// Starts a fresh PostgreSQL container, connects to it, and runs all migrations.
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

        Self { pool, _container: container }
    }

    /// Get a reference to the connection pool.
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }
}

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
        // Find containers managed by testcontainers with postgres image
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

        eprintln!("[test_db] Cleaning up {} stale testcontainer(s) from previous runs", ids.len());

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

/// Clean up all test databases.
///
/// Stops and removes stale testcontainer PostgreSQL containers from previous runs.
pub fn cleanup_all_test_databases() {
    cleanup_stale_testcontainers();
}
