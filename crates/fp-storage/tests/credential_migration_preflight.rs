//! Black-box upgrade/preflight contracts for fpv2-7f3.9.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use fp_storage::credential_migration::{
    preflight, SERIAL_CANONICAL_COLLISION, SERIAL_MALFORMED, UNREVOKED_CAP_EXCEEDED,
};
use sqlx::migrate::Migrate;
use sqlx::{Connection, Executor, PgConnection, PgPool};
use std::collections::BTreeSet;
use uuid::Uuid;

fn split_db_url(url: &str) -> (String, String) {
    let (base, _) = url.split_once('?').unwrap_or((url, ""));
    let (prefix, database) = base
        .rsplit_once('/')
        .expect("database URL has a path segment");
    (prefix.to_owned(), database.to_owned())
}

struct ScratchDatabase {
    admin_url: String,
    name: String,
    url: String,
}

impl ScratchDatabase {
    async fn create() -> Option<Self> {
        let Ok(admin_url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL") else {
            eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
            return None;
        };
        let (prefix, _) = split_db_url(&admin_url);
        let name = format!("fp_credential_preflight_{}", Uuid::now_v7().simple());
        let url = format!("{prefix}/{name}");
        let mut admin = PgConnection::connect(&admin_url)
            .await
            .expect("maintenance connect");
        admin
            .execute(format!("CREATE DATABASE {name}").as_str())
            .await
            .expect("create isolated database");
        admin.close().await.ok();
        Some(Self {
            admin_url,
            name,
            url,
        })
    }

    async fn drop(self) {
        if let Ok(mut admin) = PgConnection::connect(&self.admin_url).await {
            let _ = admin
                .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name).as_str())
                .await;
            let _ = admin.close().await;
        }
    }
}

struct LegacyFixture {
    pool: PgPool,
    dataplane_id: Uuid,
    team_id: Uuid,
}

async fn legacy_fixture(scratch_url: &str) -> LegacyFixture {
    let mut connection = PgConnection::connect(scratch_url)
        .await
        .expect("scratch connect");
    connection
        .ensure_migrations_table()
        .await
        .expect("migration table");
    for migration in fp_storage::MIGRATOR
        .iter()
        .filter(|migration| migration.version <= 33)
    {
        connection
            .apply(migration)
            .await
            .unwrap_or_else(|error| panic!("apply migration {}: {error}", migration.version));
    }

    let org_id = Uuid::now_v7();
    let team_id = Uuid::now_v7();
    let dataplane_id = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(format!("preflight-org-{}", Uuid::now_v7().simple()))
        .execute(&mut connection)
        .await
        .expect("seed organization");
    sqlx::query("INSERT INTO teams (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(org_id)
        .bind(format!("preflight-team-{}", Uuid::now_v7().simple()))
        .execute(&mut connection)
        .await
        .expect("seed team");
    sqlx::query("INSERT INTO dataplanes (id, team_id, org_id, name) VALUES ($1, $2, $3, $4)")
        .bind(dataplane_id)
        .bind(team_id)
        .bind(org_id)
        .bind(format!("preflight-dp-{}", Uuid::now_v7().simple()))
        .execute(&mut connection)
        .await
        .expect("seed dataplane");
    connection.close().await.ok();

    LegacyFixture {
        pool: fp_storage::connect(scratch_url, 4)
            .await
            .expect("scratch pool"),
        dataplane_id,
        team_id,
    }
}

async fn insert_legacy_certificate(fixture: &LegacyFixture, serial: &str) -> Uuid {
    let certificate_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO proxy_certificates \
         (id, team_id, dataplane_id, spiffe_uri, serial_number, expires_at) \
         VALUES ($1, $2, $3, $4, $5, now() + interval '1 hour')",
    )
    .bind(certificate_id)
    .bind(fixture.team_id)
    .bind(fixture.dataplane_id)
    .bind(format!(
        "spiffe://flowplane.test/preflight/{certificate_id}"
    ))
    .bind(serial)
    .execute(&fixture.pool)
    .await
    .expect("seed legacy certificate");
    certificate_id
}

#[tokio::test]
async fn preflight_reports_all_blocker_classes_by_id_without_certificate_material() {
    let Some(scratch) = ScratchDatabase::create().await else {
        return;
    };
    let fixture = legacy_fixture(&scratch.url).await;
    let serials = ["not-hex", "000a", "a", "b"];
    let mut certificate_ids = BTreeSet::new();
    for serial in serials {
        certificate_ids.insert(insert_legacy_certificate(&fixture, serial).await);
    }

    let report = preflight(&fixture.pool).await.expect("preflight report");
    let codes: BTreeSet<_> = report
        .blockers
        .iter()
        .map(|blocker| blocker.code.as_str())
        .collect();
    assert_eq!(
        codes,
        BTreeSet::from([
            SERIAL_MALFORMED,
            SERIAL_CANONICAL_COLLISION,
            UNREVOKED_CAP_EXCEEDED,
        ])
    );
    assert!(report
        .blockers
        .iter()
        .all(|blocker| blocker.dataplane_ids.contains(&fixture.dataplane_id)));
    let reported_ids: BTreeSet<_> = report
        .blockers
        .iter()
        .flat_map(|blocker| blocker.certificate_ids.iter().copied())
        .collect();
    assert_eq!(reported_ids, certificate_ids);

    let output = serde_json::to_string(&report).expect("serialize report");
    for secret_material in ["not-hex", "spiffe://", "fingerprint", "\"serial"] {
        assert!(
            !output.contains(secret_material),
            "preflight output leaked certificate material marker {secret_material}"
        );
    }

    fixture.pool.close().await;
    scratch.drop().await;
}

#[tokio::test]
async fn populated_3_1_2_schema_preflights_and_migrates_to_current() {
    let Some(scratch) = ScratchDatabase::create().await else {
        return;
    };
    let fixture = legacy_fixture(&scratch.url).await;
    let certificate_id = insert_legacy_certificate(&fixture, "000A").await;

    let report = preflight(&fixture.pool).await.expect("preflight report");
    assert!(
        report.is_ready(),
        "valid legacy state must be migration-ready"
    );
    fp_storage::migrate(&fixture.pool)
        .await
        .expect("migrate populated legacy database");

    let (serial, fingerprint, retired_at): (
        String,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT pc.serial_number, pc.fingerprint_sha256, dp.retired_at \
             FROM proxy_certificates pc JOIN dataplanes dp ON dp.id = pc.dataplane_id \
             WHERE pc.id = $1",
    )
    .bind(certificate_id)
    .fetch_one(&fixture.pool)
    .await
    .expect("read migrated lifecycle state");
    assert_eq!(serial, "a");
    assert!(fingerprint.is_none(), "legacy credential remains unpinned");
    assert!(retired_at.is_none(), "legacy dataplane remains active");

    fixture.pool.close().await;
    scratch.drop().await;
}
