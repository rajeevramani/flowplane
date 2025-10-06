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
    Init {
        /// Overwrite existing configuration file
        #[arg(short, long)]
        force: bool,
    },

    /// Show current configuration
    Show {
        /// Output format (json, yaml, or table)
        #[arg(short, long, default_value = "yaml")]
        output: String,
    },

    /// Set a configuration value
    Set {
        /// Configuration key (token, base_url, or timeout)
        key: String,

        /// Configuration value
        value: String,
    },

    /// Get configuration file path
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
            let timeout: u64 = value
                .parse()
                .context("Invalid timeout value. Must be a number in seconds")?;
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

    println!(
        "{:<15} {}",
        "token",
        config.token.as_deref().unwrap_or("<not set>")
    );
    println!(
        "{:<15} {}",
        "base_url",
        config.base_url.as_deref().unwrap_or("<not set>")
    );
    println!(
        "{:<15} {}",
        "timeout",
        config
            .timeout
            .map(|t| format!("{} seconds", t))
            .unwrap_or_else(|| "<not set>".to_string())
    );

    println!();
    println!("Config file: {}", CliConfig::config_path().map(|p| p.display().to_string()).unwrap_or_else(|_| "<unknown>".to_string()));
    println!();
}
