//! Flowplane binary: server subcommands now, CLI client subcommands from S7.

mod cli;
mod serve;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use clap_complete::Shell;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(
    name = "flowplane",
    version,
    about = "Flowplane control plane",
    after_help = "Examples:
  flowplane auth login --device-code --issuer https://issuer.example --client-id flowplane-cli
  flowplane auth login --pkce --callback-url http://127.0.0.1:8976/callback
  flowplane config set-context prod --server https://fp.example --org acme --team payments
  flowplane api create catalog --from-openapi openapi.json --team payments
  flowplane apply -f gateway.json --diff
  flowplane cluster list --team payments"
)]
struct Cli {
    #[command(flatten)]
    client: cli::GlobalOptions,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the control-plane server (REST + MCP now; xDS from S5).
    Serve,
    /// Database operations.
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    /// Print the OpenAPI document this binary serves (the exact API contract).
    Openapi,
    /// Client auth helpers.
    Auth {
        #[command(subcommand)]
        command: cli::AuthCommand,
    },
    /// Client configuration.
    Config {
        #[command(subcommand)]
        command: cli::ConfigCommand,
    },
    /// Organization management.
    Org {
        #[command(subcommand)]
        command: cli::OrgCommand,
    },
    /// Team management.
    Team {
        #[command(subcommand)]
        command: cli::TeamCommand,
    },
    /// Gateway clusters.
    Cluster {
        #[command(subcommand)]
        command: cli::ResourceCommand,
    },
    /// Gateway listeners.
    Listener {
        #[command(subcommand)]
        command: cli::ResourceCommand,
    },
    /// Route configs.
    Route {
        #[command(subcommand)]
        command: cli::RouteCommand,
    },
    /// API definitions, imported specs, and generated API tool rows.
    Api {
        #[command(subcommand)]
        command: cli::ApiCommand,
    },
    /// MCP server and generated API tool operations.
    Mcp {
        #[command(subcommand)]
        command: cli::McpCommand,
    },
    /// AI gateway resources.
    Ai {
        #[command(subcommand)]
        command: cli::AiCommand,
    },
    /// Global rate-limit domains, policies, and per-team overrides.
    RateLimit {
        #[command(subcommand)]
        command: cli::RateLimitCommand,
    },
    /// Learning capture sessions.
    Learn {
        #[command(subcommand)]
        command: cli::LearnCommand,
    },
    /// Write-only secrets.
    Secret {
        #[command(subcommand)]
        command: cli::SecretCommand,
    },
    /// Dataplane registration and certificates.
    Dataplane {
        #[command(subcommand)]
        command: cli::DataplaneCommand,
    },
    /// Expose an upstream through Envoy with cluster + route + listener resources.
    #[command(
        after_help = "Example:\n  flowplane expose 10.0.0.5:8080 --name payments-api --team payments"
    )]
    Expose {
        #[command(flatten)]
        command: cli::ExposeCommand,
    },
    /// Remove resources created by `expose`.
    #[command(after_help = "Example:\n  flowplane unexpose payments-api --team payments")]
    Unexpose {
        #[command(flatten)]
        command: cli::UnexposeCommand,
    },
    /// Team stats.
    Stats {
        #[command(subcommand)]
        command: cli::StatsCommand,
    },
    /// Operations diagnostics.
    Ops {
        #[command(subcommand)]
        command: cli::OpsCommand,
    },
    /// Apply a declarative JSON resource manifest.
    #[command(after_help = "Example:\n  flowplane apply -f gateway.json --diff")]
    Apply {
        #[command(flatten)]
        command: cli::ApplyCommand,
    },
    /// Shell completion script.
    Completion {
        /// Shell to generate the completion script for (bash, zsh, fish, …).
        shell: Shell,
    },
    /// Print version.
    Version,
    /// Print the machine-readable CLI schema (the canonical CLI contract; the MCP-derivation seam).
    Schema,
}

#[derive(Subcommand)]
enum DbCommand {
    /// Apply pending migrations (forward-only) and exit.
    Migrate,
}

fn main() {
    if let Err(err) = run() {
        // A CliError has already rendered its structured envelope to stderr (CLI-R-30);
        // exit with its resolved code (CLI-R-31) without re-printing.
        if let Some(cli_err) = err.downcast_ref::<cli::output::CliError>() {
            std::process::exit(cli_err.exit_code());
        }
        // Fallback: an unclassified internal/local error → generic exit 1 (CLI-R-31).
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    // Capture the raw ArgMatches so we can ask clap whether `--token` was passed
    // explicitly on the command line for `auth login`, as opposed to inherited from the
    // ambient FLOWPLANE_TOKEN via the global `--token` flag (Fix 3 / F-2). `Cli::parse()`
    // would discard the value-source we need.
    let matches = Cli::command().get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(err) => err.exit(),
    };
    // Only an explicit `--token <v>` (ValueSource::CommandLine) counts as a login input;
    // an env-sourced token must not conflict with `--token-stdin`/`--device`/`--pkce`.
    let auth_login_token_explicit = matches
        .subcommand_matches("auth")
        .and_then(|auth| auth.subcommand_matches("login"))
        .map(|login| {
            matches!(
                login.value_source("token"),
                Some(clap::parser::ValueSource::CommandLine)
            )
        })
        .unwrap_or(false);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    match cli.command {
        Command::Serve => runtime.block_on(serve::run()),
        Command::Db {
            command: DbCommand::Migrate,
        } => runtime.block_on(serve::migrate_only()),
        Command::Openapi => {
            let doc = fp_api::routes::openapi_document();
            println!(
                "{}",
                serde_json::to_string_pretty(&doc)
                    .map_err(|e| anyhow::anyhow!("serialize OpenAPI document: {e}"))?
            );
            Ok(())
        }
        Command::Auth { command } => runtime.block_on(cli::run_auth(
            cli.client,
            command,
            auth_login_token_explicit,
        )),
        Command::Config { command } => cli::run_config(cli.client, command),
        Command::Org { command } => runtime.block_on(cli::run_org(cli.client, command)),
        Command::Team { command } => runtime.block_on(cli::run_team(cli.client, command)),
        Command::Cluster { command } => {
            runtime.block_on(cli::run_resource(cli.client, "clusters", command))
        }
        Command::Listener { command } => {
            runtime.block_on(cli::run_resource(cli.client, "listeners", command))
        }
        Command::Route { command } => runtime.block_on(cli::run_route(cli.client, command)),
        Command::Api { command } => runtime.block_on(cli::run_api(cli.client, command)),
        Command::Mcp { command } => runtime.block_on(cli::run_mcp(cli.client, command)),
        Command::Ai { command } => runtime.block_on(cli::run_ai(cli.client, command)),
        Command::RateLimit { command } => {
            runtime.block_on(cli::run_rate_limit(cli.client, command))
        }
        Command::Learn { command } => runtime.block_on(cli::run_learn(cli.client, command)),
        Command::Secret { command } => runtime.block_on(cli::run_secret(cli.client, command)),
        Command::Dataplane { command } => runtime.block_on(cli::run_dataplane(cli.client, command)),
        Command::Expose { command } => runtime.block_on(cli::run_expose(cli.client, command)),
        Command::Unexpose { command } => runtime.block_on(cli::run_unexpose(cli.client, command)),
        Command::Stats { command } => runtime.block_on(cli::run_stats(cli.client, command)),
        Command::Ops { command } => runtime.block_on(cli::run_ops(cli.client, command)),
        Command::Apply { command } => runtime.block_on(cli::run_apply(cli.client, command)),
        Command::Completion { shell } => {
            let mut command = Cli::command();
            clap_complete::generate(shell, &mut command, "flowplane", &mut std::io::stdout());
            Ok(())
        }
        Command::Version => cli::output::render(
            &cli.client,
            "version",
            &serde_json::json!({ "version": VERSION }),
        ),
        // CLI-R-50: short-circuit before any network call — the schema is the CLI's own
        // structure, serialized from the clap tree.
        Command::Schema => cli::output::render(
            &cli.client,
            "cliSchema",
            &cli::schema::cli_schema(&Cli::command()),
        ),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn command_tree_builds() {
        Cli::command().debug_assert();
    }

    /// Mirror of `run`'s detection: was `--token` passed explicitly on the command line for
    /// `auth login` (vs. inherited from the ambient FLOWPLANE_TOKEN via the global flag)?
    /// Fix 3 / F-2. Env-sourced detection is process-global and is covered by the black-box
    /// integration matrix instead.
    fn login_token_is_explicit(argv: &[&str]) -> bool {
        let matches = Cli::command()
            .try_get_matches_from(argv)
            .expect("argv parses without env");
        matches
            .subcommand_matches("auth")
            .and_then(|auth| auth.subcommand_matches("login"))
            .map(|login| {
                matches!(
                    login.value_source("token"),
                    Some(clap::parser::ValueSource::CommandLine)
                )
            })
            .unwrap_or(false)
    }

    #[test]
    fn explicit_token_flag_is_detected_as_command_line() {
        assert!(login_token_is_explicit(&[
            "flowplane",
            "auth",
            "login",
            "--token",
            "v"
        ]));
    }

    #[test]
    fn token_stdin_alone_is_not_an_explicit_token() {
        assert!(!login_token_is_explicit(&[
            "flowplane",
            "auth",
            "login",
            "--token-stdin"
        ]));
    }

    #[test]
    fn device_flag_alone_is_not_an_explicit_token() {
        assert!(!login_token_is_explicit(&[
            "flowplane",
            "auth",
            "login",
            "--device",
            "--issuer",
            "https://issuer.example",
            "--client-id",
            "x",
        ]));
    }

    #[test]
    fn bare_login_is_not_an_explicit_token() {
        assert!(!login_token_is_explicit(&["flowplane", "auth", "login"]));
    }

    #[test]
    fn schema_has_no_drift_from_the_clap_tree() {
        // CLI-R-50: the schema is generated FROM Cli::command(); the drift guard asserts every
        // real top-level command appears in the schema (a future hardcoded list would break).
        let cmd = Cli::command();
        let expected: std::collections::BTreeSet<String> = cmd
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
        let schema = cli::schema::cli_schema(&cmd);
        let actual: std::collections::BTreeSet<String> =
            cli::schema::top_level_command_names(&schema)
                .into_iter()
                .collect();
        assert_eq!(
            actual, expected,
            "flowplane schema drifted from the clap command tree"
        );
        // The new `schema` command must itself be in the catalog (not accidentally exempt).
        assert!(
            actual.contains("schema"),
            "`schema` command missing from the catalog"
        );
        // Catalog carries its own version distinct from the envelope schemaVersion.
        assert_eq!(schema["catalogVersion"], cli::schema::CATALOG_VERSION);
        // Every arg, recursively, carries the documented type/enums/defaults fields (CLI-R-50).
        fn assert_arg_fields(node: &serde_json::Value) {
            for arg in node["args"].as_array().expect("args array") {
                for key in [
                    "name",
                    "type",
                    "required",
                    "global",
                    "takesValue",
                    "possibleValues",
                    "defaults",
                ] {
                    assert!(arg.get(key).is_some(), "arg missing `{key}`: {arg}");
                }
            }
            for sub in node["subcommands"].as_array().expect("subcommands array") {
                assert_arg_fields(sub);
            }
        }
        assert_arg_fields(&schema["command"]);
    }

    #[test]
    fn revision_is_a_global_option_on_every_update_and_delete() {
        // CLI-R-47: `--revision` must be uniform across every update/delete. It is a global
        // arg, so it is accepted on every subcommand by construction — assert that invariant.
        let cmd = Cli::command();
        let revision = cmd
            .get_arguments()
            .find(|a| a.get_id() == "revision")
            .expect("--revision global arg must exist");
        assert!(
            revision.is_global_set(),
            "--revision must be global so it is present on every update/delete"
        );
        // And it actually parses on representative update + delete forms.
        Cli::try_parse_from([
            "flowplane",
            "--revision",
            "3",
            "cluster",
            "delete",
            "x",
            "--team",
            "t",
        ])
        .expect("--revision parses on delete");
    }

    #[test]
    fn json_flag_conflicts_with_explicit_output() {
        // CLI-R-11: `-o/--output` is the single format selector; `--json` is an alias for
        // `-o json` and must not be combined with an explicit `-o`.
        let result = Cli::try_parse_from(["flowplane", "-o", "table", "--json", "version"]);
        assert!(
            result.is_err(),
            "--json with explicit -o must be a usage error"
        );
        if let Err(err) = result {
            assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
        }
        // Either alone parses fine.
        Cli::try_parse_from(["flowplane", "--json", "version"]).expect("--json alone parses");
        Cli::try_parse_from(["flowplane", "-o", "json", "version"]).expect("-o json alone parses");
    }

    #[test]
    fn cli_transcript_command_forms_parse() {
        Cli::try_parse_from([
            "flowplane",
            "auth",
            "login",
            "--device-code",
            "--issuer",
            "https://issuer.example",
            "--client-id",
            "flowplane-cli",
        ])
        .expect("device-code login form should parse");
        Cli::try_parse_from([
            "flowplane",
            "auth",
            "login",
            "--pkce",
            "--callback-url",
            "http://127.0.0.1:8976/callback",
        ])
        .expect("pkce login form should parse");
        Cli::try_parse_from(["flowplane", "apply", "-f", "gateway.json", "--diff"])
            .expect("apply diff form should parse");
        Cli::try_parse_from(["flowplane", "cluster", "list", "--team", "payments"])
            .expect("resource list form should parse");
        Cli::try_parse_from([
            "flowplane",
            "api",
            "create",
            "catalog",
            "--from-openapi",
            "openapi.json",
            "--team",
            "payments",
        ])
        .expect("api import form should parse");
        Cli::try_parse_from([
            "flowplane",
            "learn",
            "discover",
            "start",
            "public-discovery",
            "--upstream",
            "93.184.216.34:80",
            "--listener-port",
            "19080",
        ])
        .expect("learn discover start form should parse");
        Cli::try_parse_from([
            "flowplane",
            "route",
            "generate",
            "--from-spec",
            "018ff2ef-bfc6-7000-8000-000000000001",
            "--listener-port",
            "19090",
        ])
        .expect("route generate form should parse");
        Cli::try_parse_from([
            "flowplane",
            "route",
            "apply",
            "018ff2ef-bfc6-7000-8000-000000000002",
        ])
        .expect("route apply form should parse");
        Cli::try_parse_from([
            "flowplane",
            "learn",
            "start",
            "catalog-capture",
            "--api",
            "catalog",
            "--target-sample-count",
            "25",
        ])
        .expect("learn start form should parse");
        Cli::try_parse_from([
            "flowplane",
            "--out",
            "/tmp/flowplane-envoy.yaml",
            "dataplane",
            "bootstrap",
            "dp-local",
            "--mode",
            "dev",
            "--xds-host",
            "127.0.0.1",
        ])
        .expect("dev dataplane bootstrap form should parse");
        Cli::try_parse_from([
            "flowplane",
            "dataplane",
            "envoy-config",
            "dp-local",
            "--mode",
            "mtls",
            "--cert-path",
            "/certs/client.crt",
            "--key-path",
            "/certs/client.key",
            "--ca-path",
            "/certs/ca.crt",
        ])
        .expect("legacy dataplane envoy-config alias should parse");
        Cli::try_parse_from(["flowplane", "mcp", "status", "--team", "payments"])
            .expect("mcp status form should parse");
        Cli::try_parse_from([
            "flowplane",
            "mcp",
            "enable",
            "--api",
            "api_get-catalog",
            "--team",
            "payments",
        ])
        .expect("mcp enable form should parse");
        Cli::try_parse_from([
            "flowplane",
            "mcp",
            "disable",
            "--api",
            "get-catalog",
            "--team",
            "payments",
        ])
        .expect("mcp disable form should parse");
        Cli::try_parse_from(["flowplane", "mcp", "connections", "--team", "payments"])
            .expect("mcp connections form should parse");
        Cli::try_parse_from([
            "flowplane",
            "expose",
            "http://127.0.0.1:3001",
            "--name",
            "demo",
            "--path",
            "/",
            "--port",
            "10001",
            "--public-base-url",
            "https://gateway.example",
        ])
        .expect("expose shortcut form should parse");
        Cli::try_parse_from(["flowplane", "unexpose", "demo"])
            .expect("unexpose shortcut form should parse");
    }

    #[test]
    fn cli_help_contains_workflow_examples() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("flowplane auth login --device-code"));
        assert!(help.contains("flowplane auth login --pkce"));
        assert!(help.contains("flowplane config set-context prod"));
        assert!(help.contains("flowplane api create catalog"));
        assert!(help.contains("flowplane apply -f gateway.json --diff"));
    }

    #[test]
    fn chk_help_examples_parse() {
        // `chk:help-examples-parse` lint for CLI-R-06: every top-level + spine workflow command's
        // --help carries >=1 copy-pasteable `flowplane ...` example, and every extracted example
        // line PARSES via Cli::try_parse_from. Coverage is frozen and union-guarded: every leaf is
        // classified SPINE (must carry an example) or EXEMPT (none required); the two sets are
        // disjoint and their union must equal the live leaf set EXACTLY, so a new/removed command
        // fails CI until it is classified. EXEMPT leaves are intentionally NOT required to carry an
        // example — they are readers (get/list/status), single-target mutators (delete), and
        // utilities (completion/version/serve/db migrate/openapi/schema/...) whose usage is obvious
        // from `--help`. The union guard forces every FUTURE leaf to be classified one way or the
        // other. Pure in-process (no temp dir / network) so it is inherently parallel-safe.

        // 45 SPINE leaves (space-joined paths) — each must expose a parseable example.
        const SPINE: &[&str] = &[
            "auth login",
            "config set-context",
            "org create",
            "org member add",
            "team create",
            "team member add",
            "team grant add",
            "cluster create",
            "cluster update",
            "listener create",
            "listener update",
            "route create",
            "route update",
            "route generate",
            "api create",
            "api spec reject",
            "api spec publish",
            "mcp enable",
            "mcp disable",
            "ai providers create",
            "ai providers update",
            "ai routes create",
            "ai routes update",
            "ai budgets create",
            "ai budgets update",
            "ai trace",
            "rate-limit domain create",
            "rate-limit domain update",
            "rate-limit policy create",
            "rate-limit policy update",
            "rate-limit override set",
            "rate-limit override update",
            "learn start",
            "learn discover start",
            "secret create",
            "secret rotate",
            "dataplane create",
            "dataplane telemetry",
            "dataplane bootstrap",
            "dataplane cert register",
            "dataplane cert issue",
            "dataplane cert revoke",
            "expose",
            "unexpose",
            "apply",
        ];

        // 77 EXEMPT leaves (space-joined paths) — no example required.
        const EXEMPT: &[&str] = &[
            "ai budgets delete",
            "ai budgets get",
            "ai budgets list",
            "ai providers delete",
            "ai providers get",
            "ai providers list",
            "ai routes delete",
            "ai routes get",
            "ai routes list",
            "ai usage",
            "api delete",
            "api get",
            "api list",
            "api status",
            "auth logout",
            "auth token",
            "auth whoami",
            "cluster delete",
            "cluster get",
            "cluster list",
            "completion",
            "config get-contexts",
            "config path",
            "config show",
            "config use-context",
            "dataplane cert list",
            "dataplane get",
            "dataplane list",
            "db migrate",
            "learn cancel",
            "learn discover generate-spec",
            "learn discover list",
            "learn discover status",
            "learn discover stop",
            "learn generate-spec",
            "learn get",
            "learn list",
            "learn stop",
            "listener delete",
            "listener get",
            "listener list",
            "mcp connections",
            "mcp status",
            "openapi",
            "ops trace",
            "ops xds nacks",
            "ops xds status",
            "org delete",
            "org get",
            "org list",
            "org member list",
            "org member remove",
            "rate-limit domain delete",
            "rate-limit domain get",
            "rate-limit domain list",
            "rate-limit force-repush",
            "rate-limit override delete",
            "rate-limit override get",
            "rate-limit policy delete",
            "rate-limit policy get",
            "rate-limit policy list",
            "route apply",
            "route delete",
            "route get",
            "route list",
            "schema",
            "secret get",
            "secret list",
            "serve",
            "stats overview",
            "team delete",
            "team grant list",
            "team grant remove",
            "team list",
            "team member list",
            "team member remove",
            "version",
        ];

        use std::collections::BTreeSet;

        // Build the LIVE leaf set by recursively walking Cli::command() (EXCLUDING the root).
        fn collect_leaves(cmd: &clap::Command, prefix: &str, out: &mut BTreeSet<String>) {
            for sub in cmd.get_subcommands() {
                let path = if prefix.is_empty() {
                    sub.get_name().to_string()
                } else {
                    format!("{prefix} {}", sub.get_name())
                };
                if sub.get_subcommands().next().is_none() {
                    out.insert(path);
                } else {
                    collect_leaves(sub, &path, out);
                }
            }
        }

        let root = Cli::command();
        let mut live: BTreeSet<String> = BTreeSet::new();
        collect_leaves(&root, "", &mut live);

        let mut offenders: Vec<String> = Vec::new();

        // --- Step 2: union guard — disjoint, and (SPINE ∪ EXEMPT) == live EXACTLY. -----------
        let mut classified: BTreeSet<String> = BTreeSet::new();
        for (label, list) in [("SPINE", SPINE), ("EXEMPT", EXEMPT)] {
            for &leaf in list {
                if !classified.insert(leaf.to_string()) {
                    offenders.push(format!(
                        "leaf `{leaf}` classified more than once (found again in {label}); \
                         SPINE and EXEMPT must be disjoint"
                    ));
                }
            }
        }
        let missing_from_classification: Vec<&String> = live.difference(&classified).collect();
        let stale_in_lists: Vec<&String> = classified.difference(&live).collect();
        for leaf in &missing_from_classification {
            offenders.push(format!(
                "leaf `{leaf}` is live but NOT classified — add it to SPINE or EXEMPT"
            ));
        }
        for leaf in &stale_in_lists {
            offenders.push(format!(
                "leaf `{leaf}` is classified but NO LONGER live — remove it from SPINE/EXEMPT"
            ));
        }

        // --- Step 3: every SPINE leaf exposes >=1 parseable `flowplane ...` example. ---------
        for &leaf in SPINE {
            // Navigate from the root via find_subcommand for each path segment.
            let mut node = &root;
            let mut resolved = true;
            for seg in leaf.split_whitespace() {
                match node.find_subcommand(seg) {
                    Some(child) => node = child,
                    None => {
                        offenders.push(format!(
                            "{leaf}: path does not resolve in the clap tree (segment `{seg}`)"
                        ));
                        resolved = false;
                        break;
                    }
                }
            }
            if !resolved {
                continue;
            }

            let after = node
                .get_after_help()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let examples: Vec<&str> = after
                .lines()
                .filter(|line| line.trim_start().starts_with("flowplane"))
                .collect();

            if examples.is_empty() {
                offenders.push(format!("{leaf}: no flowplane example in after_help"));
                continue;
            }

            for line in examples {
                let tokens: Vec<&str> = line.split_whitespace().collect();
                if let Err(err) = Cli::try_parse_from(tokens) {
                    offenders.push(format!(
                        "{leaf}: example did not parse: {} ({:?})",
                        line.trim(),
                        err.kind()
                    ));
                }
            }
        }

        // --- Step 4: collect ALL offenders, then assert empty with a naming message. ---------
        assert!(
            offenders.is_empty(),
            "chk:help-examples-parse (CLI-R-06) found {} violation(s):\n  - {}\n\
             \n  live leaf count = {}, classified leaf count = {}",
            offenders.len(),
            offenders.join("\n  - "),
            live.len(),
            classified.len(),
        );
    }
}
