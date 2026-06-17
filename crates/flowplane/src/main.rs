//! Flowplane binary: server subcommands now, CLI client subcommands from S7.

mod cli;
mod serve;

use clap::{CommandFactory, Parser, Subcommand};
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
    /// AI gateway resources.
    Ai {
        #[command(subcommand)]
        command: cli::AiCommand,
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
    Expose {
        #[command(flatten)]
        command: cli::ExposeCommand,
    },
    /// Remove resources created by `expose`.
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
    Apply {
        #[command(flatten)]
        command: cli::ApplyCommand,
    },
    /// Shell completion script.
    Completion { shell: Shell },
    /// Print version.
    Version,
}

#[derive(Subcommand)]
enum DbCommand {
    /// Apply pending migrations (forward-only) and exit.
    Migrate,
}

fn main() {
    if let Err(err) = run() {
        if let Some(http) = err.downcast_ref::<cli::output::CliHttpError>() {
            std::process::exit(http.exit_code());
        }
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
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
        Command::Auth { command } => runtime.block_on(cli::run_auth(cli.client, command)),
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
        Command::Ai { command } => runtime.block_on(cli::run_ai(cli.client, command)),
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
        Command::Version => {
            println!("{VERSION}");
            Ok(())
        }
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
}
