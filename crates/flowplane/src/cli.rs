use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";

#[derive(Debug, Clone, Args)]
pub struct GlobalOptions {
    #[arg(long, global = true)]
    pub context: Option<String>,
    #[arg(long, global = true, env = "FLOWPLANE_SERVER")]
    pub server: Option<String>,
    #[arg(long, global = true)]
    pub team: Option<String>,
    #[arg(long, global = true)]
    pub org: Option<String>,
    #[arg(short = 'o', long, global = true, value_enum)]
    pub output: Option<OutputFormat>,
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long, global = true)]
    pub no_color: bool,
    #[arg(long, global = true)]
    pub quiet: bool,
    #[arg(long, global = true)]
    pub verbose: bool,
    #[arg(long, global = true)]
    pub dry_run: bool,
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,
    #[arg(long, global = true)]
    pub revision: Option<i64>,
    #[arg(long, global = true, default_value_t = 30)]
    pub timeout: u64,
    #[arg(long, global = true)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum OutputFormat {
    Table,
    Json,
    Yaml,
    Wide,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CliConfig {
    current_context: Option<String>,
    contexts: Vec<NamedContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NamedContext {
    name: String,
    server: String,
    org: Option<String>,
    team: Option<String>,
    token: Option<String>,
}

#[derive(Debug, Clone)]
struct EffectiveConfig {
    server: String,
    org: Option<String>,
    team: Option<String>,
    token: Option<String>,
}

impl GlobalOptions {
    fn format(&self) -> OutputFormat {
        if self.json {
            OutputFormat::Json
        } else {
            self.output.unwrap_or(OutputFormat::Table)
        }
    }
}

fn config_path() -> PathBuf {
    if let Ok(path) = std::env::var("FLOWPLANE_CONFIG") {
        return PathBuf::from(path);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".flowplane").join("config.toml")
}

fn credentials_path() -> PathBuf {
    config_path()
        .parent()
        .map(|p| p.join("credentials"))
        .unwrap_or_else(|| PathBuf::from(".flowplane/credentials"))
}

fn read_config() -> Result<CliConfig> {
    let path = config_path();
    if !path.exists() {
        return Ok(CliConfig::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

fn write_config(config: &CliConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&path, toml::to_string_pretty(config)?)
        .with_context(|| format!("write {}", path.display()))
}

fn effective(global: &GlobalOptions) -> Result<EffectiveConfig> {
    let file = read_config()?;
    let selected_name = global.context.as_ref().or(file.current_context.as_ref());
    let selected =
        selected_name.and_then(|name| file.contexts.iter().find(|ctx| &ctx.name == name));
    let token = std::env::var("FLOWPLANE_TOKEN")
        .ok()
        .or_else(|| selected.and_then(|ctx| ctx.token.clone()))
        .or_else(|| {
            fs::read_to_string(credentials_path())
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty());
    Ok(EffectiveConfig {
        server: global
            .server
            .clone()
            .or_else(|| selected.map(|ctx| ctx.server.clone()))
            .unwrap_or_else(|| DEFAULT_SERVER.to_string()),
        org: global
            .org
            .clone()
            .or_else(|| std::env::var("FLOWPLANE_ORG").ok())
            .or_else(|| selected.and_then(|ctx| ctx.org.clone())),
        team: global
            .team
            .clone()
            .or_else(|| std::env::var("FLOWPLANE_TEAM").ok())
            .or_else(|| selected.and_then(|ctx| ctx.team.clone())),
        token,
    })
}

#[derive(Debug, clap::Subcommand)]
pub enum AuthCommand {
    Whoami,
    Token,
    Login { token: String },
    Logout,
}

#[derive(Debug, clap::Subcommand)]
pub enum ConfigCommand {
    Path,
    Show,
    SetContext {
        name: String,
        #[arg(long)]
        server: String,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
    UseContext {
        name: String,
    },
    GetContexts,
}

#[derive(Debug, clap::Subcommand)]
pub enum OrgCommand {
    List,
    Get {
        org: String,
    },
    Create {
        name: String,
        #[arg(long)]
        display_name: Option<String>,
    },
    Delete {
        org: String,
    },
    Member {
        #[command(subcommand)]
        command: OrgMemberCommand,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum OrgMemberCommand {
    List {
        org: String,
    },
    Add {
        org: String,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        user_id: Option<String>,
        #[arg(long)]
        role: String,
    },
    Remove {
        org: String,
        user_id: String,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum TeamCommand {
    List,
    Create {
        name: String,
        #[arg(long)]
        display_name: Option<String>,
    },
    Delete {
        #[arg(long)]
        team: Option<String>,
    },
    Member {
        #[command(subcommand)]
        command: TeamMemberCommand,
    },
    Grant {
        #[command(subcommand)]
        command: GrantCommand,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum TeamMemberCommand {
    List {
        #[arg(long)]
        team: Option<String>,
    },
    Add {
        #[arg(long)]
        team: Option<String>,
        email: String,
    },
    Remove {
        #[arg(long)]
        team: Option<String>,
        user_id: String,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum GrantCommand {
    List {
        #[arg(long)]
        team: Option<String>,
    },
    Add {
        #[arg(long)]
        team: Option<String>,
        email: String,
        #[arg(long)]
        resource: String,
        #[arg(long)]
        action: String,
    },
    Remove {
        #[arg(long)]
        team: Option<String>,
        grant_id: String,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum ResourceCommand {
    List {
        #[arg(long)]
        team: Option<String>,
    },
    Get {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
    Create {
        #[arg(long)]
        team: Option<String>,
        #[arg(short, long)]
        file: PathBuf,
    },
    Update {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(short, long)]
        file: PathBuf,
    },
    Delete {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum SecretCommand {
    List {
        #[arg(long)]
        team: Option<String>,
    },
    Get {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
    Create {
        #[arg(long)]
        team: Option<String>,
        #[arg(short, long)]
        file: PathBuf,
    },
    Rotate {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(short, long)]
        file: PathBuf,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum DataplaneCommand {
    List {
        #[arg(long)]
        team: Option<String>,
    },
    Get {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
    Create {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    Telemetry {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(short, long)]
        file: PathBuf,
    },
    Bootstrap {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(long, default_value = "127.0.0.1")]
        xds_host: String,
        #[arg(long, default_value_t = 18000)]
        xds_port: u16,
        #[arg(long, default_value_t = 9901)]
        admin_port: u16,
        #[arg(long)]
        cert_path: String,
        #[arg(long)]
        key_path: String,
        #[arg(long)]
        ca_path: String,
    },
    Cert {
        #[command(subcommand)]
        command: CertCommand,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum CertCommand {
    List {
        #[arg(long)]
        team: Option<String>,
    },
    Register {
        #[arg(long)]
        team: Option<String>,
        #[arg(short, long)]
        file: PathBuf,
    },
    Issue {
        #[arg(long)]
        team: Option<String>,
        dataplane: String,
        #[arg(long, default_value_t = 24)]
        ttl_hours: i64,
    },
    Revoke {
        #[arg(long)]
        team: Option<String>,
        serial: String,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum StatsCommand {
    Overview {
        #[arg(long)]
        team: Option<String>,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum OpsCommand {
    Xds {
        #[command(subcommand)]
        command: XdsCommand,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum XdsCommand {
    Nacks {
        #[arg(long)]
        team: Option<String>,
    },
}

struct RestClient {
    http: reqwest::Client,
    config: EffectiveConfig,
    global: GlobalOptions,
}

impl RestClient {
    fn new(global: GlobalOptions) -> Result<Self> {
        let config = effective(&global)?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(global.timeout))
            .build()?;
        Ok(Self {
            http,
            config,
            global,
        })
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Option<Value>> {
        if self.global.dry_run && method != reqwest::Method::GET {
            let plan = json!({ "method": method.as_str(), "path": path, "body": body });
            render(&self.global, &plan)?;
            return Ok(Some(plan));
        }
        let url = format!(
            "{}/{}",
            self.config.server.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let mut req = self.http.request(method, url);
        if let Some(token) = &self.config.token {
            req = req.bearer_auth(token);
        }
        if let Some(org) = &self.config.org {
            req = req.header("X-Flowplane-Org", org);
        }
        if let Some(revision) = self.global.revision {
            req = req.header(reqwest::header::IF_MATCH, revision.to_string());
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req.send().await.context("send request")?;
        let status = response.status();
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let text = response.text().await.context("read response body")?;
        if !status.is_success() {
            return Err(render_error(status, request_id, &text));
        }
        if status == reqwest::StatusCode::NO_CONTENT || text.trim().is_empty() {
            if !self.global.quiet {
                println!("ok");
            }
            return Ok(None);
        }
        let value: Value = serde_json::from_str(&text).context("parse response JSON")?;
        render(&self.global, &value)?;
        Ok(Some(value))
    }

    fn team(&self, explicit: Option<String>) -> Result<String> {
        explicit
            .or_else(|| self.config.team.clone())
            .ok_or_else(|| anyhow::anyhow!("team is required; pass --team or configure a context"))
    }
}

fn render_error(
    status: reqwest::StatusCode,
    request_id: Option<String>,
    text: &str,
) -> anyhow::Error {
    let parsed: Option<Value> = serde_json::from_str(text).ok();
    let code = parsed
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| status.as_str());
    let message = parsed
        .as_ref()
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(text);
    let hint = parsed
        .as_ref()
        .and_then(|v| v.get("hint"))
        .and_then(Value::as_str);
    eprintln!("error ({code}): {message}");
    if let Some(hint) = hint {
        eprintln!("  -> {hint}");
    }
    if let Some(rid) = request_id.or_else(|| {
        parsed
            .as_ref()
            .and_then(|v| v.get("request_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }) {
        eprintln!("  request id: {rid}");
    }
    anyhow::anyhow!("request failed with status {status}")
}

fn render(global: &GlobalOptions, value: &Value) -> Result<()> {
    let text = match global.format() {
        OutputFormat::Json => serde_json::to_string_pretty(value)?,
        OutputFormat::Yaml => yaml_like(value, 0),
        OutputFormat::Table | OutputFormat::Wide => table(value),
    };
    if let Some(out) = &global.out {
        fs::write(out, text).with_context(|| format!("write {}", out.display()))?;
    } else {
        println!("{text}");
    }
    Ok(())
}

fn table(value: &Value) -> String {
    let rows = if let Some(items) = value.get("items").and_then(Value::as_array) {
        items.clone()
    } else if let Some(items) = value.as_array() {
        items.clone()
    } else {
        vec![value.clone()]
    };
    if rows.is_empty() {
        return "no rows".into();
    }
    let mut columns = BTreeSet::new();
    for row in &rows {
        if let Some(obj) = row.as_object() {
            for key in obj.keys() {
                if !matches!(
                    key.as_str(),
                    "spec" | "certificate_pem" | "private_key_pem" | "ca_certificate_pem"
                ) {
                    columns.insert(key.clone());
                }
            }
        }
    }
    let columns: Vec<_> = columns.into_iter().collect();
    if columns.is_empty() {
        return serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    }
    let mut out = String::new();
    out.push_str(&columns.join("\t"));
    for row in rows {
        out.push('\n');
        let cells = columns
            .iter()
            .map(|c| cell(row.get(c).unwrap_or(&Value::Null)))
            .collect::<Vec<_>>();
        out.push_str(&cells.join("\t"));
    }
    out
}

fn cell(value: &Value) -> String {
    match value {
        Value::Null => "-".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        Value::Array(v) => format!("{} items", v.len()),
        Value::Object(_) => "{...}".into(),
    }
}

fn yaml_like(value: &Value, indent: usize) -> String {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| {
                let pad = " ".repeat(indent);
                match v {
                    Value::Object(_) | Value::Array(_) => {
                        format!("{pad}{k}:\n{}", yaml_like(v, indent + 2))
                    }
                    _ => format!("{pad}{k}: {}", cell(v)),
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Array(items) => items
            .iter()
            .map(|v| {
                format!(
                    "{}- {}",
                    " ".repeat(indent),
                    yaml_like(v, indent + 2).trim_start()
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => cell(value),
    }
}

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
        AuthCommand::Login { token } => {
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
            for ctx in config.contexts {
                let mark = if Some(&ctx.name) == config.current_context.as_ref() {
                    "*"
                } else {
                    " "
                };
                println!(
                    "{mark} {}\t{}\t{}\t{}",
                    ctx.name,
                    ctx.server,
                    ctx.org.unwrap_or_default(),
                    ctx.team.unwrap_or_default()
                );
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
        assert!(rendered.contains("name"));
        assert!(rendered.contains("a"));
    }
}
