use std::{net::{IpAddr, Ipv4Addr, SocketAddr}, sync::Arc, time::Duration};

use flowplane::{
    api::start_api_server,
    config::{ApiServerConfig, ApiTlsConfig, SimpleXdsConfig},
    xds::XdsState,
};
use http::StatusCode;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper_util::{
    client::legacy::Client,
    rt::TokioExecutor,
};
use reserve_port::reserve_port;
use tokio::{net::TcpStream, time::sleep};

#[path = "../support.rs"]
mod support;

use support::TestCertificateFiles;

async fn wait_for_listener(addr: SocketAddr) {
    for _ in 0..20 {
        match TcpStream::connect(addr).await {
            Ok(stream) => {
                drop(stream);
                return;
            }
            Err(_) => sleep(Duration::from_millis(50)).await,
        }
    }
    panic!("server at {} did not become ready in time", addr);
}

#[tokio::test]
async fn http_mode_preserved_when_tls_disabled() {
    let port = reserve_port();
    let config = ApiServerConfig {
        bind_address: "127.0.0.1".to_string(),
        port,
        tls: None,
    };
    let state = Arc::new(XdsState::new(SimpleXdsConfig::default()));

    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let handle = tokio::spawn(start_api_server(config, state));
    wait_for_listener(server_addr).await;

    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(hyper::client::HttpConnector::new());
    let uri = format!("http://127.0.0.1:{port}/health/live").parse().unwrap();
    let response = client.get(uri).await.expect("http request succeeded");
    assert_eq!(response.status(), StatusCode::OK);

    handle.abort();
    let _ = handle.await;
}

#[tokio::test]
async fn https_server_serves_requests() {
    let port = reserve_port();
    let certs =
        TestCertificateFiles::localhost(time::Duration::days(60)).expect("generate certificates");
    let tls = ApiTlsConfig {
        cert_path: certs.cert_path.clone(),
        key_path: certs.key_path.clone(),
        chain_path: None,
    };
    let config = ApiServerConfig { bind_address: "127.0.0.1".to_string(), port, tls: Some(tls) };
    let state = Arc::new(XdsState::new(SimpleXdsConfig::default()));

    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let handle = tokio::spawn(start_api_server(config, state));
    wait_for_listener(server_addr).await;

    let cert_pem = std::fs::read(&certs.cert_path).expect("read certificate");
    let mut roots = rustls::RootCertStore::empty();
    let mut certs = rustls::pki_types::CertificateDer::pem_slice_iter(&cert_pem);
    let leaf = certs.next().expect("cert present").expect("valid cert");
    roots
        .add(leaf)
        .expect("add certificate to root store");

    let client_tls = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(client_tls)
        .https_only()
        .enable_http1()
        .build();

    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(https);
    let uri = format!("https://localhost:{port}/health/live").parse().unwrap();
    let response = client.get(uri).await.expect("https request succeeded");
    assert_eq!(response.status(), StatusCode::OK);

    handle.abort();
    let _ = handle.await;
}

#[tokio::test]
async fn https_startup_fails_with_mismatched_key() {
    let port = reserve_port();
    let certs =
        TestCertificateFiles::localhost(time::Duration::days(60)).expect("generate certificates");
    let mismatched_key = certs.mismatched_key().expect("generate mismatched key");
    let tls = ApiTlsConfig {
        cert_path: certs.cert_path.clone(),
        key_path: mismatched_key,
        chain_path: None,
    };
    let config = ApiServerConfig { bind_address: "127.0.0.1".to_string(), port, tls: Some(tls) };
    let state = Arc::new(XdsState::new(SimpleXdsConfig::default()));

    let err = start_api_server(config, state)
        .await
        .expect_err("TLS mismatch should fail startup");
    let message = format!("{err}");
    assert!(message.contains("TLS configuration error"));
    assert!(message.contains("do not match"));
}
