//! Negative: duplicate domains cause conflicts; second create yields 409 and no partial writes.

use tempfile::tempdir;

mod support;
use support::api::{create_pat, ensure_team_exists, post_create_api, wait_http_ready};
use support::echo::EchoServerHandle;
use support::env::ControlPlaneHandle;
use support::naming::UniqueNamer;
use support::ports::PortAllocator;

#[tokio::test]
#[ignore = "requires CP runtime"]
async fn negative_conflicts_duplicate_domain() {
    if std::env::var("RUN_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping negative conflicts (set RUN_E2E=1 to enable)");
        return;
    }

    let mut ports = PortAllocator::new();
    let echo_upstream = ports.reserve_labeled("echo-upstream");
    let api_addr: std::net::SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("api")).parse().unwrap();
    let xds_addr: std::net::SocketAddr =
        format!("127.0.0.1:{}", ports.reserve_labeled("xds")).parse().unwrap();
    let db_dir = tempdir().expect("db dir");
    let db_path = db_dir.path().join("flowplane-e2e.sqlite");
    let _cp =
        ControlPlaneHandle::start(db_path.clone(), api_addr, xds_addr).await.expect("start cp");
    wait_http_ready(api_addr).await;

    // Ensure the e2e team exists before creating API definitions
    ensure_team_exists("e2e").await.expect("create e2e team");

    // Boot echo upstream (not strictly required, but handy for endpoint)
    let echo_addr: std::net::SocketAddr = format!("127.0.0.1:{}", echo_upstream).parse().unwrap();
    let mut echo = EchoServerHandle::start(echo_addr).await;

    let namer = UniqueNamer::for_test("negative_conflicts_duplicate_domain");
    let domain = namer.domain();
    let route_path = namer.path("echo");
    let endpoint = format!("127.0.0.1:{}", echo_addr.port());
    let token = create_pat(vec!["team:e2e:openapi-import:write", "team:e2e:openapi-import:read"])
        .await
        .expect("pat");

    // First create should succeed
    let _res1 =
        post_create_api(api_addr, &token, "e2e", &domain, &route_path, &namer.test_id(), &endpoint)
            .await
            .expect("create api 1");

    // Second create with same domain should fail with 409
    // Note: In the new OpenAPI import system, duplicate domains are detected via listener names.
    // Since each import creates a unique listener name based on cluster_name, we test by
    // attempting to create with the same listener name, which should fail with a conflict.
    let result2 = post_create_api(
        api_addr,
        &token,
        "e2e",
        &domain, // Same domain
        &route_path,
        &namer.test_id(), // Same cluster name = same listener name
        &endpoint,
    )
    .await;

    // The second import should fail due to duplicate listener name
    assert!(
        result2.is_err(),
        "Second import with duplicate domain/listener should fail, but it succeeded"
    );
    let err_msg = result2.unwrap_err().to_string();
    assert!(
        err_msg.contains("409") || err_msg.contains("conflict") || err_msg.contains("Conflict"),
        "Expected 409 Conflict error, got: {}",
        err_msg
    );

    echo.stop().await;
}
