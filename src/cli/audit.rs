//! Audit log CLI commands
//!
//! Provides command-line interface for viewing the audit trail of resource changes.

use anyhow::Result;
use clap::Subcommand;

use super::client::FlowplaneClient;
use super::output::{print_output, print_table_header, truncate};

#[derive(Subcommand)]
pub enum AuditCommands {
    /// View the audit log of resource changes
    #[command(
        name = "list",
        long_about = "View the audit trail of resource changes for the current team.\n\n\
            Shows who changed what and when, with optional filters by resource type,\n\
            action, and time range.",
        after_help = "EXAMPLES:\n    flowplane audit list\n    flowplane audit list --resource-type cluster --action create\n    flowplane audit list --since 2026-04-01T00:00:00Z --limit 50\n    flowplane audit list -o json"
    )]
    List {
        /// Filter by resource type (e.g., cluster, listener, route)
        #[arg(long)]
        resource_type: Option<String>,

        /// Filter by action
        #[arg(long, value_parser = ["create", "update", "delete"])]
        action: Option<String>,

        /// Only show entries after this timestamp (ISO 8601)
        #[arg(long)]
        since: Option<String>,

        /// Maximum number of results (default: 20)
        #[arg(long, default_value = "20")]
        limit: i32,

        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },
}

pub async fn handle_audit_command(
    command: AuditCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        AuditCommands::List { resource_type, action, since, limit, output } => {
            let mut params = vec![format!("limit={limit}")];

            if let Some(rt) = &resource_type {
                params.push(format!("resource_type={rt}"));
            }
            if let Some(a) = &action {
                params.push(format!("action={a}"));
            }
            if let Some(s) = &since {
                params.push(format!("since={s}"));
            }

            let path = format!("/api/v1/teams/{team}/ops/audit?{}", params.join("&"));
            let response: serde_json::Value = client.get_json(&path).await?;

            if output == "table" {
                print_audit_table(&response);
            } else {
                print_output(&response, &output)?;
            }
        }
    }

    Ok(())
}

fn print_audit_table(data: &serde_json::Value) {
    let items = if let Some(arr) = data.get("entries").and_then(|v| v.as_array()) {
        arr.clone()
    } else if let Some(arr) = data.as_array() {
        arr.clone()
    } else {
        vec![data.clone()]
    };

    if items.is_empty() {
        println!("No audit entries found");
        return;
    }

    print_table_header(&[
        ("Timestamp", 25),
        ("User", 20),
        ("Action", 10),
        ("Resource Type", 18),
        ("Resource Name", 30),
    ]);

    for item in &items {
        let timestamp = item
            .get("timestamp")
            .or_else(|| item.get("createdAt"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let user =
            item.get("user").or_else(|| item.get("userId")).and_then(|v| v.as_str()).unwrap_or("-");
        let action = item.get("action").and_then(|v| v.as_str()).unwrap_or("-");
        let resource_type = item
            .get("resourceType")
            .or_else(|| item.get("resource_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let resource_name = item
            .get("resourceName")
            .or_else(|| item.get("resource_name"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");

        println!(
            "{:<25} {:<20} {:<10} {:<18} {}",
            truncate(timestamp, 23),
            truncate(user, 18),
            action,
            truncate(resource_type, 16),
            truncate(resource_name, 28),
        );
    }
    println!();
}
