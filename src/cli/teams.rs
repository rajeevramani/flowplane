//! Team CLI commands
//!
//! Provides command-line interface for managing teams.
//! Uses org-scoped endpoints (/api/v1/orgs/{org}/teams) by default.
//! Falls back to admin endpoints (/api/v1/admin/teams) when --admin flag is set.

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::FlowplaneClient;

#[derive(Subcommand)]
pub enum TeamCommands {
    /// Create a new team
    #[command(
        long_about = "Create a new team for multi-tenant resource isolation.\n\nTeams provide boundaries for resources and user access control.",
        after_help = "EXAMPLES:\n    # Create a team from JSON file\n    flowplane-cli team create --org acme-corp --file team-spec.json\n\n    # Create with YAML output\n    flowplane-cli team create --org acme-corp --file team.json --output yaml"
    )]
    Create {
        /// Organization name
        #[arg(long)]
        org: String,

        /// Path to JSON file with team spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// List teams in an organization
    #[command(
        long_about = "List all teams in an organization.\n\nRequires org-admin or platform-admin privileges.",
        after_help = "EXAMPLES:\n    # List teams in an org\n    flowplane-cli team list --org acme-corp\n\n    # List with table output\n    flowplane-cli team list --org acme-corp --output table\n\n    # Platform admin: list all teams across orgs\n    flowplane-cli team list --admin"
    )]
    List {
        /// Organization name
        #[arg(long, required_unless_present = "admin")]
        org: Option<String>,

        /// Use platform admin endpoint (list all teams across orgs)
        #[arg(long, default_value = "false")]
        admin: bool,

        /// Maximum number of results
        #[arg(long, value_name = "NUMBER")]
        limit: Option<i32>,

        /// Offset for pagination
        #[arg(long, value_name = "NUMBER")]
        offset: Option<i32>,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Get details of a specific team
    #[command(
        long_about = "Retrieve detailed information about a specific team.",
        after_help = "EXAMPLES:\n    # Get team details\n    flowplane-cli team get --org acme-corp engineering\n\n    # Get with YAML output\n    flowplane-cli team get --org acme-corp engineering --output yaml"
    )]
    Get {
        /// Organization name
        #[arg(long)]
        org: String,

        /// Team name
        #[arg(value_name = "TEAM")]
        name: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Update an existing team
    #[command(
        long_about = "Update an existing team with new settings.\n\nProvide a JSON file with the updated team fields.",
        after_help = "EXAMPLES:\n    # Update a team\n    flowplane-cli team update --org acme-corp engineering --file updated-team.json"
    )]
    Update {
        /// Organization name
        #[arg(long)]
        org: String,

        /// Team name
        #[arg(value_name = "TEAM")]
        name: String,

        /// Path to JSON file with update spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Delete a team
    #[command(
        long_about = "Delete a specific team from the system.\n\nWARNING: This will fail if the team owns any resources due to foreign key constraints.",
        after_help = "EXAMPLES:\n    # Delete a team (with confirmation prompt)\n    flowplane-cli team delete --org acme-corp engineering\n\n    # Delete without confirmation\n    flowplane-cli team delete --org acme-corp engineering --yes"
    )]
    Delete {
        /// Organization name
        #[arg(long)]
        org: String,

        /// Team name
        #[arg(value_name = "TEAM")]
        name: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

/// Team response structure
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamResponse {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub owner_user_id: Option<String>,
    #[serde(default)]
    pub org_id: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

/// List teams response (org-scoped)
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListOrgTeamsResponse {
    pub teams: Vec<TeamResponse>,
}

/// List teams response (admin)
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAdminTeamsResponse {
    pub teams: Vec<TeamResponse>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// Handle team commands
pub async fn handle_team_command(command: TeamCommands, client: &FlowplaneClient) -> Result<()> {
    match command {
        TeamCommands::Create { org, file, output } => {
            create_team(client, &org, file, &output).await?
        }
        TeamCommands::List { org, admin, limit, offset, output } => {
            if admin {
                list_teams_admin(client, limit, offset, &output).await?
            } else {
                let org = org.as_deref().expect("--org is required unless --admin is set");
                list_teams_org(client, org, &output).await?
            }
        }
        TeamCommands::Get { org, name, output } => get_team(client, &org, &name, &output).await?,
        TeamCommands::Update { org, name, file, output } => {
            update_team(client, &org, &name, file, &output).await?
        }
        TeamCommands::Delete { org, name, yes } => delete_team(client, &org, &name, yes).await?,
    }

    Ok(())
}

async fn create_team(
    client: &FlowplaneClient,
    org: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse JSON from file")?;

    let url = format!("/api/v1/orgs/{}/teams", org);
    let response: TeamResponse = client.post_json(&url, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn list_teams_org(client: &FlowplaneClient, org: &str, output: &str) -> Result<()> {
    let url = format!("/api/v1/orgs/{}/teams", org);
    let response: ListOrgTeamsResponse = client.get_json(&url).await?;

    match output {
        "table" => print_teams_table(&response.teams),
        "yaml" => {
            let yaml =
                serde_yaml::to_string(&response).context("Failed to serialize response to YAML")?;
            println!("{}", yaml);
        }
        _ => {
            let json = serde_json::to_string_pretty(&response)
                .context("Failed to serialize response to JSON")?;
            println!("{}", json);
        }
    }

    Ok(())
}

async fn list_teams_admin(
    client: &FlowplaneClient,
    limit: Option<i32>,
    offset: Option<i32>,
    output: &str,
) -> Result<()> {
    let mut url = "/api/v1/admin/teams?".to_string();

    if let Some(l) = limit {
        url.push_str(&format!("limit={}&", l));
    }
    if let Some(o) = offset {
        url.push_str(&format!("offset={}&", o));
    }

    let response: ListAdminTeamsResponse = client.get_json(&url).await?;

    match output {
        "table" => {
            print_teams_table(&response.teams);
            println!(
                "Total: {} | Showing {} teams (offset: {})",
                response.total,
                response.teams.len(),
                response.offset
            );
        }
        "yaml" => {
            let yaml =
                serde_yaml::to_string(&response).context("Failed to serialize response to YAML")?;
            println!("{}", yaml);
        }
        _ => {
            let json = serde_json::to_string_pretty(&response)
                .context("Failed to serialize response to JSON")?;
            println!("{}", json);
        }
    }

    Ok(())
}

fn print_teams_table(teams: &[TeamResponse]) {
    println!();
    println!("{:<20} {:<30} {:<12} {:<25}", "Name", "Display Name", "Status", "Created At");
    println!("{}", "-".repeat(90));

    for team in teams {
        println!(
            "{:<20} {:<30} {:<12} {:<25}",
            truncate_string(&team.name, 18),
            truncate_string(&team.display_name, 28),
            team.status,
            team.created_at.chars().take(19).collect::<String>()
        );
    }
    println!();
}

async fn get_team(client: &FlowplaneClient, org: &str, name: &str, output: &str) -> Result<()> {
    // Use the org-scoped list endpoint and filter — there's no single-team GET on the org endpoint
    let url = format!("/api/v1/orgs/{}/teams", org);
    let response: ListOrgTeamsResponse = client.get_json(&url).await?;

    let team = response
        .teams
        .into_iter()
        .find(|t| t.name == name)
        .ok_or_else(|| anyhow::anyhow!("Team '{}' not found in org '{}'", name, org))?;

    print_output(&team, output)?;
    Ok(())
}

async fn update_team(
    client: &FlowplaneClient,
    org: &str,
    name: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse JSON from file")?;

    let url = format!("/api/v1/orgs/{}/teams/{}", org, name);
    let response: TeamResponse = client.put_json(&url, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn delete_team(client: &FlowplaneClient, org: &str, name: &str, yes: bool) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete team '{}' from org '{}'? (y/N)", name, org);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).context("Failed to read user input")?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Deletion cancelled");
            return Ok(());
        }
    }

    let url = format!("/api/v1/orgs/{}/teams/{}", org, name);
    client.delete(&url).send().await.context("Failed to delete team")?;

    println!("Team '{}' deleted successfully from org '{}'", name, org);
    Ok(())
}

fn print_output<T: Serialize>(data: &T, output: &str) -> Result<()> {
    match output {
        "yaml" => {
            let yaml =
                serde_yaml::to_string(data).context("Failed to serialize response to YAML")?;
            println!("{}", yaml);
        }
        _ => {
            let json = serde_json::to_string_pretty(data)
                .context("Failed to serialize response to JSON")?;
            println!("{}", json);
        }
    }
    Ok(())
}

fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}
