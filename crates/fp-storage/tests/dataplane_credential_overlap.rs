//! RED contract for fpv2-7f3.6: bounded overlapping dataplane credentials.
//!
//! Authored from the approved design and plan without reading production or migration source.
//! Shared-database fixtures use unique UUIDv7 namespaces and never perform global cleanup. The
//! legacy over-cap preflight runs in a unique scratch database so migration state is isolated.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use sqlx::{Connection, Executor, PgConnection, PgPool};

fn classified_over_cap_marker(error: &str) -> Option<&str> {
    error
        .split(|character: char| !(character.is_ascii_uppercase() || character == '_'))
        .find(|token| {
            token.starts_with("FP_")
                && token.contains("CERT")
                && (token.contains("OVERLAP") || token.contains("UNREVOKED"))
                && (token.contains("CAP") || token.contains("LIMIT") || token.contains("EXCEEDED"))
        })
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7().simple())
}

fn fingerprint() -> String {
    let half = uuid::Uuid::now_v7().simple().to_string();
    format!("{half}{half}")
}

fn split_db_url(url: &str) -> (String, String) {
    let (base, _) = url.split_once('?').unwrap_or((url, ""));
    let (prefix, database) = base
        .rsplit_once('/')
        .expect("database URL has a path segment");
    (prefix.to_owned(), database.to_owned())
}

async fn seed_namespace(pool: &PgPool) -> (uuid::Uuid, uuid::Uuid, uuid::Uuid) {
    let org_id = uuid::Uuid::now_v7();
    let team_id = uuid::Uuid::now_v7();
    let dataplane_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(unique("overlap-org"))
        .execute(pool)
        .await
        .expect("organization fixture");
    sqlx::query("INSERT INTO teams (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(org_id)
        .bind(unique("overlap-team"))
        .execute(pool)
        .await
        .expect("team fixture");
    sqlx::query("INSERT INTO dataplanes (id, team_id, org_id, name) VALUES ($1, $2, $3, $4)")
        .bind(dataplane_id)
        .bind(team_id)
        .bind(org_id)
        .bind(unique("overlap-dataplane"))
        .execute(pool)
        .await
        .expect("dataplane fixture");
    (org_id, team_id, dataplane_id)
}

async fn insert_row(
    executor: impl sqlx::PgExecutor<'_>,
    team_id: uuid::Uuid,
    dataplane_id: uuid::Uuid,
    spiffe_uri: &str,
    serial: &str,
    fingerprint_sha256: &str,
) -> Result<uuid::Uuid, sqlx::Error> {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO proxy_certificates \
         (id, team_id, dataplane_id, spiffe_uri, serial_number, fingerprint_sha256, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now() + interval '1 hour')",
    )
    .bind(id)
    .bind(team_id)
    .bind(dataplane_id)
    .bind(spiffe_uri)
    .bind(serial)
    .bind(fingerprint_sha256)
    .execute(executor)
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
async fn migration_0035_drops_global_spiffe_uniqueness_and_retains_identity_uniqueness() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let pool = fp_storage::connect(&url, 8)
        .await
        .expect("connect PostgreSQL");
    fp_storage::migrate(&pool)
        .await
        .expect("migrate PostgreSQL");
    let (org_id, team_id, dataplane_id) = seed_namespace(&pool).await;
    let shared_uri = format!("spiffe://flowplane.test/dataplane/{dataplane_id}");

    insert_row(
        &pool,
        team_id,
        dataplane_id,
        &shared_uri,
        "f601",
        &fingerprint(),
    )
    .await
    .expect("initial exact credential");
    insert_row(
        &pool,
        team_id,
        dataplane_id,
        &shared_uri,
        "f602",
        &fingerprint(),
    )
    .await
    .expect("a replacement may reuse the dataplane SPIFFE URI");

    let spiffe_indexes: Vec<(String, String)> = sqlx::query_as(
        "SELECT indexname, indexdef FROM pg_indexes \
         WHERE schemaname = current_schema() \
           AND tablename = 'proxy_certificates' \
           AND indexdef ILIKE '%(spiffe_uri)%'",
    )
    .fetch_all(&pool)
    .await
    .expect("inspect SPIFFE indexes");
    assert!(
        spiffe_indexes
            .iter()
            .any(|(_, definition)| !definition.contains(" UNIQUE INDEX ")),
        "migration 0035 must add a non-unique SPIFFE URI lookup index: {spiffe_indexes:?}"
    );
    assert!(
        spiffe_indexes
            .iter()
            .all(|(_, definition)| !definition.contains(" UNIQUE INDEX ")),
        "global SPIFFE uniqueness must be removed: {spiffe_indexes:?}"
    );

    let control_dataplane_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO dataplanes (id, team_id, org_id, name) VALUES ($1, $2, $3, $4)")
        .bind(control_dataplane_id)
        .bind(team_id)
        .bind(org_id)
        .bind(unique("identity-control-dataplane"))
        .execute(&pool)
        .await
        .expect("identity-control dataplane");
    let duplicate_fingerprint = insert_row(
        &pool,
        team_id,
        control_dataplane_id,
        &format!("{shared_uri}/fingerprint-control"),
        "f603",
        &fingerprint(),
    )
    .await
    .expect("control credential");
    let stored_fingerprint: String =
        sqlx::query_scalar("SELECT fingerprint_sha256 FROM proxy_certificates WHERE id = $1")
            .bind(duplicate_fingerprint)
            .fetch_one(&pool)
            .await
            .expect("control fingerprint");
    let error = insert_row(
        &pool,
        team_id,
        control_dataplane_id,
        &format!("{shared_uri}/fingerprint-collision"),
        "f604",
        &stored_fingerprint,
    )
    .await
    .expect_err("global non-null fingerprint uniqueness must remain");
    assert_eq!(sqlstate(&error).as_deref(), Some("23505"));

    let error = insert_row(
        &pool,
        team_id,
        control_dataplane_id,
        &format!("{shared_uri}/serial-collision"),
        "f601",
        &fingerprint(),
    )
    .await
    .expect_err("team plus canonical serial uniqueness must remain");
    assert_eq!(sqlstate(&error).as_deref(), Some("23505"));
}

async fn run_over_cap_preflight(scratch_url: &str) -> Result<(), String> {
    use sqlx::migrate::Migrate;

    let mut connection = PgConnection::connect(scratch_url)
        .await
        .map_err(|error| format!("scratch connect: {error}"))?;
    connection
        .ensure_migrations_table()
        .await
        .map_err(|error| format!("create migrations table: {error}"))?;
    for migration in fp_storage::MIGRATOR
        .iter()
        .filter(|migration| migration.version <= 34)
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
                .bind(unique("over-cap-org")),
        )
        .await
        .map_err(|error| format!("seed organization: {error}"))?;
    connection
        .execute(
            sqlx::query("INSERT INTO teams (id, org_id, name) VALUES ($1, $2, $3)")
                .bind(team_id)
                .bind(org_id)
                .bind(unique("over-cap-team")),
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
            .bind(unique("over-cap-dataplane")),
        )
        .await
        .map_err(|error| format!("seed dataplane: {error}"))?;

    for serial in ["f611", "f612", "f613"] {
        connection
            .execute(
                sqlx::query(
                    "INSERT INTO proxy_certificates \
                     (id, team_id, dataplane_id, spiffe_uri, serial_number, \
                      fingerprint_sha256, expires_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, now() + interval '1 hour')",
                )
                .bind(uuid::Uuid::now_v7())
                .bind(team_id)
                .bind(dataplane_id)
                .bind(format!(
                    "spiffe://flowplane.test/legacy/{}",
                    uuid::Uuid::now_v7()
                ))
                .bind(serial)
                .bind(fingerprint()),
            )
            .await
            .map_err(|error| format!("seed over-cap row {serial}: {error}"))?;
    }

    let migration = fp_storage::MIGRATOR
        .iter()
        .find(|migration| migration.version == 35)
        .ok_or_else(|| "migration 0035 is missing".to_owned())?;
    let error = match connection.apply(migration).await {
        Ok(_) => {
            return Err(
                "migration 0035 accepted more than two unrevoked rows per dataplane".to_owned(),
            );
        }
        Err(error) => error,
    };
    let error_text = error.to_string();
    if classified_over_cap_marker(&error_text).is_none() {
        return Err(format!(
            "migration error must contain a classified FP_* credential over-cap marker; got {error_text}"
        ));
    }

    let applied: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version = 35 AND success)",
    )
    .fetch_one(&mut connection)
    .await
    .map_err(|error| format!("inspect migration history: {error}"))?;
    if applied {
        return Err("failed migration 0035 must not be recorded as successful".to_owned());
    }
    connection.close().await.ok();
    Ok(())
}

#[tokio::test]
async fn migration_0035_preflight_rejects_over_cap_legacy_state_with_classified_marker() {
    let Ok(url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let (prefix, maintenance_database) = split_db_url(&url);
    let scratch_database = format!("fp_overlap_mig_{}", uuid::Uuid::now_v7().simple());
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

    let outcome = run_over_cap_preflight(&scratch_url).await;

    if let Ok(mut admin) = PgConnection::connect(&url).await {
        let _ = admin
            .execute(format!("DROP DATABASE IF EXISTS {scratch_database} WITH (FORCE)").as_str())
            .await;
        let _ = admin.close().await;
    }
    if let Err(message) = outcome {
        panic!("{message}");
    }
}
