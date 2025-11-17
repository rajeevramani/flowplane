//! Team CLI commands
//!
//! Provides command-line interface for managing teams

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
        after_help = "EXAMPLES:\n    # Create a team from JSON file\n    flowplane-cli team create --file team-spec.json\n\n    # Create with YAML output\n    flowplane-cli team create --file team.json --output yaml\n\n    # With authentication\n    flowplane-cli team create --file team.json --token your-admin-token"
    )]
    Create {
        /// Path to JSON file with team spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// List all teams
    #[command(
        long_about = "List all teams in the system with optional filtering and pagination.",
        after_help = "EXAMPLES:\n    # List all teams\n    flowplane-cli team list\n\n    # List with table output\n    flowplane-cli team list --output table\n\n    # Paginate results\n    flowplane-cli team list --limit 10 --offset 20"
    )]
    List {
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

    /// Get details of a specific team by ID
    #[command(
        long_about = "Retrieve detailed information about a specific team.",
        after_help = "EXAMPLES:\n    # Get team details\n    flowplane-cli team get <team-id>\n\n    # Get with YAML output\n    flowplane-cli team get <team-id> --output yaml\n\n    # Get with table output\n    flowplane-cli team get <team-id> --output table"
    )]
    Get {
        /// Team ID
        #[arg(value_name = "ID")]
        id: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Update an existing team
    #[command(
        long_about = "Update an existing team with new settings.\n\nProvide a JSON file with the updated team fields.",
        after_help = "EXAMPLES:\n    # Update a team\n    flowplane-cli team update <team-id> --file updated-team.json\n\n    # Update with YAML output\n    flowplane-cli team update <team-id> --file update.json --output yaml"
    )]
    Update {
        /// Team ID
        #[arg(value_name = "ID")]
        id: String,

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
        after_help = "EXAMPLES:\n    # Delete a team (with confirmation prompt)\n    flowplane-cli team delete <team-id>\n\n    # Delete without confirmation\n    flowplane-cli team delete <team-id> --yes"
    )]
    Delete {
        /// Team ID
        #[arg(value_name = "ID")]
        id: String,

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
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

/// List teams response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTeamsResponse {
    pub teams: Vec<TeamResponse>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// Handle team commands
pub async fn handle_team_command(command: TeamCommands, client: &FlowplaneClient) -> Result<()> {
    match command {
        TeamCommands::Create { file, output } => create_team(client, file, &output).await?,
        TeamCommands::List { limit, offset, output } => {
            list_teams(client, limit, offset, &output).await?
        }
        TeamCommands::Get { id, output } => get_team(client, &id, &output).await?,
        TeamCommands::Update { id, file, output } => {
            update_team(client, &id, file, &output).await?
        }
        TeamCommands::Delete { id, yes } => delete_team(client, &id, yes).await?,
    }

    Ok(())
}

async fn create_team(client: &FlowplaneClient, file: PathBuf, output: &str) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse JSON from file")?;

    let response: TeamResponse = client.post_json("/api/v1/admin/teams", &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn list_teams(
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

    let response: ListTeamsResponse = client.get_json(&url).await?;

    match output {
        "table" => {
            println!();
            println!(
                "{:<10} {:<20} {:<30} {:<12} {:<25}",
                "ID", "Name", "Display Name", "Status", "Created At"
            );
            println!("{}", "-".repeat(100));

            let teams_count = response.teams.len();
            for team in &response.teams {
                println!(
                    "{:<10} {:<20} {:<30} {:<12} {:<25}",
                    truncate_string(&team.id, 8),
                    truncate_string(&team.name, 18),
                    truncate_string(&team.display_name, 28),
                    team.status,
                    team.created_at.chars().take(19).collect::<String>()
                );
            }

            println!();
            println!(
                "Total: {} | Showing {} teams (offset: {})",
                response.total, teams_count, response.offset
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

async fn get_team(client: &FlowplaneClient, id: &str, output: &str) -> Result<()> {
    let url = format!("/api/v1/admin/teams/{}", id);
    let response: TeamResponse = client.get_json(&url).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn update_team(
    client: &FlowplaneClient,
    id: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse JSON from file")?;

    let url = format!("/api/v1/admin/teams/{}", id);
    let response: TeamResponse = client.put_json(&url, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn delete_team(client: &FlowplaneClient, id: &str, yes: bool) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete team '{}'? (y/N)", id);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).context("Failed to read user input")?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Deletion cancelled");
            return Ok(());
        }
    }

    let url = format!("/api/v1/admin/teams/{}", id);

    // For DELETE requests that return 204 No Content, we need to send the request directly
    client.delete(&url).send().await.context("Failed to delete team")?;

    println!("Team '{}' deleted successfully", id);
    Ok(())
}

fn print_output<T: Serialize>(data: &T, output: &str) -> Result<()> {
    match output {
        "yaml" => {
            let yaml =
                serde_yaml::to_string(data).context("Failed to serialize response to YAML")?;
            println!("{}", yaml);
        }
        "table" => {
            // For table output, convert to JSON first then format
            let json = serde_json::to_string_pretty(data)
                .context("Failed to serialize response to JSON")?;
            println!("{}", json);
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
