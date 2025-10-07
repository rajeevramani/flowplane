//! Configuration management CLI commands
//!
//! Provides commands for managing ~/.flowplane/config.toml

use anyhow::{Context, Result};
use clap::Subcommand;

use super::config::CliConfig;
use super::output;

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Initialize configuration file with default values
    #[command(
        long_about = "Initialize a new configuration file at ~/.flowplane/config.toml with default settings.\n\nCreates the necessary directory structure and an empty configuration file that you can populate with your credentials and preferences.",
        after_help = "EXAMPLES:\n    # Initialize configuration file\n    flowplane-cli config init\n\n    # Force overwrite existing configuration\n    flowplane-cli config init --force\n\n    # After initialization, set your token\n    flowplane-cli config set token your-api-token"
    )]
    Init {
        /// Overwrite existing configuration file
        #[arg(short, long)]
        force: bool,
    },

    /// Show current configuration
    #[command(
        long_about = "Display the current configuration from ~/.flowplane/config.toml.\n\nShows all configured values including token (redacted), base URL, and timeout settings.",
        after_help = "EXAMPLES:\n    # Show configuration as YAML (default)\n    flowplane-cli config show\n\n    # Show configuration as JSON\n    flowplane-cli config show --output json\n\n    # Show configuration as table\n    flowplane-cli config show --output table"
    )]
    Show {
        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "yaml", value_parser = ["json", "yaml", "table"])]
        output: String,
    },

    /// Set a configuration value
    #[command(
        long_about = "Set a configuration value in ~/.flowplane/config.toml.\n\nSupported keys:\n  - token: Your Flowplane API authentication token\n  - base_url: The base URL for the Flowplane API (e.g., https://api.flowplane.io)\n  - timeout: Request timeout in seconds (default: 30)",
        after_help = "EXAMPLES:\n    # Set authentication token\n    flowplane-cli config set token fp_your_token_here\n\n    # Set API base URL\n    flowplane-cli config set base_url https://api.example.com\n\n    # Set request timeout\n    flowplane-cli config set timeout 60"
    )]
    Set {
        /// Configuration key (token, base_url, or timeout)
        #[arg(value_name = "KEY", value_parser = ["token", "base_url", "timeout"])]
        key: String,

        /// Configuration value
        #[arg(value_name = "VALUE")]
        value: String,
    },

    /// Get configuration file path
    #[command(
        long_about = "Display the path to the configuration file.\n\nShows the location where Flowplane CLI stores its configuration (typically ~/.flowplane/config.toml).",
        after_help = "EXAMPLES:\n    # Show configuration file path\n    flowplane-cli config path\n\n    # Use in scripts to locate config\n    cat $(flowplane-cli config path)"
    )]
    Path,
}

/// Handle config commands
pub async fn handle_config_command(command: ConfigCommands) -> Result<()> {
    match command {
        ConfigCommands::Init { force } => init_config(force).await?,
        ConfigCommands::Show { output } => show_config(&output).await?,
        ConfigCommands::Set { key, value } => set_config(&key, &value).await?,
        ConfigCommands::Path => show_config_path().await?,
    }

    Ok(())
}

async fn init_config(force: bool) -> Result<()> {
    let path = CliConfig::config_path()?;

    if path.exists() && !force {
        anyhow::bail!(
            "Configuration file already exists at: {}\nUse --force to overwrite",
            path.display()
        );
    }

    let config = CliConfig::default();
    config.save()?;

    println!("✅ Configuration file created at: {}", path.display());
    println!("\nYou can now set values using:");
    println!("  flowplane config set token <your-token>");
    println!("  flowplane config set base_url <api-url>");
    println!("  flowplane config set timeout <seconds>");

    Ok(())
}

async fn show_config(output_format: &str) -> Result<()> {
    let path = CliConfig::config_path()?;

    if !path.exists() {
        println!("No configuration file found at: {}", path.display());
        println!("\nRun 'flowplane config init' to create one");
        return Ok(());
    }

    let config = CliConfig::load()?;

    if output_format == "table" {
        print_config_table(&config);
    } else {
        output::print_output(&config, output_format)?;
    }

    Ok(())
}

async fn set_config(key: &str, value: &str) -> Result<()> {
    let mut config = CliConfig::load().unwrap_or_default();

    match key {
        "token" => {
            config.token = Some(value.to_string());
            println!("✅ Token set successfully");
        }
        "base_url" => {
            config.base_url = Some(value.to_string());
            println!("✅ Base URL set to: {}", value);
        }
        "timeout" => {
            let timeout: u64 =
                value.parse().context("Invalid timeout value. Must be a number in seconds")?;
            config.timeout = Some(timeout);
            println!("✅ Timeout set to: {} seconds", timeout);
        }
        _ => {
            anyhow::bail!(
                "Unknown configuration key: '{}'. Valid keys: token, base_url, timeout",
                key
            );
        }
    }

    config.save()?;

    let path = CliConfig::config_path()?;
    println!("Configuration saved to: {}", path.display());

    Ok(())
}

async fn show_config_path() -> Result<()> {
    let path = CliConfig::config_path()?;
    println!("{}", path.display());
    Ok(())
}

fn print_config_table(config: &CliConfig) {
    println!();
    println!("{:<15} {:<50}", "Key", "Value");
    output::print_separator(65);

    println!("{:<15} {}", "token", config.token.as_deref().unwrap_or("<not set>"));
    println!("{:<15} {}", "base_url", config.base_url.as_deref().unwrap_or("<not set>"));
    println!(
        "{:<15} {}",
        "timeout",
        config.timeout.map(|t| format!("{} seconds", t)).unwrap_or_else(|| "<not set>".to_string())
    );

    println!();
    println!(
        "Config file: {}",
        CliConfig::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".to_string())
    );
    println!();
}
