//! S4 (fpv2-5kn.4 / AC7) — the read-only `agent` CLI family: contract + REST data-equivalence.
//!
//! These tests drive the *built* `flowplane` binary as a subprocess and assert only against the
//! documented output contract. They never read CLI dispatch internals: every assertion is derived
//! from the S4 contract and the existing black-box CLI test patterns (see `cli_s5_schema_fields.rs`
//! and `cli_s7_coverage.rs`).
//!
//! Contract under test (a thin, read-only REST client flowing through the shared `render()` +
//! RestClient envelope, exactly like `org list` / `cluster list`):
//!   * `flowplane agent list [--team <name|uuid>] [--limit N] [--offset N]` → GET /api/v1/agents
//!     (Page envelope).
//!   * `flowplane agent show <id>`                                          → GET /api/v1/agents/{id}
//!   * `flowplane agent grants <id> [--limit N] [--offset N]`               → GET
//!     /api/v1/agents/{id}/grants (Page envelope).
//!
//! Two layers of proof:
//!   1. CLI CATALOG (no network): the `agent` command + its three leaves + their args are present
//!      and correctly shaped, read black-box from `flowplane schema -o json`. This is the most
//!      robust AC7 "CLI contract" check available without a live server.
//!   2. REST DATA-EQUIVALENCE (live round-trip): against an in-test mock of the agents surface, the
//!      three leaves hit the documented endpoints and RENDER THE SAME DATA the REST reads return —
//!      list/grants unwrap the Page envelope's items, show renders the single object. This is the
//!      direct AC7 "same data the REST reads return" contract.
//!
//! Parallel-safety: every test uses a per-test `unique_tempdir()` and (for the live layer) a
//! per-test ephemeral-port mock with per-child env; no shared global state, no `--test-threads=1`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeSet;
use std::process::Output;
use std::sync::{Arc, Mutex};

use axum::extract::{OriginalUri, Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------------------------
// Schema-catalog helpers (mirror the black-box harness in cli_s5 / cli_s7).
// ---------------------------------------------------------------------------------------------

fn exit_code(out: &Output) -> i32 {
    out.status
        .code()
        .unwrap_or_else(|| panic!("process terminated without an exit code (killed by signal?)"))
}

/// Run `flowplane schema -o json` with NO server configured (schema makes no network call) and
/// return the parsed envelope.
fn live_schema() -> Value {
    let home = common::unique_tempdir();
    let out = common::flowplane_cmd(&home)
        .args(["schema", "-o", "json"])
        .output()
        .expect("run schema -o json");
    assert_eq!(
        exit_code(&out),
        0,
        "schema -o json must exit 0; stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    // schema must make no network call → no error chatter on stderr.
    assert!(
        out.stderr.is_empty(),
        "schema must produce no stderr (no network, no error): {:?}",
        String::from_utf8_lossy(&out.stderr),
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "schema stdout is not a JSON envelope ({e}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The recursive root `command` node.
fn root_command(schema: &Value) -> Value {
    let cmd = schema["data"]["command"].clone();
    assert!(
        cmd.is_object(),
        "data.command must be the root command object"
    );
    cmd
}

/// Return a direct child subcommand node by name, or None.
fn child<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node["subcommands"]
        .as_array()
        .and_then(|subs| subs.iter().find(|s| s["name"].as_str() == Some(name)))
}

/// Names of a node's direct subcommands.
fn sub_names(node: &Value) -> BTreeSet<String> {
    node["subcommands"]
        .as_array()
        .map(|subs| {
            subs.iter()
                .map(|s| {
                    s["name"]
                        .as_str()
                        .unwrap_or_else(|| panic!("each subcommand needs a string name: {s}"))
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Find a leaf's own arg object by its `name` (positionals) or `long` (flags).
fn arg_by<'a>(node: &'a Value, key: &str) -> Option<&'a Value> {
    node["args"].as_array().and_then(|args| {
        args.iter()
            .find(|a| a["name"].as_str() == Some(key) || a["long"].as_str() == Some(key))
    })
}

// =============================================================================================
// Test 1 — `agent` is a top-level command with EXACTLY the leaves list/show/grants.
// =============================================================================================
#[test]
fn agent_is_top_level_with_exactly_three_leaves() {
    let schema = live_schema();
    let root = root_command(&schema);

    let agent = child(&root, "agent")
        .unwrap_or_else(|| panic!("`agent` must be a top-level command in the CLI catalog"));

    // Non-empty one-line `about` (CLI-R-05), and the command itself carries no positional/flag
    // of its own (it is a pure group; all args live on the leaves).
    assert!(
        agent["about"]
            .as_str()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false),
        "`agent` must render a non-empty one-line about: {agent}"
    );
    assert_eq!(
        agent["args"].as_array().map(|a| a.len()).unwrap_or(0),
        0,
        "`agent` group must not declare its own args (they live on the leaves): {}",
        agent["args"]
    );

    // Subcommands are EXACTLY {list, show, grants} — no extra (no mutators: read-only family).
    let expected: BTreeSet<String> = ["list", "show", "grants"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(
        sub_names(agent),
        expected,
        "`agent` subcommands must be EXACTLY {{list, show, grants}} (read-only), got {:?}",
        sub_names(agent)
    );

    // Each leaf is itself a leaf (no further subcommands).
    for leaf in ["list", "show", "grants"] {
        let node = child(agent, leaf).unwrap();
        assert!(
            node["subcommands"]
                .as_array()
                .map(|s| s.is_empty())
                .unwrap_or(true),
            "`agent {leaf}` must be a leaf with no further subcommands: {node}"
        );
    }
}

// =============================================================================================
// Test 2 — leaf arg shapes match the contract.
//   * agent list   : --team, --limit, --offset (all optional flags; NO positional).
//   * agent show   : positional `id` (required); no pagination.
//   * agent grants : positional `id` (required) + --limit, --offset.
// =============================================================================================
#[test]
fn agent_leaf_args_match_contract() {
    let schema = live_schema();
    let root = root_command(&schema);
    let agent = child(&root, "agent").expect("agent command present");

    // ---- agent list -------------------------------------------------------------------------
    let list = child(agent, "list").expect("agent list present");
    for flag in ["team", "limit", "offset"] {
        let a = arg_by(list, flag)
            .unwrap_or_else(|| panic!("`agent list` must expose --{flag}: {list}"));
        assert_eq!(
            a["long"].as_str(),
            Some(flag),
            "`agent list` --{flag} must be a long flag (not a positional): {a}"
        );
        assert_eq!(
            a["required"].as_bool(),
            Some(false),
            "`agent list` --{flag} must be optional: {a}"
        );
        assert_eq!(
            a["takesValue"].as_bool(),
            Some(true),
            "`agent list` --{flag} must take a value: {a}"
        );
    }
    // --limit / --offset are integer-typed pagination; --team is a string selector.
    assert_eq!(
        arg_by(list, "limit").unwrap()["type"].as_str(),
        Some("integer")
    );
    assert_eq!(
        arg_by(list, "offset").unwrap()["type"].as_str(),
        Some("integer")
    );
    assert_eq!(
        arg_by(list, "team").unwrap()["type"].as_str(),
        Some("string")
    );
    // `agent list` takes NO positional (a reader over the whole collection).
    assert!(
        list["args"]
            .as_array()
            .map(|args| args.iter().all(|a| a["long"].as_str().is_some()))
            .unwrap_or(true),
        "`agent list` must have no positional args (all its args are flags): {list}"
    );

    // ---- agent show <id> --------------------------------------------------------------------
    let show = child(agent, "show").expect("agent show present");
    let id = arg_by(show, "id")
        .unwrap_or_else(|| panic!("`agent show` must take a positional `id`: {show}"));
    assert_eq!(
        id["name"].as_str(),
        Some("id"),
        "positional must be named `id`: {id}"
    );
    assert!(
        id["long"].is_null() && id["short"].is_null(),
        "`agent show <id>` must be a POSITIONAL (no --long/-short): {id}"
    );
    assert_eq!(
        id["required"].as_bool(),
        Some(true),
        "`agent show <id>` positional must be required: {id}"
    );
    // No pagination on a single-object read.
    assert!(
        arg_by(show, "limit").is_none() && arg_by(show, "offset").is_none(),
        "`agent show` must not carry pagination flags: {show}"
    );

    // ---- agent grants <id> [--limit] [--offset] ---------------------------------------------
    let grants = child(agent, "grants").expect("agent grants present");
    let gid = arg_by(grants, "id")
        .unwrap_or_else(|| panic!("`agent grants` must take a positional `id`: {grants}"));
    assert_eq!(gid["name"].as_str(), Some("id"));
    assert!(
        gid["long"].is_null() && gid["short"].is_null(),
        "`agent grants <id>` must be a POSITIONAL: {gid}"
    );
    assert_eq!(
        gid["required"].as_bool(),
        Some(true),
        "`agent grants <id>` positional must be required: {gid}"
    );
    for flag in ["limit", "offset"] {
        let a = arg_by(grants, flag)
            .unwrap_or_else(|| panic!("`agent grants` must expose --{flag}: {grants}"));
        assert_eq!(
            a["long"].as_str(),
            Some(flag),
            "`agent grants` --{flag} must be a flag: {a}"
        );
        assert_eq!(
            a["type"].as_str(),
            Some("integer"),
            "`agent grants` --{flag} is integer: {a}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Live REST data-equivalence layer: an in-test mock of the agents read surface.
// ---------------------------------------------------------------------------------------------

type Recorder = Arc<Mutex<Vec<String>>>;

struct AgentMock {
    base_url: String,
    hits: Recorder,
    handle: JoinHandle<()>,
}

impl Drop for AgentMock {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Mock the three documented agents read endpoints, recording each request's full path+query so a
/// test can prove the leaf→endpoint mapping. Bodies mirror the REST shapes: Page envelope for
/// list/grants, a single object for show.
async fn start_agent_mock() -> AgentMock {
    let hits: Recorder = Arc::new(Mutex::new(Vec::new()));

    async fn list_agents(
        State(hits): State<Recorder>,
        OriginalUri(uri): OriginalUri,
    ) -> (StatusCode, Json<Value>) {
        hits.lock().unwrap().push(uri.to_string());
        (
            StatusCode::OK,
            Json(json!({
                "items": [
                    { "id": "agent-1", "name": "alpha-agent", "team": "payments" },
                    { "id": "agent-2", "name": "beta-agent", "team": "payments" }
                ],
                "limit": 50,
                "offset": 0,
                "total": 2
            })),
        )
    }

    async fn get_agent(
        State(hits): State<Recorder>,
        OriginalUri(uri): OriginalUri,
        Path(id): Path<String>,
    ) -> (StatusCode, Json<Value>) {
        hits.lock().unwrap().push(uri.to_string());
        (
            StatusCode::OK,
            Json(json!({ "id": id, "name": "alpha-agent", "team": "payments" })),
        )
    }

    async fn list_grants(
        State(hits): State<Recorder>,
        OriginalUri(uri): OriginalUri,
        Path(id): Path<String>,
    ) -> (StatusCode, Json<Value>) {
        hits.lock().unwrap().push(uri.to_string());
        (
            StatusCode::OK,
            Json(json!({
                "items": [
                    { "id": "grant-1", "agent_id": id, "resource": "clusters", "action": "read" }
                ],
                "limit": 50,
                "offset": 0,
                "total": 1
            })),
        )
    }

    let app = Router::new()
        .route("/api/v1/agents", get(list_agents))
        .route("/api/v1/agents/{id}", get(get_agent))
        .route("/api/v1/agents/{id}/grants", get(list_grants))
        .with_state(hits.clone());

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind agent mock");
    let addr = listener.local_addr().expect("agent mock addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    AgentMock {
        base_url: format!("http://{addr}"),
        hits,
        handle,
    }
}

fn parse_envelope(out: &Output, ctx: &str) -> Value {
    assert_eq!(
        exit_code(out),
        0,
        "{ctx}: expected exit 0, got {:?}; stderr: {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{ctx}: stdout is not a JSON envelope ({e}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

// =============================================================================================
// Test 3 — REST data-equivalence: the three leaves hit the documented agents endpoints and render
// the SAME data the REST reads return (AC7). Also proves --team/--limit/--offset reach the query
// string and the positional `id` reaches the path.
// =============================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_reads_hit_agents_endpoints_and_render_rest_data() {
    let mock = start_agent_mock().await;
    let home = common::unique_tempdir();

    let run = |args: &[&str]| {
        common::flowplane_cmd(&home)
            .env("FLOWPLANE_SERVER", &mock.base_url)
            .env("FLOWPLANE_TOKEN", "t")
            .args(args)
            .output()
            .expect("run agent command")
    };

    // ---- agent list --team payments → GET /api/v1/agents?team=payments, Page envelope. --------
    let list = run(&[
        "agent", "list", "--team", "payments", "--limit", "50", "-o", "json",
    ]);
    let list_env = parse_envelope(&list, "agent list");
    let items = list_env["data"]["items"].as_array().unwrap_or_else(|| {
        panic!(
            "`agent list` must render the Page envelope's items array: {}",
            list_env["data"]
        )
    });
    // Same data the REST read returned: both agents, with their fields intact.
    assert_eq!(
        items.len(),
        2,
        "agent list must render both mock items: {list_env}"
    );
    assert_eq!(
        items[0]["id"], "agent-1",
        "rendered item must carry the REST id: {list_env}"
    );
    assert_eq!(items[0]["name"], "alpha-agent");
    assert_eq!(
        list_env["data"]["total"], 2,
        "Page total must survive rendering: {list_env}"
    );

    // ---- agent show <id> → GET /api/v1/agents/<id>, single object. ---------------------------
    let show = run(&["agent", "show", "agent-xyz", "-o", "json"]);
    let show_env = parse_envelope(&show, "agent show");
    assert_eq!(
        show_env["data"]["id"], "agent-xyz",
        "`agent show <id>` must render the single object the REST read returned (id echoes the \
         positional through the path): {show_env}"
    );
    assert_eq!(show_env["data"]["name"], "alpha-agent");

    // ---- agent grants <id> --limit 5 → GET /api/v1/agents/<id>/grants?..., Page envelope. -----
    let grants = run(&["agent", "grants", "agent-xyz", "--limit", "5", "-o", "json"]);
    let grants_env = parse_envelope(&grants, "agent grants");
    let gitems = grants_env["data"]["items"].as_array().unwrap_or_else(|| {
        panic!(
            "`agent grants` must render the Page envelope's items array: {}",
            grants_env["data"]
        )
    });
    assert_eq!(
        gitems.len(),
        1,
        "agent grants must render the mock grant: {grants_env}"
    );
    assert_eq!(
        gitems[0]["agent_id"], "agent-xyz",
        "grant must be scoped to the path id: {grants_env}"
    );
    assert_eq!(gitems[0]["resource"], "clusters");

    // ---- endpoint mapping: exactly the three documented paths, in order, with args in query. --
    let hits = mock.hits.lock().unwrap().clone();
    assert_eq!(
        hits,
        vec![
            "/api/v1/agents?team=payments&limit=50".to_string(),
            "/api/v1/agents/agent-xyz".to_string(),
            "/api/v1/agents/agent-xyz/grants?limit=5".to_string(),
        ],
        "agent reads must map to GET /api/v1/agents, /api/v1/agents/{{id}}, \
         /api/v1/agents/{{id}}/grants with --team/--limit reaching the query string; got {hits:?}"
    );
}
