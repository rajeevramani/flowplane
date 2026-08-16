//! RED black-box contract for fpv2-7f3.9 upgrade preflight.
//!
//! Independence boundary: this test is derived only from the approved upgrade/rollback design,
//! slice plan, existing integration-test conventions, and public CLI help. It drives the built
//! binary and does not import implementation modules.
//!
//! Populated migration and blocker-redaction contracts live at the storage boundary; published
//! old-binary rejection and the rollback cutoff are exercised by release-qualification rehearsal.

#![allow(clippy::expect_used)]

use std::process::Command;

#[test]
fn db_help_exposes_the_operator_upgrade_preflight() {
    let output = Command::new(env!("CARGO_BIN_EXE_flowplane"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .args(["db", "preflight", "--help"])
        .output()
        .expect("run flowplane db preflight --help");

    assert!(
        output.status.success(),
        "`flowplane db preflight` must be the single operator preflight seam\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
