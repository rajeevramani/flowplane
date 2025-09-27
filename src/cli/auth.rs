use std::sync::Arc;

use anyhow::{anyhow, bail};
use chrono::{DateTime, Duration, Utc};
use clap::{ArgAction, Args, Subcommand};
use owo_colors::OwoColorize;
use validator::{Validate, ValidationErrors};

use crate::auth::{
    models::{PersonalAccessToken, TokenStatus},
    token_service::{TokenSecretResponse, TokenService},
    validation::CreateTokenRequest,
};
use crate::config::DatabaseConfig;
use crate::storage::{create_pool, repository_simple::AuditLogRepository};

#[derive(Subcommand, Debug)]
pub enum AuthCommands {
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
}

#[derive(Args, Debug, Default)]
pub struct ListTokensArgs {
    /// Maximum number of tokens to return (default 50, max 1000)
    #[arg(long, default_value_t = 50)]
    pub limit: i64,

    /// Offset for pagination
    #[arg(long, default_value_t = 0)]
    pub offset: i64,
}

#[derive(Args, Debug)]
pub struct TokenRefArgs {
    /// Identifier of the token
    pub id: String,
}

pub async fn handle_auth_command(
    command: AuthCommands,
    database: &DatabaseConfig,
) -> anyhow::Result<()> {
    let pool = create_pool(database).await?;
    let audit_repository = Arc::new(AuditLogRepository::new(pool.clone()));
    let service = TokenService::with_sqlx(pool, audit_repository);

    match command {
        AuthCommands::CreateToken(args) => create_token(&service, args).await?,
        AuthCommands::ListTokens(args) => list_tokens(&service, args).await?,
        AuthCommands::RevokeToken(args) => revoke_token(&service, &args.id).await?,
        AuthCommands::RotateToken(args) => rotate_token(&service, &args.id).await?,
    }

    Ok(())
}

async fn create_token(service: &TokenService, args: CreateTokenArgs) -> anyhow::Result<()> {
    let expires_at = if let Some(absolute) = args.expires_at {
        Some(absolute)
    } else if let Some(relative) = args.expires_in {
        let duration = parse_duration(&relative)?;
        Some(Utc::now() + duration)
    } else {
        None
    };

    let request = CreateTokenRequest {
        name: args.name,
        description: args.description,
        expires_at,
        scopes: args.scopes,
        created_by: args.created_by,
    };

    if let Err(err) = request.validate() {
        return Err(describe_validation_error(err));
    }

    let response = service.create_token(request).await?;
    let token = service.get_token(&response.id).await?;

    print_token_secret(&response, &token);
    Ok(())
}

async fn list_tokens(service: &TokenService, args: ListTokensArgs) -> anyhow::Result<()> {
    let tokens = service.list_tokens(args.limit.clamp(1, 1000), args.offset.max(0)).await?;

    if tokens.is_empty() {
        println!("No tokens found.");
    } else {
        print_token_table(&tokens);
    }

    Ok(())
}

async fn revoke_token(service: &TokenService, id: &str) -> anyhow::Result<()> {
    let token = service.revoke_token(id).await?;
    println!("{}", format!("Token '{}' ({}) revoked", token.name, token.id).bright_red());
    Ok(())
}

async fn rotate_token(service: &TokenService, id: &str) -> anyhow::Result<()> {
    let secret = service.rotate_token(id).await?;
    let token = service.get_token(id).await?;

    println!("{}", "Token rotated successfully".green());
    println!("  ID: {}", token.id);
    println!("  Name: {}", token.name);
    println!("  New Token: {}", secret.token.bright_yellow());
    println!("  Scopes: {}", token.scopes.join(", "));
    Ok(())
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

fn print_token_secret(secret: &TokenSecretResponse, token: &PersonalAccessToken) {
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
