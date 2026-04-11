//! Trace CLI command
//!
//! Traces a request path through the gateway configuration to show how it
//! would be routed: listener -> route config -> virtual host -> route -> cluster.

use anyhow::Result;
use clap::Args;

use super::client::FlowplaneClient;
use super::output::{print_output, truncate};

#[derive(Args)]
#[command(
    about = "Trace a request path through the gateway",
    long_about = "Trace how a request path resolves through the gateway configuration.\n\nShows each resolution step: listener -> route config -> virtual host -> route -> cluster.",
    after_help = "EXAMPLES:\n    flowplane trace /api/v1/users\n    flowplane trace /api/v1/users --port 10000\n    flowplane trace /healthz -o json"
)]
pub struct TraceArgs {
    /// Request path to trace (e.g., /api/v1/users)
    #[arg(value_name = "PATH")]
    pub path: String,

    /// Listener port to trace against
    #[arg(long)]
    pub port: Option<i64>,

    /// Output format
    #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
    pub output: String,
}

pub async fn handle_trace_command(
    args: TraceArgs,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    let mut path = format!("/api/v1/teams/{team}/ops/trace?path={}", args.path);
    if let Some(port) = args.port {
        path.push_str(&format!("&port={port}"));
    }

    let response: serde_json::Value = client.get_json(&path).await?;

    if args.output == "table" {
        print_trace_table(&response);
    } else {
        print_output(&response, &args.output)?;
    }

    // Exit code 1 if no matches
    let match_count = response
        .get("match_count")
        .or_else(|| response.get("matchCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if match_count == 0 {
        std::process::exit(1);
    }

    Ok(())
}

fn print_trace_table(data: &serde_json::Value) {
    let path = data.get("path").and_then(|v| v.as_str()).unwrap_or("?");
    let port = data.get("port").and_then(|v| v.as_i64());
    let message = data.get("message").and_then(|v| v.as_str()).unwrap_or("");
    let match_count = data
        .get("match_count")
        .or_else(|| data.get("matchCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    println!();
    if let Some(p) = port {
        println!("Trace: {} (port {})", path, p);
    } else {
        println!("Trace: {}", path);
    }
    println!("{}", "-".repeat(80));

    if match_count == 0 {
        let reason = data
            .get("unmatched_reason")
            .or_else(|| data.get("unmatchedReason"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        println!("  NO MATCH: {}", reason);
        println!();
        return;
    }

    // Print matches as resolution steps
    if let Some(matches) = data.get("matches").and_then(|v| v.as_array()) {
        println!("  {:<15} {:<25} {:<35}", "Step", "Resource", "Value");
        println!("  {}", "-".repeat(75));

        for m in matches {
            if let Some(obj) = m.as_object() {
                for (key, value) in obj {
                    let display = match value {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    println!(
                        "  {:<15} {:<25} {:<35}",
                        "",
                        truncate(key, 23),
                        truncate(&display, 33),
                    );
                }
            }
        }
    }

    // Print endpoints if present
    if let Some(endpoints) = data.get("endpoints").and_then(|v| v.as_array()) {
        if !endpoints.is_empty() {
            println!();
            println!("  Endpoints:");
            for ep in endpoints {
                let display = match ep {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                println!("    - {}", display);
            }
        }
    }

    println!();
    println!("  {}", message);
    println!();
}
