//! RED service/transaction contract for fpv2-7f3.7 dataplane retirement.
//!
//! This target was authored from the approved design and plan without reading production source.
//! It drives one centralized wished-for public service adapter and observes only real PostgreSQL
//! rows, operator audit evidence, and transactional outbox events. Fixtures are UUIDv7-isolated.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use fp_core::{GrantSet, PrincipalCtx};
use fp_domain::authz::TeamRef;
use fp_domain::{OrgRole, RequestId};
use fp_storage::repos::identity;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Barrier;

const RETIRED_EVENT: &str = "dataplane.retired";
const REVOKED_EVENT: &str = "proxy_certificate.revoked";
const RETIRE_ACTION: &str = "dataplane.retire";

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7().simple())
}

struct World {
    pool: PgPool,
    ctx: PrincipalCtx,
    team: TeamRef,
    dataplane_id: uuid::Uuid,
    name: String,
    revision: i64,
    certificate_ids: Vec<uuid::Uuid>,
    serials: Vec<String>,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 12)
        .await
        .expect("connect real PostgreSQL");
    fp_storage::migrate(&pool)
        .await
        .expect("migrate PostgreSQL");
    Some(seed_world(pool).await)
}

async fn seed_world(pool: PgPool) -> World {
    let org = identity::create_org(&pool, &unique("retire-org"), "")
        .await
        .expect("organization fixture");
    let team_row = identity::create_team(&pool, org.id, &unique("retire-team"), "")
        .await
        .expect("team fixture");
    let user = identity::upsert_user_by_subject(
        &pool,
        &unique("retire-operator"),
        "retire-operator@test.invalid",
        "Retirement Operator",
    )
    .await
    .expect("operator fixture");
    identity::add_org_membership(&pool, user, org.id, OrgRole::Admin)
        .await
        .expect("operator membership");
    let ctx = PrincipalCtx::User {
        user_id: user,
        platform_admin: false,
        org_selector_required: false,
        org: Some((org.id, OrgRole::Admin)),
        grants: GrantSet::default(),
    };
    let team = TeamRef {
        id: team_row.id,
        org_id: org.id,
    };
    let name = unique("retire-dataplane");
    let dataplane = fp_core::services::dataplanes::create_dataplane(
        &pool,
        &ctx,
        team,
        &name,
        "retirement contract fixture",
        RequestId::generate(),
    )
    .await
    .expect("dataplane fixture");
    let revision: i64 = sqlx::query_scalar("SELECT version FROM dataplanes WHERE id = $1")
        .bind(dataplane.id.as_uuid())
        .fetch_one(&pool)
        .await
        .expect("dataplane revision");

    let mut certificate_ids = Vec::new();
    let mut serials = Vec::new();
    for label in ["old", "replacement"] {
        let id = uuid::Uuid::now_v7();
        let serial = uuid::Uuid::now_v7().simple().to_string();
        sqlx::query(
            "INSERT INTO proxy_certificates \
             (id, team_id, dataplane_id, spiffe_uri, serial_number, fingerprint_sha256, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, now() + interval '1 hour')",
        )
        .bind(id)
        .bind(team.id.as_uuid())
        .bind(dataplane.id.as_uuid())
        .bind(format!("spiffe://flowplane.test/{label}/{}", dataplane.id))
        .bind(&serial)
        .bind(format!("{}{}", uuid::Uuid::now_v7().simple(), uuid::Uuid::now_v7().simple()))
        .execute(&pool)
        .await
        .expect("active credential fixture");
        certificate_ids.push(id);
        serials.push(serial);
    }

    World {
        pool,
        ctx,
        team,
        dataplane_id: dataplane.id.as_uuid(),
        name,
        revision,
        certificate_ids,
        serials,
    }
}

/// Wished-for slice-.7 public mutation. Representation choices are intentionally localized here.
async fn retire(
    pool: &PgPool,
    ctx: &PrincipalCtx,
    team: TeamRef,
    dataplane: &str,
    expected_revision: i64,
    reason: &str,
    request_id: RequestId,
) -> fp_domain::DomainResult<()> {
    let _retired: fp_domain::Dataplane = fp_core::services::dataplanes::retire_dataplane(
        pool,
        ctx,
        team,
        dataplane,
        expected_revision,
        reason,
        request_id,
    )
    .await?;
    Ok(())
}

async fn event_count(
    pool: &PgPool,
    team_id: uuid::Uuid,
    kind: &str,
    dataplane_id: uuid::Uuid,
) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE team_id = $1 AND event_type = $2 \
         AND payload->>'dataplane_id' = $3",
    )
    .bind(team_id)
    .bind(kind)
    .bind(dataplane_id.to_string())
    .fetch_one(pool)
    .await
    .expect("event count")
}

async fn certificate_event_count(pool: &PgPool, kind: &str, certificate_ids: &[uuid::Uuid]) -> i64 {
    let certificate_ids = certificate_ids
        .iter()
        .map(uuid::Uuid::to_string)
        .collect::<Vec<_>>();
    sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE event_type = $1 \
         AND payload->>'certificate_id' = ANY($2)",
    )
    .bind(kind)
    .bind(certificate_ids)
    .fetch_one(pool)
    .await
    .expect("certificate event count")
}

#[tokio::test]
async fn retirement_tombstones_bulk_revokes_and_records_exact_transactional_evidence() {
    let Some(w) = world().await else { return };
    let request_id = RequestId::generate();
    retire(
        &w.pool,
        &w.ctx,
        w.team,
        &w.name,
        w.revision,
        "operator decommission",
        request_id,
    )
    .await
    .expect("authorized retirement");

    let tombstone: (Option<chrono::DateTime<chrono::Utc>>, Option<String>) =
        sqlx::query_as("SELECT retired_at, retired_reason FROM dataplanes WHERE id = $1")
            .bind(w.dataplane_id)
            .fetch_one(&w.pool)
            .await
            .expect("retirement tombstone");
    assert!(tombstone.0.is_some());
    assert_eq!(tombstone.1.as_deref(), Some("operator decommission"));

    let revoked: Vec<(
        uuid::Uuid,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT id, revoked_at, revoked_reason FROM proxy_certificates \
             WHERE dataplane_id = $1 ORDER BY id",
    )
    .bind(w.dataplane_id)
    .fetch_all(&w.pool)
    .await
    .expect("bulk-revoked credentials");
    assert_eq!(revoked.len(), 2);
    assert!(revoked.iter().all(|row| row.1.is_some()));
    assert!(revoked
        .iter()
        .all(|row| row.2.as_deref() == Some("dataplane retired")));

    assert_eq!(
        event_count(&w.pool, w.team.id.as_uuid(), RETIRED_EVENT, w.dataplane_id).await,
        1
    );
    assert_eq!(
        certificate_event_count(&w.pool, REVOKED_EVENT, &w.certificate_ids).await,
        2
    );
    let audit: (String, String, Option<uuid::Uuid>, serde_json::Value) = sqlx::query_as(
        "SELECT action, outcome, team_id, detail FROM audit_log WHERE request_id = $1",
    )
    .bind(request_id.as_uuid())
    .fetch_one(&w.pool)
    .await
    .expect("one operator audit row");
    assert_eq!(audit.0, RETIRE_ACTION);
    assert_eq!(audit.1, "success");
    assert_eq!(audit.2, Some(w.team.id.as_uuid()));
    assert_eq!(audit.3["dataplane_id"], w.dataplane_id.to_string());
}

#[tokio::test]
async fn stale_revision_rolls_back_tombstone_revocations_audit_and_outbox() {
    let Some(w) = world().await else { return };
    let request_id = RequestId::generate();
    let result = retire(
        &w.pool,
        &w.ctx,
        w.team,
        &w.name,
        w.revision + 1,
        "stale writer",
        request_id,
    )
    .await;
    assert!(result.is_err(), "stale expected revision must conflict");

    let retired: bool =
        sqlx::query_scalar("SELECT retired_at IS NOT NULL FROM dataplanes WHERE id = $1")
            .bind(w.dataplane_id)
            .fetch_one(&w.pool)
            .await
            .expect("dataplane state");
    assert!(!retired);
    let revoked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM proxy_certificates WHERE dataplane_id = $1 AND revoked_at IS NOT NULL",
    )
    .bind(w.dataplane_id)
    .fetch_one(&w.pool)
    .await
    .expect("revocation count");
    assert_eq!(revoked, 0);
    let audit_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE request_id = $1")
            .bind(request_id.as_uuid())
            .fetch_one(&w.pool)
            .await
            .expect("failure audit count");
    assert_eq!(audit_count, 0, "failed retirement must not audit success");
    assert_eq!(
        event_count(&w.pool, w.team.id.as_uuid(), RETIRED_EVENT, w.dataplane_id).await,
        0,
        "failed retirement must not emit a retirement event"
    );
    assert_eq!(
        certificate_event_count(&w.pool, REVOKED_EVENT, &w.certificate_ids).await,
        0,
        "failed retirement must not emit credential revocation events"
    );
}

#[tokio::test]
async fn explicit_revoke_losing_retirement_race_is_classified_and_emits_no_duplicate_event() {
    let Some(w) = world().await else { return };
    let barrier = Arc::new(Barrier::new(2));
    let retire_pool = w.pool.clone();
    let retire_ctx = w.ctx.clone();
    let retire_barrier = Arc::clone(&barrier);
    let name = w.name.clone();
    let team = w.team;
    let revision = w.revision;
    let retire_task = tokio::spawn(async move {
        retire_barrier.wait().await;
        retire(
            &retire_pool,
            &retire_ctx,
            team,
            &name,
            revision,
            "race retirement",
            RequestId::generate(),
        )
        .await
    });

    let revoke_pool = w.pool.clone();
    let revoke_ctx = w.ctx.clone();
    let revoke_barrier = Arc::clone(&barrier);
    let serial = w.serials[0].clone();
    let revoke_task = tokio::spawn(async move {
        revoke_barrier.wait().await;
        fp_core::services::dataplanes::revoke_certificate(
            &revoke_pool,
            &revoke_ctx,
            team,
            &serial,
            "explicit race loser",
            RequestId::generate(),
        )
        .await
    });

    let retired = retire_task.await.expect("retire task");
    let revoked = revoke_task.await.expect("revoke task");
    assert!(
        retired.is_ok(),
        "retirement must win this lifecycle race: {retired:?}"
    );
    if let Err(error) = revoked {
        let error_text = error.to_string().to_ascii_lowercase();
        assert!(
            error_text.contains("already revoked") || error_text.contains("retired"),
            "revoke losing retirement needs a classified conflict: {error_text}"
        );
    }

    for certificate_id in &w.certificate_ids {
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE event_type = $1 \
             AND payload->>'certificate_id' = $2",
        )
        .bind(REVOKED_EVENT)
        .bind(certificate_id.to_string())
        .fetch_one(&w.pool)
        .await
        .expect("certificate revocation event count");
        assert_eq!(events, 1, "each changed credential emits exactly one event");
    }
}

#[tokio::test]
async fn retired_dataplane_rejects_issue_and_explicit_revoke_without_new_evidence() {
    let Some(w) = world().await else { return };
    retire(
        &w.pool,
        &w.ctx,
        w.team,
        &w.name,
        w.revision,
        "mutation closure",
        RequestId::generate(),
    )
    .await
    .expect("retire fixture");
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE team_id = $1 AND payload->>'dataplane_id' = $2",
    )
    .bind(w.team.id.as_uuid())
    .bind(w.dataplane_id.to_string())
    .fetch_one(&w.pool)
    .await
    .expect("baseline event count");

    let issue = fp_core::services::dataplanes::issue_certificate(
        &w.pool,
        &w.ctx,
        w.team,
        fp_core::services::dataplanes::CertificateIssueRequest {
            dataplane: &w.name,
            ttl_hours: 1,
        },
        RequestId::generate(),
    )
    .await;
    assert!(
        issue.is_err(),
        "certificate issuance must reject a retired dataplane"
    );
    let revoke = fp_core::services::dataplanes::revoke_certificate(
        &w.pool,
        &w.ctx,
        w.team,
        &w.serials[0],
        "after retirement",
        RequestId::generate(),
    )
    .await;
    assert!(
        revoke.is_err(),
        "explicit revoke must classify a retired dataplane"
    );

    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE team_id = $1 AND payload->>'dataplane_id' = $2",
    )
    .bind(w.team.id.as_uuid())
    .bind(w.dataplane_id.to_string())
    .fetch_one(&w.pool)
    .await
    .expect("final event count");
    assert_eq!(
        after, before,
        "rejected post-retirement mutations emit no events"
    );
}
