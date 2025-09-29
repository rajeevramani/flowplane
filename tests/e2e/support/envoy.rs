use http_body_util::{BodyExt, Full};
use hyper::http::header::HOST;
use hyper::{body::Buf, http::Uri, Request};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use std::fs::File;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info};

#[derive(Debug)]
pub struct EnvoyHandle {
    child: Option<Child>,
    admin_port: u16,
}

#[allow(dead_code)]
impl EnvoyHandle {
    pub fn is_available() -> bool {
        which::which("envoy").is_ok()
    }

    #[allow(dead_code)]
    pub fn start(admin_port: u16, xds_port: u16) -> anyhow::Result<Self> {
        let config_path = write_temp_config(admin_port, xds_port)?;

        let mut cmd = Command::new("envoy");
        cmd.arg("-c").arg(&config_path);
        cmd.arg("--disable-hot-restart");
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = cmd.spawn()?;
        info!(?config_path, admin_port, xds_port, "Started Envoy");
        Ok(Self { child: Some(child), admin_port })
    }

    #[allow(dead_code)]
    pub fn start_tls(
        admin_port: u16,
        xds_port: u16,
        tls_port: u16,
        cert_path: &std::path::Path,
        key_path: &std::path::Path,
    ) -> anyhow::Result<Self> {
        let config_path =
            write_temp_config_tls(admin_port, xds_port, tls_port, cert_path, key_path)?;
        let mut cmd = Command::new("envoy");
        cmd.arg("-c").arg(&config_path);
        cmd.arg("--disable-hot-restart");
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = cmd.spawn()?;
        info!(?config_path, admin_port, xds_port, tls_port, "Started Envoy (TLS listener)");
        Ok(Self { child: Some(child), admin_port })
    }

    /// Start Envoy with explicit node metadata to scope listeners/routes per team or allowlist.
    #[allow(dead_code)]
    pub fn start_with_metadata(
        admin_port: u16,
        xds_port: u16,
        metadata: serde_json::Value,
    ) -> anyhow::Result<Self> {
        let config_path = write_temp_config_with_metadata(admin_port, xds_port, &metadata)?;
        let mut cmd = Command::new("envoy");
        cmd.arg("-c").arg(&config_path);
        cmd.arg("--disable-hot-restart");
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = cmd.spawn()?;
        info!(?config_path, admin_port, xds_port, "Started Envoy (metadata scoped)");
        Ok(Self { child: Some(child), admin_port })
    }

    /// Start Envoy with ADS over mTLS for xDS_cluster.
    #[allow(dead_code)]
    pub fn start_ads_mtls(
        admin_port: u16,
        xds_port: u16,
        client_cert: Option<&std::path::Path>,
        client_key: Option<&std::path::Path>,
        ca_cert: &std::path::Path,
    ) -> anyhow::Result<Self> {
        let config_path =
            write_temp_config_ads_mtls(admin_port, xds_port, client_cert, client_key, ca_cert)?;
        let mut cmd = Command::new("envoy");
        cmd.arg("-c").arg(&config_path);
        cmd.arg("--disable-hot-restart");
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = cmd.spawn()?;
        info!(?config_path, admin_port, xds_port, "Started Envoy (ADS mTLS)");
        Ok(Self { child: Some(child), admin_port })
    }

    #[allow(dead_code)]
    pub async fn wait_admin_ready(&self) {
        let client: Client<HttpConnector, Full<bytes::Bytes>> =
            Client::builder(TokioExecutor::new()).build(HttpConnector::new());
        let uri: Uri = format!("http://127.0.0.1:{}/stats", self.admin_port).parse().unwrap();
        for _ in 0..100 {
            if let Ok(res) = client.get(uri.clone()).await {
                if res.status().is_success() {
                    break;
                }
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    #[allow(dead_code)]
    #[allow(dead_code)]
    pub async fn get_config_dump(&self) -> anyhow::Result<String> {
        let client: Client<HttpConnector, Full<bytes::Bytes>> =
            Client::builder(TokioExecutor::new()).build(HttpConnector::new());
        let uri: Uri = format!("http://127.0.0.1:{}/config_dump", self.admin_port).parse()?;
        let res = client.get(uri).await?;
        let bytes = res.into_body().collect().await?.to_bytes();
        Ok(String::from_utf8_lossy(bytes.chunk()).to_string())
    }

    #[allow(dead_code)]
    #[allow(dead_code)]
    pub async fn get_stats(&self) -> anyhow::Result<String> {
        let client: Client<HttpConnector, Full<bytes::Bytes>> =
            Client::builder(TokioExecutor::new()).build(HttpConnector::new());
        let uri: Uri = format!("http://127.0.0.1:{}/stats", self.admin_port).parse()?;
        let res = client.get(uri).await?;
        let bytes = res.into_body().collect().await?.to_bytes();
        Ok(String::from_utf8_lossy(bytes.chunk()).to_string())
    }

    /// Send a proxied GET request through the default gateway listener with Host override.
    #[allow(dead_code)]
    pub async fn proxy_get(&self, host: &str, path: &str) -> anyhow::Result<(u16, String)> {
        let port = flowplane::openapi::defaults::DEFAULT_GATEWAY_PORT;
        let connector = HttpConnector::new();
        let client: Client<HttpConnector, _> =
            Client::builder(TokioExecutor::new()).build(connector);
        let uri: Uri = format!("http://127.0.0.1:{}{}", port, path).parse()?;
        let req =
            Request::builder().uri(uri).header(HOST, host).body(Full::<bytes::Bytes>::default())?;
        let res = client.request(req).await?;
        let status = res.status().as_u16();
        let body = res.into_body().collect().await?.to_bytes();
        Ok((status, String::from_utf8_lossy(body.chunk()).to_string()))
    }

    /// Send HTTPS proxied GET to a TLS listener with custom CA trust.
    #[allow(dead_code)]
    pub async fn proxy_get_tls(
        &self,
        tls_port: u16,
        host: &str,
        path: &str,
        ca_pem: Option<&std::path::Path>,
    ) -> anyhow::Result<(u16, String)> {
        use hyper_rustls::HttpsConnectorBuilder;
        use rustls::{pki_types::CertificateDer, ClientConfig, RootCertStore};

        let https = if let Some(ca_path) = ca_pem {
            let mut store = RootCertStore::empty();
            // Minimal PEM parsing to DER
            let mut pem = String::new();
            std::io::Read::read_to_string(&mut File::open(ca_path)?, &mut pem)?;
            let der = extract_first_pem_block(&pem)
                .ok_or_else(|| anyhow::anyhow!("failed to parse CA PEM at {:?}", ca_path))?;
            let certs: Vec<CertificateDer<'static>> = vec![CertificateDer::from(der)];
            let (added, _ignored) = store.add_parsable_certificates(certs);
            if added == 0 {
                anyhow::bail!("no certificates added from {:?}", ca_path);
            }
            let cfg = ClientConfig::builder().with_root_certificates(store).with_no_client_auth();
            HttpsConnectorBuilder::new().with_tls_config(cfg).https_or_http().enable_http1().build()
        } else {
            HttpsConnectorBuilder::new().with_native_roots()?.https_or_http().enable_http1().build()
        };

        let client: Client<_, Full<bytes::Bytes>> =
            Client::builder(TokioExecutor::new()).build(https);
        let uri: Uri = format!("https://127.0.0.1:{}{}", tls_port, path).parse()?;
        let req = Request::builder().uri(uri).header(HOST, host).body(Full::default())?;
        let res = client.request(req).await?;
        let status = res.status().as_u16();
        let body = res.into_body().collect().await?.to_bytes();
        Ok((status, String::from_utf8_lossy(body.chunk()).to_string()))
    }
}

impl Drop for EnvoyHandle {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if let Err(e) = child.kill() {
                error!(error = %e, "Failed to kill Envoy process");
            }
        }
    }
}

#[allow(dead_code)]
fn write_temp_config(admin_port: u16, xds_port: u16) -> anyhow::Result<PathBuf> {
    let yaml = format!(
        r#"node:
  id: local-dataplane
  cluster: platform-apis

admin:
  access_log_path: /tmp/envoy_admin.log
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
      connect_timeout: 1s
      http2_protocol_options: {{}}
      load_assignment:
        cluster_name: xds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: 127.0.0.1
                      port_value: {xds_port}
"#,
        admin_port = admin_port,
        xds_port = xds_port
    );
    let path = std::env::temp_dir().join(format!("envoy-e2e-{}.yaml", admin_port));
    std::fs::write(&path, yaml)?;
    Ok(path)
}

#[allow(dead_code)]
fn extract_first_pem_block(pem: &str) -> Option<Vec<u8>> {
    let begin = "-----BEGIN CERTIFICATE-----";
    let end = "-----END CERTIFICATE-----";
    let start = pem.find(begin)? + begin.len();
    let stop = pem[start..].find(end)? + start;
    let b64 = pem[start..stop].lines().collect::<String>();
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    BASE64.decode(b64.trim()).ok()
}

#[allow(dead_code)]
fn write_temp_config_tls(
    admin_port: u16,
    xds_port: u16,
    tls_port: u16,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> anyhow::Result<PathBuf> {
    let yaml = format!(
        r#"node:
  id: local-dataplane
  cluster: platform-apis

admin:
  access_log_path: /tmp/envoy_admin_tls.log
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
  listeners:
    - name: edge-tls
      address:
        socket_address:
          address: 127.0.0.1
          port_value: {tls_port}
      filter_chains:
        - transport_socket:
            name: envoy.transport_sockets.tls
            typed_config:
              "@type": type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.DownstreamTlsContext
              common_tls_context:
                tls_certificates:
                  - certificate_chain: {{ filename: "{cert}" }}
                    private_key: {{ filename: "{key}" }}
          filters:
            - name: envoy.filters.network.http_connection_manager
              typed_config:
                "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
                stat_prefix: edge_tls
                rds:
                  route_config_name: default-gateway-routes
                  config_source:
                    resource_api_version: V3
                    ads: {{}}
  clusters:
    - name: xds_cluster
      type: LOGICAL_DNS
      dns_lookup_family: V4_ONLY
      connect_timeout: 1s
      http2_protocol_options: {{}}
      load_assignment:
        cluster_name: xds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: 127.0.0.1
                      port_value: {xds_port}
"#,
        admin_port = admin_port,
        xds_port = xds_port,
        tls_port = tls_port,
        cert = cert_path.display(),
        key = key_path.display(),
    );
    let path = std::env::temp_dir().join(format!("envoy-e2e-tls-{}.yaml", admin_port));
    std::fs::write(&path, yaml)?;
    Ok(path)
}

#[allow(dead_code)]
fn write_temp_config_with_metadata(
    admin_port: u16,
    xds_port: u16,
    metadata: &serde_json::Value,
) -> anyhow::Result<PathBuf> {
    // Serialize metadata block as YAML fragment
    let metadata_yaml = serde_yaml::to_string(metadata)?;
    // Indent each line by 4 spaces to fit under node.metadata
    let metadata_indented =
        metadata_yaml.lines().map(|l| format!("    {}", l)).collect::<Vec<_>>().join("\n");

    let yaml = format!(
        r#"node:
  id: local-dataplane
  cluster: platform-apis
  metadata:
{metadata_indented}

admin:
  access_log_path: /tmp/envoy_admin_meta.log
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
      connect_timeout: 1s
      http2_protocol_options: {{}}
      load_assignment:
        cluster_name: xds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: 127.0.0.1
                      port_value: {xds_port}
"#,
        metadata_indented = metadata_indented,
        admin_port = admin_port,
        xds_port = xds_port,
    );
    let path = std::env::temp_dir().join(format!("envoy-e2e-meta-{}.yaml", admin_port));
    std::fs::write(&path, yaml)?;
    Ok(path)
}

#[allow(dead_code)]
fn write_temp_config_ads_mtls(
    admin_port: u16,
    xds_port: u16,
    client_cert: Option<&std::path::Path>,
    client_key: Option<&std::path::Path>,
    ca_cert: &std::path::Path,
) -> anyhow::Result<PathBuf> {
    let client_tls = if let (Some(cert), Some(key)) = (client_cert, client_key) {
        format!(
            "tls_certificates:\n          - certificate_chain: {{ filename: \"{}\" }}\n            private_key: {{ filename: \"{}\" }}\n",
            cert.display(),
            key.display()
        )
    } else {
        String::new()
    };

    let yaml = format!(
        r#"node:
  id: local-dataplane
  cluster: platform-apis

admin:
  access_log_path: /tmp/envoy_admin_mtls.log
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
      connect_timeout: 1s
      http2_protocol_options: {{}}
      transport_socket:
        name: envoy.transport_sockets.tls
        typed_config:
          "@type": type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext
          common_tls_context:
            {client_tls}validation_context:
              trusted_ca: {{ filename: "{ca}" }}
      load_assignment:
        cluster_name: xds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: 127.0.0.1
                      port_value: {xds_port}
"#,
        admin_port = admin_port,
        xds_port = xds_port,
        client_tls = client_tls,
        ca = ca_cert.display(),
    );
    let path = std::env::temp_dir().join(format!("envoy-e2e-ads-mtls-{}.yaml", admin_port));
    std::fs::write(&path, yaml)?;
    Ok(path)
}
