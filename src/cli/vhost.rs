//! Virtual host CLI commands
//!
//! Provides command-line interface for listing and inspecting virtual hosts
//! within route configurations.

use anyhow::Result;
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use super::client::FlowplaneClient;
use super::output::{print_output, print_table_header, truncate};

#[derive(Subcommand)]
pub enum VhostCommands {
    /// List virtual hosts for a route configuration
    List {
        /// Route configuration name
        #[arg(long, value_name = "NAME")]
        route_config: String,

        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Get details of a specific virtual host
    Get {
        /// Route configuration name
        #[arg(value_name = "ROUTE_CONFIG")]
        route_config: String,

        /// Virtual host name
        #[arg(value_name = "VHOST_NAME")]
        vhost_name: String,

        /// Output format (json or yaml)
        #[arg(short, long, default_value = "json", value_parser = ["json", "yaml"])]
        output: String,
    },
}

/// Virtual host list response matching API response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListVirtualHostsResponse {
    pub route_config_name: String,
    pub virtual_hosts: Vec<VirtualHostResponse>,
}

/// Virtual host response structure
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualHostResponse {
    pub id: String,
    pub name: String,
    pub domains: Vec<String>,
    pub rule_order: i32,
    pub route_count: i64,
    pub filter_count: i64,
}

/// Handle vhost commands
pub async fn handle_vhost_command(
    command: VhostCommands,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    match command {
        VhostCommands::List { route_config, output } => {
            list_vhosts(client, team, &route_config, &output).await?
        }
        VhostCommands::Get { route_config, vhost_name, output } => {
            get_vhost(client, team, &route_config, &vhost_name, &output).await?
        }
    }

    Ok(())
}

async fn list_vhosts(
    client: &FlowplaneClient,
    team: &str,
    route_config: &str,
    output: &str,
) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/route-configs/{route_config}/virtual-hosts");
    let response: ListVirtualHostsResponse = client.get_json(&path).await?;

    if output == "table" {
        print_vhosts_table(&response.virtual_hosts);
    } else {
        print_output(&response.virtual_hosts, output)?;
    }

    Ok(())
}

async fn get_vhost(
    client: &FlowplaneClient,
    team: &str,
    route_config: &str,
    vhost_name: &str,
    output: &str,
) -> Result<()> {
    // No individual GET endpoint — filter from list
    let path = format!("/api/v1/teams/{team}/route-configs/{route_config}/virtual-hosts");
    let response: ListVirtualHostsResponse = client.get_json(&path).await?;

    let vhost = response.virtual_hosts.into_iter().find(|vh| vh.name == vhost_name);

    match vhost {
        Some(vh) => print_output(&vh, output)?,
        None => {
            anyhow::bail!(
                "Virtual host '{}' not found in route config '{}'",
                vhost_name,
                route_config
            );
        }
    }

    Ok(())
}

fn print_vhosts_table(vhosts: &[VirtualHostResponse]) {
    if vhosts.is_empty() {
        println!("No virtual hosts found");
        return;
    }

    print_table_header(&[
        ("Name", 30),
        ("Domains", 35),
        ("Order", 6),
        ("Routes", 7),
        ("Filters", 8),
    ]);

    for vh in vhosts {
        let domains = if vh.domains.len() <= 2 {
            vh.domains.join(", ")
        } else {
            format!("{}, ... (+{})", vh.domains[0], vh.domains.len() - 1)
        };

        println!(
            "{:<30} {:<35} {:<6} {:<7} {:<8}",
            truncate(&vh.name, 28),
            truncate(&domains, 33),
            vh.rule_order,
            vh.route_count,
            vh.filter_count,
        );
    }
    println!();
}
