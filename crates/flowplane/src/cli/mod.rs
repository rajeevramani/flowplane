mod client;
mod commands;
mod config;
pub(crate) mod output;

use anyhow::{Context, Result};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use client::RestClient;
pub use commands::{
    ApiCommand, ApplyCommand, AuthCommand, CertCommand, ConfigCommand, DataplaneBootstrapMode,
    DataplaneCommand, ExposeCommand, GrantCommand, LearnCommand, OpsCommand, OrgCommand,
    OrgMemberCommand, ResourceCommand, SecretCommand, StatsCommand, TeamCommand, TeamMemberCommand,
    UnexposeCommand, XdsCommand,
};
pub use config::GlobalOptions;
use config::{
    config_path, credentials_path, effective, read_config, write_config, write_private_file,
    NamedContext,
};
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
    parse_json_or_yaml(&raw)
        .with_context(|| format!("parse JSON/YAML body from {}", path.display()))
}

fn parse_json_or_yaml(raw: &str) -> Result<Value> {
    match serde_json::from_str::<Value>(raw) {
        Ok(value) => Ok(value),
        Err(json_err) => serde_yaml::from_str::<Value>(raw).with_context(|| {
            format!(
                "input is not valid JSON and could not be parsed as YAML; JSON error: {json_err}"
            )
        }),
    }
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
        AuthCommand::Login {
            token,
            token_stdin,
            device,
            pkce,
            issuer,
            client_id,
            callback_url,
            scope,
        } => {
            let token = match (token, token_stdin, device, pkce) {
                (Some(token), false, false, false) => token,
                (None, true, false, false) => {
                    let mut token = String::new();
                    io::stdin().read_to_string(&mut token)?;
                    token.trim().to_string()
                }
                (None, false, true, true) => {
                    anyhow::bail!(
                        "use only one login input: --token, --token-stdin, --device-code, or --pkce"
                    )
                }
                (None, false, explicit_device, explicit_pkce) => {
                    let config = effective(&global)?;
                    if explicit_device {
                        device_login(&global, issuer, client_id, scope).await?
                    } else if explicit_pkce
                        || (config.oidc_issuer.is_some() && config.oidc_client_id.is_some())
                    {
                        pkce_login(&global, issuer, client_id, callback_url, scope).await?
                    } else {
                        anyhow::bail!(
                            "pass --token, --token-stdin, --device-code, or configure OIDC for PKCE"
                        )
                    }
                }
                (Some(_), true, _, _)
                | (Some(_), _, true, _)
                | (Some(_), _, _, true)
                | (None, true, true, _)
                | (None, true, _, true) => {
                    anyhow::bail!(
                        "use only one login input: --token, --token-stdin, --device-code, or --pkce"
                    )
                }
            };
            save_token(&token)?;
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

fn save_token(token: &str) -> Result<()> {
    let path = credentials_path();
    write_private_file(&path, token)?;
    println!("token saved to {}", path.display());
    Ok(())
}

#[derive(Debug, Deserialize)]
struct OidcDiscovery {
    #[serde(default)]
    authorization_endpoint: Option<String>,
    #[serde(default)]
    device_authorization_endpoint: Option<String>,
    token_endpoint: String,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenSuccess {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

async fn device_login(
    global: &GlobalOptions,
    issuer: Option<String>,
    client_id: Option<String>,
    scope: String,
) -> Result<String> {
    let config = effective(global)?;
    let issuer = issuer.or(config.oidc_issuer).ok_or_else(|| {
        anyhow::anyhow!("OIDC issuer is required; pass --issuer or set oidc_issuer")
    })?;
    let client_id = client_id.or(config.oidc_client_id).ok_or_else(|| {
        anyhow::anyhow!("OIDC client id is required; pass --client-id or set oidc_client_id")
    })?;
    let scope = config.oidc_scope.unwrap_or(scope);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(global.timeout))
        .build()?;
    let discovery_url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let discovery: OidcDiscovery = http
        .get(&discovery_url)
        .send()
        .await
        .with_context(|| format!("fetch OIDC discovery from {discovery_url}"))?
        .error_for_status()
        .with_context(|| format!("OIDC discovery failed at {discovery_url}"))?
        .json()
        .await
        .context("parse OIDC discovery")?;
    let device: DeviceCodeResponse = http
        .post(
            discovery
                .device_authorization_endpoint
                .as_ref()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "OIDC provider does not advertise device_authorization_endpoint"
                    )
                })?,
        )
        .form(&[("client_id", client_id.as_str()), ("scope", scope.as_str())])
        .send()
        .await
        .context("start device authorization")?
        .error_for_status()
        .context("device authorization failed")?
        .json()
        .await
        .context("parse device authorization response")?;

    if let Some(message) = &device.message {
        println!("{message}");
    } else if let Some(complete) = &device.verification_uri_complete {
        println!("Open {complete}");
        println!("Code: {}", device.user_code);
    } else {
        println!("Open {}", device.verification_uri);
        println!("Code: {}", device.user_code);
    }

    let expires_at = Instant::now() + Duration::from_secs(device.expires_in.unwrap_or(900));
    let mut interval = Duration::from_secs(device.interval.unwrap_or(5).max(1));
    loop {
        if Instant::now() >= expires_at {
            anyhow::bail!(
                "device authorization expired; run `flowplane auth login --device` again"
            );
        }
        tokio::time::sleep(interval).await;
        let response = http
            .post(&discovery.token_endpoint)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device.device_code.as_str()),
                ("client_id", client_id.as_str()),
            ])
            .send()
            .await
            .context("poll device token endpoint")?;
        let status = response.status();
        let text = response.text().await.context("read token response")?;
        if status.is_success() {
            let token: TokenSuccess =
                serde_json::from_str(&text).context("parse token response")?;
            return token.id_token.or(token.access_token).ok_or_else(|| {
                anyhow::anyhow!("token response did not include id_token or access_token")
            });
        }
        let error: TokenError = serde_json::from_str(&text).unwrap_or(TokenError {
            error: status.to_string(),
            error_description: Some(text),
        });
        match error.error.as_str() {
            "authorization_pending" => {}
            "slow_down" => interval += Duration::from_secs(5),
            "access_denied" => anyhow::bail!("device authorization denied"),
            "expired_token" => anyhow::bail!("device authorization expired"),
            _ => {
                let description = error.error_description.unwrap_or_default();
                anyhow::bail!(
                    "device token exchange failed: {} {}",
                    error.error,
                    description
                )
            }
        }
    }
}

async fn pkce_login(
    global: &GlobalOptions,
    issuer: Option<String>,
    client_id: Option<String>,
    callback_url: Option<String>,
    scope: String,
) -> Result<String> {
    let config = effective(global)?;
    let issuer = issuer.or(config.oidc_issuer).ok_or_else(|| {
        anyhow::anyhow!("OIDC issuer is required; pass --issuer or set oidc_issuer")
    })?;
    let client_id = client_id.or(config.oidc_client_id).ok_or_else(|| {
        anyhow::anyhow!("OIDC client id is required; pass --client-id or set oidc_client_id")
    })?;
    let scope = config.oidc_scope.unwrap_or(scope);
    let callback_url = callback_url
        .or(config.callback_url)
        .unwrap_or_else(|| "http://127.0.0.1:8976/callback".to_string());
    let callback = CallbackUrl::parse(&callback_url)?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(global.timeout))
        .build()?;
    let discovery_url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let discovery: OidcDiscovery = http
        .get(&discovery_url)
        .send()
        .await
        .with_context(|| format!("fetch OIDC discovery from {discovery_url}"))?
        .error_for_status()
        .with_context(|| format!("OIDC discovery failed at {discovery_url}"))?
        .json()
        .await
        .context("parse OIDC discovery")?;
    let authorization_endpoint = discovery.authorization_endpoint.ok_or_else(|| {
        anyhow::anyhow!("OIDC provider does not advertise authorization_endpoint")
    })?;

    let code_verifier = random_base64url(32)?;
    let code_challenge = base64url(&Sha256::digest(code_verifier.as_bytes()));
    let state = random_base64url(16)?;
    let authorize_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        authorization_endpoint,
        encode_component(&client_id),
        encode_component(&callback.redirect_uri),
        encode_component(&scope),
        encode_component(&state),
        encode_component(&code_challenge),
    );

    let listener = tokio::net::TcpListener::bind((callback.host.as_str(), callback.port))
        .await
        .with_context(|| format!("listen on {}", callback.origin()))?;
    println!("Open {authorize_url}");
    println!("Waiting for callback on {}", callback.redirect_uri);

    let (code, returned_state) = receive_oauth_callback(listener, &callback.path).await?;
    if returned_state.as_deref() != Some(state.as_str()) {
        anyhow::bail!("OIDC callback state did not match; login aborted");
    }
    let response = http
        .post(&discovery.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", callback.redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            ("code_verifier", code_verifier.as_str()),
        ])
        .send()
        .await
        .context("exchange authorization code")?;
    let status = response.status();
    let text = response.text().await.context("read token response")?;
    if !status.is_success() {
        let error: TokenError = serde_json::from_str(&text).unwrap_or(TokenError {
            error: status.to_string(),
            error_description: Some(text),
        });
        let description = error.error_description.unwrap_or_default();
        anyhow::bail!(
            "authorization code exchange failed: {} {}",
            error.error,
            description
        );
    }
    let token: TokenSuccess = serde_json::from_str(&text).context("parse token response")?;
    token
        .id_token
        .or(token.access_token)
        .ok_or_else(|| anyhow::anyhow!("token response did not include id_token or access_token"))
}

#[derive(Debug, PartialEq, Eq)]
struct CallbackUrl {
    redirect_uri: String,
    host: String,
    port: u16,
    path: String,
}

impl CallbackUrl {
    fn parse(raw: &str) -> Result<Self> {
        let rest = raw
            .strip_prefix("http://")
            .ok_or_else(|| anyhow::anyhow!("callback URL must use http:// loopback"))?;
        let (authority, path) = rest.split_once('/').unwrap_or((rest, "callback"));
        let (host, port) = authority
            .rsplit_once(':')
            .ok_or_else(|| anyhow::anyhow!("callback URL must include an explicit port"))?;
        if !matches!(host, "127.0.0.1" | "localhost") {
            anyhow::bail!("callback URL must use 127.0.0.1 or localhost");
        }
        let port = port
            .parse::<u16>()
            .with_context(|| format!("invalid callback port in {raw}"))?;
        Ok(Self {
            redirect_uri: raw.to_string(),
            host: host.to_string(),
            port,
            path: format!("/{path}"),
        })
    }

    fn origin(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

async fn receive_oauth_callback(
    listener: tokio::net::TcpListener,
    expected_path: &str,
) -> Result<(String, Option<String>)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let accept = async {
        let (mut stream, _) = listener.accept().await?;
        let mut buf = vec![0_u8; 8192];
        let n = stream.read(&mut buf).await?;
        let request = String::from_utf8_lossy(&buf[..n]);
        let first_line = request
            .lines()
            .next()
            .ok_or_else(|| anyhow::anyhow!("empty OAuth callback request"))?;
        let target = first_line
            .strip_prefix("GET ")
            .and_then(|line| line.split_once(' ').map(|(target, _)| target))
            .ok_or_else(|| anyhow::anyhow!("unexpected OAuth callback request"))?;
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let result = if path == expected_path {
            callback_param(query, "error")
                .map(|err| Err(anyhow::anyhow!("OIDC provider returned error: {err}")))
                .unwrap_or_else(|| {
                    let code = callback_param(query, "code")
                        .ok_or_else(|| anyhow::anyhow!("OIDC callback did not include code"))?;
                    Ok((code, callback_param(query, "state")))
                })
        } else {
            Err(anyhow::anyhow!("unexpected OAuth callback path {path}"))
        };
        let body = if result.is_ok() {
            "Flowplane login complete. You can close this tab.\n"
        } else {
            "Flowplane login failed. Return to the terminal for details.\n"
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).await?;
        result
    };
    tokio::time::timeout(Duration::from_secs(300), accept)
        .await
        .context("timed out waiting for OIDC callback")?
}

fn random_base64url(len: usize) -> Result<String> {
    let mut bytes = vec![0_u8; len];
    getrandom::fill(&mut bytes).context("generate random bytes")?;
    Ok(base64url(&bytes))
}

fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn callback_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|part| {
        let (k, v) = part.split_once('=')?;
        if k == key {
            percent_decode(v).ok()
        } else {
            None
        }
    })
}

fn encode_component(value: &str) -> String {
    value
        .bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect::<Vec<_>>(),
        })
        .collect()
}

fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3])?;
                out.push(u8::from_str_radix(hex, 16).context("invalid percent encoding")?);
                i += 3;
            }
            b'%' => anyhow::bail!("truncated percent encoding"),
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).context("percent decoded value is not UTF-8")
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

pub async fn run_expose(global: GlobalOptions, command: ExposeCommand) -> Result<()> {
    let client = RestClient::new(global)?;
    let team = client.team(command.team)?;
    client
        .request_and_render(
            reqwest::Method::POST,
            &format!("/api/v1/teams/{team}/expose"),
            Some(json!({
                "name": command.name,
                "upstream": command.upstream,
                "path": command.path,
                "port": command.port,
            })),
        )
        .await?;
    Ok(())
}

pub async fn run_unexpose(global: GlobalOptions, command: UnexposeCommand) -> Result<()> {
    let client = RestClient::new(global)?;
    let team = client.team(command.team)?;
    client
        .request_and_render(
            reqwest::Method::DELETE,
            &format!(
                "/api/v1/teams/{team}/expose/{}",
                query_component(&command.name)
            ),
            None,
        )
        .await?;
    Ok(())
}

pub async fn run_api(global: GlobalOptions, command: ApiCommand) -> Result<()> {
    let client = RestClient::new(global)?;
    match command {
        ApiCommand::List { team } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/api-definitions"),
                    None,
                )
                .await?
        }
        ApiCommand::Get { team, name } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/api-definitions/{name}"),
                    None,
                )
                .await?
        }
        ApiCommand::Status { team, name } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/api-definitions/{name}/status"),
                    None,
                )
                .await?
        }
        ApiCommand::Create {
            team,
            name,
            display_name,
            description,
            from_openapi,
            route_config_id,
            listener_id,
            virtual_host,
            route,
        } => {
            let team = client.team(team)?;
            let mut body = Map::new();
            body.insert("name".into(), Value::String(name));
            body.insert(
                "display_name".into(),
                Value::String(display_name.unwrap_or_default()),
            );
            body.insert("description".into(), Value::String(description));
            if let Some(path) = from_openapi {
                body.insert("openapi".into(), body_from_file(&path)?);
            }
            if let Some(route_config_id) = route_config_id {
                let mut binding = Map::new();
                binding.insert("route_config_id".into(), Value::String(route_config_id));
                if let Some(listener_id) = listener_id {
                    binding.insert("listener_id".into(), Value::String(listener_id));
                }
                if let Some(virtual_host) = virtual_host {
                    binding.insert("virtual_host".into(), Value::String(virtual_host));
                }
                if let Some(route) = route {
                    binding.insert("route".into(), Value::String(route));
                }
                body.insert("route_binding".into(), Value::Object(binding));
            } else if listener_id.is_some() || virtual_host.is_some() || route.is_some() {
                anyhow::bail!(
                    "--route-config-id is required when passing --listener-id, --virtual-host, or --route"
                );
            }
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/api-definitions"),
                    Some(Value::Object(body)),
                )
                .await?
        }
        ApiCommand::Delete { team, name } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::DELETE,
                    &format!("/api/v1/teams/{team}/api-definitions/{name}"),
                    None,
                )
                .await?
        }
    };
    Ok(())
}

pub async fn run_learn(global: GlobalOptions, command: LearnCommand) -> Result<()> {
    let client = RestClient::new(global)?;
    match command {
        LearnCommand::Start {
            team,
            name,
            api,
            api_definition_id,
            route_config_id,
            listener_id,
            virtual_host,
            route,
            target_sample_count,
            max_duration_seconds,
            max_bytes,
            max_distinct_paths,
        } => {
            let team = client.team(team)?;
            let target_count = [
                api.is_some(),
                api_definition_id.is_some(),
                route_config_id.is_some(),
            ]
            .into_iter()
            .filter(|set| *set)
            .count();
            if target_count != 1 {
                anyhow::bail!(
                    "pass exactly one target: --api, --api-definition-id, or --route-config-id"
                );
            }
            if route_config_id.is_none()
                && (listener_id.is_some() || virtual_host.is_some() || route.is_some())
            {
                anyhow::bail!(
                    "--listener-id, --virtual-host, and --route require --route-config-id"
                );
            }
            client
                .request(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/learning-sessions"),
                    Some(json!({
                        "name": name,
                        "api": api,
                        "api_definition_id": api_definition_id,
                        "route_config_id": route_config_id,
                        "listener_id": listener_id,
                        "virtual_host": virtual_host,
                        "route": route,
                        "target_sample_count": target_sample_count,
                        "max_duration_seconds": max_duration_seconds,
                        "max_bytes": max_bytes,
                        "max_distinct_paths": max_distinct_paths,
                    })),
                )
                .await?
        }
        LearnCommand::List {
            team,
            status,
            limit,
            offset,
        } => {
            let team = client.team(team)?;
            let mut query = vec![("limit", limit.to_string()), ("offset", offset.to_string())];
            if let Some(status) = status {
                query.push(("status", status));
            }
            let query = query
                .into_iter()
                .map(|(key, value)| format!("{key}={}", query_component(&value)))
                .collect::<Vec<_>>()
                .join("&");
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/learning-sessions?{query}"),
                    None,
                )
                .await?
        }
        LearnCommand::Get { team, session } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!(
                        "/api/v1/teams/{team}/learning-sessions/{}",
                        query_component(&session)
                    ),
                    None,
                )
                .await?
        }
        LearnCommand::Stop { team, session } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::POST,
                    &format!(
                        "/api/v1/teams/{team}/learning-sessions/{}/stop",
                        query_component(&session)
                    ),
                    None,
                )
                .await?
        }
        LearnCommand::Cancel { team, session } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::DELETE,
                    &format!(
                        "/api/v1/teams/{team}/learning-sessions/{}",
                        query_component(&session)
                    ),
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
        SecretCommand::Rotate {
            team,
            name,
            revision,
            file,
        } => {
            let team = client.team(team)?;
            client
                .request_with_revision(
                    reqwest::Method::POST,
                    &format!("/api/v1/teams/{team}/secrets/{name}/rotate"),
                    Some(body_from_file(&file)?),
                    Some(revision),
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
            mode,
            xds_host,
            xds_port,
            admin_port,
            cert_path,
            key_path,
            ca_path,
        } => {
            let team = client.team(team)?;
            if mode == DataplaneBootstrapMode::Mtls
                && (cert_path.is_none() || key_path.is_none() || ca_path.is_none())
            {
                anyhow::bail!(
                    "--cert-path, --key-path, and --ca-path are required with --mode mtls"
                );
            }
            let mut query = vec![
                ("mode", mode.as_query_value().to_string()),
                ("xds_host", xds_host),
                ("xds_port", xds_port.to_string()),
                ("admin_port", admin_port.to_string()),
            ];
            if let Some(cert_path) = cert_path {
                query.push(("cert_path", cert_path));
            }
            if let Some(key_path) = key_path {
                query.push(("key_path", key_path));
            }
            if let Some(ca_path) = ca_path {
                query.push(("ca_path", ca_path));
            }
            let query = query
                .into_iter()
                .map(|(key, value)| format!("{key}={}", query_component(&value)))
                .collect::<Vec<_>>()
                .join("&");
            client
                .request_text(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/dataplanes/{name}/envoy-config?{query}"),
                )
                .await?;
            None
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
            command: XdsCommand::Status { team },
        } => {
            let team = client.team(team)?;
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/xds/status"),
                    None,
                )
                .await?;
        }
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
        OpsCommand::Trace {
            team,
            request_id,
            trace_id,
            path,
            limit,
        } => {
            let team = client.team(team)?;
            let mut query = Vec::new();
            if let Some(request_id) = request_id {
                query.push(("request_id", request_id));
            }
            if let Some(trace_id) = trace_id {
                query.push(("trace_id", trace_id));
            }
            if let Some(path) = path {
                query.push(("path", path));
            }
            query.push(("limit", limit.to_string()));
            let query = query
                .into_iter()
                .map(|(key, value)| format!("{key}={}", query_component(&value)))
                .collect::<Vec<_>>()
                .join("&");
            client
                .request(
                    reqwest::Method::GET,
                    &format!("/api/v1/teams/{team}/ops/trace?{query}"),
                    None,
                )
                .await?;
        }
    }
    Ok(())
}

fn query_component(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
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
        "/api/v1/teams/{team}/expose",
        "/api/v1/teams/{team}/expose/{name}",
        "/api/v1/teams/{team}/api-definitions",
        "/api/v1/teams/{team}/api-definitions/{name}",
        "/api/v1/teams/{team}/api-definitions/{name}/status",
        "/api/v1/teams/{team}/learning-sessions",
        "/api/v1/teams/{team}/learning-sessions/{session}",
        "/api/v1/teams/{team}/learning-sessions/{session}/stop",
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
        "/api/v1/teams/{team}/xds/status",
        "/api/v1/teams/{team}/ops/trace",
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
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
    fn table_flattens_api_status() {
        let rendered = table(&json!({
            "api": {
                "id": "8f000000-0000-7000-8000-000000000001",
                "name": "catalog",
                "display_name": "Catalog",
                "description": "",
                "revision": 1,
                "created_at": "2026-06-14T00:00:00Z",
                "updated_at": "2026-06-14T00:00:00Z"
            },
            "latest_spec": {
                "version": 1,
                "source_kind": "imported",
                "format": "openapi3",
                "spec_hash": "1234567890abcdef"
            },
            "route_binding_count": 0,
            "tool_count": 2
        }));
        assert!(rendered.contains("NAME"));
        assert!(rendered.contains("Catalog"));
        assert!(rendered.contains("LATEST SPEC VERSION"));
        assert!(rendered.contains("imported"));
        assert!(rendered.contains("1234567890ab"));
        assert!(!rendered.contains("{...}"));
    }

    #[test]
    fn table_flattens_xds_status() {
        let rendered = table(&json!({
            "health": "healthy",
            "total_dataplanes": 1,
            "live_dataplanes": 1,
            "stale_dataplanes": 0,
            "config_verified_dataplanes": 1,
            "total_requests": 42,
            "total_errors": 0,
            "warming_failures": 0,
            "recent_nack_count": 0,
            "dataplanes": [{
                "name": "local",
                "id": "dp-1",
                "live": true
            }]
        }));
        assert!(rendered.contains("HEALTH"));
        assert!(rendered.contains("LIVE DATAPLANES"));
        assert!(rendered.contains("healthy"));
        assert!(!rendered.contains("{...}"));
    }

    #[test]
    fn table_flattens_ops_trace_rows() {
        let rendered = table(&json!({
            "audit": [{
                "occurred_at": "2026-06-14T00:00:00Z",
                "request_id": "019f0000-0000-7000-8000-000000000001",
                "surface": "rest",
                "action": "cluster.create",
                "resource": "clusters/api",
                "outcome": "success",
                "actor_label": "dev"
            }],
            "events": [{
                "seq": 10,
                "event_type": "cluster.changed",
                "occurred_at": "2026-06-14T00:00:01Z"
            }]
        }));
        assert!(rendered.contains("SOURCE"));
        assert!(rendered.contains("audit"));
        assert!(rendered.contains("outbox"));
        assert!(rendered.contains("cluster.changed"));
        assert!(!rendered.contains("{...}"));
    }

    #[test]
    fn table_flattens_expose_response() {
        let rendered = table(&json!({
            "name": "demo",
            "upstream": "http://127.0.0.1:3001",
            "path": "/",
            "port": 10001,
            "curl_url": "http://127.0.0.1:10001/",
            "cluster": {"name": "demo-upstream", "spec": {}},
            "route_config": {"name": "demo-routes", "spec": {}},
            "listener": {"name": "demo", "spec": {}}
        }));
        assert!(rendered.contains("CURL URL"));
        assert!(rendered.contains("http://127.0.0.1:10001/"));
        assert!(rendered.contains("demo-upstream"));
        assert!(!rendered.contains("{...}"));
    }

    #[test]
    fn openapi_file_parser_accepts_json_and_yaml() -> Result<()> {
        let json_doc = parse_json_or_yaml(
            r#"{"openapi":"3.0.3","info":{"title":"Catalog","version":"1"},"paths":{}}"#,
        )?;
        assert_eq!(json_doc["openapi"], "3.0.3");

        let yaml_doc = parse_json_or_yaml(
            r#"
openapi: 3.0.3
info:
  title: Catalog
  version: "1"
paths:
  /items:
    get:
      operationId: listItems
"#,
        )?;
        assert_eq!(yaml_doc["info"]["title"], "Catalog");
        assert_eq!(
            yaml_doc["paths"]["/items"]["get"]["operationId"],
            "listItems"
        );
        Ok(())
    }

    #[test]
    fn openapi_file_parser_reports_malformed_input() {
        let err = parse_json_or_yaml("openapi: [").expect_err("malformed YAML");
        let message = format!("{err:#}");
        assert!(message.contains("not valid JSON"));
        assert!(message.contains("could not be parsed as YAML"));
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
callback_url = "http://127.0.0.1:8976/callback"
"#,
        )?;
        assert_eq!(parsed.base_url.as_deref(), Some("http://localhost:8080"));
        assert_eq!(parsed.org.as_deref(), Some("dev-org"));
        assert_eq!(parsed.team.as_deref(), Some("default"));
        assert_eq!(parsed.oidc_issuer.as_deref(), Some("http://localhost:8081"));
        assert_eq!(parsed.oidc_client_id.as_deref(), Some("376872439851843590"));
        assert_eq!(
            parsed.callback_url.as_deref(),
            Some("http://127.0.0.1:8976/callback")
        );
        assert!(parsed.contexts.is_empty());
        Ok(())
    }

    #[test]
    fn pkce_callback_url_is_loopback_only() -> Result<()> {
        let parsed = CallbackUrl::parse("http://127.0.0.1:8976/callback")?;
        assert_eq!(
            parsed,
            CallbackUrl {
                redirect_uri: "http://127.0.0.1:8976/callback".into(),
                host: "127.0.0.1".into(),
                port: 8976,
                path: "/callback".into(),
            }
        );
        assert!(CallbackUrl::parse("https://127.0.0.1:8976/callback").is_err());
        assert!(CallbackUrl::parse("http://example.com:8976/callback").is_err());
        Ok(())
    }

    #[test]
    fn pkce_encoding_helpers_match_oauth_query_rules() -> Result<()> {
        assert_eq!(encode_component("a b/c"), "a%20b%2Fc");
        assert_eq!(percent_decode("a+b%2Fc")?, "a b/c");
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
    fn apply_manifest_preserves_advanced_gateway_specs() -> Result<()> {
        let listener_spec = json!({
            "address": "0.0.0.0",
            "port": 18080,
            "protocol": "http2",
            "route_config": "edge-routes",
            "access_logs": [{"path": "/tmp/flowplane-access.log"}],
            "http_filters": [{
                "filter": {
                    "type": "global_rate_limit",
                    "domain": "flowplane",
                    "service_cluster": "flowplane-rls",
                    "timeout_ms": 50,
                    "failure_mode_deny": true,
                    "stage": 1,
                    "request_type": "external",
                    "stat_prefix": "edge_rls",
                    "enable_x_ratelimit_headers": true,
                    "disable_x_envoy_ratelimited_header": true,
                    "rate_limited_status": 429,
                    "status_on_error": 503
                }
            }]
        });
        let target = apply_target(json!({
            "kind": "listener",
            "team": "platform",
            "name": "edge",
            "spec": listener_spec
        }))?;
        assert_eq!(target.kind, ApplyKind::Listener);
        assert_eq!(target.create_body["spec"]["protocol"], "http2");
        assert_eq!(
            target.create_body["spec"]["http_filters"][0]["filter"]["type"],
            "global_rate_limit"
        );
        assert_eq!(
            target.update_body()?,
            json!({"spec": target.create_body["spec"].clone()})
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
