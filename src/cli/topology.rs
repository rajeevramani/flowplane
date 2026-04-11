//! Topology CLI command
//!
//! Displays the gateway configuration topology: listeners, route configs,
//! virtual hosts, routes, and clusters in a tree structure.

use anyhow::Result;
use clap::Args;

use super::client::FlowplaneClient;
use super::output::print_output;

#[derive(Args)]
#[command(
    about = "Show gateway configuration topology",
    long_about = "Display the gateway configuration as a topology tree.\n\nShows listeners -> route configs -> clusters with orphaned resource detection.",
    after_help = "EXAMPLES:\n    flowplane topology\n    flowplane topology -o json\n    flowplane topology -o yaml"
)]
pub struct TopologyArgs {
    /// Output format
    #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
    pub output: String,
}

pub async fn handle_topology_command(
    args: TopologyArgs,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/ops/topology");
    let response: serde_json::Value = client.get_json(&path).await?;

    if args.output == "table" {
        print_topology_tree(&response);
    } else {
        print_output(&response, &args.output)?;
    }

    Ok(())
}

fn print_topology_tree(data: &serde_json::Value) {
    println!();
    println!("Gateway Topology");
    println!("{}", "=".repeat(60));

    // Try to render rows as a tree structure
    if let Some(rows) = data.get("rows").and_then(|v| v.as_array()) {
        if rows.is_empty() {
            println!("  (no resources found)");
            println!();
            return;
        }

        for row in rows {
            print_topology_node(row, 0);
        }
    } else if let Some(obj) = data.as_object() {
        // Flat object — print key/value pairs
        for (key, value) in obj {
            if key == "rows" {
                continue;
            }
            let display = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            println!("  {}: {}", key, display);
        }
    }

    // Flag orphaned resources if present
    if let Some(orphans) = data.get("orphaned").and_then(|v| v.as_array()) {
        if !orphans.is_empty() {
            println!();
            println!("  ORPHANED RESOURCES:");
            for orphan in orphans {
                let display = match orphan {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Object(obj) => {
                        let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                        let kind = obj
                            .get("type")
                            .or_else(|| obj.get("kind"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("resource");
                        format!("[{}] {}", kind, name)
                    }
                    other => other.to_string(),
                };
                println!("    ! {}", display);
            }
        }
    }

    println!();
}

fn print_topology_node(node: &serde_json::Value, depth: usize) {
    let indent = "  ".repeat(depth + 1);
    let prefix = if depth == 0 { "" } else { "├── " };

    match node {
        serde_json::Value::Object(obj) => {
            // Try to extract a meaningful label
            let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let kind =
                obj.get("type").or_else(|| obj.get("kind")).and_then(|v| v.as_str()).unwrap_or("");

            if !name.is_empty() || !kind.is_empty() {
                if !kind.is_empty() {
                    println!("{indent}{prefix}[{kind}] {name}");
                } else {
                    println!("{indent}{prefix}{name}");
                }
            }

            // Recurse into children arrays
            for (key, value) in obj {
                if key == "name" || key == "type" || key == "kind" {
                    continue;
                }
                if let Some(children) = value.as_array() {
                    for child in children {
                        print_topology_node(child, depth + 1);
                    }
                }
            }
        }
        serde_json::Value::String(s) => {
            println!("{indent}{prefix}{s}");
        }
        other => {
            println!("{indent}{prefix}{other}");
        }
    }
}
