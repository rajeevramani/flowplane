//! Secret management CLI commands
//!
//! Provides command-line interface for managing Envoy SDS secrets

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use super::client::FlowplaneClient;

#[derive(Subcommand)]
pub enum SecretCommands {
    /// Create a new secret
    #[command(
        long_about = "Create a new secret for Envoy Secret Discovery Service.\n\nSecrets can be generic (API keys, tokens), TLS certificates,\ncertificate validation contexts (CA certs), or session ticket keys.",
        after_help = "EXAMPLES:\n    # Create a generic secret\n    flowplane secret create --name my-api-key --type generic_secret \\\n        --config '{\"secret\": \"base64-encoded-value\"}'\n\n    # Create a TLS certificate\n    flowplane secret create --name my-tls-cert --type tls_certificate \\\n        --config '{\"certificate_chain\": \"...\", \"private_key\": \"...\"}'\n\n    # Create with description and expiry\n    flowplane secret create --name my-secret --type generic_secret \\\n        --config '{\"secret\": \"dGVzdA==\"}' --description 'API key for upstream' \\\n        --expires-at '2027-01-01T00:00:00Z'"
    )]
    Create {
        /// Name of the secret (must be unique within the team)
        #[arg(long)]
        name: String,

        /// Secret type: generic_secret, tls_certificate, certificate_validation_context, session_ticket_keys
        #[arg(long = "type", value_name = "TYPE")]
        secret_type: String,

        /// Secret configuration as JSON string (varies by type)
        #[arg(long)]
        config: String,

        /// Optional description
        #[arg(long)]
        description: Option<String>,

        /// Optional expiration time (ISO 8601 format)
        #[arg(long)]
        expires_at: Option<String>,
    },

    /// List secrets
    #[command(
        long_about = "List secrets for the current team with optional filtering.",
        after_help = "EXAMPLES:\n    # List all secrets\n    flowplane secret list\n\n    # List only TLS certificates\n    flowplane secret list --type tls_certificate\n\n    # List as JSON\n    flowplane secret list -o json\n\n    # Paginate\n    flowplane secret list --limit 10 --offset 20"
    )]
    List {
        /// Filter by secret type
        #[arg(long = "type", value_name = "TYPE")]
        secret_type: Option<String>,

        /// Maximum number of results
        #[arg(long)]
        limit: Option<i64>,

        /// Offset for pagination
        #[arg(long)]
        offset: Option<i64>,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Get details of a secret
    #[command(
        long_about = "Retrieve metadata for a specific secret by ID.\n\nNote: secret values are never returned in plaintext.",
        after_help = "EXAMPLES:\n    # Get secret details\n    flowplane secret get abc-123\n\n    # Get as YAML\n    flowplane secret get abc-123 -o yaml"
    )]
    Get {
        /// Secret ID to retrieve
        #[arg(value_name = "SECRET_ID")]
        secret_id: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Delete a secret
    #[command(
        long_about = "Delete a secret by ID.\n\nRequires confirmation unless --yes is provided.",
        after_help = "EXAMPLES:\n    # Delete with confirmation prompt\n    flowplane secret delete abc-123\n\n    # Delete without confirmation\n    flowplane secret delete abc-123 --yes"
    )]
    Delete {
        /// Secret ID to delete
        #[arg(value_name = "SECRET_ID")]
        secret_id: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

/// Secret response matching the API response shape
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretResponse {
    pub id: String,
    pub name: String,
    pub secret_type: String,
    pub description: Option<String>,
    pub version: i64,
    pub source: String,
    pub team: String,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
    pub backend: Option<String>,
    pub reference: Option<String>,
    pub reference_version: Option<String>,
}

/// Request body for creating a secret
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateSecretRequest {
    name: String,
    secret_type: String,
    configuration: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<String>,
}

const VALID_SECRET_TYPES: &[&str] =
    &["generic_secret", "tls_certificate", "certificate_validation_context", "session_ticket_keys"];

/// Handle secret commands
pub async fn handle_secret_command(
    command: SecretCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        SecretCommands::Create { name, secret_type, config, description, expires_at } => {
            create_secret(client, team, name, secret_type, config, description, expires_at).await?
        }
        SecretCommands::List { secret_type, limit, offset, output } => {
            list_secrets(client, team, secret_type, limit, offset, &output).await?
        }
        SecretCommands::Get { secret_id, output } => {
            get_secret(client, team, &secret_id, &output).await?
        }
        SecretCommands::Delete { secret_id, yes } => {
            delete_secret(client, team, &secret_id, yes).await?
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn create_secret(
    client: &FlowplaneClient,
    team: &str,
    name: String,
    secret_type: String,
    config: String,
    description: Option<String>,
    expires_at: Option<String>,
) -> Result<()> {
    if !VALID_SECRET_TYPES.contains(&secret_type.as_str()) {
        anyhow::bail!(
            "Invalid secret type '{}'. Valid types: {}",
            secret_type,
            VALID_SECRET_TYPES.join(", ")
        );
    }

    let configuration: serde_json::Value =
        serde_json::from_str(&config).context("Invalid JSON in --config")?;

    let body = CreateSecretRequest { name, secret_type, configuration, description, expires_at };

    let path = format!("/api/v1/teams/{team}/secrets");
    let response: SecretResponse = client.post_json(&path, &body).await?;

    let json =
        serde_json::to_string_pretty(&response).context("Failed to serialize response to JSON")?;
    println!("{json}");

    Ok(())
}

async fn list_secrets(
    client: &FlowplaneClient,
    team: &str,
    secret_type: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    output: &str,
) -> Result<()> {
    let mut params = Vec::new();

    if let Some(ref t) = secret_type {
        if !VALID_SECRET_TYPES.contains(&t.as_str()) {
            anyhow::bail!(
                "Invalid secret type '{}'. Valid types: {}",
                t,
                VALID_SECRET_TYPES.join(", ")
            );
        }
        params.push(format!("secretType={t}"));
    }
    if let Some(l) = limit {
        params.push(format!("limit={l}"));
    }
    if let Some(o) = offset {
        params.push(format!("offset={o}"));
    }

    let path = if params.is_empty() {
        format!("/api/v1/teams/{team}/secrets")
    } else {
        format!("/api/v1/teams/{team}/secrets?{}", params.join("&"))
    };

    let response: Vec<SecretResponse> = client.get_json(&path).await?;

    if output == "table" {
        print_secrets_table(&response);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn get_secret(
    client: &FlowplaneClient,
    team: &str,
    secret_id: &str,
    output: &str,
) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/secrets/{secret_id}");
    let response: SecretResponse = client.get_json(&path).await?;

    if output == "table" {
        print_secrets_table(&[response]);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn delete_secret(
    client: &FlowplaneClient,
    team: &str,
    secret_id: &str,
    yes: bool,
) -> Result<()> {
    if !yes {
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            anyhow::bail!(
                "Cannot prompt for confirmation: stdin is not a terminal. Use --yes to skip confirmation."
            );
        }
        println!("Are you sure you want to delete secret '{}'? (y/N)", secret_id);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted");
            return Ok(());
        }
    }

    let path = format!("/api/v1/teams/{team}/secrets/{secret_id}");
    client.delete_no_content(&path).await?;

    println!("Secret '{}' deleted successfully", secret_id);
    Ok(())
}

fn print_output<T: Serialize>(data: &T, format: &str) -> Result<()> {
    match format {
        "json" => {
            let json = serde_json::to_string_pretty(data).context("Failed to serialize to JSON")?;
            println!("{json}");
        }
        "yaml" => {
            let yaml = serde_yaml::to_string(data).context("Failed to serialize to YAML")?;
            println!("{yaml}");
        }
        _ => {
            anyhow::bail!("Unsupported output format: {}. Use 'json' or 'yaml'.", format);
        }
    }
    Ok(())
}

fn print_secrets_table(secrets: &[SecretResponse]) {
    if secrets.is_empty() {
        println!("No secrets found");
        return;
    }

    println!();
    println!(
        "{:<38} {:<25} {:<30} {:<10} {:<8} {:<12}",
        "ID", "Name", "Type", "Source", "Version", "Expires"
    );
    println!("{}", "-".repeat(125));

    for secret in secrets {
        let expires = secret.expires_at.as_deref().unwrap_or("never");
        println!(
            "{:<38} {:<25} {:<30} {:<10} {:<8} {:<12}",
            truncate(&secret.id, 36),
            truncate(&secret.name, 23),
            truncate(&secret.secret_type, 28),
            truncate(&secret.source, 8),
            secret.version,
            truncate(expires, 10),
        );
    }
    println!();
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}
