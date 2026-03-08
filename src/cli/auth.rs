// CLI auth commands — temporarily disabled during Zitadel migration.
// Token management (create/list/revoke/rotate) used the custom auth system.
// Task 2.6 will redesign bootstrap for Zitadel.

use clap::{Args, Subcommand};

use crate::config::DatabaseConfig;

#[derive(Subcommand, Debug)]
pub enum AuthCommands {
    /// Bootstrap initialization - generate setup token for first-time setup
    Bootstrap(BootstrapArgs),
}

#[derive(Args, Debug)]
pub struct BootstrapArgs {
    /// API base URL
    #[arg(long)]
    pub api_url: Option<String>,
}

pub async fn handle_auth_command(
    command: AuthCommands,
    _database: &DatabaseConfig,
) -> anyhow::Result<()> {
    match command {
        AuthCommands::Bootstrap(_args) => {
            anyhow::bail!(
                "Bootstrap is being redesigned for Zitadel. \
                 Use the Zitadel console to manage users and roles."
            );
        }
    }
}
