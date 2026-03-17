//! Expose/Unexpose CLI commands
//!
//! Provides simplified CLI for exposing local services through the Envoy gateway.
//! Wraps POST /api/v1/teams/{team}/expose and DELETE /api/v1/teams/{team}/expose/{name}.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::client::FlowplaneClient;

/// Request body for the expose API (matches backend ExposeRequest)
#[derive(Debug, Serialize)]
pub struct ExposeRequest {
    pub name: String,
    pub upstream: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// Response from the expose API (matches backend ExposeResponse)
#[derive(Debug, Deserialize)]
pub struct ExposeResponse {
    pub name: String,
    pub upstream: String,
    pub port: u16,
    pub paths: Vec<String>,
    pub cluster: String,
    pub route_config: String,
    pub listener: String,
}

/// Check if a URL or host string resolves to a loopback address.
///
/// Handles URLs with and without schemes, localhost, 127.0.0.1, ::1, and [::1].
pub fn is_loopback(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }

    // Strip scheme if present
    let without_scheme = if let Some(rest) = trimmed.strip_prefix("http://") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        rest
    } else {
        trimmed
    };

    // Strip path and query
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);

    // Strip port (handle bracketed IPv6 like [::1]:8080)
    let host = if host_port.starts_with('[') {
        // IPv6 bracketed: [::1]:8080 or [::1]
        host_port.find(']').map(|i| &host_port[1..i]).unwrap_or(host_port)
    } else if host_port.contains("::") {
        // Bare IPv6 (e.g., ::1) — don't try to split on colon
        host_port
    } else {
        // Regular host:port — split on last colon
        match host_port.rsplit_once(':') {
            Some((h, port_str)) if port_str.parse::<u16>().is_ok() => h,
            _ => host_port,
        }
    };

    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// Generate a service name from an upstream URL.
///
/// Strips scheme, takes only host:port (drops path), replaces non-alphanumeric
/// chars with hyphens, truncates to 48 chars, and strips leading/trailing hyphens.
pub fn generate_expose_name(upstream_url: &str) -> String {
    // Strip scheme
    let without_scheme = if let Some(rest) = upstream_url.strip_prefix("http://") {
        rest
    } else if let Some(rest) = upstream_url.strip_prefix("https://") {
        rest
    } else {
        upstream_url
    };

    // Take only host:port (strip path)
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);

    // Replace non-alphanumeric chars with hyphens
    let name: String =
        host_port.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();

    // Truncate to 48 chars
    let truncated = if name.len() > 48 { &name[..48] } else { &name };

    // Strip leading/trailing hyphens
    truncated.trim_matches('-').to_string()
}

/// Probe Envoy admin endpoint for readiness.
///
/// Returns true if HTTP GET to `{addr}/ready` returns 200 within the given timeout.
pub async fn probe_envoy(addr: &str, timeout: Duration) -> bool {
    let url = format!("{}/ready", addr);
    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(c) => c,
        Err(_) => return false,
    };
    match client.get(&url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Handle the `flowplane expose` command.
pub async fn handle_expose_command(
    client: &FlowplaneClient,
    team: &str,
    upstream: &str,
    name: Option<&str>,
    paths: Option<Vec<String>>,
    port: Option<u16>,
    base_url: &str,
) -> Result<()> {
    let service_name = match name {
        Some(n) => n.to_string(),
        None => generate_expose_name(upstream),
    };

    let request = ExposeRequest { name: service_name, upstream: upstream.to_string(), paths, port };

    let path = format!("/api/v1/teams/{team}/expose");
    let response: ExposeResponse =
        client.post_json(&path, &request).await.context("Failed to expose service")?;

    println!("Exposed '{}' -> {}", response.name, response.upstream);
    println!("  Port:   {}", response.port);
    println!("  Paths:  {}", response.paths.join(", "));

    if is_loopback(base_url) {
        if probe_envoy("http://localhost:9901", Duration::from_secs(1)).await {
            println!("\n  curl http://localhost:{}/", response.port);
        } else {
            println!("\nEnvoy is not running. Run 'flowplane init --with-envoy' to start it.");
        }
    }

    Ok(())
}

/// Handle the `flowplane unexpose` command.
pub async fn handle_unexpose_command(
    client: &FlowplaneClient,
    team: &str,
    name: &str,
) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/expose/{name}");
    let response = client.delete(&path).send().await.context("Failed to send unexpose request")?;

    let status = response.status();

    if status == reqwest::StatusCode::NOT_FOUND {
        println!("{name} is not currently exposed");
        return Ok(());
    }

    if !status.is_success() {
        let error_text =
            response.text().await.unwrap_or_else(|_| "<unable to read error>".to_string());
        anyhow::bail!("Unexpose failed with status {}: {}", status, error_text);
    }

    println!("Removed exposed service '{name}'");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // === is_loopback tests ===

    #[test]
    fn is_loopback_localhost() {
        assert!(is_loopback("localhost"));
    }

    #[test]
    fn is_loopback_localhost_with_port() {
        assert!(is_loopback("localhost:3000"));
    }

    #[test]
    fn is_loopback_http_localhost() {
        assert!(is_loopback("http://localhost:3000"));
    }

    #[test]
    fn is_loopback_https_localhost() {
        assert!(is_loopback("https://localhost:8080/api"));
    }

    #[test]
    fn is_loopback_127_0_0_1() {
        assert!(is_loopback("127.0.0.1"));
    }

    #[test]
    fn is_loopback_127_0_0_1_with_port() {
        assert!(is_loopback("http://127.0.0.1:8080"));
    }

    #[test]
    fn is_loopback_ipv6_short() {
        assert!(is_loopback("::1"));
    }

    #[test]
    fn is_loopback_ipv6_bracketed() {
        assert!(is_loopback("[::1]:8080"));
    }

    #[test]
    fn is_loopback_ipv6_bracketed_http() {
        assert!(is_loopback("http://[::1]:8080"));
    }

    #[test]
    fn is_loopback_remote_false() {
        assert!(!is_loopback("https://remote.example.com"));
    }

    #[test]
    fn is_loopback_remote_with_port_false() {
        assert!(!is_loopback("http://api.example.com:8080"));
    }

    #[test]
    fn is_loopback_empty_false() {
        assert!(!is_loopback(""));
    }

    #[test]
    fn is_loopback_whitespace_false() {
        assert!(!is_loopback("   "));
    }

    // === generate_expose_name tests ===

    #[test]
    fn generate_name_http_localhost() {
        assert_eq!(generate_expose_name("http://localhost:3000"), "localhost-3000");
    }

    #[test]
    fn generate_name_https_with_path() {
        assert_eq!(generate_expose_name("https://api.example.com:8080/v1"), "api-example-com-8080");
    }

    #[test]
    fn generate_name_plain_host_port() {
        assert_eq!(generate_expose_name("myhost:9090"), "myhost-9090");
    }

    #[test]
    fn generate_name_strips_leading_hyphens() {
        // A URL like "http://:3000" -> after scheme strip: ":3000" -> host_port ":3000" -> "-3000"
        assert_eq!(generate_expose_name("http://:3000"), "3000");
    }

    #[test]
    fn generate_name_truncation() {
        // Create a URL with a very long hostname
        let long_host = "a".repeat(60);
        let url = format!("http://{}:8080", long_host);
        let name = generate_expose_name(&url);
        assert!(name.len() <= 48);
    }

    #[test]
    fn generate_name_special_chars() {
        assert_eq!(generate_expose_name("http://my_host.local:3000"), "my-host-local-3000");
    }

    #[test]
    fn generate_name_trailing_hyphens_stripped() {
        // "http://host:-" -> host_port "host:-" -> "host--" -> trimmed to "host"
        assert_eq!(generate_expose_name("http://host:-"), "host");
    }
}
