//! Status and Doctor CLI commands
//!
//! `flowplane status` — system health overview and per-listener lookup.
//! `flowplane doctor` — diagnostic health checks for CP, DB, and Envoy.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;

use super::client::FlowplaneClient;
use super::expose::{is_loopback, probe_envoy};
use crate::api::handlers::PaginatedResponse;

use super::listeners::ListenerResponse;

/// Health response from /health
#[derive(Debug, Deserialize)]
struct HealthResponse {
    pub status: String,
}

/// Stats overview response from /api/v1/teams/{team}/stats/overview
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct StatsOverview {
    pub total_clusters: u64,
    pub total_rps: f64,
    pub total_connections: u64,
    pub error_rate: f64,
    pub health_status: String,
    pub healthy_clusters: u64,
    pub degraded_clusters: u64,
    pub unhealthy_clusters: u64,
}

/// Handle `flowplane status [NAME]`
pub async fn handle_status_command(
    client: &FlowplaneClient,
    team: &str,
    name: Option<&str>,
) -> Result<()> {
    match name {
        Some(n) => show_listener_status(client, team, n).await,
        None => show_system_status(client, team).await,
    }
}

/// Show system-wide status: health + resource counts
async fn show_system_status(client: &FlowplaneClient, team: &str) -> Result<()> {
    // Gather counts from list endpoints (limit=1 to minimize payload; we only need total)
    let listeners_path = format!("/api/v1/teams/{team}/listeners?limit=1");
    let clusters_path = format!("/api/v1/teams/{team}/clusters?limit=1");
    let filters_path = format!("/api/v1/teams/{team}/filters?limit=1");

    // Try to get resource counts (use total from paginated responses)
    let listener_count = client
        .get_json::<PaginatedResponse<serde_json::Value>>(&listeners_path)
        .await
        .map(|r| r.total)
        .unwrap_or(0);

    let cluster_count = client
        .get_json::<PaginatedResponse<serde_json::Value>>(&clusters_path)
        .await
        .map(|r| r.total)
        .unwrap_or(0);

    let filter_count = client
        .get_json::<PaginatedResponse<serde_json::Value>>(&filters_path)
        .await
        .map(|r| r.total)
        .unwrap_or(0);

    // Try stats overview for health info
    let stats_result: std::result::Result<StatsOverview, _> =
        client.get_json(&format!("/api/v1/teams/{team}/stats/overview")).await;

    println!("Flowplane Status (team: {team})");
    println!("{}", "-".repeat(40));
    println!("Listeners:  {listener_count}");
    println!("Clusters:   {cluster_count}");
    println!("Filters:    {filter_count}");

    if let Ok(stats) = stats_result {
        println!();
        println!("Health:      {}", stats.health_status);
        println!(
            "Clusters:    {} healthy, {} degraded, {} unhealthy",
            stats.healthy_clusters, stats.degraded_clusters, stats.unhealthy_clusters
        );
        println!(
            "Traffic:     {:.1} rps, {} connections, {:.2}% errors",
            stats.total_rps,
            stats.total_connections,
            stats.error_rate * 100.0
        );
    }

    Ok(())
}

/// Show status for a specific listener by name
async fn show_listener_status(client: &FlowplaneClient, team: &str, name: &str) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/listeners/{name}");
    let listener: ListenerResponse =
        client.get_json(&path).await.with_context(|| format!("Listener '{name}' not found"))?;

    println!("Listener: {}", listener.name);
    println!("{}", "-".repeat(40));
    println!("Team:     {}", listener.team);
    println!("Address:  {}", listener.address);
    println!("Port:     {}", listener.port);
    println!("Protocol: {}", listener.protocol);

    Ok(())
}

/// Handle `flowplane doctor`
pub async fn handle_doctor_command(client: &FlowplaneClient, base_url: &str) -> Result<()> {
    println!("Flowplane Doctor");
    println!("{}", "-".repeat(40));

    // 1. Probe CP health
    let health_result: std::result::Result<HealthResponse, _> = client.get_json("/health").await;

    match health_result {
        Ok(h) if h.status == "ok" => println!("[ok]    Control plane health: {}", h.status),
        Ok(h) => println!("[warn]  Control plane health: {}", h.status),
        Err(e) => println!("[fail]  Control plane unreachable: {e}"),
    }

    // 2. Probe Envoy (only if base_url is loopback)
    if is_loopback(base_url) {
        let envoy_ready = probe_envoy("http://localhost:9901", Duration::from_secs(2)).await;
        if envoy_ready {
            println!("[ok]    Envoy proxy: ready");
        } else {
            println!("[fail]  Envoy proxy: not responding at localhost:9901");
        }
    } else {
        println!("[skip]  Envoy check skipped (remote server)");
    }

    Ok(())
}
