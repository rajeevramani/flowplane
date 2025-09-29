//! xDS mTLS matrix. Requires fixtures under tests/e2e/fixtures/tls.

mod support;
use support::api::{create_pat, post_create_api, wait_http_ready};
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::tls::TlsFixtures;

#[derive(Debug, Clone)]
struct Case {
    name: &'static str,
    envoy_presents_client_cert: bool,
    server_verifies_client: bool,
    expect_success: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "mtls_success",
            envoy_presents_client_cert: true,
            server_verifies_client: true,
            expect_success: true,
        },
        Case {
            name: "no_client_cert_server_requires",
            envoy_presents_client_cert: false,
            server_verifies_client: true,
            expect_success: false,
        },
        Case {
            name: "server_does_not_verify",
            envoy_presents_client_cert: true,
            server_verifies_client: false,
            expect_success: true,
        },
    ]
}

#[tokio::test]
#[ignore = "requires TLS fixtures and envoy"]
async fn mtls_xds_matrix() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping mtls xds (set RUN_E2E=1 to enable)");
        return;
    }
    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping");
        return;
    }

    let Some(fx) = TlsFixtures::load() else {
        eprintln!("TLS fixtures not found; generate per README to enable");
        return;
    };

    for case in cases() {
        // Reserve ports
        let mut ports = PortAllocator::new();
        let envoy_admin = ports.reserve_labeled("envoy-admin");
        let echo_upstream = ports.reserve_labeled("echo-upstream");

        // DB per case
        let db_dir = tempfile::tempdir().expect("db dir");
        let db_path = db_dir.path().join(format!("flowplane-e2e-{}.sqlite", case.name));

        // Boot echo upstream
        let echo_addr: std::net::SocketAddr =
            format!("127.0.0.1:{}", echo_upstream).parse().unwrap();
        let mut echo = EchoServerHandle::start(echo_addr).await;

        // Boot CP with xDS TLS
        let api_addr: std::net::SocketAddr =
            format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
        let xds_addr: std::net::SocketAddr =
            format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
        let xds_tls = flowplane::config::XdsTlsConfig {
            cert_path: fx.server_cert.to_string_lossy().to_string(),
            key_path: fx.server_key.to_string_lossy().to_string(),
            client_ca_path: Some(fx.ca.to_string_lossy().to_string()),
            require_client_cert: case.server_verifies_client,
        };
        let _cp = ControlPlaneHandle::start_with_xds_tls(
            db_path.clone(),
            api_addr,
            xds_addr,
            Some(xds_tls),
        )
        .await
        .expect("start cp with xds tls");
        wait_http_ready(api_addr).await;

        // Start Envoy with ADS mTLS config
        let envoy = EnvoyHandle::start_ads_mtls(
            envoy_admin,
            xds_addr.port(),
            case.envoy_presents_client_cert.then_some(fx.client_cert.as_path()),
            case.envoy_presents_client_cert.then_some(fx.client_key.as_path()),
            fx.ca.as_path(),
        )
        .expect("start envoy ads mtls");
        envoy.wait_admin_ready().await;

        // Create API via Platform API to force config push
        let namer = UniqueNamer::for_test(case.name);
        let domain = namer.domain();
        let base_path = namer.base_path();
        let route_path = namer.path("echo");

        let token =
            create_pat(vec!["routes:write", "routes:read", "listeners:read", "clusters:read"])
                .await
                .expect("pat");
        let endpoint = format!("127.0.0.1:{}", echo_addr.port());
        let _resp = post_create_api(
            api_addr,
            &token,
            "e2e",
            &domain,
            &base_path,
            &namer.test_id(),
            &endpoint,
        )
        .await
        .expect("create api");

        // Give some time for ADS sync and inspect stats
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let stats = envoy.get_stats().await.expect("stats");
        let cds_ok = find_counter(&stats, "cluster_manager.cds.update_success").unwrap_or(0);
        let cds_fail = find_counter(&stats, "cluster_manager.cds.update_failure").unwrap_or(0);
        let lds_ok = find_counter(&stats, "listener_manager.lds.update_success").unwrap_or(0);
        let lds_fail = find_counter(&stats, "listener_manager.lds.update_failure").unwrap_or(0);

        if case.expect_success {
            assert!(cds_ok > 0 && lds_ok > 0, "expected ADS success for case {}", case.name);
            assert_eq!(cds_fail, 0, "expected no CDS failures for {}", case.name);
            assert_eq!(lds_fail, 0, "expected no LDS failures for {}", case.name);
            // Optional: verify routing via HTTP (default gateway listener)
            let mut ok = false;
            for _ in 0..40 {
                match EnvoyHandle::proxy_get(&envoy, &domain, &route_path).await {
                    Ok((200, body)) if body.starts_with("echo:") => {
                        ok = true;
                        break;
                    }
                    _ => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
                }
            }
            assert!(ok, "routing did not converge for {}", case.name);
        } else {
            // Failure expected: zero successes or >0 failures
            assert!(
                cds_ok == 0 || lds_ok == 0 || cds_fail > 0 || lds_fail > 0,
                "expected ADS failure for {}",
                case.name
            );
        }

        // Cleanup echo for this case
        echo.stop().await;
    }
}

fn find_counter(stats: &str, key: &str) -> Option<u64> {
    for line in stats.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == key {
                return v.trim().parse::<u64>().ok();
            }
        }
    }
    None
}
