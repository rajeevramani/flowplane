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
        after_help = "EXAMPLES:\n    # Create a generic secret (type is auto-injected from --type)\n    flowplane secret create --name my-api-key --type generic_secret \\\n        --config '{\"secret\": \"base64-encoded-value\"}'\n\n    # Create a TLS certificate\n    flowplane secret create --name my-tls-cert --type tls_certificate \\\n        --config '{\"certificate_chain\": \"...\", \"private_key\": \"...\"}'\n\n    # Create with description and expiry\n    flowplane secret create --name my-secret --type generic_secret \\\n        --config '{\"secret\": \"dGVzdA==\"}' --description 'API key for upstream' \\\n        --expires-at '2027-01-01T00:00:00Z'\n\n    # Explicit type in config (must match --type)\n    flowplane secret create --name my-key --type generic_secret \\\n        --config '{\"type\": \"generic_secret\", \"secret\": \"dGVzdA==\"}'"
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

    /// Rotate a secret (bump version with optional new config)
    #[command(
        long_about = "Rotate a secret by ID, bumping its version.\n\nOptionally provide new configuration to replace the secret value during rotation.",
        after_help = "EXAMPLES:\n    # Rotate a secret (bump version, keep existing config)\n    flowplane secret rotate abc-123\n\n    # Rotate with new config\n    flowplane secret rotate abc-123 --config '{\"secret\": \"new-base64-value\"}'\n\n    # Rotate and output as JSON\n    flowplane secret rotate abc-123 -o json"
    )]
    Rotate {
        /// Secret ID to rotate
        #[arg(value_name = "SECRET_ID")]
        secret_id: String,

        /// Optional new configuration as JSON string
        #[arg(long)]
        config: Option<String>,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
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

/// Paginated response wrapper for secret list endpoint
#[derive(Debug, Deserialize)]
struct PaginatedSecrets {
    items: Vec<SecretResponse>,
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
        SecretCommands::Rotate { secret_id, config, output } => {
            rotate_secret(client, team, &secret_id, config, &output).await?
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

    let mut configuration: serde_json::Value =
        serde_json::from_str(&config).context("Invalid JSON in --config")?;

    inject_type_into_config(&mut configuration, &secret_type)?;

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

    let response: PaginatedSecrets = client.get_json(&path).await?;

    if output == "table" {
        print_secrets_table(&response.items);
    } else {
        print_output(&response.items, output)?;
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

async fn rotate_secret(
    client: &FlowplaneClient,
    team: &str,
    secret_id: &str,
    config: Option<String>,
    output: &str,
) -> Result<()> {
    let body = match config {
        Some(ref c) => {
            let value: serde_json::Value =
                serde_json::from_str(c).context("Invalid JSON in --config")?;
            serde_json::json!({ "configuration": value })
        }
        None => serde_json::json!({ "configuration": {} }),
    };

    let path = format!("/api/v1/teams/{team}/secrets/{secret_id}/rotate");
    let response: SecretResponse = client.post_json(&path, &body).await?;

    if output == "table" {
        print_secrets_table(&[response]);
    } else {
        print_output(&response, output)?;
    }

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

/// Inject the `type` tag from `--type` flag into the config JSON object.
///
/// The API requires a `"type"` discriminator inside the configuration for
/// `SecretSpec` deserialization. This auto-injects it so users don't need
/// to redundantly specify the type in both `--type` and `--config`.
fn inject_type_into_config(configuration: &mut serde_json::Value, secret_type: &str) -> Result<()> {
    if let Some(obj) = configuration.as_object_mut() {
        match obj.get("type") {
            Some(existing_type) => {
                if let Some(existing_str) = existing_type.as_str() {
                    if existing_str != secret_type {
                        anyhow::bail!(
                            "Conflicting types: --type '{}' but config JSON has type '{}'. \
                             Remove the type field from --config or make them match.",
                            secret_type,
                            existing_str
                        );
                    }
                }
            }
            None => {
                obj.insert("type".to_string(), serde_json::Value::String(secret_type.to_string()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inject_type_adds_missing_type() {
        let mut config = json!({"secret": "dGVzdA=="});
        inject_type_into_config(&mut config, "generic_secret").unwrap();
        assert_eq!(config["type"], "generic_secret");
        assert_eq!(config["secret"], "dGVzdA==");
    }

    #[test]
    fn inject_type_matching_type_is_ok() {
        let mut config = json!({"type": "generic_secret", "secret": "dGVzdA=="});
        inject_type_into_config(&mut config, "generic_secret").unwrap();
        assert_eq!(config["type"], "generic_secret");
    }

    #[test]
    fn inject_type_conflicting_type_errors() {
        let mut config = json!({"type": "tls_certificate", "secret": "dGVzdA=="});
        let result = inject_type_into_config(&mut config, "generic_secret");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Conflicting types"));
        assert!(err.contains("generic_secret"));
        assert!(err.contains("tls_certificate"));
    }

    #[test]
    fn inject_type_non_string_type_field_is_ignored() {
        // If someone puts a non-string "type" value, don't error — let the API reject it
        let mut config = json!({"type": 42, "secret": "dGVzdA=="});
        let result = inject_type_into_config(&mut config, "generic_secret");
        assert!(result.is_ok());
        // Original non-string value preserved
        assert_eq!(config["type"], 42);
    }

    #[test]
    fn inject_type_non_object_config_is_noop() {
        let mut config = json!("not-an-object");
        let result = inject_type_into_config(&mut config, "generic_secret");
        assert!(result.is_ok());
        assert_eq!(config, json!("not-an-object"));
    }

    #[test]
    fn inject_type_all_secret_types() {
        for secret_type in VALID_SECRET_TYPES {
            let mut config = json!({"secret": "dGVzdA=="});
            inject_type_into_config(&mut config, secret_type).unwrap();
            assert_eq!(config["type"], *secret_type);
        }
    }

    #[test]
    fn inject_type_preserves_other_fields() {
        let mut config = json!({
            "certificate_chain": "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----",
            "private_key": "-----BEGIN PRIVATE KEY-----\ntest\n-----END PRIVATE KEY-----"
        });
        inject_type_into_config(&mut config, "tls_certificate").unwrap();
        assert_eq!(config["type"], "tls_certificate");
        assert!(config["certificate_chain"].is_string());
        assert!(config["private_key"].is_string());
    }
}
