//! Organization CLI commands
//!
//! Provides command-line interface for managing organizations (admin-scoped).

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::FlowplaneClient;
use super::config_file;

#[derive(Subcommand)]
pub enum OrgCommands {
    /// List all organizations
    #[command(
        long_about = "List all organizations in the platform.\n\nRequires platform-admin privileges.",
        after_help = "EXAMPLES:\n    # List organizations\n    flowplane org list\n\n    # List as JSON\n    flowplane org list -o json"
    )]
    List {
        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Get details of a specific organization
    #[command(
        long_about = "Retrieve detailed information about a specific organization.",
        after_help = "EXAMPLES:\n    # Get organization details\n    flowplane org get acme-corp\n\n    # Get with YAML output\n    flowplane org get acme-corp -o yaml"
    )]
    Get {
        /// Organization name or ID
        #[arg(value_name = "NAME")]
        name: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Create a new organization
    #[command(
        long_about = "Create a new organization.\n\nRequires platform-admin privileges.",
        after_help = "EXAMPLES:\n    # Create from JSON file\n    flowplane org create -f org-spec.json\n\n    # Create with JSON output\n    flowplane org create -f org.json -o json"
    )]
    Create {
        /// Path to YAML or JSON file with resource spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json or yaml)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml"])]
        output: String,
    },

    /// Delete an organization
    #[command(
        long_about = "Delete an organization from the platform.\n\nWARNING: This will fail if the organization has teams or resources.",
        after_help = "EXAMPLES:\n    # Delete (with confirmation)\n    flowplane org delete acme-corp\n\n    # Delete without confirmation\n    flowplane org delete acme-corp --yes"
    )]
    Delete {
        /// Organization name or ID
        #[arg(value_name = "NAME")]
        name: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// List members of an organization
    #[command(
        long_about = "List all members in an organization.\n\nShows user details and their roles within the organization.",
        after_help = "EXAMPLES:\n    # List members\n    flowplane org members acme-corp\n\n    # List as JSON\n    flowplane org members acme-corp -o json"
    )]
    Members {
        /// Organization name or ID
        #[arg(value_name = "NAME")]
        name: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },
}

/// Organization response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgResponse {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// List organizations response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListOrgsResponse {
    pub items: Vec<OrgResponse>,
}

/// Org member response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgMemberResponse {
    pub user_id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// List org members response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListOrgMembersResponse {
    pub members: Vec<OrgMemberResponse>,
}

/// Handle org commands
pub async fn handle_org_command(command: OrgCommands, client: &FlowplaneClient) -> Result<()> {
    match command {
        OrgCommands::List { output } => list_orgs(client, &output).await?,
        OrgCommands::Get { name, output } => get_org(client, &name, &output).await?,
        OrgCommands::Create { file, output } => create_org(client, file, &output).await?,
        OrgCommands::Delete { name, yes } => delete_org(client, &name, yes).await?,
        OrgCommands::Members { name, output } => list_org_members(client, &name, &output).await?,
    }
    Ok(())
}

async fn list_orgs(client: &FlowplaneClient, output: &str) -> Result<()> {
    let response: ListOrgsResponse = client.get_json("/api/v1/admin/organizations").await?;

    if output == "table" {
        print_orgs_table(&response.items);
    } else {
        print_output(&response, output)?;
    }
    Ok(())
}

async fn get_org(client: &FlowplaneClient, name: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/admin/organizations/{name}");
    let response: OrgResponse = client.get_json(&path).await?;

    if output == "table" {
        print_orgs_table(&[response]);
    } else {
        print_output(&response, output)?;
    }
    Ok(())
}

async fn create_org(client: &FlowplaneClient, file: PathBuf, output: &str) -> Result<()> {
    let mut body = config_file::load_config_file(&file)?;
    config_file::strip_kind_field(&mut body);

    let response: OrgResponse = client.post_json("/api/v1/admin/organizations", &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn delete_org(client: &FlowplaneClient, name: &str, yes: bool) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete organization '{}'? (y/N)", name);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/admin/organizations/{name}");
    client.delete_no_content(&path).await?;

    println!("Organization '{}' deleted successfully", name);
    Ok(())
}

async fn list_org_members(client: &FlowplaneClient, name: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/admin/organizations/{name}/members");
    let response: ListOrgMembersResponse = client.get_json(&path).await?;

    if output == "table" {
        print_members_table(&response.members);
    } else {
        print_output(&response, output)?;
    }
    Ok(())
}

fn print_orgs_table(orgs: &[OrgResponse]) {
    if orgs.is_empty() {
        println!("No organizations found");
        return;
    }

    println!();
    println!("{:<20} {:<30} {:<12} {:<25}", "Name", "Display Name", "Status", "Created At");
    println!("{}", "-".repeat(90));

    for org in orgs {
        println!(
            "{:<20} {:<30} {:<12} {:<25}",
            truncate(&org.name, 18),
            truncate(org.display_name.as_deref().unwrap_or("-"), 28),
            org.status.as_deref().unwrap_or("-"),
            org.created_at
                .as_deref()
                .map(|s| s.chars().take(19).collect::<String>())
                .unwrap_or_else(|| "-".to_string()),
        );
    }
    println!();
}

fn print_members_table(members: &[OrgMemberResponse]) {
    if members.is_empty() {
        println!("No members found");
        return;
    }

    println!();
    println!("{:<38} {:<30} {:<30} {:<15}", "User ID", "Email", "Display Name", "Role");
    println!("{}", "-".repeat(115));

    for member in members {
        println!(
            "{:<38} {:<30} {:<30} {:<15}",
            truncate(&member.user_id, 36),
            truncate(member.email.as_deref().unwrap_or("-"), 28),
            truncate(member.display_name.as_deref().unwrap_or("-"), 28),
            member.role.as_deref().unwrap_or("-"),
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
