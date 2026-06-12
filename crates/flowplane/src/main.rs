//! Flowplane binary: server subcommands now, CLI client subcommands from S7.

mod serve;

use clap::{Parser, Subcommand};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(name = "flowplane", version, about = "Flowplane control plane")]
struct Cli {
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
    }
}
