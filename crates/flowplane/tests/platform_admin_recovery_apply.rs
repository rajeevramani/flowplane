//! Independent black-box contracts for fpv2-92f.2 recovery apply.
//!
//! Independence boundary: this target is derived only from the approved design/plan, Cargo
//! manifests, migration schema, and existing integration-test conventions. It drives the built
//! `flowplane` binary and imports no recovery implementation modules.

#![allow(clippy::expect_used, clippy::panic)]

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::Row;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const PRIVATE_SUBJECT: &str = "fp-recovery-private-subject-canary-92f2";
const PRIVATE_TOKEN: &str = "fp-recovery-private-token-canary-92f2";
const VALID_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("flowplane-platform-admin-apply-{}", Uuid::now_v7()));
        fs::create_dir_all(&path).expect("create isolated test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flowplane"));
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
        .env("HOME", home)
        .env("FLOWPLANE_CONFIG", home.join("config.toml"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run(mut command: Command, stdin: &[u8]) -> Output {
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("spawn built flowplane binary: {error}"));
    if let Some(mut child_stdin) = child.stdin.take() {
        child_stdin
            .write_all(stdin)
            .expect("write bounded private input to child stdin");
    }

    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        match child.try_wait().expect("poll flowplane child") {
            Some(_) => return child.wait_with_output().expect("collect flowplane output"),
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("flowplane command exceeded {COMMAND_TIMEOUT:?}");
            }
        }
    }
}

fn run_recovery_plan(
    home: &Path,
    config_path: &Path,
    database_url: &str,
    subject: &str,
    tenant: &str,
) -> Output {
    let mut cmd = command(home);
    cmd.env("FLOWPLANE_CONFIG", config_path)
        .env("FLOWPLANE_DATABASE_URL", database_url)
        .env("FLOWPLANE_API_INSECURE", "true")
        .env("FLOWPLANE_TOKEN", PRIVATE_TOKEN)
        .args([
            "db",
            "recover-platform-admin",
            "plan",
            "--subject-stdin",
            "--transfer-owned-org",
            tenant,
        ]);
    run(cmd, subject.as_bytes())
}

fn run_recovery_apply(
    home: &Path,
    config_path: &Path,
    database_url: &str,
    subject: &str,
    tenant: &str,
    expected_plan: &str,
) -> Output {
    let mut cmd = command(home);
    cmd.env("FLOWPLANE_CONFIG", config_path)
        .env("FLOWPLANE_DATABASE_URL", database_url)
        .env("FLOWPLANE_API_INSECURE", "true")
        .env("FLOWPLANE_TOKEN", PRIVATE_TOKEN)
        .args([
            "db",
            "recover-platform-admin",
            "apply",
            "--subject-stdin",
            "--transfer-owned-org",
            tenant,
            "--expected-plan",
            expected_plan,
            "--yes",
        ]);
    run(cmd, subject.as_bytes())
}

fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_apply_recognized(output: &Output) {
    let text = combined(output).to_ascii_lowercase();
    assert!(
        !text.contains("unrecognized subcommand") && !text.contains("unexpected subcommand"),
        "the built CLI must recognize `db recover-platform-admin apply`"
    );
}

fn assert_private_material_absent(output: &Output, private_paths: &[&Path], extras: &[&str]) {
    let text = combined(output);
    for canary in [PRIVATE_SUBJECT, PRIVATE_TOKEN]
        .into_iter()
        .chain(extras.iter().copied())
    {
        assert!(
            !text.contains(canary),
            "private material appeared in process output"
        );
    }
    for path in private_paths {
        assert!(
            !text.contains(path.to_string_lossy().as_ref()),
            "private path appeared in process output"
        );
    }
}

#[test]
fn help_exposes_digest_confirmation_and_private_input_contract() {
    let temp = TempDir::new();
    let mut cmd = command(temp.path());
    cmd.args(["db", "recover-platform-admin", "apply", "--help"]);
    let output = run(cmd, b"");

    assert_apply_recognized(&output);
    assert!(output.status.success(), "recovery apply help must succeed");
    let help = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--subject-stdin",
        "--subject-file",
        "--transfer-owned-org",
        "--expected-plan",
        "--yes",
    ] {
        assert!(help.contains(flag), "apply help must expose `{flag}`");
    }
    assert!(
        !help.to_ascii_lowercase().contains("subject <"),
        "help must not advertise a subject positional argument"
    );
}

#[test]
fn missing_confirmation_fails_before_database_access_without_disclosure() {
    let temp = TempDir::new();
    let mut cmd = command(temp.path());
    cmd.args([
        "db",
        "recover-platform-admin",
        "apply",
        "--subject-stdin",
        "--expected-plan",
        VALID_DIGEST,
    ]);
    let output = run(cmd, PRIVATE_SUBJECT.as_bytes());

    assert_apply_recognized(&output);
    assert!(!output.status.success(), "apply without --yes must fail");
    assert!(
        combined(&output).contains("--yes"),
        "confirmation failure must direct the operator to --yes"
    );
    assert_private_material_absent(&output, &[], &[]);
}

#[test]
fn malformed_expected_digests_fail_before_database_access_without_disclosure() {
    let temp = TempDir::new();
    for digest in [
        "sha256:abcd",
        "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "sha1:0000000000000000000000000000000000000000000000000000000000000000",
    ] {
        let mut cmd = command(temp.path());
        cmd.args([
            "db",
            "recover-platform-admin",
            "apply",
            "--subject-stdin",
            "--expected-plan",
            digest,
            "--yes",
        ]);
        let output = run(cmd, PRIVATE_SUBJECT.as_bytes());

        assert_apply_recognized(&output);
        assert!(!output.status.success(), "malformed digest must fail");
        let text = combined(&output).to_ascii_lowercase();
        assert!(
            text.contains("expected-plan") || text.contains("digest"),
            "malformed digest must produce a digest-scoped diagnostic"
        );
        assert_private_material_absent(&output, &[], &[]);
    }
}

#[test]
fn conflicting_private_inputs_fail_without_subject_or_path_disclosure() {
    let temp = TempDir::new();
    let subject_file = temp.path().join("private-subject-canary-file");
    fs::write(&subject_file, PRIVATE_SUBJECT).expect("write private subject fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&subject_file, fs::Permissions::from_mode(0o600))
            .expect("set owner-only subject fixture mode");
    }

    let mut cmd = command(temp.path());
    cmd.args([
        "db",
        "recover-platform-admin",
        "apply",
        "--subject-stdin",
        "--subject-file",
        subject_file.to_str().expect("UTF-8 fixture path"),
        "--expected-plan",
        VALID_DIGEST,
        "--yes",
    ]);
    let output = run(cmd, PRIVATE_SUBJECT.as_bytes());

    assert_apply_recognized(&output);
    assert!(!output.status.success(), "conflicting inputs must fail");
    assert_private_material_absent(&output, &[&subject_file], &[]);
}

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
async fn apply_transfers_only_explicit_owners_and_writes_one_redacted_audit() {
    let Some(admin_url) = std::env::var("FLOWPLANE_TEST_DATABASE_URL").ok() else {
        eprintln!("skipping: FLOWPLANE_TEST_DATABASE_URL not set");
        return;
    };
    let admin_pool = fp_storage::connect(&admin_url, 2)
        .await
        .expect("test database admin connection");
    let database_name = format!("fp_recovery_apply_{}", Uuid::now_v7().simple());
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
        .expect("test-only singleton fixture lock");

    let platform_org = Uuid::now_v7();
    let selected_org = Uuid::now_v7();
    let unselected_org = Uuid::now_v7();
    let source_user = Uuid::now_v7();
    let replacement_user = Uuid::now_v7();
    let platform_membership = Uuid::now_v7();
    let selected_membership = Uuid::now_v7();
    let unselected_membership = Uuid::now_v7();
    let team = Uuid::now_v7();
    let team_membership = Uuid::now_v7();
    let grant = Uuid::now_v7();
    let suffix = &platform_org.simple().to_string()[..12];
    let platform_name = format!("recovery-platform-{suffix}");
    let selected_name = format!("recovery-selected-{suffix}");
    let unselected_name = format!("recovery-unselected-{suffix}");
    let source_subject = format!("recovery-source-subject-{suffix}");
    let replacement_subject = format!("{PRIVATE_SUBJECT}-{suffix}");
    let source_email = format!("source-private-{suffix}@invalid.example");
    let replacement_email = format!("replacement-private-{suffix}@invalid.example");

    for (org_id, name) in [
        (platform_org, &platform_name),
        (selected_org, &selected_name),
        (unselected_org, &unselected_name),
    ] {
        sqlx::query(
            "INSERT INTO organizations (id, name, display_name, status) VALUES ($1, $2, $2, 'active')",
        )
        .bind(org_id)
        .bind(name)
        .execute(&pool)
        .await
        .expect("insert organization fixture");
    }
    for (user_id, subject, email) in [
        (source_user, &source_subject, &source_email),
        (replacement_user, &replacement_subject, &replacement_email),
    ] {
        sqlx::query(
            "INSERT INTO users (id, subject, email, name, status) VALUES ($1, $2, $3, $2, 'active')",
        )
        .bind(user_id)
        .bind(subject)
        .bind(email)
        .execute(&pool)
        .await
        .expect("insert user fixture");
    }
    for (membership_id, org_id) in [
        (platform_membership, platform_org),
        (selected_membership, selected_org),
        (unselected_membership, unselected_org),
    ] {
        sqlx::query(
            "INSERT INTO org_memberships (id, org_id, user_id, role) VALUES ($1, $2, $3, 'owner')",
        )
        .bind(membership_id)
        .bind(org_id)
        .bind(source_user)
        .execute(&pool)
        .await
        .expect("insert owner membership fixture");
    }
    sqlx::query(
        "INSERT INTO instance_meta (key, value) VALUES ('platform_org_id', $1) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(platform_org.to_string())
    .execute(&pool)
    .await
    .expect("set platform organization marker");
    sqlx::query(
        "INSERT INTO teams (id, org_id, name, display_name, status) \
         VALUES ($1, $2, 'recovery-team', 'recovery-team', 'active')",
    )
    .bind(team)
    .bind(unselected_org)
    .execute(&pool)
    .await
    .expect("insert team fixture");
    sqlx::query("INSERT INTO team_memberships (id, user_id, team_id) VALUES ($1, $2, $3)")
        .bind(team_membership)
        .bind(source_user)
        .bind(team)
        .execute(&pool)
        .await
        .expect("insert team membership fixture");
    sqlx::query(
        "INSERT INTO user_grants \
         (id, user_id, org_id, team_id, resource, action, created_by) \
         VALUES ($1, $2, $3, $4, 'clusters', 'read', $2)",
    )
    .bind(grant)
    .bind(source_user)
    .bind(unselected_org)
    .bind(team)
    .execute(&pool)
    .await
    .expect("insert grant fixture");

    let temp = TempDir::new();
    let private_config_path = temp.path().join("private-config-canary.toml");
    fs::write(&private_config_path, "").expect("write isolated empty config");

    let plan_output = run_recovery_plan(
        temp.path(),
        &private_config_path,
        &database_url,
        &replacement_subject,
        &selected_name,
    );
    let plan_json: serde_json::Value = serde_json::from_slice(&plan_output.stdout)
        .expect("eligible setup must produce a JSON recovery plan");
    let expected_plan = plan_json["digest"]
        .as_str()
        .expect("plan digest string")
        .to_owned();

    sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
        .bind(replacement_user)
        .execute(&pool)
        .await
        .expect("suspend replacement after plan");
    let status_drift_output = run_recovery_apply(
        temp.path(),
        &private_config_path,
        &database_url,
        &replacement_subject,
        &selected_name,
        &expected_plan,
    );
    sqlx::query("UPDATE users SET status = 'active' WHERE id = $1")
        .bind(replacement_user)
        .execute(&pool)
        .await
        .expect("restore replacement after drift check");
    let owner_after_status_drift: Uuid =
        sqlx::query_scalar("SELECT user_id FROM org_memberships WHERE id = $1")
            .bind(platform_membership)
            .fetch_one(&pool)
            .await
            .expect("read platform owner after status drift");

    sqlx::query(
        "CREATE FUNCTION reject_recovery_audit() RETURNS trigger AS $$ \
         BEGIN RAISE EXCEPTION 'injected recovery audit failure'; END; $$ LANGUAGE plpgsql",
    )
    .execute(&pool)
    .await
    .expect("install audit failure function");
    sqlx::query(
        "CREATE TRIGGER reject_recovery_audit BEFORE INSERT ON audit_log \
         FOR EACH ROW WHEN (NEW.actor_label = 'offline-platform-admin-recovery') \
         EXECUTE FUNCTION reject_recovery_audit()",
    )
    .execute(&pool)
    .await
    .expect("install audit failure trigger");
    let fault_output = run_recovery_apply(
        temp.path(),
        &private_config_path,
        &database_url,
        &replacement_subject,
        &selected_name,
        &expected_plan,
    );
    let owner_after_fault: Uuid =
        sqlx::query_scalar("SELECT user_id FROM org_memberships WHERE id = $1")
            .bind(platform_membership)
            .fetch_one(&pool)
            .await
            .expect("read platform owner after audit fault");
    let audit_after_fault: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&pool)
        .await
        .expect("count audit after injected failure");
    sqlx::query("DROP TRIGGER reject_recovery_audit ON audit_log")
        .execute(&pool)
        .await
        .expect("remove audit failure trigger");
    sqlx::query("DROP FUNCTION reject_recovery_audit()")
        .execute(&pool)
        .await
        .expect("remove audit failure function");

    let apply_output = run_recovery_apply(
        temp.path(),
        &private_config_path,
        &database_url,
        &replacement_subject,
        &selected_name,
        &expected_plan,
    );
    let stale_output = run_recovery_apply(
        temp.path(),
        &private_config_path,
        &database_url,
        &replacement_subject,
        &selected_name,
        &expected_plan,
    );

    let membership_rows = sqlx::query(
        "SELECT id, user_id, role FROM org_memberships \
         WHERE id = ANY($1) ORDER BY id",
    )
    .bind(vec![
        platform_membership,
        selected_membership,
        unselected_membership,
    ])
    .fetch_all(&pool)
    .await
    .expect("read resulting owner memberships");
    let replacement_membership_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM org_memberships WHERE user_id = $1")
            .bind(replacement_user)
            .fetch_one(&pool)
            .await
            .expect("count replacement memberships");
    let team_membership_user: Uuid =
        sqlx::query_scalar("SELECT user_id FROM team_memberships WHERE id = $1")
            .bind(team_membership)
            .fetch_one(&pool)
            .await
            .expect("read unchanged team membership");
    let grant_principal: Uuid = sqlx::query_scalar("SELECT user_id FROM user_grants WHERE id = $1")
        .bind(grant)
        .fetch_one(&pool)
        .await
        .expect("read unchanged grant");
    let audit_rows = sqlx::query(
        "SELECT actor_type, actor_id, actor_label, surface, action, outcome, detail \
         FROM audit_log ORDER BY occurred_at, id",
    )
    .fetch_all(&pool)
    .await
    .expect("read recovery audit rows");
    let replacement_principal =
        fp_storage::repos::identity::load_principal(&pool, &replacement_subject)
            .await
            .expect("load replacement principal")
            .expect("replacement principal exists");
    let replacement_membership_orgs = replacement_principal
        .memberships
        .iter()
        .map(|(org_id, _)| org_id.as_uuid())
        .collect::<std::collections::BTreeSet<_>>();

    let rollback_plan_output = run_recovery_plan(
        temp.path(),
        &private_config_path,
        &database_url,
        &source_subject,
        &selected_name,
    );
    let rollback_plan_json: serde_json::Value =
        serde_json::from_slice(&rollback_plan_output.stdout)
            .expect("fresh rollback plan must be JSON");
    let rollback_digest = rollback_plan_json["digest"]
        .as_str()
        .expect("rollback digest")
        .to_owned();
    let rollback_output = run_recovery_apply(
        temp.path(),
        &private_config_path,
        &database_url,
        &source_subject,
        &selected_name,
        &rollback_digest,
    );
    let final_membership_users: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM org_memberships WHERE id = ANY($1) ORDER BY id")
            .bind(vec![platform_membership, selected_membership])
            .fetch_all(&pool)
            .await
            .expect("read rolled-back memberships");
    let final_audit_count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&pool)
        .await
        .expect("count forward and rollback audits");

    sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
        .bind(source_user)
        .execute(&pool)
        .await
        .expect("suspend prior owner fixture");
    let suspended_plan_output = run_recovery_plan(
        temp.path(),
        &private_config_path,
        &database_url,
        &replacement_subject,
        &selected_name,
    );
    let suspended_plan_json: serde_json::Value =
        serde_json::from_slice(&suspended_plan_output.stdout).expect("suspended-source plan JSON");
    let suspended_digest = suspended_plan_json["digest"]
        .as_str()
        .expect("suspended-source digest")
        .to_owned();
    let suspended_apply_output = run_recovery_apply(
        temp.path(),
        &private_config_path,
        &database_url,
        &replacement_subject,
        &selected_name,
        &suspended_digest,
    );
    let suspended_apply_json: serde_json::Value =
        serde_json::from_slice(&suspended_apply_output.stdout)
            .expect("suspended-source apply result JSON");
    let final_audit_after_suspended: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&pool)
        .await
        .expect("count suspended-source recovery audit");

    let platform_only_user = Uuid::now_v7();
    let platform_only_subject = format!("recovery-platform-only-{suffix}");
    sqlx::query("INSERT INTO users (id, subject, name, status) VALUES ($1, $2, $2, 'active')")
        .bind(platform_only_user)
        .bind(&platform_only_subject)
        .execute(&pool)
        .await
        .expect("insert platform-only replacement");
    let platform_only_plan =
        fp_core::services::platform_admin_recovery::plan(&pool, &platform_only_subject, &[])
            .await
            .expect("platform-only plan");
    let platform_only_result = fp_core::services::platform_admin_recovery::apply(
        &pool,
        &platform_only_subject,
        &[],
        &platform_only_plan.digest,
    )
    .await
    .expect("platform-only apply");
    let platform_only_principal =
        fp_storage::repos::identity::load_principal(&pool, &platform_only_subject)
            .await
            .expect("load platform-only principal")
            .expect("platform-only principal exists");
    let platform_only_audit_count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&pool)
        .await
        .expect("count platform-only audit");

    drop(test_lock);
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE \"{database_name}\" WITH (FORCE)"))
        .execute(&admin_pool)
        .await
        .expect("drop isolated test database");

    assert!(plan_output.status.success(), "eligible plan must succeed");
    assert!(
        !status_drift_output.status.success(),
        "status drift must fail apply"
    );
    assert_eq!(
        owner_after_status_drift, source_user,
        "status drift commits no owner change"
    );
    assert!(
        !fault_output.status.success(),
        "audit failure must fail apply"
    );
    assert_eq!(
        owner_after_fault, source_user,
        "audit failure rolls back owner"
    );
    assert_eq!(audit_after_fault, 0, "failed apply writes no audit row");
    assert_apply_recognized(&apply_output);
    let mut safe_diagnostic = combined(&apply_output);
    for private in [
        source_subject.as_str(),
        replacement_subject.as_str(),
        source_email.as_str(),
        replacement_email.as_str(),
        PRIVATE_TOKEN,
        database_url.as_str(),
        private_config_path.to_string_lossy().as_ref(),
    ] {
        safe_diagnostic = safe_diagnostic.replace(private, "[REDACTED]");
    }
    assert!(
        apply_output.status.success(),
        "eligible digest-bound confirmed apply must succeed: {safe_diagnostic}"
    );
    assert_private_material_absent(
        &apply_output,
        &[&private_config_path],
        &[
            &source_subject,
            &replacement_subject,
            &source_email,
            &replacement_email,
            &database_url,
        ],
    );
    let apply_result: serde_json::Value =
        serde_json::from_slice(&apply_output.stdout).expect("successful apply result JSON");
    assert_eq!(apply_result["rollback_available"], true);
    assert!(
        !stale_output.status.success(),
        "stale digest replay must fail"
    );
    assert_private_material_absent(&stale_output, &[&private_config_path], &[&database_url]);
    assert_eq!(membership_rows.len(), 3, "fixture membership rows remain");
    for row in membership_rows {
        let membership_id: Uuid = row.get("id");
        let user_id: Uuid = row.get("user_id");
        let role: String = row.get("role");
        assert_eq!(role, "owner", "transferred roles remain owner");
        let expected_user = if membership_id == unselected_membership {
            source_user
        } else {
            replacement_user
        };
        assert_eq!(user_id, expected_user, "only selected owners transfer");
    }
    assert_eq!(
        replacement_membership_count, 2,
        "replacement receives platform plus exactly one selected tenant"
    );
    assert!(replacement_principal.platform_admin);
    assert_eq!(
        replacement_membership_orgs,
        [platform_org, selected_org].into_iter().collect(),
        "normal principal loading grants only platform governance and the selected tenant"
    );
    assert_eq!(
        team_membership_user, source_user,
        "team membership must not transfer"
    );
    assert_eq!(grant_principal, source_user, "grant must not transfer");
    assert_eq!(audit_rows.len(), 1, "apply writes exactly one audit row");
    let audit = &audit_rows[0];
    assert_eq!(audit.get::<String, _>("actor_type"), "system");
    assert_eq!(audit.get::<Option<Uuid>, _>("actor_id"), None);
    assert_eq!(
        audit.get::<String, _>("actor_label"),
        "offline-platform-admin-recovery"
    );
    assert_eq!(audit.get::<String, _>("surface"), "cli");
    assert_eq!(audit.get::<String, _>("action"), "platform_admin.recover");
    assert_eq!(audit.get::<String, _>("outcome"), "success");
    let audit_detail: serde_json::Value = audit.get("detail");
    let audit_text = audit_detail.to_string();
    for private in [
        source_subject.as_str(),
        replacement_subject.as_str(),
        source_email.as_str(),
        replacement_email.as_str(),
        PRIVATE_TOKEN,
        database_url.as_str(),
        private_config_path.to_string_lossy().as_ref(),
    ] {
        assert!(
            !audit_text.contains(private),
            "private material appeared in audit detail"
        );
    }
    assert!(
        rollback_plan_output.status.success(),
        "fresh rollback plan succeeds"
    );
    assert!(
        rollback_output.status.success(),
        "fresh rollback apply succeeds"
    );
    assert_private_material_absent(
        &rollback_output,
        &[&private_config_path],
        &[&source_subject, &replacement_subject, &database_url],
    );
    assert_eq!(
        final_membership_users,
        vec![source_user, source_user],
        "fresh rollback restores both selected owner rows"
    );
    assert_eq!(final_audit_count, 2, "forward and rollback each audit once");
    assert!(suspended_plan_output.status.success());
    assert!(suspended_apply_output.status.success());
    assert_eq!(suspended_apply_json["rollback_available"], false);
    assert_eq!(
        final_audit_after_suspended, 3,
        "suspended-source recovery adds one atomic audit"
    );
    assert!(platform_only_result.tenant_org_ids.is_empty());
    assert_eq!(platform_only_result.transferred_memberships, 1);
    assert!(platform_only_principal.platform_admin);
    assert_eq!(
        platform_only_principal.memberships.len(),
        1,
        "platform governance alone grants no tenant membership"
    );
    assert_eq!(platform_only_audit_count, 4);
}
