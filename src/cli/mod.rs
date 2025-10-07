//! # Command Line Interface
//!
//! Provides CLI commands for database management, personal access token administration,
//! API definition management, and native resource management via HTTP client.

pub mod api_definition;
pub mod auth;
pub mod client;
pub mod clusters;
pub mod config;
pub mod config_cmd;
pub mod listeners;
pub mod output;
pub mod routes;

use crate::config::DatabaseConfig;
use crate::storage::{create_pool, run_db_migrations, validate_migrations, MigrationInfo};
use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

#[derive(Parser)]
#[command(name = "flowplane")]
#[command(about = "Flowplane Envoy Control Plane tooling")]
#[command(version = env!("CARGO_PKG_VERSION"))]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Database URL override
    #[arg(long)]
    pub database_url: Option<String>,

    /// Enable verbose logging
    #[arg(short, long)]
    pub verbose: bool,

    /// Personal access token for API authentication
    #[arg(long, global = true)]
    pub token: Option<String>,

    /// Path to file containing personal access token
    #[arg(long, global = true)]
    pub token_file: Option<std::path::PathBuf>,

    /// Base URL for the Flowplane API
    #[arg(long, global = true)]
    pub base_url: Option<String>,

    /// Request timeout in seconds
    #[arg(long, global = true)]
    pub timeout: Option<u64>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Database management commands
    Database {
        #[command(subcommand)]
        command: DatabaseCommands,
    },

    /// Personal access token administration commands
    Auth {
        #[command(subcommand)]
        command: auth::AuthCommands,
    },

    /// API definition management commands
    Api {
        #[command(subcommand)]
        command: api_definition::ApiCommands,
    },

    /// Cluster management commands
    Cluster {
        #[command(subcommand)]
        command: clusters::ClusterCommands,
    },

    /// Listener management commands
    Listener {
        #[command(subcommand)]
        command: listeners::ListenerCommands,
    },

    /// Route management commands
    Route {
        #[command(subcommand)]
        command: routes::RouteCommands,
    },

    /// Configuration management commands
    Config {
        #[command(subcommand)]
        command: config_cmd::ConfigCommands,
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
}

/// Run CLI commands
pub async fn run_cli() -> anyhow::Result<()> {
    let cli = Cli::parse();

    initialise_logging(cli.verbose)?;

    let mut database = DatabaseConfig::from_env();
    if let Some(url) = cli.database_url {
        database.url = url;
    }

    match cli.command {
        Commands::Database { command } => handle_database_command(command, &database).await?,
        Commands::Auth { command } => auth::handle_auth_command(command, &database).await?,
        Commands::Api { command } => {
            api_definition::handle_api_command(
                command,
                cli.token,
                cli.token_file,
                cli.base_url,
                cli.timeout,
                cli.verbose,
            )
            .await?
        }
        Commands::Cluster { command } => {
            let client = create_http_client(
                cli.token,
                cli.token_file,
                cli.base_url,
                cli.timeout,
                cli.verbose,
            )?;
            clusters::handle_cluster_command(command, &client).await?
        }
        Commands::Listener { command } => {
            let client = create_http_client(
                cli.token,
                cli.token_file,
                cli.base_url,
                cli.timeout,
                cli.verbose,
            )?;
            listeners::handle_listener_command(command, &client).await?
        }
        Commands::Route { command } => {
            let client = create_http_client(
                cli.token,
                cli.token_file,
                cli.base_url,
                cli.timeout,
                cli.verbose,
            )?;
            routes::handle_route_command(command, &client).await?
        }
        Commands::Config { command } => config_cmd::handle_config_command(command).await?,
    }

    Ok(())
}

/// Create HTTP client with resolved authentication
fn create_http_client(
    token: Option<String>,
    token_file: Option<std::path::PathBuf>,
    base_url: Option<String>,
    timeout: Option<u64>,
    verbose: bool,
) -> anyhow::Result<client::FlowplaneClient> {
    let token = config::resolve_token(token, token_file)?;
    let base_url = config::resolve_base_url(base_url);
    let timeout = config::resolve_timeout(timeout);

    let config = client::ClientConfig { base_url, token, timeout, verbose };

    client::FlowplaneClient::new(config)
}

fn initialise_logging(verbose: bool) -> anyhow::Result<()> {
    let default_level = if verbose { "debug" } else { "info" };
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", default_level);
    }

    if tracing::subscriber::set_global_default(
        FmtSubscriber::builder().with_env_filter(EnvFilter::from_default_env()).finish(),
    )
    .is_err()
    {
        // Subscriber already set elsewhere (e.g. integration tests); ignore.
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
                std::process::exit(1);
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
                std::process::exit(1);
            }
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
}

/// Truncate a string to a maximum length
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}
