//! RED black-box contracts for fpv2-92f.1 platform-admin recovery planning.
//!
//! Independence boundary: this target is derived only from the approved design/plan, manifests,
//! migration schema contracts, public configuration docs, and existing integration-test
//! conventions. It drives the built `flowplane` binary and imports no production modules.
//!
//! A real-PostgreSQL success fixture is intentionally not constructed here: the allowed migration
//! and integration-test surfaces do not define the singleton `instance_meta` platform-organization
//! marker needed for exact setup/restore. Guessing that private storage contract would make a
//! shared-database test unsafe. Storage/core coverage owns that matrix until a public fixture seam
//! exists.

#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use uuid::Uuid;

const PRIVATE_SUBJECT: &str = "fp-recovery-private-subject-canary-92f1";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "flowplane-platform-admin-recovery-{}",
            Uuid::now_v7()
        ));
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

fn flowplane(home: &Path) -> Command {
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

fn run(args: &[&str], stdin: &[u8], home: &Path) -> Output {
    let mut child = flowplane(home)
        .args(args)
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

fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_recovery_command_was_recognized(output: &Output) {
    let text = combined(output).to_ascii_lowercase();
    assert!(
        !text.contains("unrecognized subcommand") && !text.contains("unexpected subcommand"),
        "the built CLI must recognize `db recover-platform-admin plan`"
    );
}

fn assert_private_material_absent(output: &Output, private_paths: &[&Path]) {
    let text = combined(output);
    assert!(
        !text.contains(PRIVATE_SUBJECT),
        "private subject material appeared in process output"
    );
    for path in private_paths {
        let rendered = path.to_string_lossy();
        assert!(
            !text.contains(rendered.as_ref()),
            "private subject-file path appeared in process output"
        );
    }
}

#[test]
fn help_exposes_the_nested_plan_contract_and_private_input_flags() {
    let temp = TempDir::new();
    let output = run(
        &["db", "recover-platform-admin", "plan", "--help"],
        b"",
        temp.path(),
    );

    assert!(
        output.status.success(),
        "the recovery plan help leaf must be available"
    );
    let help = String::from_utf8_lossy(&output.stdout);
    for flag in ["--subject-stdin", "--subject-file", "--transfer-owned-org"] {
        assert!(help.contains(flag), "plan help must expose `{flag}`");
    }
    assert!(
        !help.to_ascii_lowercase().contains("subject <"),
        "help must not advertise a subject positional argument"
    );
}

#[test]
fn conflicting_private_input_flags_fail_without_disclosure() {
    let temp = TempDir::new();
    let subject_file = temp.path().join("replacement-subject");
    fs::write(&subject_file, PRIVATE_SUBJECT).expect("write private-input fixture");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&subject_file, fs::Permissions::from_mode(0o600))
            .expect("set owner-only fixture mode");
    }

    let output = run(
        &[
            "db",
            "recover-platform-admin",
            "plan",
            "--subject-stdin",
            "--subject-file",
            subject_file.to_str().expect("UTF-8 fixture path"),
        ],
        PRIVATE_SUBJECT.as_bytes(),
        temp.path(),
    );

    assert_recovery_command_was_recognized(&output);
    assert!(!output.status.success(), "conflicting inputs must fail");
    assert_private_material_absent(&output, &[&subject_file]);
}

#[test]
fn subject_on_argv_is_rejected_without_echoing_it() {
    let temp = TempDir::new();
    let output = run(
        &["db", "recover-platform-admin", "plan", PRIVATE_SUBJECT],
        b"",
        temp.path(),
    );

    assert_recovery_command_was_recognized(&output);
    assert!(
        !output.status.success(),
        "subject positional input must fail"
    );
    assert_private_material_absent(&output, &[]);
}

#[test]
fn empty_stdin_fails_and_nonempty_stdin_reaches_the_offline_command() {
    let temp = TempDir::new();
    let empty = run(
        &["db", "recover-platform-admin", "plan", "--subject-stdin"],
        b" \n\t",
        temp.path(),
    );
    assert_recovery_command_was_recognized(&empty);
    assert!(!empty.status.success(), "trimmed-empty stdin must fail");
    assert_private_material_absent(&empty, &[]);

    // With no database URL in this child-only environment, valid private input cannot produce a
    // plan, but it must pass argv/private-input parsing and fail without disclosing the subject.
    let nonempty = run(
        &["db", "recover-platform-admin", "plan", "--subject-stdin"],
        format!("  {PRIVATE_SUBJECT}\n").as_bytes(),
        temp.path(),
    );
    assert_recovery_command_was_recognized(&nonempty);
    assert!(
        !nonempty.status.success(),
        "a plan without database configuration must fail closed"
    );
    assert_private_material_absent(&nonempty, &[]);
}

#[cfg(unix)]
#[test]
fn unix_subject_files_reject_symlinks_and_broad_modes_without_disclosure() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let temp = TempDir::new();
    let target = temp.path().join("subject-target");
    fs::write(&target, PRIVATE_SUBJECT).expect("write private-input target");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
        .expect("set target owner-only mode");

    let link = temp.path().join("subject-link");
    symlink(&target, &link).expect("create private-input symlink");
    let symlink_output = run(
        &[
            "db",
            "recover-platform-admin",
            "plan",
            "--subject-file",
            link.to_str().expect("UTF-8 fixture path"),
        ],
        b"",
        temp.path(),
    );
    assert_recovery_command_was_recognized(&symlink_output);
    assert!(!symlink_output.status.success(), "symlink input must fail");
    assert_private_material_absent(&symlink_output, &[&link, &target]);

    let broad = temp.path().join("subject-broad-mode");
    fs::write(&broad, PRIVATE_SUBJECT).expect("write broad-mode fixture");
    fs::set_permissions(&broad, fs::Permissions::from_mode(0o640)).expect("set broad fixture mode");
    let broad_output = run(
        &[
            "db",
            "recover-platform-admin",
            "plan",
            "--subject-file",
            broad.to_str().expect("UTF-8 fixture path"),
        ],
        b"",
        temp.path(),
    );
    assert_recovery_command_was_recognized(&broad_output);
    assert!(!broad_output.status.success(), "broad file mode must fail");
    assert_private_material_absent(&broad_output, &[&broad]);
}
