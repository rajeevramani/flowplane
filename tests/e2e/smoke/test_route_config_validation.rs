//! E2E smoke tests for route-config POST validation (fp-dq0).
//!
//! Regression coverage for the bug where POSTing a route config that referenced a
//! non-existent cluster returned 409 "already exists" (mislabeled FK violation).
//! The fix adds pre-flight cluster-existence validation at the handler layer.
//!
//! ```bash
//! FLOWPLANE_E2E_AUTH_MODE=dev RUN_E2E=1 cargo test --test e2e dev_route_config_validation -- --ignored --nocapture
//! ```

use crate::common::harness::dev_harness;
use serde_json::json;

fn route_config_body(name: &str, clusters: &[&str], weighted: bool) -> serde_json::Value {
    let action = if weighted {
        let weight = 100 / clusters.len().max(1) as u32;
        json!({
            "type": "weighted",
            "clusters": clusters.iter().map(|c| json!({"name": c, "weight": weight})).collect::<Vec<_>>()
        })
    } else {
        json!({"type": "forward", "cluster": clusters[0]})
    };

    json!({
        "name": name,
        "virtualHosts": [{
            "name": format!("{name}-vh"),
            "domains": ["*"],
            "routes": [{
                "name": format!("{name}-r"),
                "match": {"path": {"type": "prefix", "value": "/"}},
                "action": action
            }]
        }]
    })
}

#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_route_config_missing_cluster_returns_400() {
    let harness = dev_harness("rc_miss_cluster").await.expect("harness should start");
    let team = &harness.team;

    let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let rc_name = format!("rc-{id}");
    let missing_cluster = format!("does-not-exist-{id}");

    let body = route_config_body(&rc_name, &[&missing_cluster], false);
    let resp = harness
        .authed_post(&format!("/api/v1/teams/{team}/route-configs"), &body)
        .await
        .expect("request sent");

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    assert_eq!(status.as_u16(), 400, "expected 400 for missing cluster, got {status}: {text}");
    assert!(
        text.contains("Referenced cluster(s) do not exist"),
        "expected missing-cluster message, got: {text}"
    );
    assert!(text.contains(&missing_cluster), "expected missing cluster name in message: {text}");
}

#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_route_config_weighted_multiple_missing_clusters_all_named() {
    let harness = dev_harness("rc_miss_weighted").await.expect("harness should start");
    let team = &harness.team;

    let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let rc_name = format!("rc-w-{id}");
    let missing_a = format!("gone-a-{id}");
    let missing_b = format!("gone-b-{id}");

    let body = route_config_body(&rc_name, &[&missing_a, &missing_b], true);
    let resp = harness
        .authed_post(&format!("/api/v1/teams/{team}/route-configs"), &body)
        .await
        .expect("request sent");

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    assert_eq!(status.as_u16(), 400, "expected 400, got {status}: {text}");
    assert!(text.contains(&missing_a), "expected '{missing_a}' in message: {text}");
    assert!(text.contains(&missing_b), "expected '{missing_b}' in message: {text}");
}

#[tokio::test]
#[ignore = "requires RUN_E2E=1 and FLOWPLANE_E2E_AUTH_MODE=dev"]
async fn dev_route_config_duplicate_name_returns_409_already_exists() {
    // Regression test: unique-name conflict must still produce 409 with a clear
    // "already exists" message (not the vague "constraint violation" wording that
    // showed up briefly during the fp-dq0 fix).
    let harness = dev_harness("rc_dup_name").await.expect("harness should start");
    let team = &harness.team;

    let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let cluster_name = format!("cl-{id}");
    let rc_name = format!("rc-dup-{id}");

    // Create a cluster first so the route config is valid.
    let cluster_body = json!({
        "name": cluster_name,
        "endpoints": [{"host": "127.0.0.1", "port": 8000}]
    });
    let resp = harness
        .authed_post(&format!("/api/v1/teams/{team}/clusters"), &cluster_body)
        .await
        .expect("cluster create request sent");
    assert!(resp.status().is_success(), "cluster create should succeed");

    let body = route_config_body(&rc_name, &[&cluster_name], false);

    let first = harness
        .authed_post(&format!("/api/v1/teams/{team}/route-configs"), &body)
        .await
        .expect("first POST sent");
    assert_eq!(first.status().as_u16(), 201, "first POST should succeed");

    let second = harness
        .authed_post(&format!("/api/v1/teams/{team}/route-configs"), &body)
        .await
        .expect("second POST sent");
    let status = second.status();
    let text = second.text().await.unwrap_or_default();

    assert_eq!(status.as_u16(), 409, "expected 409 for duplicate, got {status}: {text}");
    assert!(text.contains("already exists"), "expected 'already exists' wording, got: {text}");
    assert!(text.contains(&rc_name), "expected route config name in message: {text}");
}
