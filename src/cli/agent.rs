//! Agent CLI commands
//!
//! Provides command-line interface for managing org-scoped agents.

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::client::FlowplaneClient;

#[derive(Subcommand)]
pub enum AgentCommands {
    /// List agents in an organization
    #[command(
        long_about = "List all agents registered in an organization.\n\nAgents are machine identities used for programmatic access.",
        after_help = "EXAMPLES:\n    # List agents in an org\n    flowplane agent list --org acme-corp\n\n    # List as JSON\n    flowplane agent list --org acme-corp -o json"
    )]
    List {
        /// Organization name
        #[arg(long)]
        org: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Create a new agent in an organization
    #[command(
        long_about = "Create a new agent (machine identity) in an organization.\n\nProvide a JSON file with the agent specification.",
        after_help = "EXAMPLES:\n    # Create an agent from spec file\n    flowplane agent create --org acme-corp -f agent-spec.json\n\n    # Create with JSON output\n    flowplane agent create --org acme-corp -f agent.json -o json"
    )]
    Create {
        /// Organization name
        #[arg(long)]
        org: String,

        /// Path to JSON file with agent spec
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Output format (json or yaml)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml"])]
        output: String,
    },

    /// Delete an agent from an organization
    #[command(
        long_about = "Delete an agent from an organization.\n\nWARNING: This will revoke the agent's access immediately.",
        after_help = "EXAMPLES:\n    # Delete an agent (with confirmation)\n    flowplane agent delete --org acme-corp my-agent\n\n    # Delete without confirmation\n    flowplane agent delete --org acme-corp my-agent --yes"
    )]
    Delete {
        /// Organization name
        #[arg(long)]
        org: String,

        /// Agent name
        #[arg(value_name = "NAME")]
        name: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

/// Agent response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResponse {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub org_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// List agents response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentsResponse {
    pub agents: Vec<AgentResponse>,
}

/// Handle agent commands
pub async fn handle_agent_command(command: AgentCommands, client: &FlowplaneClient) -> Result<()> {
    match command {
        AgentCommands::List { org, output } => list_agents(client, &org, &output).await?,
        AgentCommands::Create { org, file, output } => {
            create_agent(client, &org, file, &output).await?
        }
        AgentCommands::Delete { org, name, yes } => delete_agent(client, &org, &name, yes).await?,
    }
    Ok(())
}

async fn list_agents(client: &FlowplaneClient, org: &str, output: &str) -> Result<()> {
    let path = format!("/api/v1/orgs/{org}/agents");
    let response: ListAgentsResponse = client.get_json(&path).await?;

    if output == "table" {
        print_agents_table(&response.agents);
    } else {
        print_output(&response, output)?;
    }
    Ok(())
}

async fn create_agent(
    client: &FlowplaneClient,
    org: &str,
    file: PathBuf,
    output: &str,
) -> Result<()> {
    let contents = std::fs::read_to_string(&file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let body: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse JSON from file")?;

    let path = format!("/api/v1/orgs/{org}/agents");
    let response: AgentResponse = client.post_json(&path, &body).await?;

    print_output(&response, output)?;
    Ok(())
}

async fn delete_agent(client: &FlowplaneClient, org: &str, name: &str, yes: bool) -> Result<()> {
    if !yes {
        println!("Are you sure you want to delete agent '{}' from org '{}'? (y/N)", name, org);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/orgs/{org}/agents/{name}");
    client.delete_no_content(&path).await?;

    println!("Agent '{}' deleted from org '{}'", name, org);
    Ok(())
}

fn print_agents_table(agents: &[AgentResponse]) {
    if agents.is_empty() {
        println!("No agents found");
        return;
    }

    println!();
    println!("{:<25} {:<35} {:<12} {:<25}", "Name", "Description", "Status", "Created At");
    println!("{}", "-".repeat(100));

    for agent in agents {
        println!(
            "{:<25} {:<35} {:<12} {:<25}",
            truncate(&agent.name, 23),
            truncate(agent.description.as_deref().unwrap_or("-"), 33),
            agent.status.as_deref().unwrap_or("-"),
            agent
                .created_at
                .as_deref()
                .map(|s| s.chars().take(19).collect::<String>())
                .unwrap_or_else(|| "-".to_string()),
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
