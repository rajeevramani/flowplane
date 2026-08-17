//! End-to-end real-PostgreSQL smoke for the offline recovery plan command.

#![allow(clippy::expect_used, clippy::panic)]

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::io::Write;
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

fn database_url_for(raw: &str, database_name: &str) -> String {
    let (base, query) = raw
        .split_once('?')
        .map_or((raw, None), |(base, query)| (base, Some(query)));
    let slash = base.rfind('/').expect("PostgreSQL URL database path");
    let mut result = format!("{}/{database_name}", &base[..slash]);
    if let Some(query) = query {
        result.push('?');
        result.push_str(query);
    }
    result
}

#[tokio::test]
async fn built_binary_prints_a_redacted_digest_bound_read_only_plan() {
    let Some(admin_url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL").ok() else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let admin_pool = fp_storage::connect(&admin_url, 2)
        .await
        .expect("test database admin connection");
    let database_name = format!("fp_recovery_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE DATABASE \"{database_name}\""))
        .execute(&admin_pool)
        .await
        .expect("create isolated test database");
    let database_url = database_url_for(&admin_url, &database_name);
    let options = PgConnectOptions::from_str(&database_url)
        .expect("parse isolated database URL")
        .database(&database_name);
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options)
        .await
        .expect("connect isolated test database");
    fp_storage::migrate(&pool).await.expect("migrations");
    let mut test_lock = pool.acquire().await.expect("test lock connection");
    sqlx::query("SELECT pg_advisory_lock(420001)")
        .execute(&mut *test_lock)
        .await
        .expect("identity-fixture lock");

    let platform_org = Uuid::now_v7();
    let source_user = Uuid::now_v7();
    let replacement_user = Uuid::now_v7();
    let membership = Uuid::now_v7();
    let suffix = &platform_org.simple().to_string()[..12];
    let source_subject = format!("recovery-source-{suffix}");
    let replacement_subject = format!("recovery-target-{suffix}");
    sqlx::query(
        "INSERT INTO organizations (id, name, display_name, status) VALUES ($1, $2, $2, 'active')",
    )
    .bind(platform_org)
    .bind(format!("recovery-platform-{suffix}"))
    .execute(&pool)
    .await
    .expect("insert platform organization");
    for (user, subject) in [
        (source_user, &source_subject),
        (replacement_user, &replacement_subject),
    ] {
        sqlx::query("INSERT INTO users (id, subject, name, status) VALUES ($1, $2, $2, 'active')")
            .bind(user)
            .bind(subject)
            .execute(&pool)
            .await
            .expect("insert user");
    }
    sqlx::query(
        "INSERT INTO org_memberships (id, org_id, user_id, role) VALUES ($1, $2, $3, 'owner')",
    )
    .bind(membership)
    .bind(platform_org)
    .bind(source_user)
    .execute(&pool)
    .await
    .expect("insert platform owner");
    sqlx::query(
        "INSERT INTO instance_meta (key, value) VALUES ('platform_org_id', $1) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(platform_org.to_string())
    .execute(&pool)
    .await
    .expect("set platform marker");

    let mut command = Command::new(env!("CARGO_BIN_EXE_flowplane"));
    command
        .env_clear()
        .env("FLOWPLANE_DATABASE_URL", &database_url)
        .env("FLOWPLANE_API_INSECURE", "true")
        .args(["db", "recover-platform-admin", "plan", "--subject-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn built flowplane binary");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(replacement_subject.as_bytes())
        .expect("write private subject to stdin");
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let output = loop {
        match child.try_wait().expect("poll flowplane child") {
            Some(_) => break Some(child.wait_with_output().expect("collect child output")),
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let membership_user: Uuid =
        sqlx::query_scalar("SELECT user_id FROM org_memberships WHERE id = $1")
            .bind(membership)
            .fetch_one(&pool)
            .await
            .expect("read unchanged membership");
    let audit_count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&pool)
        .await
        .expect("count audit rows after read-only plan");

    drop(test_lock);
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE \"{database_name}\" WITH (FORCE)"))
        .execute(&admin_pool)
        .await
        .expect("drop isolated test database");

    let output = output.expect("flowplane recovery plan command stayed within timeout");
    assert!(
        output.status.success(),
        "eligible recovery plan command must succeed"
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 plan output");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains(&source_subject));
    assert!(!stdout.contains(&replacement_subject));
    assert!(!stderr.contains(&source_subject));
    assert!(!stderr.contains(&replacement_subject));
    let plan: serde_json::Value = serde_json::from_str(&stdout).expect("JSON plan output");
    let keys = plan
        .as_object()
        .expect("plan object")
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        keys,
        [
            "digest",
            "platform_membership_id",
            "platform_org_id",
            "platform_role",
            "replacement_user_id",
            "replacement_user_status",
            "source_user_id",
            "source_user_status",
            "tenant_transfers",
            "version",
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(plan["platform_org_id"], platform_org.to_string());
    assert_eq!(plan["source_user_id"], source_user.to_string());
    assert_eq!(plan["replacement_user_id"], replacement_user.to_string());
    assert_eq!(plan["platform_membership_id"], membership.to_string());
    assert!(plan["digest"]
        .as_str()
        .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71));
    assert_eq!(membership_user, source_user, "plan must remain read-only");
    assert_eq!(audit_count, 0, "plan must not emit an audit mutation");
}
