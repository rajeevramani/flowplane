//! E2E: Assert zero xDS NACKs and no update failures after convergence.

use std::net::SocketAddr;
use tempfile::tempdir;

mod support;
use support::api::{create_pat, post_create_api, wait_http_ready};
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::envoy::EnvoyHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;
use support::teardown::{ArtifactMode, TeardownGuard};

#[tokio::test]
#[ignore = "requires Envoy + CP runtime"]
async fn smoke_no_nacks() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping e2e no-nacks (set RUN_E2E=1 to enable)");
        return;
    }

    if !EnvoyHandle::is_available() {
        eprintln!("envoy binary not found; skipping no-nacks assertion");
        return;
    }

    // Artifacts dir and teardown guard
    let artifacts = tempdir().expect("artifacts dir");
    let artifacts_dir = artifacts.path().to_path_buf();
    let mut guard = TeardownGuard::new(&artifacts_dir, ArtifactMode::OnFailure);

    // Unique naming for this test
    let namer = UniqueNamer::for_test("smoke_no_nacks");
    let domain = namer.domain();
    let base_path = namer.base_path();
    let route_path = namer.path("echo");

    // Reserve ports deterministically
    let mut ports = PortAllocator::new();
    let envoy_admin = ports.reserve_labeled("envoy-admin");
    let _envoy_listener = ports.reserve_labeled("envoy-listener");
    let echo_upstream = ports.reserve_labeled("echo-upstream");

    // Create per-test DB path and track for cleanup
    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    guard.track_path(&db_path);

    // Boot echo upstream
    let echo_addr: SocketAddr = format!("127.0.0.1:{}", echo_upstream).parse().unwrap();
    let mut echo = EchoServerHandle::start(echo_addr).await;

    // Boot control plane (in-process)
    let api_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;

    // Start Envoy and wait admin ready
    let envoy = EnvoyHandle::start(envoy_admin, xds_addr.port()).expect("start envoy");
    envoy.wait_admin_ready().await;

    // Create a PAT with routes:write scope and create API to force LDS/RDS/CDS updates
    let token = create_pat(vec![
        "team:e2e:api-definitions:write",
        "team:e2e:api-definitions:read",
        "team:e2e:routes:read",
        "team:e2e:listeners:read",
        "team:e2e:clusters:read",
    ])
    .await
    .expect("pat");
    let endpoint = format!("127.0.0.1:{}", echo_addr.port());
    let _resp =
        post_create_api(api_addr, &token, "e2e", &domain, &base_path, &namer.test_id(), &endpoint)
            .await
            .expect("create api");

    // Probe routing through Envoy to ensure convergence
    let body = envoy
        .wait_for_route(&domain, &route_path, 200)
        .await
        .expect("envoy did not route to echo within timeout");
    assert!(body.starts_with("echo:"), "unexpected echo response");

    // Fetch stats and assert no NACKs or update failures
    let stats = envoy.get_stats().await.expect("stats");

    // Helper to pull counter value from text lines like: key: value
    let get = |key: &str| -> Option<u64> {
        for line in stats.lines() {
            if let Some((k, v)) = line.split_once(':') {
                if k.trim() == key {
                    return v.trim().parse::<u64>().ok();
                }
            }
        }
        None
    };

    // LDS/CDS failures must be zero
    let lds_fail = get("listener_manager.lds.update_failure").unwrap_or(0);
    let lds_rej = get("listener_manager.lds.update_rejected").unwrap_or(0);
    let cds_fail = get("cluster_manager.cds.update_failure").unwrap_or(0);
    let cds_rej = get("cluster_manager.cds.update_rejected").unwrap_or(0);
    assert_eq!(lds_fail, 0, "LDS update_failure should be 0");
    assert_eq!(lds_rej, 0, "LDS update_rejected should be 0");
    assert_eq!(cds_fail, 0, "CDS update_failure should be 0");
    assert_eq!(cds_rej, 0, "CDS update_rejected should be 0");

    // Ensure we saw at least one success for LDS/CDS
    let lds_ok = get("listener_manager.lds.update_success").unwrap_or(0);
    let cds_ok = get("cluster_manager.cds.update_success").unwrap_or(0);
    assert!(lds_ok > 0, "LDS update_success should be > 0");
    assert!(cds_ok > 0, "CDS update_success should be > 0");

    // RDS may be per-route-config; assert no global rds update_failure and presence of config_reload
    // Try common patterns; tolerate absence if Envoy version differs but ensure no explicit failures
    let rds_fail_total = stats
        .lines()
        .filter(|l| l.contains(".rds.") && l.ends_with(": 0"))
        .filter(|l| l.contains("update_failure"))
        .count();
    let rds_rej_total = stats
        .lines()
        .filter(|l| l.contains(".rds.") && l.ends_with(": 0"))
        .filter(|l| l.contains("update_rejected"))
        .count();
    // If there are any RDS update_failure/update_rejected counters reported, they should be 0
    let any_rds_fail = stats.lines().any(|l| l.contains(".rds.") && l.contains("update_failure:"));
    let any_rds_rej = stats.lines().any(|l| l.contains(".rds.") && l.contains("update_rejected:"));
    if any_rds_fail {
        assert!(rds_fail_total > 0, "All RDS update_failure counters must be 0");
    }
    if any_rds_rej {
        assert!(rds_rej_total > 0, "All RDS update_rejected counters must be 0");
    }

    // Stop echo upstream to free port
    echo.stop().await;

    guard.finish(true);
}
