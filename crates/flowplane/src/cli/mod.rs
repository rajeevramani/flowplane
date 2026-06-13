mod client;
mod commands;
mod config;
mod output;

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

use client::RestClient;
pub use commands::{
    AuthCommand, CertCommand, ConfigCommand, DataplaneCommand, GrantCommand, OpsCommand,
    OrgCommand, OrgMemberCommand, ResourceCommand, SecretCommand, StatsCommand, TeamCommand,
    TeamMemberCommand, XdsCommand,
};
pub use config::GlobalOptions;
use config::{config_path, credentials_path, effective, read_config, write_config, NamedContext};
use output::format_row;

#[cfg(test)]
use std::collections::BTreeSet;

fn body_from_file(path: &PathBuf) -> Result<Value> {
    let raw = if path == &PathBuf::from("-") {
        let mut raw = String::new();
        io::stdin().read_to_string(&mut raw)?;
        raw
    } else {
        fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
    };
    serde_json::from_str(&raw).with_context(|| format!("parse JSON body from {}", path.display()))
}

pub async fn run_auth(global: GlobalOptions, command: AuthCommand) -> Result<()> {
    match command {
        AuthCommand::Whoami => RestClient::new(global)?
            .request(reqwest::Method::GET, "/api/v1/auth/whoami", None)
            .await
            .map(|_| ()),
        AuthCommand::Token => {
            let token = effective(&global)?.token.unwrap_or_default();
            println!("{token}");
            Ok(())
        }
        AuthCommand::Login { token, token_stdin } => {
            let token = match (token, token_stdin) {
                (Some(token), false) => token,
                (None, true) => {
                    let mut token = String::new();
                    io::stdin().read_to_string(&mut token)?;
                    token.trim().to_string()
                }
                (None, false) => anyhow::bail!("pass --token or --token-stdin"),
                (Some(_), true) => anyhow::bail!("use only one of --token or --token-stdin"),
            };
            let path = credentials_path();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, token)?;
            println!("token saved to {}", path.display());
            Ok(())
        }
        AuthCommand::Logout => {
            let path = credentials_path();
            if path.exists() {
                fs::remove_file(&path)?;
            }
            println!("logged out");
            Ok(())
        }
    }
}

pub fn run_config(_global: GlobalOptions, command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Path => println!("{}", config_path().display()),
        ConfigCommand::Show => println!("{}", toml::to_string_pretty(&read_config()?)?),
        ConfigCommand::SetContext {
            name,
            server,
            org,
            team,
            token,
        } => {
            let mut config = read_config()?;
            config.contexts.retain(|ctx| ctx.name != name);
            config.contexts.push(NamedContext {
                name: name.clone(),
                server,
                org,
                team,
                token,
            });
            config.current_context.get_or_insert(name);
            write_config(&config)?;
            println!("context saved");
        }
        ConfigCommand::UseContext { name } => {
            let mut config = read_config()?;
            if !config.contexts.iter().any(|ctx| ctx.name == name) {
                anyhow::bail!("context \"{name}\" does not exist");
            }
            config.current_context = Some(name);
            write_config(&config)?;
            println!("context selected");
        }
        ConfigCommand::GetContexts => {
            let config = read_config()?;
            let mut rows = config
                .contexts
                .iter()
                .map(|ctx| {
                    let mark = if config.current_context.as_deref() == Some(ctx.name.as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    vec![
                        mark.to_string(),
                        ctx.name.clone(),
                        ctx.server.clone(),
                        ctx.org.clone().unwrap_or_default(),
                        ctx.team.clone().unwrap_or_default(),
                    ]
                })
                .collect::<Vec<_>>();
            if rows.is_empty() && config.base_url.is_some() {
                let mark = if config.current_context.is_none() {
                    "*"
                } else {
                    " "
                };
                rows.push(vec![
                    mark.to_string(),
                    "legacy".to_string(),
                    config.base_url.clone().unwrap_or_default(),
                    config.org.clone().unwrap_or_default(),
                    config.team.clone().unwrap_or_default(),
                ]);
            }
            if rows.is_empty() {
                println!("no contexts");
            } else {
                let headers = ["", "NAME", "SERVER", "ORG", "TEAM"]
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                let widths = (0..headers.len())
                    .map(|i| {
                        std::iter::once(headers[i].len())
                            .chain(rows.iter().map(|row| row[i].len()))
                            .max()
                            .unwrap_or(0)
                    })
                    .collect::<Vec<_>>();
                println!("{}", format_row(&headers, &widths));
                for row in rows {
                    println!("{}", format_row(&row, &widths));
                }
            }
        }
    }
    Ok(())
}

pub async fn run_org(global: GlobalOptions, command: OrgCommand) -> Result<()> {
    let client = RestClient::new(global)?;
    match command {
        OrgCommand::List => {
            client
                .request(reqwest::Method::GET, "/api/v1/orgs", None)
                .await?
        }
        OrgCommand::Get { org } => {
            client
                .request(reqwest::Method::GET, &format!("/api/v1/orgs/{org}"), None)
                .await?
        }
        OrgCommand::Create { name, display_name } => {
            client
                .request(
                    reqwest::Method::POST,
                    "/api/v1/orgs",
                    Some(json!({"name": name, "display_name": display_name.unwrap_or_default()})),
                )
                .await?
        }
        OrgCommand::Delete { org } => {
            client
                .request(
                    reqwest::Method::DELETE,
                    &format!("/api/v1/orgs/{org}"),
                    None,
                )
                .await?
        }
        OrgCommand::Member { command } => return run_org_member(client, command).await,
    };
    Ok(())
}

async fn run_org_member(client: RestClient, command: OrgMemberCommand) -> Result<()> {
    match command {
        OrgMemberCommand::List { org } => {
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/orgs/{org}/members"),
                    None,
                )
                .await?
        }
        OrgMemberCommand::Add {
            org,
            email,
            subject,
            user_id,
            role,
        } => {
            let mut body = Map::new();
            body.insert("role".into(), Value::String(role));
            if let Some(email) = email {
                body.insert("email".into(), Value::String(email));
            }
            if let Some(subject) = subject {
                body.insert("subject".into(), Value::String(subject));
            }
            if let Some(user_id) = user_id {
                body.insert("user_id".into(), Value::String(user_id));
            }
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/orgs/{org}/members"),
                    Some(Value::Object(body)),
                )
                .await?
        }
        OrgMemberCommand::Remove { org, user_id } => {
            client
                .request(
                    reqwest::Method::DELETE,
                    &format!("/api/v1/orgs/{org}/members/{user_id}"),
                    None,
                )
                .await?
        }
    };
    Ok(())
}

pub async fn run_team(global: GlobalOptions, command: TeamCommand) -> Result<()> {
    let client = RestClient::new(global)?;
    match command {
        TeamCommand::List => {
            client
                .request(reqwest::Method::GET, "/api/v1/teams", None)
                .await?
        }
        TeamCommand::Create { name, display_name } => {
            client
                .request(
                    reqwest::Method::POST,
                    "/api/v1/teams",
                    Some(json!({"name": name, "display_name": display_name.unwrap_or_default()})),
                )
                .await?
        }
        TeamCommand::Delete { team } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::DELETE,
                    &format!("/api/v1/teams/{team}"),
                    None,
                )
                .await?
        }
        TeamCommand::Member { command } => return run_team_member(client, command).await,
        TeamCommand::Grant { command } => return run_grant(client, command).await,
    };
    Ok(())
}

async fn run_team_member(client: RestClient, command: TeamMemberCommand) -> Result<()> {
    match command {
        TeamMemberCommand::List { team } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/members"),
                    None,
                )
                .await?
        }
        TeamMemberCommand::Add { team, email } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/members"),
                    Some(json!({"email": email})),
                )
                .await?
        }
        TeamMemberCommand::Remove { team, user_id } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::DELETE,
                    &format!("/api/v1/teams/{team}/members/{user_id}"),
                    None,
                )
                .await?
        }
    };
    Ok(())
}

async fn run_grant(client: RestClient, command: GrantCommand) -> Result<()> {
    match command {
        GrantCommand::List { team } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/grants"),
                    None,
                )
                .await?
        }
        GrantCommand::Add {
            team,
            email,
            resource,
            action,
        } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/grants"),
                    Some(json!({"email": email, "resource": resource, "action": action})),
                )
                .await?
        }
        GrantCommand::Remove { team, grant_id } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::DELETE,
                    &format!("/api/v1/teams/{team}/grants/{grant_id}"),
                    None,
                )
                .await?
        }
    };
    Ok(())
}

pub async fn run_resource(
    global: GlobalOptions,
    segment: &str,
    command: ResourceCommand,
) -> Result<()> {
    let client = RestClient::new(global)?;
    match command {
        ResourceCommand::List { team } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/{segment}"),
                    None,
                )
                .await?
        }
        ResourceCommand::Get { team, name } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/{segment}/{name}"),
                    None,
                )
                .await?
        }
        ResourceCommand::Create { team, file } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/{segment}"),
                    Some(body_from_file(&file)?),
                )
                .await?
        }
        ResourceCommand::Update { team, name, file } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::PATCH,
                    &format!("/api/v1/teams/{team}/{segment}/{name}"),
                    Some(body_from_file(&file)?),
                )
                .await?
        }
        ResourceCommand::Delete { team, name } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::DELETE,
                    &format!("/api/v1/teams/{team}/{segment}/{name}"),
                    None,
                )
                .await?
        }
    };
    Ok(())
}

pub async fn run_secret(global: GlobalOptions, command: SecretCommand) -> Result<()> {
    let client = RestClient::new(global)?;
    match command {
        SecretCommand::List { team } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/secrets"),
                    None,
                )
                .await?
        }
        SecretCommand::Get { team, name } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/secrets/{name}"),
                    None,
                )
                .await?
        }
        SecretCommand::Create { team, file } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/secrets"),
                    Some(body_from_file(&file)?),
                )
                .await?
        }
        SecretCommand::Rotate { team, name, file } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/secrets/{name}/rotate"),
                    Some(body_from_file(&file)?),
                )
                .await?
        }
    };
    Ok(())
}

pub async fn run_dataplane(global: GlobalOptions, command: DataplaneCommand) -> Result<()> {
    let client = RestClient::new(global)?;
    match command {
        DataplaneCommand::List { team } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/dataplanes"),
                    None,
                )
                .await?
        }
        DataplaneCommand::Get { team, name } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/dataplanes/{name}"),
                    None,
                )
                .await?
        }
        DataplaneCommand::Create {
            team,
            name,
            description,
        } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/dataplanes"),
                    Some(json!({"name": name, "description": description})),
                )
                .await?
        }
        DataplaneCommand::Telemetry { team, name, file } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/dataplanes/{name}/telemetry"),
                    Some(body_from_file(&file)?),
                )
                .await?
        }
        DataplaneCommand::Bootstrap {
            team,
            name,
            xds_host,
            xds_port,
            admin_port,
            cert_path,
            key_path,
            ca_path,
        } => {
            let team = client.team(team)?;
            let query = format!("xds_host={xds_host}&xds_port={xds_port}&admin_port={admin_port}&cert_path={cert_path}&key_path={key_path}&ca_path={ca_path}");
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/dataplanes/{name}/envoy-config?{query}"),
                    None,
                )
                .await?
        }
        DataplaneCommand::Cert { command } => return run_cert(client, command).await,
    };
    Ok(())
}

async fn run_cert(client: RestClient, command: CertCommand) -> Result<()> {
    match command {
        CertCommand::List { team } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/proxy-certificates"),
                    None,
                )
                .await?
        }
        CertCommand::Register { team, file } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/proxy-certificates"),
                    Some(body_from_file(&file)?),
                )
                .await?
        }
        CertCommand::Issue {
            team,
            dataplane,
            ttl_hours,
        } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/proxy-certificates/issue"),
                    Some(json!({"dataplane": dataplane, "ttl_hours": ttl_hours})),
                )
                .await?
        }
        CertCommand::Revoke {
            team,
            serial,
            reason,
        } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/proxy-certificates/{serial}/revoke"),
                    Some(json!({"reason": reason})),
                )
                .await?
        }
    };
    Ok(())
}

pub async fn run_stats(global: GlobalOptions, command: StatsCommand) -> Result<()> {
    let client = RestClient::new(global)?;
    match command {
        StatsCommand::Overview { team } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/stats/overview"),
                    None,
                )
                .await?;
        }
    }
    Ok(())
}

pub async fn run_ops(global: GlobalOptions, command: OpsCommand) -> Result<()> {
    let client = RestClient::new(global)?;
    match command {
        OpsCommand::Xds {
            command: XdsCommand::Nacks { team },
        } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/xds/nacks"),
                    None,
                )
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
fn cli_endpoint_templates() -> BTreeSet<&'static str> {
    [
        "/api/v1/auth/whoami",
        "/api/v1/orgs",
        "/api/v1/orgs/{org}",
        "/api/v1/orgs/{org}/members",
        "/api/v1/orgs/{org}/members/{user_id}",
        "/api/v1/teams",
        "/api/v1/teams/{team}",
        "/api/v1/teams/{team}/members",
        "/api/v1/teams/{team}/members/{user_id}",
        "/api/v1/teams/{team}/grants",
        "/api/v1/teams/{team}/grants/{grant_id}",
        "/api/v1/teams/{team}/clusters",
        "/api/v1/teams/{team}/clusters/{name}",
        "/api/v1/teams/{team}/listeners",
        "/api/v1/teams/{team}/listeners/{name}",
        "/api/v1/teams/{team}/route-configs",
        "/api/v1/teams/{team}/route-configs/{name}",
        "/api/v1/teams/{team}/dataplanes",
        "/api/v1/teams/{team}/dataplanes/{name}",
        "/api/v1/teams/{team}/dataplanes/{name}/telemetry",
        "/api/v1/teams/{team}/dataplanes/{name}/envoy-config",
        "/api/v1/teams/{team}/proxy-certificates",
        "/api/v1/teams/{team}/proxy-certificates/issue",
        "/api/v1/teams/{team}/proxy-certificates/{serial_number}/revoke",
        "/api/v1/teams/{team}/secrets",
        "/api/v1/teams/{team}/secrets/{name}",
        "/api/v1/teams/{team}/secrets/{name}/rotate",
        "/api/v1/teams/{team}/stats/overview",
        "/api/v1/teams/{team}/xds/nacks",
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::config::CliConfig;
    use crate::cli::output::table;

    #[test]
    fn shipped_s2_to_s6_openapi_paths_have_cli_templates() {
        let doc = fp_api::routes::openapi_document();
        let cli = cli_endpoint_templates();
        let mut missing = Vec::new();
        for path in doc.paths.paths.keys() {
            if path.starts_with("/api/v1/bootstrap") {
                continue;
            }
            if !cli.contains(path.as_str()) {
                missing.push(path.clone());
            }
        }
        assert!(missing.is_empty(), "missing CLI coverage for: {missing:?}");
    }

    #[test]
    fn table_renders_page_items() {
        let rendered = table(&json!({"items":[{"name":"a","revision":1}]}));
        assert!(rendered.contains("NAME"));
        assert!(rendered.contains("a"));
    }

    #[test]
    fn legacy_scalar_config_parses_with_defaults() -> Result<()> {
        let parsed: CliConfig = toml::from_str(
            r#"
base_url = "http://localhost:8080"
team = "default"
org = "dev-org"
oidc_issuer = "http://localhost:8081"
oidc_client_id = "376872439851843590"
"#,
        )?;
        assert_eq!(parsed.base_url.as_deref(), Some("http://localhost:8080"));
        assert_eq!(parsed.org.as_deref(), Some("dev-org"));
        assert_eq!(parsed.team.as_deref(), Some("default"));
        assert!(parsed.contexts.is_empty());
        Ok(())
    }
}
