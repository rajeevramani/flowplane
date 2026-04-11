//! Validate CLI command
//!
//! Validates the gateway configuration and reports issues with severity,
//! category, resource, and message. Returns exit code 1 if any issues found.

use anyhow::Result;
use clap::Args;

use super::client::FlowplaneClient;
use super::output::{print_output, truncate};

#[derive(Args)]
#[command(
    about = "Validate gateway configuration",
    long_about = "Validate the gateway configuration and report issues.\n\nReturns exit code 0 if the configuration is clean, exit code 1 if any issues are found.\nUseful in CI pipelines to gate deployments.",
    after_help = "EXAMPLES:\n    flowplane validate\n    flowplane validate -o json\n    flowplane validate -o yaml"
)]
pub struct ValidateArgs {
    /// Output format
    #[arg(short, long, default_value = "table", value_parser = ["json", "yaml", "table"])]
    pub output: String,
}

pub async fn handle_validate_command(
    args: ValidateArgs,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/ops/validate");
    let response: serde_json::Value = client.get_json(&path).await?;

    if args.output == "table" {
        print_validate_table(&response);
    } else {
        print_output(&response, &args.output)?;
    }

    // Exit code 1 if any issues found
    let has_issues = response
        .get("issues")
        .and_then(|v| v.as_array())
        .map(|arr| !arr.is_empty())
        .unwrap_or(false);

    let is_valid = response.get("valid").and_then(|v| v.as_bool()).unwrap_or(true);

    if has_issues || !is_valid {
        std::process::exit(1);
    }

    Ok(())
}

fn print_validate_table(data: &serde_json::Value) {
    let valid = data.get("valid").and_then(|v| v.as_bool()).unwrap_or(true);

    println!();

    if let Some(issues) = data.get("issues").and_then(|v| v.as_array()) {
        if issues.is_empty() {
            println!("Configuration is valid - no issues found");
            println!();
            return;
        }

        println!("{:<10} {:<18} {:<25} Message", "Severity", "Category", "Resource");
        println!("{}", "-".repeat(90));

        for issue in issues {
            let severity = issue.get("severity").and_then(|v| v.as_str()).unwrap_or("?");
            let category = issue.get("category").and_then(|v| v.as_str()).unwrap_or("?");
            let resource = issue.get("resource").and_then(|v| v.as_str()).unwrap_or("?");
            let message = issue.get("message").and_then(|v| v.as_str()).unwrap_or("?");

            println!(
                "{:<10} {:<18} {:<25} {}",
                severity,
                truncate(category, 16),
                truncate(resource, 23),
                message,
            );
        }

        println!();
    } else if valid {
        println!("Configuration is valid - no issues found");
        println!();
        return;
    }

    // Print summary if present
    if let Some(summary) = data.get("summary").and_then(|v| v.as_object()) {
        let total = summary
            .get("total_issues")
            .or_else(|| summary.get("totalIssues"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let warnings = summary.get("warnings").and_then(|v| v.as_u64()).unwrap_or(0);
        let errors = summary.get("errors").and_then(|v| v.as_u64()).unwrap_or(0);

        println!("Summary: {} issue(s) - {} error(s), {} warning(s)", total, errors, warnings);
        println!();
    }
}
