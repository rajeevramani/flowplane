//! mTLS and certificate CLI commands
//!
//! Provides command-line interface for mTLS status and proxy certificate management.

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::FlowplaneClient;

#[derive(Subcommand)]
pub enum MtlsCommands {
    /// Show mTLS status for the gateway
    #[command(
        long_about = "Display the current mTLS configuration status.\n\nShows whether mTLS is enabled, the trust domain, and PKI configuration.",
        after_help = "EXAMPLES:\n    # Check mTLS status\n    flowplane mtls status\n\n    # Output as JSON\n    flowplane mtls status -o json"
    )]
    Status {
        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },
}

#[derive(Subcommand)]
pub enum CertCommands {
    /// List proxy certificates for the current team
    #[command(
        long_about = "List all proxy certificates issued for the current team.\n\nShows certificate details including status, expiry, and serial number.",
        after_help = "EXAMPLES:\n    # List certificates\n    flowplane cert list\n\n    # List as JSON\n    flowplane cert list -o json"
    )]
    List {
        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Get details of a specific proxy certificate
    #[command(
        long_about = "Retrieve detailed information about a specific proxy certificate.",
        after_help = "EXAMPLES:\n    # Get certificate details\n    flowplane cert get abc123\n\n    # Get with YAML output\n    flowplane cert get abc123 -o yaml"
    )]
    Get {
        /// Certificate ID
        #[arg(value_name = "ID")]
        id: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Create (generate) a new proxy certificate
    #[command(
        long_about = "Generate a new proxy certificate for mTLS authentication.\n\nProvide a JSON file with the certificate request parameters.",
        after_help = "EXAMPLES:\n    # Generate a certificate from spec file\n    flowplane cert create -f cert-request.json\n\n    # Generate with JSON output\n    flowplane cert create -f cert-request.json -o json"
    )]
    Create {
        /// Path to JSON file with certificate request spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json or yaml)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml"])]
        output: String,
    },

    /// Revoke a proxy certificate
    #[command(
        long_about = "Revoke a proxy certificate, preventing it from being used for mTLS.\n\nWARNING: This action cannot be undone.",
        after_help = "EXAMPLES:\n    # Revoke a certificate (with confirmation)\n    flowplane cert revoke abc123\n\n    # Revoke without confirmation\n    flowplane cert revoke abc123 --yes"
    )]
    Revoke {
        /// Certificate ID
        #[arg(value_name = "ID")]
        id: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

/// mTLS status response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MtlsStatusResponse {
    pub enabled: bool,
    #[serde(default)]
    pub trust_domain: Option<String>,
    #[serde(default)]
    pub pki_mount_path: Option<String>,
    #[serde(default)]
    pub pki_role_name: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Certificate response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateResponse {
    pub id: String,
    #[serde(default)]
    pub team: Option<String>,
    #[serde(default)]
    pub serial_number: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub common_name: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Handle mTLS commands
pub async fn handle_mtls_command(command: MtlsCommands, client: &FlowplaneClient) -> Result<()> {
    match command {
        MtlsCommands::Status { output } => mtls_status(client, &output).await?,
    }
    Ok(())
}

/// Handle cert commands
pub async fn handle_cert_command(
    command: CertCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        CertCommands::List { output } => list_certs(client, team, &output).await?,
        CertCommands::Get { id, output } => get_cert(client, team, &id, &output).await?,
        CertCommands::Create { file, output } => create_cert(client, team, file, &output).await?,
        CertCommands::Revoke { id, yes } => revoke_cert(client, team, &id, yes).await?,
    }
    Ok(())
}

async fn mtls_status(client: &FlowplaneClient, output: &str) -> Result<()> {
    let response: MtlsStatusResponse = client.get_json("/api/v1/mtls/status").await?;

    if output == "table" {
        print_mtls_status_table(&response);
    } else {
        print_output(&response, output)?;
    }
    Ok(())
}

async fn list_certs(client: &FlowplaneClient, team: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/proxy-certificates");
    let response: Vec<CertificateResponse> = client.get_json(&path).await?;

    if output == "table" {
        print_certs_table(&response);
    } else {
        print_output(&response, output)?;
    }
    Ok(())
}

async fn get_cert(client: &FlowplaneClient, team: &str, id: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/proxy-certificates/{id}");
    let response: CertificateResponse = client.get_json(&path).await?;

    if output == "table" {
        print_certs_table(&[response]);
    } else {
        print_output(&response, output)?;
    }
    Ok(())
}

async fn create_cert(
    client: &FlowplaneClient,
    team: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse JSON from file")?;

    let path = format!("/api/v1/teams/{team}/proxy-certificates");
    let response: CertificateResponse = client.post_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn revoke_cert(client: &FlowplaneClient, team: &str, id: &str, yes: bool) -> Result<()> {
    if !yes {
        println!(
            "Are you sure you want to revoke certificate '{}'? This cannot be undone. (y/N)",
            id
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/teams/{team}/proxy-certificates/{id}/revoke");
    let body = serde_json::json!({});
    let response: serde_json::Value = client.post_json(&path, &body).await?;

    println!("Certificate '{}' revoked successfully", id);
    if !response.is_null() && response != serde_json::json!({}) {
        let json =
            serde_json::to_string_pretty(&response).context("Failed to serialize response")?;
        println!("{}", json);
    }
    Ok(())
}

fn print_mtls_status_table(status: &MtlsStatusResponse) {
    println!();
    println!("{:<20} {}", "mTLS Enabled:", if status.enabled { "Yes" } else { "No" });
    if let Some(ref domain) = status.trust_domain {
        println!("{:<20} {}", "Trust Domain:", domain);
    }
    if let Some(ref mount) = status.pki_mount_path {
        println!("{:<20} {}", "PKI Mount:", mount);
    }
    if let Some(ref role) = status.pki_role_name {
        println!("{:<20} {}", "PKI Role:", role);
    }
    println!();
}

fn print_certs_table(certs: &[CertificateResponse]) {
    if certs.is_empty() {
        println!("No certificates found");
        return;
    }

    println!();
    println!("{:<38} {:<12} {:<30} {:<25}", "ID", "Status", "Common Name", "Expires At");
    println!("{}", "-".repeat(110));

    for cert in certs {
        println!(
            "{:<38} {:<12} {:<30} {:<25}",
            truncate(&cert.id, 36),
            cert.status.as_deref().unwrap_or("-"),
            truncate(cert.common_name.as_deref().unwrap_or("-"), 28),
            cert.expires_at.as_deref().unwrap_or("-"),
        );
    }
    println!();
}

fn print_output<T: Serialize>(data: &T, format: &str) -> Result<()> {
    match format {
        "json" => {
            let json = serde_json::to_string_pretty(data).context("Failed to serialize to JSON")?;
            println!("{}", json);
        }
        "yaml" => {
            let yaml = serde_yaml::to_string(data).context("Failed to serialize to YAML")?;
            println!("{}", yaml);
        }
        _ => {
            anyhow::bail!("Unsupported output format: {}. Use 'json' or 'yaml'.", format);
        }
    }
    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}
