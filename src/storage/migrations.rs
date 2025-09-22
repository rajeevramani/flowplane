//! # Database Migration Management
//!
//! This module handles database schema evolution using embedded SQL migrations.
//! Migrations are embedded in the binary for production deployment and executed automatically
//! on application startup when auto_migrate is enabled.

use crate::errors::{MagayaError, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};
use tracing::{error, info, warn};

/// Migration information structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationInfo {
    pub version: i64,
    pub description: String,
    pub installed_on: chrono::DateTime<chrono::Utc>,
    pub execution_time: i64,
    pub checksum: Vec<u8>,
}

/// Get migrations directory path
fn get_migrations_dir() -> std::path::PathBuf {
    // Try to find migrations directory relative to current working directory
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let migrations_dir = cwd.join("migrations");

    if migrations_dir.exists() {
        migrations_dir
    } else {
        // Fallback: try relative to executable location
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        exe_dir.join("migrations")
    }
}

/// Load migration files from filesystem
fn load_migrations() -> Result<Vec<(String, String)>> {
    let migrations_dir = get_migrations_dir();

    if !migrations_dir.exists() {
        return Err(MagayaError::validation(format!(
            "Migrations directory not found: {}",
            migrations_dir.display()
        )));
    }

    let mut migrations = Vec::new();
    let entries = std::fs::read_dir(&migrations_dir).map_err(|e| {
        MagayaError::validation(format!(
            "Failed to read migrations directory {}: {}",
            migrations_dir.display(),
            e
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            MagayaError::validation(format!("Failed to read migration file entry: {}", e))
        })?;

        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("sql") {
            let filename = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
                MagayaError::validation(format!("Invalid migration filename: {}", path.display()))
            })?;

            let content = std::fs::read_to_string(&path).map_err(|e| {
                MagayaError::validation(format!(
                    "Failed to read migration file {}: {}",
                    path.display(),
                    e
                ))
            })?;

            migrations.push((filename.to_string(), content));
        }
    }

    // Sort migrations by filename (which should include timestamp)
    migrations.sort_by(|a, b| a.0.cmp(&b.0));

    if migrations.is_empty() {
        return Err(MagayaError::validation(format!(
            "No migration files found in {}",
            migrations_dir.display()
        )));
    }

    info!(
        "Loaded {} migration files from {}",
        migrations.len(),
        migrations_dir.display()
    );
    Ok(migrations)
}

/// Run all pending database migrations
pub async fn run_migrations(pool: &Pool<Sqlite>) -> Result<()> {
    info!("Starting database migration process");

    // Create migration tracking table if it doesn't exist
    create_migration_table(pool).await?;

    // Load migration files from filesystem
    let migrations = load_migrations()?;

    // Get applied migrations
    let applied = get_applied_migration_versions(pool).await?;

    // Run pending migrations
    let mut migrations_run = 0;
    for (filename, sql) in &migrations {
        let version = extract_version_from_filename(filename)?;

        if applied.contains(&version) {
            info!(version = version, "Migration already applied: {}", filename);
            continue;
        }

        info!(version = version, "Running migration: {}", filename);
        let start_time = std::time::Instant::now();

        // Execute migration in a transaction
        let mut tx = pool.begin().await.map_err(|e| {
            MagayaError::database(e, "Failed to start migration transaction".to_string())
        })?;

        // Run the migration SQL
        sqlx::query(sql).execute(&mut *tx).await.map_err(|e| {
            error!(error = %e, migration = filename, "Migration failed");
            MagayaError::database(e, format!("Migration failed: {}", filename))
        })?;

        // Record migration
        let execution_time = start_time.elapsed().as_millis() as i64;
        let checksum = calculate_checksum(sql);
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

        sqlx::query(
            "INSERT INTO _magaya_migrations (version, description, checksum, execution_time, installed_on) VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(version)
        .bind(filename)
        .bind(&checksum)
        .bind(execution_time)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            error!(error = %e, migration = filename, "Failed to record migration");
            MagayaError::database(e, format!("Failed to record migration: {}", filename))
        })?;

        tx.commit().await.map_err(|e| {
            MagayaError::database(e, "Failed to commit migration transaction".to_string())
        })?;

        migrations_run += 1;
        info!(
            version = version,
            execution_time_ms = execution_time,
            "Migration completed: {}",
            filename
        );
    }

    if migrations_run > 0 {
        info!(count = migrations_run, "Database migrations completed");
    } else {
        info!("No pending migrations");
    }

    Ok(())
}

/// Create the migration tracking table
async fn create_migration_table(pool: &Pool<Sqlite>) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS _magaya_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            checksum BLOB NOT NULL,
            execution_time BIGINT NOT NULL,
            installed_on DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
    "#,
    )
    .execute(pool)
    .await
    .map_err(|e| {
        MagayaError::database(e, "Failed to create migration tracking table".to_string())
    })?;

    Ok(())
}

/// Get list of applied migration versions
async fn get_applied_migration_versions(pool: &Pool<Sqlite>) -> Result<Vec<i64>> {
    let rows = sqlx::query("SELECT version FROM _magaya_migrations ORDER BY version")
        .fetch_all(pool)
        .await;

    match rows {
        Ok(rows) => Ok(rows
            .into_iter()
            .map(|row| row.get::<i64, _>("version"))
            .collect()),
        Err(sqlx::Error::Database(db_err))
            if db_err
                .message()
                .contains("no such table: _magaya_migrations") =>
        {
            Ok(Vec::new())
        }
        Err(e) => Err(MagayaError::database(
            e,
            "Failed to get applied migrations".to_string(),
        )),
    }
}

/// Extract version number from migration filename
fn extract_version_from_filename(filename: &str) -> Result<i64> {
    let version_str = filename.split('_').next().ok_or_else(|| {
        MagayaError::validation(format!("Invalid migration filename: {}", filename))
    })?;

    version_str
        .parse::<i64>()
        .map_err(|_| MagayaError::validation(format!("Invalid version in filename: {}", filename)))
}

/// Calculate checksum for migration content
fn calculate_checksum(content: &str) -> Vec<u8> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish().to_le_bytes().to_vec()
}

/// Validate that all migrations are applied correctly
pub async fn validate_migrations(pool: &Pool<Sqlite>) -> Result<bool> {
    info!("Validating migration integrity");

    let migrations = load_migrations()?;
    let applied_versions = get_applied_migration_versions(pool).await?;
    let expected_versions: Vec<i64> = migrations
        .iter()
        .map(|(filename, _)| extract_version_from_filename(filename))
        .collect::<Result<Vec<_>>>()?;

    // Check that all expected migrations are applied
    for expected in &expected_versions {
        if !applied_versions.contains(expected) {
            warn!(version = expected, "Missing migration");
            return Ok(false);
        }
    }

    // Check for unexpected migrations
    for applied in &applied_versions {
        if !expected_versions.contains(applied) {
            warn!(version = applied, "Unexpected migration found");
            return Ok(false);
        }
    }

    info!("Migration validation successful");
    Ok(true)
}

/// Get the current migration version (highest applied)
pub async fn get_migration_version(pool: &Pool<Sqlite>) -> Result<i64> {
    let applied = get_applied_migration_versions(pool).await?;
    Ok(applied.into_iter().max().unwrap_or(0))
}

/// List all applied migrations
pub async fn list_applied_migrations(pool: &Pool<Sqlite>) -> Result<Vec<MigrationInfo>> {
    let rows = sqlx::query("SELECT version, description, checksum, execution_time, installed_on FROM _magaya_migrations ORDER BY version")
        .fetch_all(pool)
        .await;

    match rows {
        Ok(rows) => {
            let migrations = rows
                .into_iter()
                .map(|row| MigrationInfo {
                    version: row.get("version"),
                    description: row.get("description"),
                    installed_on: row.get("installed_on"),
                    execution_time: row.get("execution_time"),
                    checksum: row.get("checksum"),
                })
                .collect();
            Ok(migrations)
        }
        Err(sqlx::Error::Database(db_err))
            if db_err
                .message()
                .contains("no such table: _magaya_migrations") =>
        {
            Ok(Vec::new())
        }
        Err(e) => Err(MagayaError::database(
            e,
            "Failed to list applied migrations".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_migrations() {
        let pool = sqlx::sqlite::SqlitePool::connect("sqlite://:memory:")
            .await
            .unwrap();

        // Run migrations
        let result = run_migrations(&pool).await;
        assert!(result.is_ok());

        // Check that all migrations were applied
        let applied = get_applied_migration_versions(&pool).await.unwrap();
        let migrations = load_migrations().unwrap();
        assert_eq!(applied.len(), migrations.len());

        // Running again should be idempotent
        let result = run_migrations(&pool).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_migrations() {
        let pool = sqlx::sqlite::SqlitePool::connect("sqlite://:memory:")
            .await
            .unwrap();

        // Run migrations first
        run_migrations(&pool).await.unwrap();

        // Validation should pass
        let result = validate_migrations(&pool).await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_get_migration_version() {
        let pool = sqlx::sqlite::SqlitePool::connect("sqlite://:memory:")
            .await
            .unwrap();

        // Before migrations
        let version = get_migration_version(&pool).await.unwrap();
        assert_eq!(version, 0);

        // After migrations
        run_migrations(&pool).await.unwrap();
        let version = get_migration_version(&pool).await.unwrap();
        assert!(version > 0);
    }

    #[tokio::test]
    async fn test_list_applied_migrations() {
        let pool = sqlx::sqlite::SqlitePool::connect("sqlite://:memory:")
            .await
            .unwrap();

        // Before migrations
        let migrations = list_applied_migrations(&pool).await.unwrap();
        assert!(migrations.is_empty());

        // After migrations
        run_migrations(&pool).await.unwrap();
        let applied_migrations = list_applied_migrations(&pool).await.unwrap();
        let loaded_migrations = load_migrations().unwrap();
        assert_eq!(applied_migrations.len(), loaded_migrations.len());
    }

    #[test]
    fn test_extract_version_from_filename() {
        assert_eq!(
            extract_version_from_filename("20241201000001_create_clusters_table").unwrap(),
            20241201000001
        );
        assert!(extract_version_from_filename("invalid_filename").is_err());
    }

    #[test]
    fn test_calculate_checksum() {
        let content1 = "CREATE TABLE test (id INTEGER);";
        let content2 = "CREATE TABLE test (id INTEGER);";
        let content3 = "CREATE TABLE other (id INTEGER);";

        let checksum1 = calculate_checksum(content1);
        let checksum2 = calculate_checksum(content2);
        let checksum3 = calculate_checksum(content3);

        assert_eq!(checksum1, checksum2);
        assert_ne!(checksum1, checksum3);
    }
}
