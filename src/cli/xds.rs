//! xDS status and NACK CLI commands
//!
//! Provides command-line interface for viewing Envoy xDS sync status and NACKs.

use anyhow::Result;
use clap::Subcommand;

use super::client::FlowplaneClient;
use super::output::{print_output, print_table_header, truncate};

#[derive(Subcommand)]
pub enum XdsCommands {
    /// Show xDS sync status for dataplanes
    #[command(
        long_about = "Show the current xDS synchronisation status for each dataplane.\n\n\
            ACK means no NACKs have been recorded — it does not imply a positive\n\
            confirmation was received from Envoy.",
        after_help = "EXAMPLES:\n    flowplane xds status\n    flowplane xds status --dataplane my-dp\n    flowplane xds status -o json"
    )]
    Status {
        /// Filter by dataplane name
        #[arg(long)]
        dataplane: Option<String>,

        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// List xDS NACKs (configuration rejections)
    #[command(
        long_about = "List xDS NACKs — configurations that Envoy rejected.\n\n\
            ACK means no NACKs have been recorded — it does not imply a positive\n\
            confirmation was received from Envoy.",
        after_help = "EXAMPLES:\n    flowplane xds nacks\n    flowplane xds nacks --dataplane my-dp --type CDS\n    flowplane xds nacks --since 2026-04-01T00:00:00Z --limit 50"
    )]
    Nacks {
        /// Filter by dataplane name
        #[arg(long)]
        dataplane: Option<String>,

        /// Filter by xDS type (CDS, RDS, LDS, EDS)
        #[arg(long, value_name = "TYPE", value_parser = ["CDS", "RDS", "LDS", "EDS"])]
        r#type: Option<String>,

        /// Only show NACKs after this timestamp (ISO 8601)
        #[arg(long)]
        since: Option<String>,

        /// Maximum number of results
        #[arg(long)]
        limit: Option<i32>,

        /// Output format
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },
}

pub async fn handle_xds_command(
    command: XdsCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        XdsCommands::Status { dataplane, output } => {
            let mut path = format!("/api/v1/teams/{team}/ops/xds/status");
            if let Some(dp) = &dataplane {
                path.push_str(&format!("?dataplane={dp}"));
            }

            let response: serde_json::Value = client.get_json(&path).await?;

            if output == "table" {
                print_xds_status_table(&response);
            } else {
                print_output(&response, &output)?;
            }
        }
        XdsCommands::Nacks { dataplane, r#type, since, limit, output } => {
            let mut params = Vec::new();
            if let Some(dp) = &dataplane {
                params.push(format!("dataplane={dp}"));
            }
            if let Some(t) = &r#type {
                params.push(format!("type={t}"));
            }
            if let Some(s) = &since {
                params.push(format!("since={s}"));
            }
            if let Some(l) = limit {
                params.push(format!("limit={l}"));
            }

            let mut path = format!("/api/v1/teams/{team}/ops/xds/nacks");
            if !params.is_empty() {
                path.push('?');
                path.push_str(&params.join("&"));
            }

            let response: serde_json::Value = client.get_json(&path).await?;

            if output == "table" {
                print_xds_nacks_table(&response);
            } else {
                print_output(&response, &output)?;
            }
        }
    }

    Ok(())
}

fn print_xds_status_table(data: &serde_json::Value) {
    let items = match data.as_array() {
        Some(arr) => arr.clone(),
        None => vec![data.clone()],
    };

    if items.is_empty() {
        println!("No xDS status entries found");
        return;
    }

    print_table_header(&[
        ("Dataplane", 25),
        ("Type", 6),
        ("Status", 10),
        ("Version", 12),
        ("Error", 40),
    ]);

    for item in &items {
        let dataplane = item.get("dataplane").and_then(|v| v.as_str()).unwrap_or("-");
        let xds_type = item
            .get("type")
            .or_else(|| item.get("xdsType"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("-");
        let version = item.get("version").and_then(|v| v.as_str()).unwrap_or("-");
        let error = item
            .get("error")
            .or_else(|| item.get("errorMessage"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        println!(
            "{:<25} {:<6} {:<10} {:<12} {}",
            truncate(dataplane, 23),
            xds_type,
            status,
            truncate(version, 10),
            truncate(error, 38),
        );
    }
    println!();
}

fn print_xds_nacks_table(data: &serde_json::Value) {
    let items = match data.as_array() {
        Some(arr) => arr.clone(),
        None => vec![data.clone()],
    };

    if items.is_empty() {
        println!("No xDS NACKs found");
        return;
    }

    print_table_header(&[
        ("Timestamp", 25),
        ("Dataplane", 25),
        ("Type", 6),
        ("Version", 12),
        ("Error", 40),
    ]);

    for item in &items {
        let timestamp = item
            .get("timestamp")
            .or_else(|| item.get("createdAt"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let dataplane = item.get("dataplane").and_then(|v| v.as_str()).unwrap_or("-");
        let xds_type = item
            .get("type")
            .or_else(|| item.get("xdsType"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let version = item.get("version").and_then(|v| v.as_str()).unwrap_or("-");
        let error = item
            .get("error")
            .or_else(|| item.get("errorMessage"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        println!(
            "{:<25} {:<25} {:<6} {:<12} {}",
            truncate(timestamp, 23),
            truncate(dataplane, 23),
            xds_type,
            truncate(version, 10),
            truncate(error, 38),
        );
    }
    println!();
}
