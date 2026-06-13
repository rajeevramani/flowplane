use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    Whoami,
    Token,
    Login {
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        token_stdin: bool,
        #[arg(long, alias = "device-code")]
        device: bool,
        #[arg(long)]
        pkce: bool,
        #[arg(long)]
        issuer: Option<String>,
        #[arg(long)]
        client_id: Option<String>,
        #[arg(long)]
        callback_url: Option<String>,
        #[arg(long, default_value = "openid email profile")]
        scope: String,
    },
    Logout,
}

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
pub enum ApiCommand {
    List {
        #[arg(long)]
        team: Option<String>,
    },
    Get {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
    Status {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
    Create {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long, default_value = "")]
        description: String,
        #[arg(long)]
        from_openapi: Option<PathBuf>,
        #[arg(long)]
        route_config_id: Option<String>,
        #[arg(long)]
        listener_id: Option<String>,
        #[arg(long)]
        virtual_host: Option<String>,
        #[arg(long)]
        route: Option<String>,
    },
    Delete {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
}

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
pub enum StatsCommand {
    Overview {
        #[arg(long)]
        team: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum OpsCommand {
    Xds {
        #[command(subcommand)]
        command: XdsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum XdsCommand {
    Nacks {
        #[arg(long)]
        team: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct ApplyCommand {
    #[arg(short, long)]
    pub file: PathBuf,
    #[arg(long)]
    pub diff: bool,
}
