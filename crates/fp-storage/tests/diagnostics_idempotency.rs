//! Regression coverage for diagnostics-report idempotency and dataplane liveness.

#![allow(clippy::expect_used)]

use fp_domain::authz::TeamRef;
use fp_storage::repos::{dataplanes, identity};
use sqlx::PgPool;
use std::time::Duration;

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

struct World {
    pool: PgPool,
    team: TeamRef,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let org = identity::create_org(&pool, &unique("diagnostics-idempotency-org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("diagnostics-idempotency-team"), "")
        .await
        .expect("team");

    Some(World {
        pool,
        team: TeamRef {
            id: team.id,
            org_id: org.id,
        },
    })
}

#[tokio::test]
async fn replayed_diagnostics_report_does_not_advance_counters_or_liveness() {
    let Some(w) = world().await else { return };
    let dataplane_name = unique("diagnostics-idempotency-dataplane");
    let mut tx = w.pool.begin().await.expect("create dataplane transaction");
    let dataplane = dataplanes::create_dataplane(&mut tx, w.team, &dataplane_name, "")
        .await
        .expect("dataplane");
    tx.commit().await.expect("commit dataplane");

    let first_key = unique("report-x");
    let second_key = unique("report-y");
    let delta = dataplanes::TelemetryDelta {
        idempotency_key: &first_key,
        requests_delta: 17,
        errors_delta: 3,
        warming_failures_delta: 2,
        config_verified: false,
    };

    let first = dataplanes::record_telemetry_by_id(&w.pool, w.team.id, dataplane.id, delta)
        .await
        .expect("apply report X");
    let first_heartbeat = first
        .last_heartbeat_at
        .expect("a newly accepted report records a heartbeat");
    assert_eq!(first.total_requests, 17);
    assert_eq!(first.total_errors, 3);
    assert_eq!(first.warming_failures, 2);

    tokio::time::sleep(Duration::from_millis(25)).await;
    let replay = dataplanes::record_telemetry_by_id(&w.pool, w.team.id, dataplane.id, delta)
        .await
        .expect("replay report X");
    assert_eq!(
        replay.total_requests, first.total_requests,
        "replaying X must not count requests twice"
    );
    assert_eq!(
        replay.total_errors, first.total_errors,
        "replaying X must not count errors twice"
    );
    assert_eq!(
        replay.warming_failures, first.warming_failures,
        "replaying X must not count warming failures twice"
    );
    assert_eq!(
        replay.last_heartbeat_at,
        Some(first_heartbeat),
        "an idempotent replay must not refresh dataplane liveness"
    );

    tokio::time::sleep(Duration::from_millis(25)).await;
    let second = dataplanes::record_telemetry_by_id(
        &w.pool,
        w.team.id,
        dataplane.id,
        dataplanes::TelemetryDelta {
            idempotency_key: &second_key,
            ..delta
        },
    )
    .await
    .expect("apply report Y");
    assert_eq!(second.total_requests, first.total_requests + 17);
    assert_eq!(second.total_errors, first.total_errors + 3);
    assert_eq!(second.warming_failures, first.warming_failures + 2);
    assert!(
        second
            .last_heartbeat_at
            .expect("new report Y records a heartbeat")
            > first_heartbeat,
        "a newly accepted report must advance dataplane liveness"
    );
}
