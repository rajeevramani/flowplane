//! # Command Line Interface
//!
//! This module provides CLI commands for database management and application control.

use crate::config::{load_config, DatabaseConfig};
use crate::storage::{create_pool, run_db_migrations, validate_migrations, MigrationInfo};
use clap::{Parser, Subcommand};
use std::process;

#[derive(Parser)]
#[command(name = "magaya")]
#[command(about = "Magaya Envoy Control Plane")]
#[command(version = env!("CARGO_PKG_VERSION"))]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Configuration file path
    #[arg(short, long, default_value = "config.yml")]
    pub config: String,

    /// Database URL override
    #[arg(long)]
    pub database_url: Option<String>,

    /// Enable verbose logging
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the control plane server
    Serve {
        /// Port to bind to
        #[arg(short, long, default_value = "8080")]
        port: u16,

        /// Address to bind to
        #[arg(short, long, default_value = "127.0.0.1")]
        addr: String,
    },

    /// Database management commands
    Database {
        #[command(subcommand)]
        command: DatabaseCommands,
    },
}

#[derive(Subcommand)]
pub enum DatabaseCommands {
    /// Run pending migrations
    Migrate {
        /// Dry run - show what would be migrated
        #[arg(long)]
        dry_run: bool,
    },

    /// Show migration status
    Status,

    /// List all applied migrations
    List,

    /// Validate database schema
    Validate,

    /// Revert migrations to a specific version (development only)
    #[cfg(debug_assertions)]
    Revert {
        /// Target version to revert to
        version: i64,
    },
}

/// Run CLI commands
pub async fn run_cli() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    if cli.verbose {
        std::env::set_var("RUST_LOG", "debug");
    } else if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }

    crate::observability::logging::init_logging()?;

    // Load configuration
    let mut config = load_config(&cli.config)?;

    // Override database URL if provided
    if let Some(url) = cli.database_url {
        config.database.url = url;
    }

    match cli.command {
        Some(Commands::Serve { port, addr }) => {
            tracing::info!(
                addr = %addr,
                port = port,
                "Starting Magaya control plane server"
            );

            // Start the server (this would be implemented in your main server module)
            crate::run_server(config, &format!("{}:{}", addr, port)).await?;
        }

        Some(Commands::Database { command }) => {
            handle_database_command(command, &config.database).await?;
        }

        None => {
            // Default action - start the server
            tracing::info!("Starting Magaya control plane server with default configuration");
            crate::run_server(config, "127.0.0.1:8080").await?;
        }
    }

    Ok(())
}

/// Handle database management commands
async fn handle_database_command(
    command: DatabaseCommands,
    config: &DatabaseConfig,
) -> anyhow::Result<()> {
    let pool = create_pool(config).await?;

    match command {
        DatabaseCommands::Migrate { dry_run } => {
            if dry_run {
                println!("Dry run mode - showing pending migrations:");
                // In a full implementation, you'd show what migrations would be applied
                println!("This would apply all pending migrations from the migrations/ directory");
            } else {
                println!("Running database migrations...");
                run_db_migrations(&pool).await?;
                println!("Migrations completed successfully!");
            }
        }

        DatabaseCommands::Status => {
            let is_valid = validate_migrations(&pool).await?;
            if is_valid {
                println!("✅ Database schema is up to date");
            } else {
                println!("⚠️  Database schema has pending migrations");
                process::exit(1);
            }
        }

        DatabaseCommands::List => {
            let migrations = crate::storage::migrations::list_applied_migrations(&pool).await?;
            if migrations.is_empty() {
                println!("No migrations have been applied");
            } else {
                println!("Applied migrations:");
                print_migrations_table(&migrations);
            }
        }

        DatabaseCommands::Validate => {
            println!("Validating database schema...");
            let is_valid = validate_migrations(&pool).await?;
            if is_valid {
                println!("✅ Database schema validation passed");
            } else {
                println!("❌ Database schema validation failed");
                process::exit(1);
            }
        }

        #[cfg(debug_assertions)]
        DatabaseCommands::Revert { version } => {
            println!("⚠️  WARNING: Reverting migrations in development mode");
            println!("Target version: {}", version);

            use crate::storage::migrations::revert_migrations;
            revert_migrations(&pool, version).await?;
            println!("Migrations reverted to version {}", version);
        }
    }

    Ok(())
}

/// Print migrations in a formatted table
fn print_migrations_table(migrations: &[MigrationInfo]) {
    println!();
    println!("{:<15} {:<50} {:<25} {:<10}", "Version", "Description", "Applied On", "Time (ms)");
    println!("{}", "-".repeat(100));

    for migration in migrations {
        println!(
            "{:<15} {:<50} {:<25} {:<10}",
            migration.version,
            truncate_string(&migration.description, 48),
            migration.installed_on.format("%Y-%m-%d %H:%M:%S"),
            migration.execution_time
        );
    }
    println!();
}

/// Truncate string to fit in table column
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parsing() {
        let cli = Cli::try_parse_from(&["magaya", "database", "status"]).unwrap();

        match cli.command {
            Some(Commands::Database { command: DatabaseCommands::Status }) => {
                // Test passed
            }
            _ => panic!("Failed to parse database status command"),
        }
    }

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("short", 10), "short");
        assert_eq!(truncate_string("this is a very long string", 10), "this is...");
    }
}