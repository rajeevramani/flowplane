//! Test database utilities for integration tests.
//!
//! Provides file-based SQLite databases under `data/test/` for test isolation
//! and easier debugging of test failures.

#![allow(clippy::duplicate_mod)]

use flowplane::storage::{self, DbPool};
use sqlx::sqlite::SqlitePoolOptions;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

/// Counter for generating unique database names within a test run
static DB_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Get the test database directory path
fn test_db_dir() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(manifest_dir).join("data").join("test")
}

/// Generate a unique database filename for a test
fn unique_db_name(prefix: &str) -> String {
    let counter = DB_COUNTER.fetch_add(1, Ordering::SeqCst);
    let uuid_short = &Uuid::new_v4().to_string()[..8];
    format!("{}_{}_{}_{}.db", prefix, std::process::id(), counter, uuid_short)
}

/// A test database that automatically cleans up on drop.
pub struct TestDatabase {
    pub pool: DbPool,
    pub path: PathBuf,
    cleanup_on_drop: bool,
}

impl TestDatabase {
    /// Create a new test database with automatic cleanup and migrations applied.
    ///
    /// The database file is created under `data/test/` with a unique name.
    /// It will be automatically deleted when this struct is dropped.
    pub async fn new(prefix: &str) -> Self {
        Self::with_options(prefix, true, true).await
    }

    /// Create a new test database without running migrations.
    ///
    /// Use this for unit tests that need to set up their own minimal schema.
    /// The database file is created under `data/test/` with a unique name.
    pub async fn new_without_migrations(prefix: &str) -> Self {
        Self::with_options(prefix, true, false).await
    }

    /// Create a new test database with configurable cleanup behavior.
    ///
    /// Set `cleanup_on_drop` to `false` to preserve the database file after
    /// the test completes (useful for debugging test failures).
    pub async fn with_cleanup(prefix: &str, cleanup_on_drop: bool) -> Self {
        Self::with_options(prefix, cleanup_on_drop, true).await
    }

    /// Create a new test database with full control over options.
    ///
    /// - `cleanup_on_drop`: Whether to delete the database file when dropped
    /// - `run_migrations`: Whether to run schema migrations
    pub async fn with_options(prefix: &str, cleanup_on_drop: bool, run_migrations: bool) -> Self {
        let db_dir = test_db_dir();

        // Ensure the test directory exists
        std::fs::create_dir_all(&db_dir).expect("create test database directory");

        let db_name = unique_db_name(prefix);
        let path = db_dir.join(&db_name);
        let url = format!("sqlite://{}?mode=rwc", path.display());

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .expect("create test database pool");

        // Run migrations to set up schema if requested
        if run_migrations {
            storage::run_migrations(&pool).await.expect("run migrations for test database");
        }

        Self { pool, path, cleanup_on_drop }
    }

    /// Create a test database that persists after the test (for debugging).
    pub async fn persistent(prefix: &str) -> Self {
        Self::with_cleanup(prefix, false).await
    }

    /// Get a reference to the connection pool.
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Get the database file path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Disable cleanup on drop (useful for debugging).
    pub fn keep_on_drop(&mut self) {
        self.cleanup_on_drop = false;
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            // Best effort cleanup - don't panic in drop
            if let Err(e) = std::fs::remove_file(&self.path) {
                eprintln!("Warning: Failed to cleanup test database {:?}: {}", self.path, e);
            }
            // Also try to remove WAL and SHM files if they exist
            let wal_path = self.path.with_extension("db-wal");
            let shm_path = self.path.with_extension("db-shm");
            let _ = std::fs::remove_file(wal_path);
            let _ = std::fs::remove_file(shm_path);
        }
    }
}

/// Create a test database pool under `data/test/`.
///
/// This is a convenience function for tests that don't need the full
/// `TestDatabase` wrapper. Note that cleanup is NOT automatic with this
/// function - use `TestDatabase` for automatic cleanup.
pub async fn create_test_pool(prefix: &str) -> DbPool {
    let db = TestDatabase::new(prefix).await;
    // Leak the TestDatabase to prevent cleanup, returning just the pool
    let pool = db.pool.clone();
    std::mem::forget(db);
    pool
}

/// Clean up all test databases in the test directory.
///
/// This can be called at the start of a test run to ensure a clean slate.
pub fn cleanup_all_test_databases() {
    let db_dir = test_db_dir();
    if let Ok(entries) = std::fs::read_dir(&db_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "db") {
                let _ = std::fs::remove_file(&path);
                // Also remove WAL/SHM files
                let wal_path = path.with_extension("db-wal");
                let shm_path = path.with_extension("db-shm");
                let _ = std::fs::remove_file(wal_path);
                let _ = std::fs::remove_file(shm_path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_database_creates_file() {
        let db = TestDatabase::new("test_creates_file").await;
        assert!(db.path.exists(), "Database file should exist");

        // Verify we can query the database
        let result: (i64,) =
            sqlx::query_as("SELECT 1").fetch_one(&db.pool).await.expect("query should succeed");
        assert_eq!(result.0, 1);
    }

    #[tokio::test]
    async fn test_database_cleanup_on_drop() {
        let path = {
            let db = TestDatabase::new("test_cleanup").await;
            let path = db.path.clone();
            assert!(path.exists(), "Database file should exist before drop");
            path
        };
        // After drop, file should be removed
        assert!(!path.exists(), "Database file should be removed after drop");
    }

    #[tokio::test]
    async fn test_persistent_database() {
        let path = {
            let db = TestDatabase::persistent("test_persistent").await;
            db.path.clone()
        };
        // File should still exist after drop
        assert!(path.exists(), "Persistent database should remain after drop");
        // Clean up manually
        std::fs::remove_file(&path).ok();
    }
}
