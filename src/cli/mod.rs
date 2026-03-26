//! # Command Line Interface
//!
//! Provides CLI commands for database management, personal access token administration,
//! API definition management, native resource management via HTTP client, and the
//! `serve` subcommand that starts the control plane server.

pub mod auth;
pub mod client;
pub mod clusters;
pub mod compose;
pub mod compose_runner;
pub mod config;
pub mod config_cmd;
pub mod credentials;
pub mod expose;
pub mod filter;
pub mod import;
pub mod learn;
pub mod list;
pub mod listeners;
pub mod logs;
pub mod output;
pub mod routes;
pub mod status;
pub mod teams;

use std::sync::Arc;

use crate::config::DatabaseConfig;
use crate::storage::{create_pool, run_db_migrations, validate_migrations, MigrationInfo};
use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

#[derive(Parser)]
#[command(name = "flowplane")]
#[command(about = "Flowplane Envoy Control Plane")]
#[command(version = env!("CARGO_PKG_VERSION"))]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

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

    /// Team context for resource commands
    #[arg(long, global = true)]
    pub team: Option<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the Flowplane control plane server
    Serve {
        /// Run in dev mode (synthetic identity, no Zitadel)
        #[arg(long)]
        dev: bool,
    },

    /// Bootstrap a local dev environment (PostgreSQL + control plane via Docker/Podman)
    Init {
        /// Also start an Envoy sidecar proxy
        #[arg(long)]
        with_envoy: bool,
        /// Also start an httpbin test backend (available at localhost:8000)
        #[arg(long)]
        with_httpbin: bool,
    },

    /// Stop the local dev environment started by `flowplane init`
    Down {
        /// Also remove persistent volumes (deletes database data)
        #[arg(long)]
        volumes: bool,
    },

    /// Database management commands
    Database {
        #[command(subcommand)]
        command: DatabaseCommands,
    },

    /// Authentication commands (login, token, whoami, logout)
    Auth {
        #[command(subcommand)]
        command: auth::AuthCommands,
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

    /// Team management commands
    Team {
        #[command(subcommand)]
        command: teams::TeamCommands,
    },

    /// Expose a local service through the gateway
    Expose {
        /// Upstream URL to expose (e.g., http://localhost:3000)
        upstream: String,
        /// Name for the exposed service (auto-generated if omitted)
        #[arg(long)]
        name: Option<String>,
        /// Path prefix to route (can be specified multiple times)
        #[arg(long)]
        path: Option<Vec<String>>,
        /// Port override (auto-assigned if omitted)
        #[arg(long)]
        port: Option<u16>,
    },

    /// Remove an exposed service
    Unexpose {
        /// Name of the exposed service to remove
        name: String,
    },

    /// Import API definitions
    Import {
        #[command(subcommand)]
        command: import::ImportCommands,
    },

    /// Manage API learning sessions
    Learn {
        #[command(subcommand)]
        command: learn::LearnCommands,
    },

    /// Filter management commands (CRUD + attach/detach)
    Filter {
        #[command(subcommand)]
        command: filter::FilterCommands,
    },

    /// Show system status or lookup a specific listener
    Status {
        /// Listener name to look up (omit for system overview)
        name: Option<String>,
    },

    /// Run diagnostic health checks
    Doctor,

    /// List exposed services
    List,

    /// View local dev stack logs
    Logs {
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
    },
    // MCP is available via HTTP at /api/v1/mcp (no CLI command needed)
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
pub async fn run_cli() -> crate::Result<()> {
    let cli = Cli::parse();

    let command = match cli.command {
        Some(cmd) => cmd,
        None => {
            // No subcommand provided — print help and exit
            use clap::CommandFactory;
            Cli::command()
                .print_help()
                .map_err(|e| crate::Error::config(format!("Failed to print help: {e}")))?;
            println!(); // trailing newline after help
            return Ok(());
        }
    };

    match command {
        Commands::Serve { dev } => {
            run_server(dev).await?;
        }
        Commands::Init { with_envoy, with_httpbin } => {
            compose::handle_init(with_envoy, with_httpbin)
                .map_err(|e| crate::Error::config(format!("{e:#}")))?;
        }
        Commands::Down { volumes } => {
            compose::handle_down(volumes).map_err(|e| crate::Error::config(format!("{e:#}")))?;
        }
        Commands::Auth { command } => {
            initialise_logging(cli.verbose).map_err(|e| crate::Error::config(format!("{e:#}")))?;
            auth::handle_auth_command(command)
                .await
                .map_err(|e| crate::Error::config(format!("{e:#}")))?;
        }
        _ => {
            run_cli_commands(
                cli.database_url,
                cli.verbose,
                cli.token,
                cli.token_file,
                cli.base_url,
                cli.timeout,
                cli.team,
                command,
            )
            .await
            .map_err(|e| crate::Error::config(format!("{e:#}")))?;
        }
    }

    Ok(())
}

/// Handle non-serve CLI commands (database, auth, cluster, etc.)
#[allow(clippy::too_many_arguments)]
async fn run_cli_commands(
    database_url: Option<String>,
    verbose: bool,
    token: Option<String>,
    token_file: Option<std::path::PathBuf>,
    base_url: Option<String>,
    timeout: Option<u64>,
    team_flag: Option<String>,
    command: Commands,
) -> anyhow::Result<()> {
    initialise_logging(verbose)?;

    let mut database = DatabaseConfig::from_env();
    if let Some(url) = database_url {
        database.url = url;
    }

    match command {
        Commands::Serve { .. }
        | Commands::Init { .. }
        | Commands::Down { .. }
        | Commands::Auth { .. } => unreachable!(),
        Commands::Import { command } => {
            let client = create_http_client(token, token_file, base_url, timeout, verbose)?;
            let team = config::resolve_team(team_flag)?;
            import::handle_import_command(&client, &team, command).await?
        }
        Commands::Expose { upstream, name, path, port } => {
            let client = create_http_client(token, token_file, base_url.clone(), timeout, verbose)?;
            let team = config::resolve_team(team_flag)?;
            let resolved_base_url = config::resolve_base_url(base_url);
            expose::handle_expose_command(
                &client,
                &team,
                &upstream,
                name.as_deref(),
                path,
                port,
                &resolved_base_url,
            )
            .await?
        }
        Commands::Unexpose { name } => {
            let client = create_http_client(token, token_file, base_url, timeout, verbose)?;
            let team = config::resolve_team(team_flag)?;
            expose::handle_unexpose_command(&client, &team, &name).await?
        }
        Commands::Database { command } => handle_database_command(command, &database).await?,
        Commands::Cluster { command } => {
            let client = create_http_client(token, token_file, base_url, timeout, verbose)?;
            let team = config::resolve_team(team_flag)?;
            clusters::handle_cluster_command(command, &client, &team).await?
        }
        Commands::Listener { command } => {
            let client = create_http_client(token, token_file, base_url, timeout, verbose)?;
            let team = config::resolve_team(team_flag)?;
            listeners::handle_listener_command(command, &client, &team).await?
        }
        Commands::Route { command } => {
            let client = create_http_client(token, token_file, base_url, timeout, verbose)?;
            let team = config::resolve_team(team_flag)?;
            routes::handle_route_command(command, &client, &team).await?
        }
        Commands::Learn { command } => {
            let client = create_http_client(token, token_file, base_url, timeout, verbose)?;
            let team = config::resolve_team(team_flag)?;
            learn::handle_learn_command(command, &client, &team).await?
        }
        Commands::Config { command } => config_cmd::handle_config_command(command).await?,
        Commands::Team { command } => {
            let client = create_http_client(token, token_file, base_url, timeout, verbose)?;
            teams::handle_team_command(command, &client).await?
        }
        Commands::Filter { command } => {
            let client = create_http_client(token, token_file, base_url, timeout, verbose)?;
            let team = config::resolve_team(team_flag)?;
            filter::handle_filter_command(command, &client, &team).await?
        }
        Commands::Status { name } => {
            let client = create_http_client(token, token_file, base_url, timeout, verbose)?;
            let team = config::resolve_team(team_flag)?;
            status::handle_status_command(&client, &team, name.as_deref()).await?
        }
        Commands::Doctor => {
            let client = create_http_client(token, token_file, base_url.clone(), timeout, verbose)?;
            let resolved_base_url = config::resolve_base_url(base_url);
            status::handle_doctor_command(&client, &resolved_base_url).await?
        }
        Commands::List => {
            let client = create_http_client(token, token_file, base_url, timeout, verbose)?;
            let team = config::resolve_team(team_flag)?;
            list::handle_list_command(&client, &team).await?
        }
        Commands::Logs { follow } => {
            let resolved_base_url = config::resolve_base_url(base_url);
            logs::handle_logs_command(&resolved_base_url, follow).await?
        }
    }

    Ok(())
}

/// Start the Flowplane control plane server.
///
/// If `dev` is true, sets `FLOWPLANE_AUTH_MODE=dev` so that the server runs with
/// synthetic identity and no Zitadel dependency.
pub async fn run_server(dev: bool) -> crate::Result<()> {
    use crate::{
        api::start_api_server,
        auth::scope_registry::init_scope_registry,
        config::{ApiServerConfig, ObservabilityConfig, SimpleXdsConfig},
        domain::filter_schema::FilterSchemaRegistry,
        observability::init_observability,
        secrets::SecretBackendRegistry,
        services::{LearningSessionService, SchemaAggregator, WebhookService},
        storage::repositories::{
            AggregatedSchemaRepository, InferredSchemaRepository, LearningSessionRepository,
        },
        xds::{
            services::{
                access_log_service::FlowplaneAccessLogService,
                ext_proc_service::FlowplaneExtProcService,
            },
            start_database_xds_server_with_state, XdsState,
        },
        AuthMode, Config, APP_NAME, VERSION,
    };
    use tokio::signal;
    use tokio::try_join;
    use tracing::{error, info, warn};

    // If --dev flag is passed, set the env var before config is read
    if dev {
        std::env::set_var("FLOWPLANE_AUTH_MODE", "dev");
    }

    // Load .env file if it exists (optional — won't fail if missing)
    // This must happen before any config is read from environment
    if let Err(e) = dotenvy::dotenv() {
        // Only warn if the error is NOT "file not found"
        if !e.to_string().contains("not found") {
            eprintln!("Warning: Error loading .env file: {}", e);
        }
    }

    let observability_config = ObservabilityConfig::from_env();
    let (_health_checker, tracer_provider) = init_observability(&observability_config).await?;

    info!(app_name = APP_NAME, version = VERSION, "Starting Flowplane Envoy Control Plane");

    // Load configuration from environment variables
    let config = Config::from_env()?;

    // Guard C3: Refuse startup if dev mode with Zitadel configured
    if config.auth_mode == AuthMode::Dev
        && crate::auth::zitadel::ZitadelConfig::from_env().is_some()
    {
        return Err(crate::Error::config(
            "Cannot start in dev mode while Zitadel is configured. \
             Remove Zitadel environment variables or set FLOWPLANE_AUTH_MODE=prod.",
        ));
    }

    info!(
        xds_port = config.xds.port,
        xds_bind_address = %config.xds.bind_address,
        metrics_enabled = %observability_config.enable_metrics,
        tracing_enabled = %observability_config.enable_tracing,
        "Loaded configuration from environment"
    );

    // Initialize database configuration and pool
    let db_config = DatabaseConfig::from_env();
    info!(database = "postgresql", "Creating database connection pool");
    let pool = create_pool(&db_config).await?;

    // Mode-aware startup: dev skips Zitadel, prod uses full bootstrap
    if config.auth_mode == AuthMode::Dev {
        let _dev_user_id = crate::startup::seed_dev_resources(&pool).await?;
        info!("Dev mode: seeded dev resources, skipping Zitadel startup");
    } else {
        // Handle first-time startup: check Zitadel configuration
        crate::startup::handle_first_time_startup().await?;

        // Ensure platform org + team exist (idempotent, synchronous)
        use crate::api::handlers::bootstrap::{ensure_platform_resources, seed_superadmin};
        use crate::auth::zitadel_admin::ZitadelAdminClient;
        let has_owner = ensure_platform_resources(&pool).await?;

        // Spawn background superadmin seeding if no platform owner exists yet
        if !has_owner {
            if let Some(admin_client) = ZitadelAdminClient::from_env() {
                let pool_clone = pool.clone();
                tokio::spawn(async move {
                    seed_superadmin(pool_clone, admin_client).await;
                });
            }
        }
    }

    // Initialize scope registry for scope validation (code-only constants, no DB)
    init_scope_registry();
    info!("Scope registry initialized");

    // Check mTLS configuration status
    if crate::secrets::PkiConfig::is_mtls_enabled() {
        let pki_config = crate::secrets::PkiConfig::from_env()
            .expect("PKI config should be available when mTLS is enabled");
        info!(
            pki_mount = %pki_config.mount_path,
            pki_role = %pki_config.role_name,
            trust_domain = %pki_config.trust_domain,
            "mTLS enabled - proxies will be authenticated via client certificates"
        );
    } else {
        warn!(
            "mTLS disabled - FLOWPLANE_VAULT_PKI_MOUNT_PATH not configured. \
             Proxies will not be authenticated. This is insecure for production use."
        );
    }

    // Create shutdown signal handler
    let simple_xds_config: SimpleXdsConfig = config.xds.clone();
    let api_config: ApiServerConfig = config.api.clone();

    // Create Access Log Service for learning sessions and wire repository + processor
    let (access_log_service, log_rx) = FlowplaneAccessLogService::new();
    // Attach repository so ALS can increment sample counts for sessions
    let als_session_repo = LearningSessionRepository::new(pool.clone());
    let access_log_service = Arc::new(access_log_service.with_repository(als_session_repo));

    // Create External Processor Service for request/response body capture during learning sessions
    let (ext_proc_service, ext_proc_rx) = FlowplaneExtProcService::new();
    let ext_proc_service = Arc::new(ext_proc_service);

    // Create Webhook Service for learning session event notifications
    let (webhook_service, _webhook_rx) = WebhookService::new();
    let webhook_service = Arc::new(webhook_service);

    // Create Schema Aggregator for learning session completion
    let inferred_schema_repo = InferredSchemaRepository::new(pool.clone());
    let aggregated_schema_repo = AggregatedSchemaRepository::new(pool.clone());
    let schema_aggregator =
        Arc::new(SchemaAggregator::new(inferred_schema_repo, aggregated_schema_repo));

    // Create XdsState first (without LearningSessionService) to resolve circular dependency
    // We initialize with access log and ext proc services directly
    let mut state_struct = XdsState::with_database(simple_xds_config.clone(), pool.clone());
    state_struct.access_log_service = Some(access_log_service.clone());
    state_struct.ext_proc_service = Some(ext_proc_service.clone());

    // Initialize secret backend registry for external secrets (Vault, AWS, GCP)
    if let Some(encryption) = state_struct.encryption_service.clone() {
        let cache_ttl =
            std::env::var("FLOWPLANE_SECRET_CACHE_TTL_SECS").ok().and_then(|s| s.parse().ok());

        match SecretBackendRegistry::from_env(pool.clone(), Some(encryption), cache_ttl).await {
            Ok(registry) => {
                info!(
                    backends = ?registry.registered_backends(),
                    "Initialized secret backend registry"
                );
                state_struct.set_secret_backend_registry(registry);
            }
            Err(e) => {
                warn!(error = %e, "Failed to initialize secret backend registry - external secrets will not work");
            }
        }
    }

    // Initialize filter schema registry with custom schemas from disk
    // This enables support for custom filter types like WASM, Lua, etc.
    {
        let schema_dir = std::path::Path::new("filter-schemas");
        if schema_dir.exists() {
            match FilterSchemaRegistry::load_from_directory(schema_dir) {
                Ok(registry) => {
                    info!(
                        path = %schema_dir.display(),
                        schema_count = registry.len(),
                        "Loaded filter schemas from directory (including custom filters)"
                    );
                    state_struct.set_filter_schema_registry(registry);
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to load filter schemas from directory, using built-in only"
                    );
                }
            }
        } else {
            info!("No filter-schemas directory found, using built-in schemas only");
        }
    }

    // Load custom WASM filter schemas from the database and register them
    // This enables users to upload custom WASM filters and have them available as filter types
    {
        use crate::services::CustomWasmFilterService;
        if let Some(ref repo) = state_struct.custom_wasm_filter_repository {
            match repo.list_all().await {
                Ok(custom_filters) if !custom_filters.is_empty() => {
                    let registry = &mut state_struct.filter_schema_registry;
                    let mut registered_count = 0;
                    for custom_filter in &custom_filters {
                        let schema =
                            CustomWasmFilterService::generate_schema_definition(custom_filter);
                        if let Err(e) = registry.register_custom_schema(schema) {
                            warn!(
                                filter_name = %custom_filter.name,
                                error = %e,
                                "Failed to register custom WASM filter schema"
                            );
                        } else {
                            registered_count += 1;
                        }
                    }
                    if registered_count > 0 {
                        info!(
                            count = registered_count,
                            "Registered custom WASM filter schemas from database"
                        );
                    }
                }
                Ok(_) => {
                    info!("No custom WASM filters found in database");
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to load custom WASM filters from database"
                    );
                }
            }
        }
    }

    let state = Arc::new(state_struct);

    // Create LearningSessionService with XdsState (Weak reference)
    let learning_session_repo = LearningSessionRepository::new(pool.clone());
    let learning_session_service = LearningSessionService::new(learning_session_repo)
        .with_access_log_service(access_log_service.clone())
        .with_ext_proc_service(ext_proc_service.clone())
        .with_webhook_service(webhook_service.clone())
        .with_schema_aggregator(schema_aggregator.clone())
        .with_xds_state(state.clone());
    let learning_session_service = Arc::new(learning_session_service);

    // Update XdsState with LearningSessionService using safe interior mutability
    state.set_learning_session_service(learning_session_service.clone());

    // Start background Access Log Processor to handle inference + persistence
    // Wire ExtProc body channel for request/response body capture (Task 12.3)
    // IMPORTANT: Keep _processor_handle alive for the process lifetime so shutdown_tx
    // isn't dropped. Dropping it causes shutdown_rx.changed() to return Err immediately,
    // creating a CPU spin loop in workers and breaking cleanup/batcher tasks.
    let _processor_handle = {
        use crate::services::AccessLogProcessor;
        let db_pool = Some(pool.clone());
        let processor = AccessLogProcessor::new(log_rx, Some(ext_proc_rx), db_pool, None);
        processor.spawn_workers()
    };

    // Spawn background worker for learning session auto-completion
    let learning_session_service_bg = learning_session_service.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            match learning_session_service_bg.check_all_active_sessions().await {
                Ok(completed) if !completed.is_empty() => {
                    info!(
                        count = completed.len(),
                        sessions = ?completed,
                        "Background worker completed learning sessions"
                    );
                }
                Ok(_) => {} // No sessions completed
                Err(e) => {
                    error!(
                        error = %e,
                        "Background worker failed to check active sessions"
                    );
                }
            }
        }
    });

    // Sync existing active sessions with Access Log Service on startup
    if let Err(e) = learning_session_service.sync_active_sessions_with_access_log_service().await {
        error!(error = %e, "Failed to sync active sessions with Access Log Service on startup");
    }

    let xds_state = state.clone();
    let xds_task = async move {
        start_database_xds_server_with_state(xds_state, async {
            signal::ctrl_c().await.expect("Failed to install CTRL+C signal handler");
            info!("Shutdown signal received for xDS server");
        })
        .await
    };

    let api_state = state.clone();
    let api_task = async move { start_api_server(api_config, api_state).await };

    if let Err(e) = try_join!(xds_task, api_task) {
        error!("Control plane services terminated with error: {}", e);
        std::process::exit(1);
    }

    // Shutdown OpenTelemetry tracer provider to flush any pending spans
    // Must use spawn_blocking to avoid deadlock with tokio runtime
    if let Some(provider) = tracer_provider {
        info!("Flushing OpenTelemetry traces before shutdown");
        if let Err(e) = tokio::task::spawn_blocking(move || provider.shutdown()).await {
            error!("Error shutting down OpenTelemetry tracer provider: {}", e);
        } else {
            info!("OpenTelemetry tracer provider shutdown completed");
        }
    }

    info!("Control plane shutdown completed");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_no_args_shows_help() {
        // Verify that command is Optional (no subcommand = None)
        let result = Cli::try_parse_from(["flowplane"]);
        // With Option<Commands>, no subcommand should parse successfully
        assert!(result.is_ok());
        let cli = result.unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_cli_serve_subcommand() {
        let result = Cli::try_parse_from(["flowplane", "serve"]);
        assert!(result.is_ok());
        let cli = result.unwrap();
        assert!(matches!(cli.command, Some(Commands::Serve { dev: false })));
    }

    #[test]
    fn test_cli_serve_dev_flag() {
        let result = Cli::try_parse_from(["flowplane", "serve", "--dev"]);
        assert!(result.is_ok());
        let cli = result.unwrap();
        assert!(matches!(cli.command, Some(Commands::Serve { dev: true })));
    }

    #[test]
    fn test_cli_database_subcommand_still_works() {
        let result = Cli::try_parse_from(["flowplane", "database", "status"]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_cli_init_subcommand() {
        let result = Cli::try_parse_from(["flowplane", "init"]);
        assert!(result.is_ok());
        let cli = result.unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Init { with_envoy: false, with_httpbin: false })
        ));
    }

    #[test]
    fn test_cli_init_with_envoy() {
        let result = Cli::try_parse_from(["flowplane", "init", "--with-envoy"]);
        assert!(result.is_ok());
        let cli = result.unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Init { with_envoy: true, with_httpbin: false })
        ));
    }

    #[test]
    fn test_cli_init_with_httpbin() {
        let result = Cli::try_parse_from(["flowplane", "init", "--with-httpbin"]);
        assert!(result.is_ok());
        let cli = result.unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Init { with_envoy: false, with_httpbin: true })
        ));
    }

    #[test]
    fn test_cli_init_with_envoy_and_httpbin() {
        let result = Cli::try_parse_from(["flowplane", "init", "--with-envoy", "--with-httpbin"]);
        assert!(result.is_ok());
        let cli = result.unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Init { with_envoy: true, with_httpbin: true })
        ));
    }

    #[test]
    fn test_cli_down_subcommand() {
        let result = Cli::try_parse_from(["flowplane", "down"]);
        assert!(result.is_ok());
        let cli = result.unwrap();
        assert!(matches!(cli.command, Some(Commands::Down { volumes: false })));
    }

    #[test]
    fn test_cli_down_with_volumes() {
        let result = Cli::try_parse_from(["flowplane", "down", "--volumes"]);
        assert!(result.is_ok());
        let cli = result.unwrap();
        assert!(matches!(cli.command, Some(Commands::Down { volumes: true })));
    }
}
