use std::sync::Arc;

use anyhow::{anyhow, bail};
use chrono::{DateTime, Duration, Utc};
use clap::{ArgAction, Args, Subcommand};
use owo_colors::OwoColorize;
use validator::{Validate, ValidationErrors};

use crate::auth::{
    models::{PersonalAccessToken, TokenStatus},
    token_service::{TokenSecretResponse, TokenService},
    validation::CreateTokenRequest as TokenServiceRequest,
};
use crate::cli::client::{CreateTokenRequest as ApiCreateTokenRequest, FlowplaneClient};
use crate::config::DatabaseConfig;
use crate::storage::{create_pool, repository::AuditLogRepository};

#[derive(Subcommand, Debug)]
pub enum AuthCommands {
    /// Bootstrap initialization - generate setup token for first-time setup
    Bootstrap(BootstrapArgs),
    /// Create a new personal access token
    CreateToken(CreateTokenArgs),
    /// List personal access tokens
    ListTokens(ListTokensArgs),
    /// Revoke (disable) an existing token
    RevokeToken(TokenRefArgs),
    /// Rotate a token's secret value
    RotateToken(TokenRefArgs),
}

#[derive(Args, Debug)]
pub struct BootstrapArgs {
    /// Email address for the system administrator
    #[arg(long)]
    pub email: Option<String>,

    /// Password for the admin user account
    #[arg(long)]
    pub password: Option<String>,

    /// Full name of the system administrator
    #[arg(long)]
    pub name: Option<String>,

    /// API base URL (required for API mode)
    #[arg(long)]
    pub api_url: Option<String>,
}

#[derive(Args, Debug)]
pub struct CreateTokenArgs {
    /// Display name for the token (3-64 alphanumeric characters)
    #[arg(long)]
    pub name: String,

    /// Optional human-readable description
    #[arg(long)]
    pub description: Option<String>,

    /// Scopes granted to the token, e.g. --scope clusters:read (repeat for multiple scopes)
    #[arg(long = "scope", required = true, action = ArgAction::Append)]
    pub scopes: Vec<String>,

    /// RFC3339 expiration timestamp (UTC)
    #[arg(long, conflicts_with = "expires_in", value_parser = parse_rfc3339)]
    pub expires_at: Option<DateTime<Utc>>,

    /// Relative expiration (e.g. 90d, 12h, 30m, 45s)
    #[arg(long, conflicts_with = "expires_at")]
    pub expires_in: Option<String>,

    /// Optional creator identifier recorded with the token metadata
    #[arg(long)]
    pub created_by: Option<String>,

    /// API base URL (enables API mode instead of direct database access)
    #[arg(long)]
    pub api_url: Option<String>,
}

#[derive(Args, Debug, Default)]
pub struct ListTokensArgs {
    /// Maximum number of tokens to return (default 50, max 1000)
    #[arg(long, default_value_t = 50)]
    pub limit: i64,

    /// Offset for pagination
    #[arg(long, default_value_t = 0)]
    pub offset: i64,

    /// API base URL (enables API mode instead of direct database access)
    #[arg(long)]
    pub api_url: Option<String>,
}

#[derive(Args, Debug)]
pub struct TokenRefArgs {
    /// Identifier of the token
    pub id: String,

    /// API base URL (enables API mode instead of direct database access)
    #[arg(long)]
    pub api_url: Option<String>,
}

pub async fn handle_auth_command(
    command: AuthCommands,
    database: &DatabaseConfig,
) -> anyhow::Result<()> {
    match command {
        AuthCommands::Bootstrap(args) => handle_bootstrap(args).await?,
        AuthCommands::CreateToken(args) => {
            let api_url = args.api_url.clone();
            if let Some(api_url) = api_url {
                handle_create_token_api(args, &api_url).await?
            } else {
                handle_create_token_db(args, database).await?
            }
        }
        AuthCommands::ListTokens(args) => {
            let api_url = args.api_url.clone();
            if let Some(api_url) = api_url {
                handle_list_tokens_api(args, &api_url).await?
            } else {
                handle_list_tokens_db(args, database).await?
            }
        }
        AuthCommands::RevokeToken(args) => {
            if let Some(api_url) = args.api_url.clone() {
                handle_revoke_token_api(&args.id, &api_url).await?
            } else {
                handle_revoke_token_db(&args.id, database).await?
            }
        }
        AuthCommands::RotateToken(args) => {
            if let Some(api_url) = args.api_url.clone() {
                handle_rotate_token_api(&args.id, &api_url).await?
            } else {
                handle_rotate_token_db(&args.id, database).await?
            }
        }
    }

    Ok(())
}

// === Bootstrap Command ===

async fn handle_bootstrap(args: BootstrapArgs) -> anyhow::Result<()> {
    // Resolve API URL from args or environment
    let api_url = args.api_url.or_else(|| std::env::var("FLOWPLANE_BASE_URL").ok())
        .ok_or_else(|| anyhow!("API URL required for bootstrap. Provide via --api-url or FLOWPLANE_BASE_URL environment variable"))?;

    // Resolve admin email from args or environment
    let email = args.email
        .or_else(|| std::env::var("FLOWPLANE_ADMIN_EMAIL").ok())
        .ok_or_else(|| anyhow!("Admin email required. Provide via --email or FLOWPLANE_ADMIN_EMAIL environment variable"))?;

    // Resolve password from args or environment
    let password = args.password
        .or_else(|| std::env::var("FLOWPLANE_ADMIN_PASSWORD").ok())
        .ok_or_else(|| anyhow!("Admin password required. Provide via --password or FLOWPLANE_ADMIN_PASSWORD environment variable"))?;

    // Resolve name from args or environment
    let name = args.name
        .or_else(|| std::env::var("FLOWPLANE_ADMIN_NAME").ok())
        .ok_or_else(|| anyhow!("Admin name required. Provide via --name or FLOWPLANE_ADMIN_NAME environment variable"))?;

    // Create a temporary client with empty token (no authentication needed for bootstrap)
    let config = crate::cli::client::ClientConfig {
        base_url: api_url,
        token: String::new(),
        timeout: 30,
        verbose: false,
    };
    let client = FlowplaneClient::new(config)?;

    // Call bootstrap API to create admin user and generate setup token
    let response = client.bootstrap_initialize(&email, &password, &name).await?;

    println!("{}", "Bootstrap complete! Admin user created and setup token generated!".green());
    println!();
    println!("  Setup Token: {}", response.setup_token.bright_yellow());
    println!("  Expires At: {}", response.expires_at.to_rfc3339());
    println!("  Max Usage: {}", response.max_usage_count);
    println!();
    println!("{}", response.message);
    println!();
    println!("{}", "Next Steps:".bright_blue().bold());
    for (i, step) in response.next_steps.iter().enumerate() {
        println!("  {}. {}", i + 1, step);
    }
    println!();
    println!("{}", "IMPORTANT: Save this setup token securely!".bright_yellow().bold());
    println!("You can also store it as an environment variable:");
    println!("  export FLOWPLANE_SETUP_TOKEN={}", response.setup_token);

    Ok(())
}

// === Database Mode Functions ===

async fn handle_create_token_db(
    args: CreateTokenArgs,
    database: &DatabaseConfig,
) -> anyhow::Result<()> {
    let pool = create_pool(database).await?;
    let audit_repository = Arc::new(AuditLogRepository::new(pool.clone()));
    let service = TokenService::with_sqlx(pool, audit_repository);

    let expires_at = if let Some(absolute) = args.expires_at {
        Some(absolute)
    } else if let Some(relative) = args.expires_in {
        let duration = parse_duration(&relative)?;
        Some(Utc::now() + duration)
    } else {
        None
    };

    let request = TokenServiceRequest {
        name: args.name,
        description: args.description,
        expires_at,
        scopes: args.scopes,
        created_by: args.created_by,
        user_id: None,
        user_email: None,
    };

    if let Err(err) = request.validate() {
        return Err(describe_validation_error(err));
    }

    let response = service.create_token(request).await?;
    let token = service.get_token(&response.id).await?;

    print_token_secret_from_service(&response, &token);
    Ok(())
}

async fn handle_list_tokens_db(
    args: ListTokensArgs,
    database: &DatabaseConfig,
) -> anyhow::Result<()> {
    let pool = create_pool(database).await?;
    let audit_repository = Arc::new(AuditLogRepository::new(pool.clone()));
    let service = TokenService::with_sqlx(pool, audit_repository);

    let tokens = service.list_tokens(args.limit.clamp(1, 1000), args.offset.max(0), None).await?;

    if tokens.is_empty() {
        println!("No tokens found.");
    } else {
        print_token_table(&tokens);
    }

    Ok(())
}

async fn handle_revoke_token_db(id: &str, database: &DatabaseConfig) -> anyhow::Result<()> {
    let pool = create_pool(database).await?;
    let audit_repository = Arc::new(AuditLogRepository::new(pool.clone()));
    let service = TokenService::with_sqlx(pool, audit_repository);

    let token = service.revoke_token(id).await?;
    println!("{}", format!("Token '{}' ({}) revoked", token.name, token.id).bright_red());
    Ok(())
}

async fn handle_rotate_token_db(id: &str, database: &DatabaseConfig) -> anyhow::Result<()> {
    let pool = create_pool(database).await?;
    let audit_repository = Arc::new(AuditLogRepository::new(pool.clone()));
    let service = TokenService::with_sqlx(pool, audit_repository);

    let secret = service.rotate_token(id).await?;
    let token = service.get_token(id).await?;

    println!("{}", "Token rotated successfully".green());
    println!("  ID: {}", token.id);
    println!("  Name: {}", token.name);
    println!("  New Token: {}", secret.token.bright_yellow());
    println!("  Scopes: {}", token.scopes.join(", "));
    Ok(())
}

// === API Mode Functions ===

async fn handle_create_token_api(args: CreateTokenArgs, api_url: &str) -> anyhow::Result<()> {
    let client = create_api_client(api_url)?;

    let expires_at = if let Some(absolute) = args.expires_at {
        Some(absolute)
    } else if let Some(relative) = args.expires_in {
        let duration = parse_duration(&relative)?;
        Some(Utc::now() + duration)
    } else {
        None
    };

    let request = ApiCreateTokenRequest {
        name: args.name,
        description: args.description,
        expires_at,
        scopes: args.scopes,
        created_by: args.created_by,
    };

    let response = client.create_token(request).await?;
    let token = client.get_token(&response.id).await?;

    print_token_secret_from_api(&response, &token);
    Ok(())
}

async fn handle_list_tokens_api(args: ListTokensArgs, api_url: &str) -> anyhow::Result<()> {
    let client = create_api_client(api_url)?;

    let tokens = client.list_tokens(args.limit.clamp(1, 1000), args.offset.max(0)).await?;

    if tokens.is_empty() {
        println!("No tokens found.");
    } else {
        print_token_table(&tokens);
    }

    Ok(())
}

async fn handle_revoke_token_api(id: &str, api_url: &str) -> anyhow::Result<()> {
    let client = create_api_client(api_url)?;

    let token = client.revoke_token(id).await?;
    println!("{}", format!("Token '{}' ({}) revoked", token.name, token.id).bright_red());
    Ok(())
}

async fn handle_rotate_token_api(_id: &str, _api_url: &str) -> anyhow::Result<()> {
    bail!("Token rotation is not yet supported via API mode. Please use database mode.")
}

// === Helper Functions ===

fn create_api_client(api_url: &str) -> anyhow::Result<FlowplaneClient> {
    // Resolve token from environment or config
    let token = std::env::var("FLOWPLANE_TOKEN")
        .or_else(|_| {
            crate::cli::config::CliConfig::load()
                .ok()
                .and_then(|cfg| cfg.token)
                .ok_or_else(|| anyhow!("No token found"))
        })
        .map_err(|_| {
            anyhow!(
                "Authentication token required for API mode. Set via:\n\
             - FLOWPLANE_TOKEN environment variable\n\
             - ~/.flowplane/config.toml\n\
             - flowplane config set-token <token>"
            )
        })?;

    let config = crate::cli::client::ClientConfig {
        base_url: api_url.to_string(),
        token,
        timeout: 30,
        verbose: false,
    };

    FlowplaneClient::new(config)
}

fn print_token_secret_from_service(secret: &TokenSecretResponse, token: &PersonalAccessToken) {
    println!("{}", "Token created successfully!".green());
    println!("  ID: {}", token.id);
    println!("  Name: {}", token.name);
    println!("  Token: {}", secret.token.bright_yellow());
    if let Some(expires_at) = token.expires_at {
        println!("  Expires: {}", expires_at.to_rfc3339());
    } else {
        println!("  Expires: never");
    }
    println!("  Scopes: {}", token.scopes.join(", "));
    if let Some(created_by) = &token.created_by {
        println!("  Created by: {}", created_by);
    }
}

fn print_token_secret_from_api(
    secret: &crate::cli::client::CreateTokenResponse,
    token: &PersonalAccessToken,
) {
    println!("{}", "Token created successfully!".green());
    println!("  ID: {}", token.id);
    println!("  Name: {}", token.name);
    println!("  Token: {}", secret.token.bright_yellow());
    if let Some(expires_at) = token.expires_at {
        println!("  Expires: {}", expires_at.to_rfc3339());
    } else {
        println!("  Expires: never");
    }
    println!("  Scopes: {}", token.scopes.join(", "));
    if let Some(created_by) = &token.created_by {
        println!("  Created by: {}", created_by);
    }
}

fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| format!("Invalid RFC3339 timestamp '{}': {}", value, err))
}

fn parse_duration(value: &str) -> anyhow::Result<Duration> {
    if value.len() < 2 {
        bail!("Invalid duration '{}': expected format like 90d, 12h, 30m, 45s", value);
    }

    let (number, unit) = value.split_at(value.len() - 1);
    let quantity: i64 =
        number.parse().map_err(|err| anyhow!("Invalid duration '{}': {}", value, err))?;

    let duration = match unit {
        "d" | "D" => Duration::days(quantity),
        "h" | "H" => Duration::hours(quantity),
        "m" | "M" => Duration::minutes(quantity),
        "s" | "S" => Duration::seconds(quantity),
        _ => bail!(
            "Invalid duration unit '{}': expected one of d (days), h (hours), m (minutes), s (seconds)",
            unit
        ),
    };

    Ok(duration)
}

fn print_token_table(tokens: &[PersonalAccessToken]) {
    println!(
        "{:<38} {:<20} {:<10} {:<25} {:<25} {:<40}",
        "ID", "Name", "Status", "Created", "Expires", "Scopes"
    );
    println!("{}", "-".repeat(170));

    for token in tokens {
        println!(
            "{:<38} {:<20} {:<10} {:<25} {:<25} {:<40}",
            token.id,
            truncate(&token.name, 20),
            format_status(token.status),
            token.created_at.to_rfc3339(),
            token.expires_at.map(|dt| dt.to_rfc3339()).unwrap_or_else(|| "--".into()),
            truncate(&token.scopes.join(","), 40)
        );
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else if max > 3 {
        format!("{}...", &value[..max - 3])
    } else {
        value[..max].to_string()
    }
}

fn format_status(status: TokenStatus) -> String {
    match status {
        TokenStatus::Active => "active".green().to_string(),
        TokenStatus::Revoked => "revoked".red().to_string(),
        TokenStatus::Expired => "expired".yellow().to_string(),
    }
}

fn describe_validation_error(err: ValidationErrors) -> anyhow::Error {
    if err.field_errors().contains_key("name") {
        return anyhow!(
            "Invalid token name. Use 3-64 characters containing only letters, numbers, underscores, or hyphens (no spaces)."
        );
    }
    anyhow!("Validation failed: {}", err)
}
