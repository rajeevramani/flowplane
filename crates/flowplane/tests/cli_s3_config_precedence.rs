//! S3 — CLI config-precedence conformance (black-box).
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against
//! the documented precedence contract (CLI-R-40/41). They never look at CLI internals: every
//! assertion is derived from the acceptance criteria, not the implementation.
//!
//! Contract under test (CLI-R-40/41):
//!   * Uniform precedence for EVERY configuration value: `flag > env > context > file > default`.
//!   * Token source ladder: `--token` flag > `FLOWPLANE_TOKEN` env > selected context token >
//!     config-file token > `~/.flowplane/credentials` file.
//!   * `FLOWPLANE_TIMEOUT` feeds `--timeout` (precedence flag > env > default 30).
//!   * `server`/`org`/`team` follow the same `flag > env > context > file > default` rule.
//!   * `config show` redacts token material (never prints a real token).
//!
//! Observation technique: `flowplane cluster get probe --team payments -o json` hits a mock
//! whose `data` echoes what the server received — `data.received_authorization` (the `Bearer …`
//! header) and `data.received_org` (the `X-Flowplane-Org` header). Setting a config value via
//! different sources and reading the echoed header proves which source actually won. Each test
//! is adversarial: lower-priority sources are ALSO populated, so the assertion proves the
//! higher-priority source wins — not merely that it works in isolation.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::Path;
use std::process::Output;

use serde_json::Value;

/// Write `<home>/config.toml` (the path `flowplane_cmd` points `FLOWPLANE_CONFIG` at).
fn write_config(home: &Path, contents: &str) {
    std::fs::write(home.join("config.toml"), contents).expect("write config.toml");
}

/// Write the credentials file that lives next to the config path as `<config dir>/credentials`.
/// With `FLOWPLANE_CONFIG=<home>/config.toml`, that is `<home>/credentials`.
fn write_credentials(home: &Path, token: &str) {
    std::fs::write(home.join("credentials"), token).expect("write credentials");
}

/// Parse the probe success envelope `{schemaVersion,kind,data}` from stdout, asserting exit 0.
fn parse_probe(out: &Output, ctx: &str) -> Value {
    assert!(
        out.status.success(),
        "{ctx}: probe must exit 0, got {:?}; stderr: {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{ctx}: stdout must be a JSON envelope ({e}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The `Bearer …` authorization header the CLI actually sent, as echoed by the probe.
fn received_authorization(out: &Output, ctx: &str) -> String {
    let v = parse_probe(out, ctx);
    v["data"]["received_authorization"]
        .as_str()
        .unwrap_or_else(|| panic!("{ctx}: probe must echo received_authorization: {v}"))
        .to_string()
}

/// The `X-Flowplane-Org` header the CLI actually sent, as echoed by the probe (may be null).
fn received_org(out: &Output, ctx: &str) -> Option<String> {
    let v = parse_probe(out, ctx);
    v["data"]["received_org"].as_str().map(str::to_string)
}

// ---------------------------------------------------------------------------------------------
// Criterion 1: token precedence ladder — flag > env > context > file > credentials.
// Each rung populates ALL lower-priority sources, so winning proves precedence, not isolation.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_precedence_ladder() {
    let mock = common::start_mock().await;

    // A config file that supplies BOTH a context token and a file token (and a credentials
    // file alongside it), so every rung below the flag/env is also present.
    let config = format!(
        r#"base_url = "{server}"
token = "file-tok"
current_context = "prod"

[[contexts]]
name = "prod"
server = "{server}"
token = "ctx-tok"
"#,
        server = mock.base_url()
    );

    // Rung 1 — flag wins over env + context + file + credentials.
    {
        let home = common::unique_tempdir();
        write_config(&home, &config);
        write_credentials(&home, "cred-tok");
        let out = common::flowplane_cmd(&home)
            .env("FLOWPLANE_TOKEN", "env-tok")
            .args([
                "cluster", "get", "probe", "--team", "payments", "--token", "flag-tok", "-o",
                "json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            received_authorization(&out, "rung1 flag"),
            "Bearer flag-tok",
            "the --token flag must beat env, context, file, and credentials"
        );
    }

    // Rung 2 — env wins over context + file + credentials (no flag).
    {
        let home = common::unique_tempdir();
        write_config(&home, &config);
        write_credentials(&home, "cred-tok");
        let out = common::flowplane_cmd(&home)
            .env("FLOWPLANE_TOKEN", "env-tok")
            .args([
                "cluster", "get", "probe", "--team", "payments", "-o", "json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            received_authorization(&out, "rung2 env"),
            "Bearer env-tok",
            "FLOWPLANE_TOKEN must beat context, file, and credentials when no flag is set"
        );
    }

    // Rung 3 — selected context token wins over file + credentials (no flag/env).
    {
        let home = common::unique_tempdir();
        write_config(&home, &config);
        write_credentials(&home, "cred-tok");
        let out = common::flowplane_cmd(&home)
            .args([
                "cluster", "get", "probe", "--team", "payments", "-o", "json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            received_authorization(&out, "rung3 context"),
            "Bearer ctx-tok",
            "the selected context's token must beat the file token and credentials"
        );
    }

    // Rung 4 — file token wins over credentials (no flag/env/context).
    {
        let home = common::unique_tempdir();
        write_config(
            &home,
            &format!(
                "base_url = \"{server}\"\ntoken = \"file-tok\"\n",
                server = mock.base_url()
            ),
        );
        write_credentials(&home, "cred-tok");
        let out = common::flowplane_cmd(&home)
            .args([
                "cluster", "get", "probe", "--team", "payments", "-o", "json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            received_authorization(&out, "rung4 file"),
            "Bearer file-tok",
            "the config-file token must beat the credentials file"
        );
    }

    // Rung 5 — credentials file only (no flag/env/context/file token).
    {
        let home = common::unique_tempdir();
        write_config(
            &home,
            &format!("base_url = \"{server}\"\n", server = mock.base_url()),
        );
        write_credentials(&home, "cred-tok");
        let out = common::flowplane_cmd(&home)
            .args([
                "cluster", "get", "probe", "--team", "payments", "-o", "json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            received_authorization(&out, "rung5 credentials"),
            "Bearer cred-tok",
            "the ~/.flowplane/credentials file is the lowest token tier and must be used last"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Criterion 3 (explicit): the `--token` flag exists and resolves flag-first over env + file.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_flag_resolves_flag_first() {
    let mock = common::start_mock().await;
    let home = common::unique_tempdir();
    write_config(
        &home,
        &format!(
            "base_url = \"{server}\"\ntoken = \"file-tok\"\n",
            server = mock.base_url()
        ),
    );

    let out = common::flowplane_cmd(&home)
        .env("FLOWPLANE_TOKEN", "env-tok")
        .args([
            "cluster", "get", "probe", "--team", "payments", "--token", "flag-tok", "-o", "json",
        ])
        .output()
        .unwrap();

    assert_eq!(
        received_authorization(&out, "token flag-first"),
        "Bearer flag-tok",
        "the --token global flag must exist and resolve flag-first"
    );
}

// ---------------------------------------------------------------------------------------------
// Criterion 2: org precedence — flag --org beats env FLOWPLANE_ORG beats file `org`.
// Observed via the echoed X-Flowplane-Org header.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn org_precedence_flag_beats_env_beats_file() {
    let mock = common::start_mock().await;

    let config = format!(
        "base_url = \"{server}\"\ntoken = \"file-tok\"\norg = \"file-org\"\n",
        server = mock.base_url()
    );

    // Flag wins over env + file.
    {
        let home = common::unique_tempdir();
        write_config(&home, &config);
        let out = common::flowplane_cmd(&home)
            .env("FLOWPLANE_ORG", "env-org")
            .args([
                "cluster", "get", "probe", "--team", "payments", "--org", "flag-org", "-o", "json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            received_org(&out, "org flag").as_deref(),
            Some("flag-org"),
            "--org must beat FLOWPLANE_ORG and the file org"
        );
    }

    // Env wins over file (no flag).
    {
        let home = common::unique_tempdir();
        write_config(&home, &config);
        let out = common::flowplane_cmd(&home)
            .env("FLOWPLANE_ORG", "env-org")
            .args([
                "cluster", "get", "probe", "--team", "payments", "-o", "json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            received_org(&out, "org env").as_deref(),
            Some("env-org"),
            "FLOWPLANE_ORG must beat the file org when no flag is set"
        );
    }

    // File wins as the lowest set tier (no flag/env).
    {
        let home = common::unique_tempdir();
        write_config(&home, &config);
        let out = common::flowplane_cmd(&home)
            .args([
                "cluster", "get", "probe", "--team", "payments", "-o", "json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            received_org(&out, "org file").as_deref(),
            Some("file-org"),
            "the file org must be used when neither flag nor env is set"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Criterion 4: FLOWPLANE_TIMEOUT is accepted (feeds --timeout); a literal --timeout flag parses.
// The timeout value isn't black-box observable, so we only prove acceptance + a valid envelope.
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timeout_env_and_flag_are_accepted() {
    let mock = common::start_mock().await;

    // Env form: FLOWPLANE_TIMEOUT=5 must be accepted and produce a clean probe envelope.
    {
        let home = common::unique_tempdir();
        let out = common::flowplane_cmd(&home)
            .env("FLOWPLANE_SERVER", mock.base_url())
            .env("FLOWPLANE_TOKEN", "env-tok")
            .env("FLOWPLANE_TIMEOUT", "5")
            .args([
                "cluster", "get", "probe", "--team", "payments", "-o", "json",
            ])
            .output()
            .unwrap();
        let v = parse_probe(&out, "FLOWPLANE_TIMEOUT env");
        assert_eq!(
            v["data"]["received_authorization"], "Bearer env-tok",
            "FLOWPLANE_TIMEOUT=5 must be accepted without error and the request still go through: {v}"
        );
    }

    // Flag form: --timeout 5 must also parse and run cleanly.
    {
        let home = common::unique_tempdir();
        let out = common::flowplane_cmd(&home)
            .env("FLOWPLANE_SERVER", mock.base_url())
            .env("FLOWPLANE_TOKEN", "env-tok")
            .args([
                "cluster",
                "get",
                "probe",
                "--team",
                "payments",
                "--timeout",
                "5",
                "-o",
                "json",
            ])
            .output()
            .unwrap();
        let v = parse_probe(&out, "--timeout flag");
        assert_eq!(
            v["data"]["received_authorization"], "Bearer env-tok",
            "--timeout 5 must parse and the request still go through: {v}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Criterion 5: `config show` redacts token material (never prints a real token).
// ---------------------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_show_redacts_token() {
    let home = common::unique_tempdir();
    write_config(
        &home,
        "base_url = \"http://example.invalid\"\ntoken = \"super-secret-123\"\n",
    );

    let out = common::flowplane_cmd(&home)
        .args(["config", "show"])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "config show must exit 0, got {:?}; stderr: {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stdout.contains("super-secret-123"),
        "config show must REDACT the token, but stdout contained the raw secret: {stdout:?}"
    );
    assert!(
        !stderr.contains("super-secret-123"),
        "config show must REDACT the token everywhere, but stderr contained the raw secret: {stderr:?}"
    );
}
