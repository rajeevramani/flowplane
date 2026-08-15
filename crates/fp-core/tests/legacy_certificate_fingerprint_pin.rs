//! RED contract for fpv2-7f3.3: the service-owned legacy certificate fingerprint pin.
//!
//! This target is intentionally authored from the approved design and slice plan without reading
//! `fp-core` or `fp-storage` production implementation. It drives the wished-for public
//! `fp_core::services::dataplanes::pin_legacy_certificate_fingerprint` seam and observes only the
//! registry row, audit log, and transactional outbox in a real PostgreSQL database.
//!
//! Every shared-database case owns unique org/team/dataplane/certificate rows and performs no
//! global cleanup. The otherwise schema-impossible ambiguity case uses a unique scratch database
//! because slice .3 intentionally retains global SPIFFE uniqueness until slice .6.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::event::DomainEvent;
use fp_domain::RequestId;
use sha2::{Digest, Sha256};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use std::sync::Arc;
use tokio::sync::Barrier;

const PIN_EVENT: &str = "proxy_certificate.fingerprint_pinned";
const PIN_ACTION: &str = "proxy_certificate.fingerprint_pin";
const XDS_ACTOR: &str = "xds-authenticator";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

fn fingerprint(seed: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(std::process::id().to_be_bytes());
    digest.update(seed.as_bytes());
    format!("{:x}", digest.finalize())
}

struct World {
    pool: PgPool,
    org_id: uuid::Uuid,
    team_id: uuid::Uuid,
    dataplane_id: uuid::Uuid,
}

async fn world() -> Option<World> {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = fp_storage::connect(&url, 8).await.expect("connect");
    fp_storage::migrate(&pool).await.expect("migrate");
    Some(seed_world(pool).await)
}

async fn seed_world(pool: PgPool) -> World {
    let org_id = uuid::Uuid::now_v7();
    let team_id = uuid::Uuid::now_v7();
    let dataplane_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(unique("legacy-pin-org"))
        .execute(&pool)
        .await
        .expect("organization fixture");
    sqlx::query("INSERT INTO teams (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(org_id)
        .bind(unique("legacy-pin-team"))
        .execute(&pool)
        .await
        .expect("team fixture");
    sqlx::query("INSERT INTO dataplanes (id, team_id, org_id, name) VALUES ($1, $2, $3, $4)")
        .bind(dataplane_id)
        .bind(team_id)
        .bind(org_id)
        .bind(unique("legacy-pin-dataplane"))
        .execute(&pool)
        .await
        .expect("dataplane fixture");
    World {
        pool,
        org_id,
        team_id,
        dataplane_id,
    }
}

async fn insert_certificate(
    w: &World,
    spiffe_uri: &str,
    serial: &str,
    fingerprint_sha256: Option<&str>,
    expiry_sql: &str,
    revoked: bool,
) -> uuid::Uuid {
    let certificate_id = uuid::Uuid::now_v7();
    let statement = format!(
        "INSERT INTO proxy_certificates \
           (id, team_id, dataplane_id, spiffe_uri, serial_number, fingerprint_sha256, \
            expires_at, revoked_at, revoked_reason) \
         VALUES ($1, $2, $3, $4, $5, $6, {expiry_sql}, \
                 CASE WHEN $7 THEN now() ELSE NULL END, \
                 CASE WHEN $7 THEN 'test revocation' ELSE NULL END)"
    );
    sqlx::query(&statement)
        .bind(certificate_id)
        .bind(w.team_id)
        .bind(w.dataplane_id)
        .bind(spiffe_uri)
        .bind(serial)
        .bind(fingerprint_sha256)
        .bind(revoked)
        .execute(&w.pool)
        .await
        .expect("certificate fixture");
    certificate_id
}

async fn stored_fingerprint(pool: &PgPool, certificate_id: uuid::Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT fingerprint_sha256 FROM proxy_certificates WHERE id = $1")
        .bind(certificate_id)
        .fetch_one(pool)
        .await
        .expect("stored fingerprint")
}

async fn audit_count(pool: &PgPool, request_id: RequestId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE request_id = $1")
        .bind(request_id.as_uuid())
        .fetch_one(pool)
        .await
        .expect("audit count")
}

/// Wished-for internal xDS-authenticator seam for slice fpv2-7f3.3.
///
/// The scalar adapter deliberately centralizes the proposed signature. If production chooses an
/// identity input object instead, only this adapter should need to change; the behavioral contract
/// below remains independent of that representation.
async fn pin(
    pool: &PgPool,
    spiffe_uri: &str,
    canonical_serial: &str,
    fingerprint_sha256: &str,
    request_id: RequestId,
) -> fp_domain::DomainResult<fp_domain::ProxyCertificate> {
    fp_core::services::dataplanes::pin_legacy_certificate_fingerprint(
        pool,
        spiffe_uri,
        canonical_serial,
        fingerprint_sha256,
        request_id,
    )
    .await
}

async fn pin_event_count(pool: &PgPool, team_id: uuid::Uuid, certificate_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events \
         WHERE event_type = $1 AND team_id = $2 AND payload->>'certificate_id' = $3",
    )
    .bind(PIN_EVENT)
    .bind(team_id)
    .bind(certificate_id.to_string())
    .fetch_one(pool)
    .await
    .expect("pin event count")
}

async fn assert_no_evidence(w: &World, request_id: RequestId, certificate_id: uuid::Uuid) {
    assert_eq!(audit_count(&w.pool, request_id).await, 0);
    assert_eq!(pin_event_count(&w.pool, w.team_id, certificate_id).await, 0);
}

#[test]
fn fingerprint_pinned_event_has_a_stable_public_wire_contract() {
    let certificate_id = uuid::Uuid::now_v7();
    let event = DomainEvent::ProxyCertificateFingerprintPinned {
        certificate_id,
        spiffe_uri: "spiffe://flowplane.test/org/o/team/t/proxy/p".to_owned(),
        fingerprint_sha256: fingerprint("a"),
    };
    let json = serde_json::to_value(&event).expect("serialize fingerprint-pinned event");
    assert_eq!(event.kind(), PIN_EVENT);
    assert_eq!(json["type"], PIN_EVENT);
    assert_eq!(json["certificate_id"], certificate_id.to_string());
    assert_eq!(json["fingerprint_sha256"], fingerprint("a"));
}

#[tokio::test]
async fn canonical_uri_and_numeric_serial_pin_exactly_one_active_legacy_row_with_transactional_evidence(
) {
    let Some(w) = world().await else { return };
    let spiffe_uri = format!("spiffe://flowplane.test/{}", unique("canonical"));
    let certificate_id = insert_certificate(
        &w,
        &spiffe_uri,
        "a",
        None,
        "now() + interval '1 hour'",
        false,
    )
    .await;
    let request_id = RequestId::generate();
    let expected_fingerprint = fingerprint("a");

    let pinned = pin(
        &w.pool,
        &spiffe_uri,
        "000A",
        &expected_fingerprint,
        request_id,
    )
    .await
    .expect("one active legacy candidate pins");

    assert_eq!(pinned.id.as_uuid(), certificate_id);
    assert_eq!(pinned.serial_number, "a");
    assert_eq!(
        pinned.fingerprint_sha256.as_deref(),
        Some(expected_fingerprint.as_str())
    );
    assert_eq!(
        stored_fingerprint(&w.pool, certificate_id).await.as_deref(),
        Some(expected_fingerprint.as_str())
    );

    let audit: (
        String,
        String,
        String,
        String,
        Option<uuid::Uuid>,
        Option<uuid::Uuid>,
        serde_json::Value,
    ) = sqlx::query_as(
        "SELECT actor_type, actor_label, surface, action, org_id, team_id, detail \
             FROM audit_log WHERE request_id = $1",
    )
    .bind(request_id.as_uuid())
    .fetch_one(&w.pool)
    .await
    .expect("system/xDS audit row");
    assert_eq!(audit.0, "system");
    assert_eq!(audit.1, XDS_ACTOR);
    assert_eq!(audit.2, "xds");
    assert_eq!(audit.3, PIN_ACTION);
    assert_eq!(audit.4, Some(w.org_id));
    assert_eq!(audit.5, Some(w.team_id));
    assert_eq!(audit.6["certificate_id"], certificate_id.to_string());
    assert_eq!(audit.6["dataplane_id"], w.dataplane_id.to_string());
    assert_eq!(audit.6["fingerprint_sha256"], expected_fingerprint);

    let event: (uuid::Uuid, uuid::Uuid, serde_json::Value) = sqlx::query_as(
        "SELECT org_id, team_id, payload FROM events \
         WHERE event_type = $1 AND team_id = $2 AND payload->>'certificate_id' = $3",
    )
    .bind(PIN_EVENT)
    .bind(w.team_id)
    .bind(certificate_id.to_string())
    .fetch_one(&w.pool)
    .await
    .expect("fingerprint-pinned outbox event");
    assert_eq!(event.0, w.org_id);
    assert_eq!(event.1, w.team_id);
    assert_eq!(event.2["type"], PIN_EVENT);
    assert_eq!(event.2["certificate_id"], certificate_id.to_string());
    assert_eq!(event.2["fingerprint_sha256"], fingerprint("a"));
}

#[tokio::test]
async fn invalid_fingerprint_shapes_fail_closed_without_mutation_or_evidence() {
    for invalid in ["A".repeat(64), "a".repeat(63), "g".repeat(64)] {
        let Some(w) = world().await else { return };
        let spiffe_uri = format!("spiffe://flowplane.test/{}", unique("bad-fingerprint"));
        let certificate_id = insert_certificate(
            &w,
            &spiffe_uri,
            "b",
            None,
            "now() + interval '1 hour'",
            false,
        )
        .await;
        let request_id = RequestId::generate();

        let result = pin(&w.pool, &spiffe_uri, "b", &invalid, request_id).await;
        assert!(
            result.is_err(),
            "invalid fingerprint {invalid:?} must fail closed"
        );
        assert_eq!(stored_fingerprint(&w.pool, certificate_id).await, None);
        assert_no_evidence(&w, request_id, certificate_id).await;
    }
}

#[tokio::test]
async fn repeated_same_fingerprint_is_side_effect_idempotent_and_mismatch_fails_closed() {
    let Some(w) = world().await else { return };
    let spiffe_uri = format!("spiffe://flowplane.test/{}", unique("idempotent"));
    let certificate_id = insert_certificate(
        &w,
        &spiffe_uri,
        "c",
        None,
        "now() + interval '1 hour'",
        false,
    )
    .await;
    let pinned_fingerprint = fingerprint("c");
    let first_request = RequestId::generate();
    pin(
        &w.pool,
        &spiffe_uri,
        "c",
        &pinned_fingerprint,
        first_request,
    )
    .await
    .expect("first pin");

    let repeat_request = RequestId::generate();
    let _repeat = pin(
        &w.pool,
        &spiffe_uri,
        "000c",
        &pinned_fingerprint,
        repeat_request,
    )
    .await;
    assert_eq!(
        stored_fingerprint(&w.pool, certificate_id).await.as_deref(),
        Some(pinned_fingerprint.as_str())
    );
    assert_eq!(
        pin_event_count(&w.pool, w.team_id, certificate_id).await,
        1,
        "an idempotent repeat may return success or closed failure, but must not duplicate the event"
    );
    assert_eq!(
        audit_count(&w.pool, repeat_request).await,
        0,
        "an idempotent no-op must not claim a second mutation"
    );

    let mismatch_request = RequestId::generate();
    let mismatch = pin(
        &w.pool,
        &spiffe_uri,
        "c",
        &fingerprint("d"),
        mismatch_request,
    )
    .await;
    assert!(
        mismatch.is_err(),
        "a different leaf must never replace a pinned fingerprint"
    );
    assert_eq!(
        stored_fingerprint(&w.pool, certificate_id).await.as_deref(),
        Some(pinned_fingerprint.as_str())
    );
    assert_eq!(audit_count(&w.pool, mismatch_request).await, 0);
    assert_eq!(pin_event_count(&w.pool, w.team_id, certificate_id).await, 1);
}

#[tokio::test]
async fn revoked_expired_and_zero_candidate_paths_fail_closed() {
    for (prefix, expiry, revoked) in [
        ("revoked", "now() + interval '1 hour'", true),
        ("expired", "now() - interval '1 second'", false),
    ] {
        let Some(w) = world().await else { return };
        let spiffe_uri = format!("spiffe://flowplane.test/{}", unique(prefix));
        let certificate_id = insert_certificate(&w, &spiffe_uri, "e", None, expiry, revoked).await;
        let request_id = RequestId::generate();
        let result = pin(&w.pool, &spiffe_uri, "e", &fingerprint("e"), request_id).await;
        assert!(result.is_err(), "{prefix} candidate must fail closed");
        assert_eq!(stored_fingerprint(&w.pool, certificate_id).await, None);
        assert_no_evidence(&w, request_id, certificate_id).await;
    }

    let Some(w) = world().await else { return };
    let missing_id = uuid::Uuid::now_v7();
    let request_id = RequestId::generate();
    let missing = pin(
        &w.pool,
        &format!("spiffe://flowplane.test/{}", unique("missing")),
        "f",
        &fingerprint("f"),
        request_id,
    )
    .await;
    assert!(missing.is_err(), "zero candidates must fail closed");
    assert_no_evidence(&w, request_id, missing_id).await;
}

#[tokio::test]
async fn database_uniqueness_failure_rolls_back_row_audit_and_outbox() {
    let Some(w) = world().await else { return };
    let occupied_fingerprint = fingerprint("1");
    let occupied_uri = format!("spiffe://flowplane.test/{}", unique("occupied"));
    insert_certificate(
        &w,
        &occupied_uri,
        "10",
        Some(&occupied_fingerprint),
        "now() + interval '1 hour'",
        false,
    )
    .await;

    let candidate_uri = format!("spiffe://flowplane.test/{}", unique("db-failure"));
    let candidate_id = insert_certificate(
        &w,
        &candidate_uri,
        "11",
        None,
        "now() + interval '1 hour'",
        false,
    )
    .await;
    let request_id = RequestId::generate();
    let result = pin(
        &w.pool,
        &candidate_uri,
        "11",
        &occupied_fingerprint,
        request_id,
    )
    .await;

    assert!(
        result.is_err(),
        "injected PostgreSQL 23505 must fail the service call"
    );
    assert_eq!(stored_fingerprint(&w.pool, candidate_id).await, None);
    assert_no_evidence(&w, request_id, candidate_id).await;
}

#[tokio::test]
async fn simultaneous_conflicting_pins_have_exactly_one_winner() {
    let Some(w) = world().await else { return };
    let spiffe_uri = format!("spiffe://flowplane.test/{}", unique("race"));
    let certificate_id = insert_certificate(
        &w,
        &spiffe_uri,
        "12",
        None,
        "now() + interval '1 hour'",
        false,
    )
    .await;
    let barrier = Arc::new(Barrier::new(2));
    let pool_a = w.pool.clone();
    let pool_b = w.pool.clone();
    let uri_a = spiffe_uri.clone();
    let uri_b = spiffe_uri.clone();
    let barrier_a = Arc::clone(&barrier);
    let barrier_b = Arc::clone(&barrier);
    let fingerprint_a = fingerprint("2");
    let fingerprint_b = fingerprint("3");
    let expected_a = fingerprint_a.clone();
    let expected_b = fingerprint_b.clone();

    let first = tokio::spawn(async move {
        barrier_a.wait().await;
        pin(
            &pool_a,
            &uri_a,
            "0012",
            &fingerprint_a,
            RequestId::generate(),
        )
        .await
    });
    let second = tokio::spawn(async move {
        barrier_b.wait().await;
        pin(&pool_b, &uri_b, "12", &fingerprint_b, RequestId::generate()).await
    });
    let first = first.await.expect("first pin task");
    let second = second.await.expect("second pin task");
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(
        usize::from(first.is_err()) + usize::from(second.is_err()),
        1
    );

    let winner = stored_fingerprint(&w.pool, certificate_id)
        .await
        .expect("one fingerprint must win");
    assert!(winner == expected_a || winner == expected_b);
    assert_eq!(pin_event_count(&w.pool, w.team_id, certificate_id).await, 1);
    let audit_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE team_id = $1 AND action = $2 \
         AND detail->>'certificate_id' = $3",
    )
    .bind(w.team_id)
    .bind(PIN_ACTION)
    .bind(certificate_id.to_string())
    .fetch_one(&w.pool)
    .await
    .expect("race audit count");
    assert_eq!(audit_rows, 1, "only the winning pin records audit evidence");
}

fn split_db_url(url: &str) -> String {
    let (base, _query) = url.split_once('?').unwrap_or((url, ""));
    base.rsplit_once('/')
        .expect("database URL has a path segment")
        .0
        .to_owned()
}

#[tokio::test]
async fn injected_outbox_failure_rolls_back_all_effects_and_multiple_candidates_fail_closed() {
    let Ok(admin_url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let scratch_name = format!("fp_legacy_pin_{}", uuid::Uuid::now_v7().simple());
    let scratch_url = format!("{}/{}", split_db_url(&admin_url), scratch_name);
    let mut admin = PgConnection::connect(&admin_url)
        .await
        .expect("maintenance connect");
    admin
        .execute(format!("CREATE DATABASE {scratch_name}").as_str())
        .await
        .expect("create isolated ambiguity database");
    admin.close().await.ok();

    let outcome = async {
        let pool = fp_storage::connect(&scratch_url, 4)
            .await
            .map_err(|error| format!("scratch connect: {error}"))?;
        fp_storage::migrate(&pool)
            .await
            .map_err(|error| format!("scratch migrate: {error}"))?;

        // Fail at the outbox boundary, after the service has had an opportunity to stage the row
        // update and audit write. The scratch database isolates this trigger from parallel tests.
        sqlx::query(
            "CREATE FUNCTION fp_test_reject_pin_event() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN \
               IF NEW.event_type = 'proxy_certificate.fingerprint_pinned' THEN \
                 RAISE EXCEPTION 'injected fingerprint-pin outbox failure'; \
               END IF; \
               RETURN NEW; \
             END $$",
        )
        .execute(&pool)
        .await
        .map_err(|error| format!("install isolated outbox failure function: {error}"))?;
        sqlx::query(
            "CREATE TRIGGER fp_test_reject_pin_event \
             BEFORE INSERT ON events FOR EACH ROW \
             EXECUTE FUNCTION fp_test_reject_pin_event()",
        )
        .execute(&pool)
        .await
        .map_err(|error| format!("install isolated outbox failure trigger: {error}"))?;
        let failure_world = seed_world(pool.clone()).await;
        let failure_uri = format!("spiffe://flowplane.test/{}", unique("outbox-failure"));
        let failure_certificate_id = insert_certificate(
            &failure_world,
            &failure_uri,
            "14",
            None,
            "now() + interval '1 hour'",
            false,
        )
        .await;
        let failure_request_id = RequestId::generate();
        let failure = pin(
            &pool,
            &failure_uri,
            "0014",
            &fingerprint("5"),
            failure_request_id,
        )
        .await;
        assert!(
            failure.is_err(),
            "injected outbox failure must abort the pin"
        );
        assert_eq!(
            stored_fingerprint(&pool, failure_certificate_id).await,
            None,
            "the row update must roll back when outbox insertion fails"
        );
        assert_eq!(audit_count(&pool, failure_request_id).await, 0);
        assert_eq!(
            pin_event_count(&pool, failure_world.team_id, failure_certificate_id).await,
            0
        );
        sqlx::query("DROP TRIGGER fp_test_reject_pin_event ON events")
            .execute(&pool)
            .await
            .map_err(|error| format!("remove isolated outbox failure trigger: {error}"))?;
        sqlx::query("DROP FUNCTION fp_test_reject_pin_event()")
            .execute(&pool)
            .await
            .map_err(|error| format!("remove isolated outbox failure function: {error}"))?;

        sqlx::query(
            "ALTER TABLE proxy_certificates DROP CONSTRAINT proxy_certificates_spiffe_uri_key",
        )
        .execute(&pool)
        .await
        .map_err(|error| format!("drop scratch SPIFFE uniqueness: {error}"))?;
        let first = seed_world(pool.clone()).await;
        let second = seed_world(pool.clone()).await;
        let spiffe_uri = format!("spiffe://flowplane.test/{}", unique("ambiguous"));
        let first_id = insert_certificate(
            &first,
            &spiffe_uri,
            "13",
            None,
            "now() + interval '1 hour'",
            false,
        )
        .await;
        let second_id = insert_certificate(
            &second,
            &spiffe_uri,
            "13",
            None,
            "now() + interval '1 hour'",
            false,
        )
        .await;
        let request_id = RequestId::generate();
        let result = pin(&pool, &spiffe_uri, "0013", &fingerprint("4"), request_id).await;
        assert!(
            result.is_err(),
            "multiple matching active legacy rows must fail closed"
        );
        assert_eq!(stored_fingerprint(&pool, first_id).await, None);
        assert_eq!(stored_fingerprint(&pool, second_id).await, None);
        assert_eq!(audit_count(&pool, request_id).await, 0);
        assert_eq!(pin_event_count(&pool, first.team_id, first_id).await, 0);
        assert_eq!(pin_event_count(&pool, second.team_id, second_id).await, 0);
        pool.close().await;
        Ok::<(), String>(())
    }
    .await;

    if let Ok(mut admin) = PgConnection::connect(&admin_url).await {
        let _ = admin
            .execute(format!("DROP DATABASE IF EXISTS {scratch_name} WITH (FORCE)").as_str())
            .await;
        let _ = admin.close().await;
    }
    outcome.expect("isolated ambiguity contract");
}
