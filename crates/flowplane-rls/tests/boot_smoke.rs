//! E2E boot smoke for the fail-closed startup contract (fpv2-9sf S1): drives the REAL
//! `flowplane-rls` binary (`CARGO_BIN_EXE_*`) with a controlled environment and asserts the
//! documented refusal/boot behavior. Parallel-safe: loopback binds on distinct ephemeral
//! ports, no shared state.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn rls_cmd(envs: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flowplane-rls"));
    cmd.env_clear();
    // Keep the platform basics the process runner needs.
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    cmd
}

fn stderr_of(child: Child) -> String {
    let out = child.wait_with_output().unwrap();
    assert!(
        !out.status.success(),
        "process must exit non-zero, got {:?}",
        out.status
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// AC8 (breaking-change shape): ZERO-config startup — which previously served wildcard
/// plaintext — now fails closed. The refusal is the intended 3.0.0 tightening.
#[test]
fn zero_config_boot_fails_closed() {
    let child = rls_cmd(&[]).spawn().unwrap();
    let err = stderr_of(child);
    assert!(
        err.contains("FLOWPLANE_RLS_ALLOW_INSECURE_GRPC")
            || err.contains("FLOWPLANE_RLS_GRPC_TLS_CERT"),
        "zero-config refusal must name the fail-closed material/hatch: {err}"
    );
}

/// Non-loopback plaintext gRPC bind: the binary refuses to start and names the TLS material.
#[test]
fn non_loopback_plaintext_refuses_to_boot() {
    let child = rls_cmd(&[("FLOWPLANE_RLS_GRPC_LISTEN", "0.0.0.0:0")])
        .spawn()
        .unwrap();
    let err = stderr_of(child);
    assert!(
        err.contains("FLOWPLANE_RLS_GRPC_TLS_CERT"),
        "refusal must name the missing TLS material: {err}"
    );
}

/// Loopback bind without the explicit escape hatch: still refuses (plaintext is opt-in).
#[test]
fn loopback_plaintext_without_hatch_refuses_to_boot() {
    let child = rls_cmd(&[
        ("FLOWPLANE_RLS_GRPC_LISTEN", "127.0.0.1:0"),
        ("FLOWPLANE_RLS_ADMIN_LISTEN", "127.0.0.1:0"),
    ])
    .spawn()
    .unwrap();
    let err = stderr_of(child);
    assert!(
        err.contains("FLOWPLANE_RLS_ALLOW_INSECURE_GRPC"),
        "refusal must name the escape hatch: {err}"
    );
}

/// The gRPC hatch alone is not enough (S2): the admin listener has its own escape hatch.
#[test]
fn grpc_hatch_alone_refuses_to_boot() {
    let child = rls_cmd(&[
        ("FLOWPLANE_RLS_GRPC_LISTEN", "127.0.0.1:0"),
        ("FLOWPLANE_RLS_ADMIN_LISTEN", "127.0.0.1:0"),
        (
            "FLOWPLANE_RLS_ALLOW_INSECURE_GRPC",
            "yes-this-is-local-only",
        ),
    ])
    .spawn()
    .unwrap();
    let err = stderr_of(child);
    assert!(
        err.contains("FLOWPLANE_RLS_ALLOW_INSECURE_ADMIN"),
        "refusal must name the admin escape hatch: {err}"
    );
}

/// AC8 (documented dev path works end-to-end): with the new loopback defaults + BOTH
/// escape hatches — no TLS, no token — the real binary serves plaintext gRPC (a
/// `ShouldRateLimit` call succeeds) and the admin accepts an unauthenticated policy push
/// (`204`) that is then enforced. This pins the whole documented dev contract, not just
/// process liveness.
#[tokio::test]
async fn dev_path_serves_plaintext_grpc_and_open_admin() {
    use envoy_types::pb::envoy::extensions::common::ratelimit::v3::{
        rate_limit_descriptor::Entry, RateLimitDescriptor,
    };
    use envoy_types::pb::envoy::service::ratelimit::v3::{
        rate_limit_service_client::RateLimitServiceClient, RateLimitRequest,
    };

    // A killed-on-drop child guard: a panic anywhere below must never leak a running RLS
    // process into the other tests (cross-test interference).
    struct ChildGuard(Option<Child>);
    impl ChildGuard {
        fn inner(&mut self) -> &mut Child {
            self.0.as_mut().unwrap()
        }
    }
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(mut child) = self.0.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    // Port strategy: bind BOTH probe listeners simultaneously (guarantees two distinct
    // ports), drop them, spawn the child on those ports. The drop→child-bind window can
    // still race another process, so the whole boot is retried with fresh ports; "address
    // in use" is retryable, any other exit is a real failure.
    let http = reqwest::Client::new();
    let mut booted: Option<(ChildGuard, String, String)> = None;
    'attempts: for attempt in 0..5 {
        let (grpc_listen, admin_listen) = {
            let a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            (
                format!("127.0.0.1:{}", a.local_addr().unwrap().port()),
                format!("127.0.0.1:{}", b.local_addr().unwrap().port()),
            )
        };
        let mut child = ChildGuard(Some(
            rls_cmd(&[
                ("FLOWPLANE_RLS_GRPC_LISTEN", grpc_listen.as_str()),
                ("FLOWPLANE_RLS_ADMIN_LISTEN", admin_listen.as_str()),
                (
                    "FLOWPLANE_RLS_ALLOW_INSECURE_GRPC",
                    "yes-this-is-local-only",
                ),
                (
                    "FLOWPLANE_RLS_ALLOW_INSECURE_ADMIN",
                    "yes-this-is-local-only",
                ),
            ])
            .spawn()
            .unwrap(),
        ));

        let admin_base = format!("http://{admin_listen}");
        for _ in 0..100 {
            if let Some(status) = child.inner().try_wait().unwrap() {
                let out = child.0.take().unwrap().wait_with_output().unwrap();
                let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                if stderr.to_ascii_lowercase().contains("address") {
                    // Lost the rebind race to another process — retry with fresh ports.
                    continue 'attempts;
                }
                panic!("dev-path binary exited {status:?} (attempt {attempt}): {stderr}");
            }
            if let Ok(resp) = http.get(format!("{admin_base}/healthz")).send().await {
                if resp.status() == reqwest::StatusCode::OK {
                    booted = Some((child, grpc_listen, admin_listen));
                    break 'attempts;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if booted.is_none() {
            // Health never answered and the child didn't exit: fail with the guard armed.
            panic!("admin /healthz never came up on {admin_base} (attempt {attempt})");
        }
    }
    let (_child, grpc_listen, admin_listen) =
        booted.expect("could not boot the dev-path RLS after 5 port attempts");
    let admin_base = format!("http://{admin_listen}");

    // Unauthenticated policy push is accepted on the dev path.
    let push = http
        .post(format!("{admin_base}/api/v1/admin/rls/policies"))
        .json(&serde_json::json!({
            "policies": [{
                "domain": "dev|dev|smoke",
                "descriptors": {"k": "v"},
                "requests_per_unit": 1,
                "unit": "minute"
            }]
        }))
        .send()
        .await
        .expect("push reaches the open dev admin");
    assert_eq!(push.status(), reqwest::StatusCode::NO_CONTENT);

    // Plaintext gRPC serves and enforces the pushed policy (rpu=1: OK then OVER_LIMIT).
    let mut grpc = RateLimitServiceClient::connect(format!("http://{grpc_listen}"))
        .await
        .expect("plaintext gRPC dial succeeds on the dev path");
    let request = || RateLimitRequest {
        domain: "dev|dev|smoke".to_string(),
        descriptors: vec![RateLimitDescriptor {
            entries: vec![Entry {
                key: "k".into(),
                value: "v".into(),
            }],
            limit: None,
            hits_addend: None,
        }],
        hits_addend: 0,
    };
    let first = grpc
        .should_rate_limit(request())
        .await
        .expect("first call succeeds")
        .into_inner()
        .overall_code;
    let second = grpc
        .should_rate_limit(request())
        .await
        .expect("second call succeeds")
        .into_inner()
        .overall_code;
    use envoy_types::pb::envoy::service::ratelimit::v3::rate_limit_response::Code;
    assert_eq!(first, Code::Ok as i32, "first call under rpu=1 is OK");
    assert_eq!(
        second,
        Code::OverLimit as i32,
        "second call proves the pushed policy is live"
    );
    // `_child` (ChildGuard) kills the process on drop — including on any panic above.
}

/// The documented loopback dev path (both hatches, ephemeral ports) boots and stays up.
#[test]
fn loopback_dev_path_boots_with_hatch() {
    let mut child = rls_cmd(&[
        ("FLOWPLANE_RLS_GRPC_LISTEN", "127.0.0.1:0"),
        ("FLOWPLANE_RLS_ADMIN_LISTEN", "127.0.0.1:0"),
        (
            "FLOWPLANE_RLS_ALLOW_INSECURE_GRPC",
            "yes-this-is-local-only",
        ),
        (
            "FLOWPLANE_RLS_ALLOW_INSECURE_ADMIN",
            "yes-this-is-local-only",
        ),
    ])
    .spawn()
    .unwrap();
    // Give it time to fail if it is going to; a fail-closed refusal exits within millis.
    std::thread::sleep(Duration::from_millis(700));
    match child.try_wait().unwrap() {
        None => {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        Some(status) => {
            let out = child.wait_with_output().unwrap();
            panic!(
                "dev path must keep running, exited {status:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}
