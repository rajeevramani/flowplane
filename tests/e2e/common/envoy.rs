//! Envoy proxy lifecycle management for E2E tests
//!
//! Manages startup, health checks, and shutdown of Envoy proxy instances
//! with proper timeout handling.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use bytes::Buf;
use http_body_util::{BodyExt, Full};
use hyper::http::{header::HOST, Method, Uri};
use hyper::Request;
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use tracing::{debug, error, info};

use super::timeout::{retry_with_timeout, STARTUP_TIMEOUT};

/// Envoy configuration options
#[derive(Debug, Clone)]
pub struct EnvoyConfig {
    /// Admin port for stats and config_dump
    pub admin_port: u16,
    /// xDS server address (host:port)
    pub xds_address: String,
    /// Node ID for xDS subscription
    pub node_id: String,
    /// Node cluster for xDS subscription
    pub node_cluster: String,
    /// Optional node metadata for scoping
    pub metadata: Option<serde_json::Value>,
    /// TLS config for xDS connection
    pub xds_tls: Option<EnvoyXdsTlsConfig>,
}

/// TLS configuration for Envoy's xDS connection
#[derive(Debug, Clone)]
pub struct EnvoyXdsTlsConfig {
    /// CA certificate path
    pub ca_cert: PathBuf,
    /// Client certificate path (for mTLS)
    pub client_cert: Option<PathBuf>,
    /// Client key path (for mTLS)
    pub client_key: Option<PathBuf>,
}

/// Response from a proxied request with headers
#[derive(Debug)]
pub struct ProxyResponse {
    /// HTTP status code
    pub status: u16,
    /// Response headers
    pub headers: HashMap<String, String>,
    /// Response body
    pub body: String,
}

impl EnvoyConfig {
    /// Create a basic config pointing to local xDS
    pub fn new(admin_port: u16, xds_port: u16) -> Self {
        Self {
            admin_port,
            xds_address: format!("127.0.0.1:{}", xds_port),
            node_id: "e2e-dataplane".to_string(),
            node_cluster: "platform-apis".to_string(),
            metadata: None,
            xds_tls: None,
        }
    }

    /// Set custom node ID
    pub fn with_node_id(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = node_id.into();
        self
    }

    /// Set node metadata for listener scoping
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Configure mTLS for xDS connection
    pub fn with_xds_tls(mut self, tls: EnvoyXdsTlsConfig) -> Self {
        self.xds_tls = Some(tls);
        self
    }
}

/// Handle to a running Envoy process
#[derive(Debug)]
pub struct EnvoyHandle {
    child: Option<Child>,
    admin_port: u16,
    config_path: PathBuf,
}

impl EnvoyHandle {
    /// Check if Envoy binary is available on PATH
    pub fn is_available() -> bool {
        which::which("envoy").is_ok()
    }

    /// Start Envoy with the given configuration
    pub fn start(config: EnvoyConfig) -> anyhow::Result<Self> {
        let config_path = write_envoy_config(&config)?;

        info!(
            ?config_path,
            admin_port = config.admin_port,
            xds = config.xds_address,
            "Starting Envoy with config"
        );

        // Log the config file contents for debugging
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            debug!(config = %contents, "Envoy bootstrap config");
        }

        let mut cmd = Command::new("envoy");
        cmd.arg("-c").arg(&config_path);
        cmd.arg("--disable-hot-restart");
        cmd.arg("-l").arg("warn"); // Set envoy log level to warn to reduce noise
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let child = cmd.spawn().map_err(|e| {
            anyhow::anyhow!("Failed to spawn Envoy process: {}. Is 'envoy' in your PATH?", e)
        })?;

        info!(
            pid = child.id(),
            admin_port = config.admin_port,
            xds = config.xds_address,
            "Envoy process started"
        );

        Ok(Self { child: Some(child), admin_port: config.admin_port, config_path })
    }

    /// Wait for Envoy admin endpoint to be ready
    pub async fn wait_ready(&self) -> anyhow::Result<()> {
        let admin_port = self.admin_port;

        retry_with_timeout(
            STARTUP_TIMEOUT,
            Duration::from_millis(200),
            "Envoy admin ready",
            move || async move {
                let client: Client<HttpConnector, Full<bytes::Bytes>> =
                    Client::builder(TokioExecutor::new()).build(HttpConnector::new());
                let uri: Uri = format!("http://127.0.0.1:{}/ready", admin_port).parse().unwrap();

                let res =
                    client.get(uri).await.map_err(|e| format!("Admin request failed: {}", e))?;

                if res.status().is_success() {
                    Ok(())
                } else {
                    Err(format!("Envoy not ready: {}", res.status()))
                }
            },
        )
        .await?;

        info!(admin_port = self.admin_port, "Envoy ready");
        Ok(())
    }

    /// Wait for a specific route to be available and responding
    pub async fn wait_for_route(
        &self,
        port: u16,
        host: &str,
        path: &str,
        expected_status: u16,
    ) -> anyhow::Result<String> {
        let host = host.to_string();
        let path = path.to_string();

        retry_with_timeout(
            Duration::from_secs(30),
            Duration::from_millis(200),
            &format!("Route {}:{}{}", host, port, path),
            move || {
                let host = host.clone();
                let path = path.clone();
                async move {
                    match proxy_get(port, &host, &path).await {
                        Ok((status, body)) => {
                            if status == expected_status && !body.is_empty() {
                                Ok(body)
                            } else {
                                Err(format!(
                                    "Expected status {}, got {} (body len: {})",
                                    expected_status,
                                    status,
                                    body.len()
                                ))
                            }
                        }
                        Err(e) => Err(format!("Request failed: {}", e)),
                    }
                }
            },
        )
        .await
    }

    /// Send a GET request through Envoy proxy
    pub async fn proxy_get(
        &self,
        port: u16,
        host: &str,
        path: &str,
    ) -> anyhow::Result<(u16, String)> {
        proxy_get(port, host, path).await
    }

    /// Send a GET request through Envoy proxy and return response with headers
    pub async fn proxy_get_with_headers(
        &self,
        port: u16,
        host: &str,
        path: &str,
    ) -> anyhow::Result<ProxyResponse> {
        let (status, headers, body) =
            self.proxy_request(port, Method::GET, host, path, HashMap::new(), None).await?;
        Ok(ProxyResponse { status, headers, body })
    }

    /// Send a request with custom headers through Envoy proxy
    pub async fn proxy_request(
        &self,
        port: u16,
        method: Method,
        host: &str,
        path: &str,
        headers: HashMap<String, String>,
        body: Option<String>,
    ) -> anyhow::Result<(u16, HashMap<String, String>, String)> {
        let connector = HttpConnector::new();
        let client: Client<HttpConnector, Full<bytes::Bytes>> =
            Client::builder(TokioExecutor::new()).build(connector);

        let uri: Uri = format!("http://127.0.0.1:{}{}", port, path).parse()?;
        let mut builder = Request::builder().method(method).uri(uri).header(HOST, host);

        for (key, value) in &headers {
            builder = builder.header(key.as_str(), value.as_str());
        }

        let request_body: Full<bytes::Bytes> = body
            .map(|b| Full::from(bytes::Bytes::from(b.into_bytes())))
            .unwrap_or_else(|| Full::from(bytes::Bytes::new()));
        let req = builder.body(request_body)?;

        let res = client.request(req).await?;
        let status = res.status().as_u16();

        let response_headers: HashMap<String, String> = res
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let body_bytes = res.into_body().collect().await?.to_bytes();
        let body_str = String::from_utf8_lossy(body_bytes.chunk()).to_string();

        Ok((status, response_headers, body_str))
    }

    /// Get Envoy config dump
    pub async fn get_config_dump(&self) -> anyhow::Result<String> {
        let client: Client<HttpConnector, Full<bytes::Bytes>> =
            Client::builder(TokioExecutor::new()).build(HttpConnector::new());
        let uri: Uri = format!("http://127.0.0.1:{}/config_dump", self.admin_port).parse()?;

        let res = client.get(uri).await?;
        let bytes = res.into_body().collect().await?.to_bytes();
        Ok(String::from_utf8_lossy(bytes.chunk()).to_string())
    }

    /// Get Envoy stats
    pub async fn get_stats(&self) -> anyhow::Result<String> {
        let client: Client<HttpConnector, Full<bytes::Bytes>> =
            Client::builder(TokioExecutor::new()).build(HttpConnector::new());
        let uri: Uri = format!("http://127.0.0.1:{}/stats", self.admin_port).parse()?;

        let res = client.get(uri).await?;
        let bytes = res.into_body().collect().await?.to_bytes();
        Ok(String::from_utf8_lossy(bytes.chunk()).to_string())
    }

    /// Get Envoy stats as JSON
    pub async fn get_stats_json(&self) -> anyhow::Result<serde_json::Value> {
        let client: Client<HttpConnector, Full<bytes::Bytes>> =
            Client::builder(TokioExecutor::new()).build(HttpConnector::new());
        let uri: Uri = format!("http://127.0.0.1:{}/stats?format=json", self.admin_port).parse()?;

        let res = client.get(uri).await?;
        let bytes = res.into_body().collect().await?.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(bytes.chunk())?;
        Ok(json)
    }

    /// Wait for config to contain expected content
    pub async fn wait_for_config_content(&self, expected: &str) -> anyhow::Result<()> {
        let expected = expected.to_string();
        let admin_port = self.admin_port;

        retry_with_timeout(
            Duration::from_secs(30),
            Duration::from_millis(500),
            &format!("Config contains '{}'", expected),
            move || {
                let expected = expected.clone();
                async move {
                    let client: Client<HttpConnector, Full<bytes::Bytes>> =
                        Client::builder(TokioExecutor::new()).build(HttpConnector::new());
                    let uri: Uri =
                        format!("http://127.0.0.1:{}/config_dump", admin_port).parse().unwrap();

                    let res = client.get(uri).await.map_err(|e| e.to_string())?;
                    let bytes =
                        res.into_body().collect().await.map_err(|e| e.to_string())?.to_bytes();
                    let dump = String::from_utf8_lossy(bytes.chunk()).to_string();

                    if dump.contains(&expected) {
                        Ok(())
                    } else {
                        Err(format!("Config does not contain '{}'", expected))
                    }
                }
            },
        )
        .await
    }

    /// Shutdown Envoy gracefully
    pub fn shutdown(&mut self) {
        if let Some(mut child) = self.child.take() {
            debug!("Shutting down Envoy");
            if let Err(e) = child.kill() {
                error!(error = %e, "Failed to kill Envoy process");
            }
        }
    }
}

impl Drop for EnvoyHandle {
    fn drop(&mut self) {
        self.shutdown();
        // Clean up temp config file
        let _ = std::fs::remove_file(&self.config_path);
    }
}

/// Send a GET request through the proxy
async fn proxy_get(port: u16, host: &str, path: &str) -> anyhow::Result<(u16, String)> {
    let connector = HttpConnector::new();
    let client: Client<HttpConnector, _> = Client::builder(TokioExecutor::new()).build(connector);

    let uri: Uri = format!("http://127.0.0.1:{}{}", port, path).parse()?;
    let req =
        Request::builder().uri(uri).header(HOST, host).body(Full::<bytes::Bytes>::default())?;

    let res = client.request(req).await?;
    let status = res.status().as_u16();
    let body = res.into_body().collect().await?.to_bytes();
    Ok((status, String::from_utf8_lossy(body.chunk()).to_string()))
}

/// Write Envoy bootstrap config to temp file
fn write_envoy_config(config: &EnvoyConfig) -> anyhow::Result<PathBuf> {
    let metadata_yaml = if let Some(ref meta) = config.metadata {
        let yaml = serde_yaml::to_string(meta)?;
        let indented = yaml.lines().map(|l| format!("    {}", l)).collect::<Vec<_>>().join("\n");
        format!("  metadata:\n{}", indented)
    } else {
        String::new()
    };

    let xds_tls_config = if let Some(ref tls) = config.xds_tls {
        let client_tls = if let (Some(cert), Some(key)) = (&tls.client_cert, &tls.client_key) {
            format!(
                r#"tls_certificates:
              - certificate_chain: {{ filename: "{}" }}
                private_key: {{ filename: "{}" }}"#,
                cert.display(),
                key.display()
            )
        } else {
            String::new()
        };

        format!(
            r#"transport_socket:
        name: envoy.transport_sockets.tls
        typed_config:
          "@type": type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext
          common_tls_context:
            {}validation_context:
              trusted_ca: {{ filename: "{}" }}"#,
            client_tls,
            tls.ca_cert.display()
        )
    } else {
        String::new()
    };

    let yaml = format!(
        r#"node:
  id: {node_id}
  cluster: {node_cluster}
{metadata}

admin:
  address:
    socket_address:
      address: 127.0.0.1
      port_value: {admin_port}

dynamic_resources:
  lds_config:
    ads: {{}}
  cds_config:
    ads: {{}}
  ads_config:
    api_type: GRPC
    transport_api_version: V3
    grpc_services:
      - envoy_grpc:
          cluster_name: xds_cluster

static_resources:
  clusters:
    - name: xds_cluster
      type: LOGICAL_DNS
      dns_lookup_family: V4_ONLY
      connect_timeout: 5s
      http2_protocol_options: {{}}
      {xds_tls}
      load_assignment:
        cluster_name: xds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: {xds_host}
                      port_value: {xds_port}
"#,
        node_id = config.node_id,
        node_cluster = config.node_cluster,
        metadata = metadata_yaml,
        admin_port = config.admin_port,
        xds_host = config.xds_address.split(':').next().unwrap_or("127.0.0.1"),
        xds_port = config.xds_address.split(':').nth(1).unwrap_or("15010"),
        xds_tls = xds_tls_config,
    );

    let path = std::env::temp_dir().join(format!("envoy-e2e-{}.yaml", config.admin_port));
    std::fs::write(&path, yaml)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_envoy_config_creation() {
        let config = EnvoyConfig::new(9901, 15010);
        assert_eq!(config.admin_port, 9901);
        assert_eq!(config.xds_address, "127.0.0.1:15010");
        assert_eq!(config.node_id, "e2e-dataplane");
    }

    #[test]
    fn test_write_envoy_config() {
        let config = EnvoyConfig::new(9901, 15010);
        let path = write_envoy_config(&config).unwrap();
        assert!(path.exists());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("port_value: 9901"));
        assert!(content.contains("port_value: 15010"));

        std::fs::remove_file(path).unwrap();
    }
}
