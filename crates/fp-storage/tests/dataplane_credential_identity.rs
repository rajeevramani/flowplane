//! RED contract for fpv2-7f3.1: exact dataplane credential metadata at rest.
//!
//! These tests are intentionally black-box at the PostgreSQL/repository boundary. They derive
//! their expectations from the approved slice contract and do not inspect migration or
//! production implementation source. Every fixture owns a fresh org/team/dataplane namespace;
//! no test deletes shared state.
//!
//! The isolated scratch-database test also applies migrations only through `0033`, seeds legacy
//! rows, and proves that `0034` aborts atomically with classified preflight errors.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::authz::TeamRef;
use fp_domain::event::DomainEvent;
use fp_domain::RequestId;
use fp_storage::repos::{audit, dataplanes, identity};
use sqlx::{PgPool, Postgres, Transaction};

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    )
}

fn fingerprint() -> String {
    let half = uuid::Uuid::now_v7().simple().to_string();
    format!("{half}{half}")
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

    let org = identity::create_org(&pool, &unique("credential-identity-org"), "")
        .await
        .expect("org");
    let team = identity::create_team(&pool, org.id, &unique("credential-identity-team"), "")
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

async fn create_dataplane(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
) -> fp_domain::DataplaneId {
    dataplanes::create_dataplane(tx, team, &unique("credential-identity-dp"), "")
        .await
        .expect("dataplane")
        .id
}

async fn insert_certificate(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    dataplane_id: fp_domain::DataplaneId,
    spiffe_uri: &str,
    serial_number: &str,
    fingerprint_sha256: Option<&str>,
) -> Result<uuid::Uuid, sqlx::Error> {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO proxy_certificates \
           (id, team_id, dataplane_id, spiffe_uri, serial_number, fingerprint_sha256, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now() + interval '1 hour')",
    )
    .bind(id)
    .bind(team.id.as_uuid())
    .bind(dataplane_id.as_uuid())
    .bind(spiffe_uri)
    .bind(serial_number)
    .bind(fingerprint_sha256)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

async fn insert_legacy_certificate(
    tx: &mut Transaction<'_, Postgres>,
    team: TeamRef,
    dataplane_id: fp_domain::DataplaneId,
    spiffe_uri: &str,
    serial_number: &str,
) -> Result<uuid::Uuid, sqlx::Error> {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO proxy_certificates \
           (id, team_id, dataplane_id, spiffe_uri, serial_number, expires_at) \
         VALUES ($1, $2, $3, $4, $5, now() + interval '1 hour')",
    )
    .bind(id)
    .bind(team.id.as_uuid())
    .bind(dataplane_id.as_uuid())
    .bind(spiffe_uri)
    .bind(serial_number)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

fn sqlstate(error: &sqlx::Error) -> Option<String> {
    match error {
        sqlx::Error::Database(database) => database.code().map(|code| code.into_owned()),
        _ => None,
    }
}

#[tokio::test]
async fn schema_adds_nullable_fingerprint_and_keeps_spiffe_uri_globally_unique() {
    let Some(w) = world().await else { return };

    let mut tx = w.pool.begin().await.expect("tx");
    let first_dataplane = create_dataplane(&mut tx, w.team).await;
    let second_dataplane = create_dataplane(&mut tx, w.team).await;
    let shared_spiffe = format!("spiffe://flowplane.test/{}", unique("shared"));

    // Exercise the retained URI constraint independently of the new fingerprint column.
    sqlx::query(
        "INSERT INTO proxy_certificates \
           (id, team_id, dataplane_id, spiffe_uri, serial_number, expires_at) \
         VALUES ($1, $2, $3, $4, '1a', now() + interval '1 hour')",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(w.team.id.as_uuid())
    .bind(first_dataplane.as_uuid())
    .bind(&shared_spiffe)
    .execute(&mut *tx)
    .await
    .expect("first URI registration");

    let duplicate_uri = sqlx::query(
        "INSERT INTO proxy_certificates \
           (id, team_id, dataplane_id, spiffe_uri, serial_number, expires_at) \
         VALUES ($1, $2, $3, $4, '1b', now() + interval '1 hour')",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(w.team.id.as_uuid())
    .bind(second_dataplane.as_uuid())
    .bind(&shared_spiffe)
    .execute(&mut *tx)
    .await
    .expect_err("SPIFFE URI uniqueness remains global in slice .1");
    assert_eq!(sqlstate(&duplicate_uri).as_deref(), Some("23505"));
    tx.rollback().await.expect("clear aborted tx");

    let column: Option<(String,)> = sqlx::query_as(
        "SELECT is_nullable FROM information_schema.columns \
         WHERE table_schema = current_schema() \
           AND table_name = 'proxy_certificates' \
           AND column_name = 'fingerprint_sha256'",
    )
    .fetch_optional(&w.pool)
    .await
    .expect("inspect proxy_certificates schema");
    assert_eq!(
        column.as_ref().map(|row| row.0.as_str()),
        Some("YES"),
        "migration 0034 must expose nullable proxy_certificates.fingerprint_sha256"
    );
}

#[tokio::test]
async fn non_null_fingerprint_is_globally_unique_while_null_remains_legacy_compatible() {
    let Some(w) = world().await else { return };
    let mut tx = w.pool.begin().await.expect("tx");
    let first_dataplane = create_dataplane(&mut tx, w.team).await;
    let second_dataplane = create_dataplane(&mut tx, w.team).await;
    let fingerprint = fingerprint();

    insert_certificate(
        &mut tx,
        w.team,
        first_dataplane,
        &format!("spiffe://flowplane.test/{}", unique("fingerprint-a")),
        "2a",
        Some(&fingerprint),
    )
    .await
    .expect("first exact credential");

    let duplicate = insert_certificate(
        &mut tx,
        w.team,
        second_dataplane,
        &format!("spiffe://flowplane.test/{}", unique("fingerprint-b")),
        "2b",
        Some(&fingerprint),
    )
    .await
    .expect_err("the same non-null leaf fingerprint must not identify two registry rows");
    assert_eq!(sqlstate(&duplicate).as_deref(), Some("23505"));
    tx.rollback().await.expect("clear aborted tx");

    // Legacy rows have no DER-derived identity; NULL must remain insertable more than once.
    let mut tx = w.pool.begin().await.expect("legacy tx");
    let first_dataplane = create_dataplane(&mut tx, w.team).await;
    let second_dataplane = create_dataplane(&mut tx, w.team).await;
    insert_certificate(
        &mut tx,
        w.team,
        first_dataplane,
        &format!("spiffe://flowplane.test/{}", unique("legacy-a")),
        "2c",
        None,
    )
    .await
    .expect("first legacy null fingerprint");
    insert_certificate(
        &mut tx,
        w.team,
        second_dataplane,
        &format!("spiffe://flowplane.test/{}", unique("legacy-b")),
        "2d",
        None,
    )
    .await
    .expect("second legacy null fingerprint");
    tx.commit().await.expect("commit legacy rows");
}

#[tokio::test]
async fn leading_zero_serial_forms_collide_within_a_team_after_numeric_canonicalization() {
    let Some(w) = world().await else { return };
    let mut tx = w.pool.begin().await.expect("tx");
    let first_dataplane = create_dataplane(&mut tx, w.team).await;
    let second_dataplane = create_dataplane(&mut tx, w.team).await;

    insert_legacy_certificate(
        &mut tx,
        w.team,
        first_dataplane,
        &format!("spiffe://flowplane.test/{}", unique("serial-leading-zero")),
        "000a",
    )
    .await
    .expect("first numeric serial form");

    let collision = insert_legacy_certificate(
        &mut tx,
        w.team,
        second_dataplane,
        &format!("spiffe://flowplane.test/{}", unique("serial-canonical")),
        "a",
    )
    .await;
    let error = collision.expect_err(
        "000a and a are the same unsigned numeric serial and must collide within one team",
    );
    assert_eq!(sqlstate(&error).as_deref(), Some("23505"));
    tx.rollback().await.expect("rollback collision fixture");
}

#[tokio::test]
async fn malformed_hex_serial_fails_closed_at_the_registry_boundary() {
    let Some(w) = world().await else { return };
    let mut tx = w.pool.begin().await.expect("tx");
    let dataplane_id = create_dataplane(&mut tx, w.team).await;

    let malformed = insert_legacy_certificate(
        &mut tx,
        w.team,
        dataplane_id,
        &format!("spiffe://flowplane.test/{}", unique("malformed")),
        "not-hex",
    )
    .await;
    assert!(
        malformed.is_err(),
        "malformed legacy serial text must fail closed rather than enter canonical storage"
    );
    tx.rollback().await.expect("rollback malformed fixture");
}

#[tokio::test]
async fn exact_registry_metadata_round_trips_without_rewriting_historical_serial_text() {
    let Some(w) = world().await else { return };
    let mut tx = w.pool.begin().await.expect("tx");
    let dataplane_id = create_dataplane(&mut tx, w.team).await;
    let request_id = RequestId::generate();
    let historical_serial = "000a";
    let canonical_serial = "a";
    let certificate_id = uuid::Uuid::now_v7();
    let fingerprint = fingerprint();

    audit::record_in_tx(
        &mut tx,
        &audit::AuditEntry {
            request_id: Some(request_id),
            actor_type: audit::ActorType::System,
            actor_id: None,
            actor_label: "credential-identity-red-test".to_string(),
            surface: audit::Surface::Rest,
            action: "proxy_certificate.register".to_string(),
            resource: format!("proxy-certificates/serial/{historical_serial}"),
            org_id: Some(w.team.org_id),
            team_id: Some(w.team.id),
            outcome: audit::Outcome::Success,
            detail: serde_json::json!({
                "certificate_id": certificate_id,
                "serial_number": historical_serial,
            }),
        },
    )
    .await
    .expect("historical audit row");

    let historical_event = serde_json::to_value(DomainEvent::ProxyCertificateRegistered {
        certificate_id,
        spiffe_uri: format!("spiffe://flowplane.test/history/{historical_serial}"),
    })
    .expect("serialize historical domain event");
    sqlx::query(
        "INSERT INTO events (id, event_type, org_id, team_id, payload) \
         VALUES ($1, 'proxy_certificate.registered', $2, $3, $4)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(w.team.org_id.as_uuid())
    .bind(w.team.id.as_uuid())
    .bind(&historical_event)
    .execute(&mut *tx)
    .await
    .expect("historical outbox event");

    sqlx::query(
        "INSERT INTO proxy_certificates \
           (id, team_id, dataplane_id, spiffe_uri, serial_number, fingerprint_sha256, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now() + interval '1 hour')",
    )
    .bind(certificate_id)
    .bind(w.team.id.as_uuid())
    .bind(dataplane_id.as_uuid())
    .bind(format!("spiffe://flowplane.test/{}", unique("round-trip")))
    .bind(canonical_serial)
    .bind(&fingerprint)
    .execute(&mut *tx)
    .await
    .expect("new exact registry row");
    tx.commit()
        .await
        .expect("commit exact credential and history");

    let (stored_fingerprint, stored_serial): (Option<String>, String) = sqlx::query_as(
        "SELECT fingerprint_sha256, serial_number FROM proxy_certificates WHERE id = $1",
    )
    .bind(certificate_id)
    .fetch_one(&w.pool)
    .await
    .expect("round-trip registry metadata");
    assert_eq!(stored_fingerprint.as_deref(), Some(fingerprint.as_str()));
    assert_eq!(stored_serial, canonical_serial);

    let (audit_resource, audit_serial): (String, String) = sqlx::query_as(
        "SELECT resource, detail->>'serial_number' FROM audit_log WHERE request_id = $1",
    )
    .bind(request_id.as_uuid())
    .fetch_one(&w.pool)
    .await
    .expect("historical audit text");
    assert!(audit_resource.ends_with(historical_serial));
    assert_eq!(audit_serial, historical_serial);

    let (stored_event,): (serde_json::Value,) = sqlx::query_as(
        "SELECT payload FROM events \
         WHERE event_type = 'proxy_certificate.registered' \
           AND team_id = $1 \
           AND payload->>'certificate_id' = $2",
    )
    .bind(w.team.id.as_uuid())
    .bind(certificate_id.to_string())
    .fetch_one(&w.pool)
    .await
    .expect("historical event payload");
    assert_eq!(stored_event, historical_event);
}

fn split_db_url(url: &str) -> (String, String) {
    let (base, _query) = url.split_once('?').unwrap_or((url, ""));
    let (prefix, database) = base
        .rsplit_once('/')
        .expect("database URL has a path segment");
    (prefix.to_owned(), database.to_owned())
}

async fn run_preflight_failure_case(
    scratch_url: &str,
    serials: &[&str],
    expected_code: &str,
) -> Result<(), String> {
    use sqlx::migrate::Migrate;
    use sqlx::{Connection, Executor, PgConnection};

    let mut connection = PgConnection::connect(scratch_url)
        .await
        .map_err(|error| format!("scratch connect: {error}"))?;
    connection
        .ensure_migrations_table()
        .await
        .map_err(|error| format!("create migrations table: {error}"))?;
    for migration in fp_storage::MIGRATOR
        .iter()
        .filter(|migration| migration.version <= 33)
    {
        connection
            .apply(migration)
            .await
            .map_err(|error| format!("apply migration {}: {error}", migration.version))?;
    }

    let org_id = uuid::Uuid::now_v7();
    let team_id = uuid::Uuid::now_v7();
    let dataplane_id = uuid::Uuid::now_v7();
    connection
        .execute(
            sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
                .bind(org_id)
                .bind(unique("migration-org")),
        )
        .await
        .map_err(|error| format!("seed organization: {error}"))?;
    connection
        .execute(
            sqlx::query("INSERT INTO teams (id, org_id, name) VALUES ($1, $2, $3)")
                .bind(team_id)
                .bind(org_id)
                .bind(unique("migration-team")),
        )
        .await
        .map_err(|error| format!("seed team: {error}"))?;
    connection
        .execute(
            sqlx::query(
                "INSERT INTO dataplanes (id, team_id, org_id, name) VALUES ($1, $2, $3, $4)",
            )
            .bind(dataplane_id)
            .bind(team_id)
            .bind(org_id)
            .bind(unique("migration-dp")),
        )
        .await
        .map_err(|error| format!("seed dataplane: {error}"))?;
    for serial in serials {
        connection
            .execute(
                sqlx::query(
                    "INSERT INTO proxy_certificates \
                       (id, team_id, dataplane_id, spiffe_uri, serial_number, expires_at) \
                     VALUES ($1, $2, $3, $4, $5, now() + interval '1 hour')",
                )
                .bind(uuid::Uuid::now_v7())
                .bind(team_id)
                .bind(dataplane_id)
                .bind(format!(
                    "spiffe://flowplane.test/{}",
                    unique("migration-cert")
                ))
                .bind(serial),
            )
            .await
            .map_err(|error| format!("seed legacy serial {serial:?}: {error}"))?;
    }

    let migration = fp_storage::MIGRATOR
        .iter()
        .find(|migration| migration.version == 34)
        .ok_or_else(|| "migration 0034 is missing".to_owned())?;
    let error = connection
        .apply(migration)
        .await
        .expect_err("invalid legacy state must abort migration 0034");
    if !error.to_string().contains(expected_code) {
        return Err(format!(
            "migration error must contain {expected_code}; got {error}"
        ));
    }
    let fingerprint_column_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_name = 'proxy_certificates' AND column_name = 'fingerprint_sha256')",
    )
    .fetch_one(&mut connection)
    .await
    .map_err(|error| format!("inspect failed migration: {error}"))?;
    if fingerprint_column_exists {
        return Err("failed migration must not leave partial schema changes".to_owned());
    }
    connection.close().await.ok();
    Ok(())
}

#[tokio::test]
async fn migration_0034_preflight_aborts_atomically_with_classified_errors() {
    use sqlx::{Connection, Executor, PgConnection};

    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let (prefix, maintenance_database) = split_db_url(&url);
    let cases: [(&[&str], &str); 2] = [
        (&["not-hex"], "FP_CERT_SERIAL_MALFORMED"),
        (&["000a", "a"], "FP_CERT_SERIAL_CANONICAL_COLLISION"),
    ];

    for (serials, expected_code) in cases {
        let scratch_database = format!("fp_cert_mig_{}", uuid::Uuid::now_v7().simple());
        let scratch_url = format!("{prefix}/{scratch_database}");
        let mut admin = PgConnection::connect(&url)
            .await
            .expect("maintenance connect");
        admin
            .execute(format!("CREATE DATABASE {scratch_database}").as_str())
            .await
            .unwrap_or_else(|error| {
                panic!("create database {scratch_database} on {maintenance_database}: {error}")
            });
        admin.close().await.ok();

        let outcome = run_preflight_failure_case(&scratch_url, serials, expected_code).await;

        if let Ok(mut admin) = PgConnection::connect(&url).await {
            let _ = admin
                .execute(
                    format!("DROP DATABASE IF EXISTS {scratch_database} WITH (FORCE)").as_str(),
                )
                .await;
            let _ = admin.close().await;
        }
        if let Err(message) = outcome {
            panic!("{message}");
        }
    }
}
