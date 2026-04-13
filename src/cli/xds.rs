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
    let dataplanes = data
        .get("dataplanes")
        .and_then(|v| v.as_array())
        .cloned()
        .or_else(|| data.as_array().cloned())
        .unwrap_or_default();

    if dataplanes.is_empty() {
        println!("No xDS status entries found");
        return;
    }

    print_table_header(&[
        ("Dataplane", 25),
        ("Agent", 14),
        ("Last Verify", 22),
        ("Type", 6),
        ("Status", 10),
        ("Version", 12),
        ("Error", 28),
    ]);

    for dp in &dataplanes {
        let dataplane = dp.get("name").and_then(|v| v.as_str()).unwrap_or("-");
        let agent_status = dp
            .get("agent_status")
            .or_else(|| dp.get("agentStatus"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let last_verify = dp
            .get("last_config_verify")
            .or_else(|| dp.get("lastConfigVerify"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");

        if let Some(resource_types) = dp.get("resource_types").and_then(|v| v.as_object()) {
            let mut types: Vec<_> = resource_types.keys().collect();
            types.sort();
            for xds_type in types {
                let info = &resource_types[xds_type];
                let status = info.get("status").and_then(|v| v.as_str()).unwrap_or("-");
                let version = info.get("version").and_then(|v| v.as_str()).unwrap_or("-");
                let error = info
                    .get("error")
                    .or_else(|| info.get("errorMessage"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                println!(
                    "{:<25} {:<14} {:<22} {:<6} {:<10} {:<12} {}",
                    truncate(dataplane, 23),
                    truncate(agent_status, 12),
                    truncate(last_verify, 20),
                    xds_type,
                    status,
                    truncate(version, 10),
                    truncate(error, 26),
                );
            }
        } else {
            let xds_type = dp
                .get("type")
                .or_else(|| dp.get("xdsType"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let status = dp.get("status").and_then(|v| v.as_str()).unwrap_or("-");
            let version = dp.get("version").and_then(|v| v.as_str()).unwrap_or("-");
            let error = dp.get("error").and_then(|v| v.as_str()).unwrap_or("");

            println!(
                "{:<25} {:<14} {:<22} {:<6} {:<10} {:<12} {}",
                truncate(dataplane, 23),
                truncate(agent_status, 12),
                truncate(last_verify, 20),
                xds_type,
                status,
                truncate(version, 10),
                truncate(error, 26),
            );
        }
    }
    if dataplanes
        .iter()
        .any(|dp| dp.get("agent_status").and_then(|v| v.as_str()) == Some("NOT_MONITORED"))
    {
        println!();
        println!(
            "Hint: dataplanes marked NOT_MONITORED have never reported via the \
             flowplane-agent diagnostics service. Install or start the agent on \
             each dataplane to surface warming failures."
        );
    }
    println!();
}

fn print_xds_nacks_table(data: &serde_json::Value) {
    let items = if let Some(arr) = data.get("events").and_then(|v| v.as_array()) {
        arr.clone()
    } else if let Some(arr) = data.as_array() {
        arr.clone()
    } else {
        vec![data.clone()]
    };

    if items.is_empty() {
        println!("No xDS NACKs found");
        return;
    }

    print_table_header(&[
        ("Timestamp", 25),
        ("Source", 16),
        ("Dataplane", 22),
        ("Type", 6),
        ("Version", 12),
        ("Error", 32),
    ]);

    for item in &items {
        let timestamp = item
            .get("timestamp")
            .or_else(|| item.get("createdAt"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let source = item.get("source").and_then(|v| v.as_str()).unwrap_or("stream");
        let dataplane = item
            .get("dataplane_name")
            .or_else(|| item.get("dataplane"))
            .or_else(|| item.get("dataplaneName"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let xds_type = item
            .get("resource_type")
            .or_else(|| item.get("type"))
            .or_else(|| item.get("xdsType"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        // version_rejected may be null for warming_report rows — render as "-"
        let version = item
            .get("version_rejected")
            .or_else(|| item.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let error = item
            .get("error_message")
            .or_else(|| item.get("error"))
            .or_else(|| item.get("errorMessage"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        println!(
            "{:<25} {:<16} {:<22} {:<6} {:<12} {}",
            truncate(timestamp, 23),
            truncate(source, 14),
            truncate(dataplane, 20),
            xds_type,
            truncate(version, 10),
            truncate(error, 30),
        );
    }
    println!();
}
