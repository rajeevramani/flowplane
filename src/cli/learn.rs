//! Learning session CLI commands
//!
//! Provides command-line interface for managing API learning sessions

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use super::client::FlowplaneClient;

#[derive(Subcommand)]
pub enum LearnCommands {
    /// Start a new learning session to record API traffic
    #[command(
        long_about = "Start a new learning session that records API traffic patterns.\n\nLearning sessions observe traffic matching a route pattern and collect samples\nto reverse-engineer API schemas.",
        after_help = "EXAMPLES:\n    # Start learning all /api/v2 traffic with 500 samples\n    flowplane learn start --route-pattern '^/api/v2/.*' --target-sample-count 500\n\n    # Start learning with cluster filter and HTTP method filter\n    flowplane learn start --route-pattern '^/api/v1/users/.*' --cluster-name users-api \\\n        --http-methods GET POST --target-sample-count 1000\n\n    # Start with a max duration of 2 hours\n    flowplane learn start --route-pattern '^/api/.*' --target-sample-count 500 \\\n        --max-duration-seconds 7200 --triggered-by 'deploy-v2.3'"
    )]
    Start {
        /// Route pattern (regex) to match for learning
        #[arg(long)]
        route_pattern: String,

        /// Cluster name to filter traffic
        #[arg(long)]
        cluster_name: Option<String>,

        /// HTTP methods to filter (e.g., GET POST PUT)
        #[arg(long, num_args = 1..)]
        http_methods: Option<Vec<String>>,

        /// Target number of samples to collect
        #[arg(long)]
        target_sample_count: i64,

        /// Maximum duration in seconds
        #[arg(long)]
        max_duration_seconds: Option<i64>,

        /// Who or what triggered this session
        #[arg(long)]
        triggered_by: Option<String>,

        /// Deployment version being learned
        #[arg(long)]
        deployment_version: Option<String>,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Cancel an active learning session
    #[command(
        long_about = "Cancel an active or pending learning session.\n\nOnly sessions in pending, active, or completing states can be cancelled.\nCompleted, cancelled, or failed sessions cannot be cancelled.",
        after_help = "EXAMPLES:\n    # Cancel a session by ID\n    flowplane learn cancel abc-123-def\n\n    # Cancel without confirmation\n    flowplane learn cancel abc-123-def --yes"
    )]
    Cancel {
        /// Session ID to cancel
        #[arg(value_name = "SESSION_ID")]
        session_id: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// List learning sessions
    #[command(
        long_about = "List learning sessions with optional filtering by status.\n\nShows all learning sessions for the current team, with pagination support.",
        after_help = "EXAMPLES:\n    # List all sessions\n    flowplane learn list\n\n    # List only active sessions\n    flowplane learn list --status active\n\n    # List with table output\n    flowplane learn list --output table\n\n    # Paginate results\n    flowplane learn list --limit 10 --offset 20"
    )]
    List {
        /// Filter by status (pending, active, completing, completed, failed, cancelled)
        #[arg(long)]
        status: Option<String>,

        /// Maximum number of results
        #[arg(long)]
        limit: Option<i32>,

        /// Offset for pagination
        #[arg(long)]
        offset: Option<i32>,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Get details of a learning session
    #[command(
        long_about = "Retrieve detailed information about a specific learning session by ID.\n\nShows route pattern, status, sample counts, progress, and timing information.",
        after_help = "EXAMPLES:\n    # Get session details in JSON\n    flowplane learn get abc-123-def\n\n    # Get in table format\n    flowplane learn get abc-123-def --output table"
    )]
    Get {
        /// Session ID to retrieve
        #[arg(value_name = "SESSION_ID")]
        session_id: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml", "table"])]
        output: String,
    },
}

/// Learning session response matching the API response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearningSessionResponse {
    pub id: String,
    pub team: String,
    pub route_pattern: String,
    pub cluster_name: Option<String>,
    pub http_methods: Option<Vec<String>>,
    pub status: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub ends_at: Option<String>,
    pub completed_at: Option<String>,
    pub target_sample_count: i64,
    pub current_sample_count: i64,
    pub progress_percentage: f64,
    pub triggered_by: Option<String>,
    pub deployment_version: Option<String>,
    pub error_message: Option<String>,
}

/// Request body for creating a learning session
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateLearningSessionRequest {
    route_pattern: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cluster_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    http_methods: Option<Vec<String>>,
    target_sample_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_duration_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    triggered_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deployment_version: Option<String>,
}

/// Handle learn commands
pub async fn handle_learn_command(
    command: LearnCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        LearnCommands::Start {
            route_pattern,
            cluster_name,
            http_methods,
            target_sample_count,
            max_duration_seconds,
            triggered_by,
            deployment_version,
            output,
        } => {
            start_session(
                client,
                team,
                route_pattern,
                cluster_name,
                http_methods,
                target_sample_count,
                max_duration_seconds,
                triggered_by,
                deployment_version,
                &output,
            )
            .await?
        }
        LearnCommands::Cancel { session_id, yes } => {
            cancel_session(client, team, &session_id, yes).await?
        }
        LearnCommands::List { status, limit, offset, output } => {
            list_sessions(client, team, status, limit, offset, &output).await?
        }
        LearnCommands::Get { session_id, output } => {
            get_session(client, team, &session_id, &output).await?
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn start_session(
    client: &FlowplaneClient,
    team: &str,
    route_pattern: String,
    cluster_name: Option<String>,
    http_methods: Option<Vec<String>>,
    target_sample_count: i64,
    max_duration_seconds: Option<i64>,
    triggered_by: Option<String>,
    deployment_version: Option<String>,
    output: &str,
) -> Result<()> {
    let body = CreateLearningSessionRequest {
        route_pattern,
        cluster_name,
        http_methods,
        target_sample_count,
        max_duration_seconds,
        triggered_by,
        deployment_version,
    };

    let path = format!("/api/v1/teams/{team}/learning-sessions");
    let response: LearningSessionResponse = client.post_json(&path, &body).await?;

    if output == "table" {
        print_sessions_table(&[response]);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn cancel_session(
    client: &FlowplaneClient,
    team: &str,
    session_id: &str,
    yes: bool,
) -> Result<()> {
    if !yes {
        println!("Are you sure you want to cancel learning session '{}'? (y/N)", session_id);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let path = format!("/api/v1/teams/{team}/learning-sessions/{session_id}");
    client.delete_no_content(&path).await?;

    println!("Learning session '{}' cancelled successfully", session_id);
    Ok(())
}

async fn list_sessions(
    client: &FlowplaneClient,
    team: &str,
    status: Option<String>,
    limit: Option<i32>,
    offset: Option<i32>,
    output: &str,
) -> Result<()> {
    let mut path = format!("/api/v1/teams/{team}/learning-sessions?");
    let mut params = Vec::new();

    if let Some(s) = status {
        params.push(format!("status={s}"));
    }
    if let Some(l) = limit {
        params.push(format!("limit={l}"));
    }
    if let Some(o) = offset {
        params.push(format!("offset={o}"));
    }

    path.push_str(&params.join("&"));

    let response: Vec<LearningSessionResponse> = client.get_json(&path).await?;

    if output == "table" {
        print_sessions_table(&response);
    } else {
        print_output(&response, output)?;
    }

    Ok(())
}

async fn get_session(
    client: &FlowplaneClient,
    team: &str,
    session_id: &str,
    output: &str,
) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/learning-sessions/{session_id}");
    let response: LearningSessionResponse = client.get_json(&path).await?;

    if output == "table" {
        print_sessions_table(&[response]);
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

fn print_sessions_table(sessions: &[LearningSessionResponse]) {
    if sessions.is_empty() {
        println!("No learning sessions found");
        return;
    }

    println!();
    println!(
        "{:<38} {:<12} {:<30} {:<8} {:<8} {:<8}",
        "ID", "Status", "Route Pattern", "Samples", "Target", "Progress"
    );
    println!("{}", "-".repeat(110));

    for session in sessions {
        println!(
            "{:<38} {:<12} {:<30} {:<8} {:<8} {:.1}%",
            truncate(&session.id, 36),
            session.status,
            truncate(&session.route_pattern, 28),
            session.current_sample_count,
            session.target_sample_count,
            session.progress_percentage,
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
