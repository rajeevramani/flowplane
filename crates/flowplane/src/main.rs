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
        command: cli::ResourceCommand,
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

fn main() -> anyhow::Result<()> {
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
        Command::Route { command } => {
            runtime.block_on(cli::run_resource(cli.client, "route-configs", command))
        }
        Command::Secret { command } => runtime.block_on(cli::run_secret(cli.client, command)),
        Command::Dataplane { command } => runtime.block_on(cli::run_dataplane(cli.client, command)),
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
    }

    #[test]
    fn cli_help_contains_workflow_examples() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("flowplane auth login --device-code"));
        assert!(help.contains("flowplane auth login --pkce"));
        assert!(help.contains("flowplane config set-context prod"));
        assert!(help.contains("flowplane apply -f gateway.json --diff"));
    }
}
