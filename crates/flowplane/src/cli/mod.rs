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
    ApplyCommand, AuthCommand, CertCommand, ConfigCommand, DataplaneCommand, GrantCommand,
    OpsCommand, OrgCommand, OrgMemberCommand, ResourceCommand, SecretCommand, StatsCommand,
    TeamCommand, TeamMemberCommand, XdsCommand,
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

pub async fn run_apply(global: GlobalOptions, command: ApplyCommand) -> Result<()> {
    let manifests = read_apply_manifests(&command.file)?;
    let client = RestClient::new(global.clone())?;
    let mut diff_lines = Vec::new();
    for manifest in manifests {
        let target = apply_target(manifest)?;
        let team = client.team(target.team.clone())?;
        let path = format!(
            "/api/v1/teams/{team}/{}/{}",
            target.kind.segment(),
            target.name
        );
        let existing = client.get_optional(&path).await?;
        if command.diff || global.dry_run {
            diff_lines.push(diff_line(&target, existing.as_ref()));
            continue;
        }
        match existing {
            None => {
                client
                    .request(
                        reqwest::Method::POST,
                        &format!("/api/v1/teams/{team}/{}", target.kind.segment()),
                        Some(target.create_body),
                    )
                    .await?;
            }
            Some(existing) => apply_existing(&client, &path, target, existing).await?,
        }
    }
    if command.diff || global.dry_run {
        let text = diff_lines.join("\n");
        if let Some(out) = &global.out {
            fs::write(out, text).with_context(|| format!("write {}", out.display()))?;
        } else if !global.quiet {
            println!("{text}");
        }
    }
    Ok(())
}

async fn apply_existing(
    client: &RestClient,
    path: &str,
    target: ApplyTarget,
    existing: Value,
) -> Result<()> {
    match target.kind {
        ApplyKind::Cluster | ApplyKind::Listener | ApplyKind::RouteConfig => {
            if unchanged(&target, &existing) {
                println!("unchanged {} \"{}\"", target.kind.label(), target.name);
                return Ok(());
            }
            let revision = existing.get("revision").and_then(Value::as_i64);
            client
                .request_with_revision(
                    reqwest::Method::PATCH,
                    path,
                    Some(target.update_body()?),
                    revision,
                )
                .await?;
        }
        ApplyKind::Secret => {
            client
                .request(
                    reqwest::Method::POST,
                    &format!("{path}/rotate"),
                    Some(target.update_body()?),
                )
                .await?;
        }
        ApplyKind::Dataplane => {
            if unchanged(&target, &existing) {
                println!("unchanged {} \"{}\"", target.kind.label(), target.name);
                return Ok(());
            }
            anyhow::bail!(
                "dataplane \"{}\" exists and cannot be updated by the current API",
                target.name
            );
        }
    }
    Ok(())
}

fn read_apply_manifests(path: &PathBuf) -> Result<Vec<Value>> {
    let value = body_from_file(path)?;
    if let Some(items) = value.as_array() {
        return Ok(items.clone());
    }
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        return Ok(items.clone());
    }
    Ok(vec![value])
}

#[derive(Debug, Clone)]
struct ApplyTarget {
    kind: ApplyKind,
    name: String,
    team: Option<String>,
    create_body: Value,
}

impl ApplyTarget {
    fn update_body(&self) -> Result<Value> {
        match self.kind {
            ApplyKind::Cluster | ApplyKind::Listener | ApplyKind::RouteConfig => {
                let spec = self
                    .create_body
                    .get("spec")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("{} requires spec", self.kind.label()))?;
                Ok(json!({ "spec": spec }))
            }
            ApplyKind::Secret => {
                let spec = self
                    .create_body
                    .get("spec")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("secret requires spec"))?;
                let mut body = Map::new();
                body.insert("spec".into(), spec);
                if let Some(expires_at) = self.create_body.get("expires_at") {
                    body.insert("expires_at".into(), expires_at.clone());
                }
                Ok(Value::Object(body))
            }
            ApplyKind::Dataplane => Ok(json!({
                "name": self.name,
                "description": self.create_body.get("description").cloned().unwrap_or(Value::String(String::new())),
            })),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyKind {
    Cluster,
    Listener,
    RouteConfig,
    Secret,
    Dataplane,
}

impl ApplyKind {
    fn parse(raw: &str) -> Result<Self> {
        match raw.to_ascii_lowercase().replace('_', "-").as_str() {
            "cluster" | "clusters" => Ok(Self::Cluster),
            "listener" | "listeners" => Ok(Self::Listener),
            "route" | "route-config" | "route-configs" | "routeconfig" | "routeconfigs" => {
                Ok(Self::RouteConfig)
            }
            "secret" | "secrets" => Ok(Self::Secret),
            "dataplane" | "dataplanes" => Ok(Self::Dataplane),
            other => anyhow::bail!("unsupported apply kind \"{other}\""),
        }
    }

    fn segment(self) -> &'static str {
        match self {
            Self::Cluster => "clusters",
            Self::Listener => "listeners",
            Self::RouteConfig => "route-configs",
            Self::Secret => "secrets",
            Self::Dataplane => "dataplanes",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Cluster => "cluster",
            Self::Listener => "listener",
            Self::RouteConfig => "route-config",
            Self::Secret => "secret",
            Self::Dataplane => "dataplane",
        }
    }
}

fn apply_target(value: Value) -> Result<ApplyTarget> {
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("apply manifest must be a JSON object"))?;
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("apply manifest requires string field kind"))
        .and_then(ApplyKind::parse)?;
    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| obj.get("body")?.get("name")?.as_str())
        .ok_or_else(|| anyhow::anyhow!("apply manifest requires string field name"))?
        .to_string();
    let team = obj.get("team").and_then(Value::as_str).map(str::to_string);
    let create_body = if let Some(body) = obj.get("body") {
        with_name(body.clone(), &name)?
    } else {
        build_apply_body(kind, &name, obj)?
    };
    Ok(ApplyTarget {
        kind,
        name,
        team,
        create_body,
    })
}

fn with_name(mut body: Value, name: &str) -> Result<Value> {
    let obj = body
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("body must be a JSON object"))?;
    if let Some(body_name) = obj.get("name").and_then(Value::as_str) {
        if body_name != name {
            anyhow::bail!("manifest name and body.name differ");
        }
    } else {
        obj.insert("name".into(), Value::String(name.to_string()));
    }
    Ok(body)
}

fn build_apply_body(kind: ApplyKind, name: &str, obj: &Map<String, Value>) -> Result<Value> {
    match kind {
        ApplyKind::Cluster | ApplyKind::Listener | ApplyKind::RouteConfig => {
            let spec = obj
                .get("spec")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("{} requires spec", kind.label()))?;
            Ok(json!({ "name": name, "spec": spec }))
        }
        ApplyKind::Secret => {
            let spec = obj
                .get("spec")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("secret requires spec"))?;
            let mut body = Map::new();
            body.insert("name".into(), Value::String(name.to_string()));
            body.insert("spec".into(), spec);
            if let Some(description) = obj.get("description") {
                body.insert("description".into(), description.clone());
            }
            if let Some(expires_at) = obj.get("expires_at") {
                body.insert("expires_at".into(), expires_at.clone());
            }
            Ok(Value::Object(body))
        }
        ApplyKind::Dataplane => Ok(json!({
            "name": name,
            "description": obj.get("description").cloned().unwrap_or(Value::String(String::new())),
        })),
    }
}

fn diff_line(target: &ApplyTarget, existing: Option<&Value>) -> String {
    match existing {
        None => format!("+ {} \"{}\" create", target.kind.label(), target.name),
        Some(existing) if unchanged(target, existing) => {
            format!("= {} \"{}\" unchanged", target.kind.label(), target.name)
        }
        Some(_) if target.kind == ApplyKind::Secret => {
            format!(
                "~ {} \"{}\" rotate (value write-only)",
                target.kind.label(),
                target.name
            )
        }
        Some(_) if target.kind == ApplyKind::Dataplane => {
            format!(
                "! {} \"{}\" update unsupported",
                target.kind.label(),
                target.name
            )
        }
        Some(_) => format!("~ {} \"{}\" update spec", target.kind.label(), target.name),
    }
}

fn unchanged(target: &ApplyTarget, existing: &Value) -> bool {
    match target.kind {
        ApplyKind::Cluster | ApplyKind::Listener | ApplyKind::RouteConfig => {
            existing.get("spec") == target.create_body.get("spec")
        }
        ApplyKind::Secret => false,
        ApplyKind::Dataplane => {
            existing.get("description") == target.create_body.get("description")
        }
    }
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

    #[test]
    fn apply_manifest_builds_gateway_bodies() -> Result<()> {
        let target = apply_target(json!({
            "kind": "route-config",
            "team": "platform",
            "name": "edge",
            "spec": {"virtual_hosts": []}
        }))?;
        assert_eq!(target.kind, ApplyKind::RouteConfig);
        assert_eq!(target.team.as_deref(), Some("platform"));
        assert_eq!(target.create_body["name"], "edge");
        assert_eq!(
            target.update_body()?,
            json!({"spec": {"virtual_hosts": []}})
        );
        Ok(())
    }

    #[test]
    fn apply_diff_reports_update_or_noop() -> Result<()> {
        let target = apply_target(json!({
            "kind": "cluster",
            "name": "api",
            "spec": {"type": "strict_dns", "connect_timeout_ms": 250}
        }))?;
        let current = json!({
            "name": "api",
            "spec": {"type": "strict_dns", "connect_timeout_ms": 500},
            "revision": 7
        });
        assert_eq!(
            diff_line(&target, Some(&current)),
            "~ cluster \"api\" update spec"
        );
        let current = json!({
            "name": "api",
            "spec": {"type": "strict_dns", "connect_timeout_ms": 250},
            "revision": 7
        });
        assert_eq!(
            diff_line(&target, Some(&current)),
            "= cluster \"api\" unchanged"
        );
        Ok(())
    }

    #[test]
    fn apply_secret_existing_is_write_only_rotate() -> Result<()> {
        let target = apply_target(json!({
            "kind": "secret",
            "name": "api-key",
            "spec": {"type": "generic_secret", "secret": "new-value"}
        }))?;
        let current = json!({"name": "api-key", "value_redacted": true, "revision": 1});
        assert_eq!(
            diff_line(&target, Some(&current)),
            "~ secret \"api-key\" rotate (value write-only)"
        );
        assert_eq!(
            target.update_body()?,
            json!({"spec": {"type": "generic_secret", "secret": "new-value"}})
        );
        Ok(())
    }
}
