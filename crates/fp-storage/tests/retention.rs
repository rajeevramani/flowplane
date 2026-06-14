#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::authz::TeamRef;
use fp_storage::repos::{api_lifecycle, identity, secrets, xds_nacks};
use sqlx::types::chrono::Utc;
use sqlx::{PgPool, Row};

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

struct World {
    pool: PgPool,
    team_a: TeamRef,
    team_b: TeamRef,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");

    let org_a = identity::create_org(&pool, &unique("org-a"), "")
        .await
        .expect("org a");
    let org_b = identity::create_org(&pool, &unique("org-b"), "")
        .await
        .expect("org b");
    let team_a = identity::create_team(&pool, org_a.id, &unique("team-a"), "")
        .await
        .expect("team a");
    let team_b = identity::create_team(&pool, org_b.id, &unique("team-b"), "")
        .await
        .expect("team b");

    Some(World {
        pool,
        team_a: TeamRef {
            id: team_a.id,
            org_id: org_a.id,
        },
        team_b: TeamRef {
            id: team_b.id,
            org_id: org_b.id,
        },
    })
}

#[tokio::test]
async fn expired_secret_reaper_is_team_scoped() {
    let Some(w) = world().await else { return };
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO secrets \
           (id, team_id, org_id, name, description, secret_type, configuration_encrypted, nonce, \
            encryption_key_id, expires_at) \
         VALUES \
           ($1, $2, $3, $4, '', 'generic_secret', 'cipher'::bytea, '123456789012'::bytea, 'default', $10::timestamptz - interval '5 minutes'), \
           ($5, $2, $3, $6, '', 'generic_secret', 'cipher'::bytea, '123456789012'::bytea, 'default', $10::timestamptz + interval '5 minutes'), \
           ($7, $8, $9, $11, '', 'generic_secret', 'cipher'::bytea, '123456789012'::bytea, 'default', $10::timestamptz - interval '5 minutes')",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(w.team_a.id.as_uuid())
    .bind(w.team_a.org_id.as_uuid())
    .bind(unique("expired-a"))
    .bind(uuid::Uuid::now_v7())
    .bind(unique("current-a"))
    .bind(uuid::Uuid::now_v7())
    .bind(w.team_b.id.as_uuid())
    .bind(w.team_b.org_id.as_uuid())
    .bind(now)
    .bind(unique("expired-b"))
    .execute(&w.pool)
    .await
    .expect("insert secrets");

    let deleted = secrets::delete_expired_for_team(&w.pool, w.team_a.id, now)
        .await
        .expect("reap secrets");
    assert_eq!(deleted, 1);
    assert_eq!(
        secrets::count_for_team(&w.pool, w.team_a.id)
            .await
            .expect("count a"),
        1
    );
    assert_eq!(
        secrets::count_for_team(&w.pool, w.team_b.id)
            .await
            .expect("count b"),
        1
    );
}

#[tokio::test]
async fn expired_raw_observation_reaper_is_team_scoped() {
    let Some(w) = world().await else { return };
    let now = Utc::now();
    let route_a = uuid::Uuid::now_v7();
    let route_b = uuid::Uuid::now_v7();
    let session_a = uuid::Uuid::now_v7();
    let session_b = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO route_configs (id, team_id, org_id, name, spec) VALUES \
           ($1, $2, $3, $4, '{\"virtual_hosts\":[]}'::jsonb), \
           ($5, $6, $7, $8, '{\"virtual_hosts\":[]}'::jsonb); \
         INSERT INTO capture_sessions \
           (id, team_id, org_id, name, status, route_config_id, target_sample_count, \
            max_duration_seconds, max_bytes, max_distinct_paths) \
         VALUES \
           ($9, $2, $3, $10, 'capturing', $1, 10, 60, 4096, 10), \
           ($11, $6, $7, $12, 'capturing', $5, 10, 60, 4096, 10); \
         INSERT INTO raw_observations \
           (id, team_id, org_id, capture_session_id, request_id, method, path, \
            request_headers, response_headers, metadata_seen, observed_at, expires_at) \
         VALUES \
           ($13, $2, $3, $9, 'expired-a', 'GET', '/expired', '{}'::jsonb, '{}'::jsonb, true, $16, $16::timestamptz - interval '5 minutes'), \
           ($14, $2, $3, $9, 'current-a', 'GET', '/current', '{}'::jsonb, '{}'::jsonb, true, $16, $16::timestamptz + interval '5 minutes'), \
           ($15, $6, $7, $11, 'expired-b', 'GET', '/expired', '{}'::jsonb, '{}'::jsonb, true, $16, $16::timestamptz - interval '5 minutes')",
    )
    .bind(route_a)
    .bind(w.team_a.id.as_uuid())
    .bind(w.team_a.org_id.as_uuid())
    .bind(unique("rc-a"))
    .bind(route_b)
    .bind(w.team_b.id.as_uuid())
    .bind(w.team_b.org_id.as_uuid())
    .bind(unique("rc-b"))
    .bind(session_a)
    .bind(unique("session-a"))
    .bind(session_b)
    .bind(unique("session-b"))
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::now_v7())
    .bind(now)
    .execute(&w.pool)
    .await
    .expect("insert raw observations");

    let deleted =
        api_lifecycle::delete_expired_raw_observations_for_team(&w.pool, w.team_a.id, now)
            .await
            .expect("reap raw observations");
    assert_eq!(deleted, 1);

    let count_a: i64 = sqlx::query("SELECT count(*) FROM raw_observations WHERE team_id = $1")
        .bind(w.team_a.id.as_uuid())
        .fetch_one(&w.pool)
        .await
        .expect("count a")
        .get(0);
    let count_b: i64 = sqlx::query("SELECT count(*) FROM raw_observations WHERE team_id = $1")
        .bind(w.team_b.id.as_uuid())
        .fetch_one(&w.pool)
        .await
        .expect("count b")
        .get(0);
    assert_eq!(count_a, 1);
    assert_eq!(count_b, 1);
}

#[tokio::test]
async fn old_xds_nack_reaper_is_team_scoped() {
    let Some(w) = world().await else { return };
    let cutoff = Utc::now();
    sqlx::query(
        "INSERT INTO xds_nack_events \
           (id, team_id, org_id, node_id, type_url, version_rejected, error_message, created_at) \
         VALUES \
           ($1, $2, $3, 'node-a', 'type.googleapis.com/envoy.config.listener.v3.Listener', '1', 'old a', $4::timestamptz - interval '1 minute'), \
           ($5, $2, $3, 'node-a', 'type.googleapis.com/envoy.config.listener.v3.Listener', '2', 'new a', $4::timestamptz + interval '1 minute'), \
           ($6, $7, $8, 'node-b', 'type.googleapis.com/envoy.config.listener.v3.Listener', '1', 'old b', $4::timestamptz - interval '1 minute')",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(w.team_a.id.as_uuid())
    .bind(w.team_a.org_id.as_uuid())
    .bind(cutoff)
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::now_v7())
    .bind(w.team_b.id.as_uuid())
    .bind(w.team_b.org_id.as_uuid())
    .execute(&w.pool)
    .await
    .expect("insert nacks");

    let deleted = xds_nacks::delete_older_than_for_team(&w.pool, w.team_a.id, cutoff)
        .await
        .expect("reap nacks");
    assert_eq!(deleted, 1);

    let count_a: i64 = sqlx::query("SELECT count(*) FROM xds_nack_events WHERE team_id = $1")
        .bind(w.team_a.id.as_uuid())
        .fetch_one(&w.pool)
        .await
        .expect("count a")
        .get(0);
    let count_b: i64 = sqlx::query("SELECT count(*) FROM xds_nack_events WHERE team_id = $1")
        .bind(w.team_b.id.as_uuid())
        .fetch_one(&w.pool)
        .await
        .expect("count b")
        .get(0);
    assert_eq!(count_a, 1);
    assert_eq!(count_b, 1);
}
