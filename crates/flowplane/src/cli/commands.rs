use clap::{Args, Subcommand, ValueEnum};
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
        #[arg(long)]
        token_stdin: bool,
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
pub enum RouteCommand {
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
    Generate {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        from_spec: String,
        #[arg(long)]
        listener_port: u16,
    },
    Apply {
        #[arg(long)]
        team: Option<String>,
        plan_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum AiCommand {
    Providers {
        #[command(subcommand)]
        command: ResourceCommand,
    },
    Routes {
        #[command(subcommand)]
        command: ResourceCommand,
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
    Spec {
        #[command(subcommand)]
        command: ApiSpecCommand,
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
pub enum ApiSpecCommand {
    Reject {
        #[arg(long)]
        team: Option<String>,
        api: String,
        version: i64,
        #[arg(long, default_value = "")]
        reason: String,
    },
    Publish {
        #[arg(long)]
        team: Option<String>,
        api: String,
        version: i64,
        #[arg(long, default_value = "")]
        reason: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum LearnCommand {
    Discover {
        #[command(subcommand)]
        command: LearnDiscoverCommand,
    },
    Start {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(long)]
        api: Option<String>,
        #[arg(long)]
        api_definition_id: Option<String>,
        #[arg(long)]
        route_config_id: Option<String>,
        #[arg(long)]
        listener_id: Option<String>,
        #[arg(long)]
        virtual_host: Option<String>,
        #[arg(long)]
        route: Option<String>,
        #[arg(long, default_value_t = 1000)]
        target_sample_count: i32,
        #[arg(long)]
        max_duration_seconds: Option<i32>,
        #[arg(long, default_value_t = 10 * 1024 * 1024)]
        max_bytes: i64,
        #[arg(long, default_value_t = 500)]
        max_distinct_paths: i32,
    },
    List {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        #[arg(long, default_value_t = 0)]
        offset: i64,
    },
    Get {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
    Stop {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
    GenerateSpec {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
    Cancel {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum LearnDiscoverCommand {
    Start {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(long)]
        upstream: String,
        #[arg(long)]
        listener_port: i32,
        #[arg(long)]
        upstream_tls: bool,
        #[arg(long, default_value_t = 1000)]
        target_sample_count: i32,
        #[arg(long)]
        max_duration_seconds: Option<i32>,
        #[arg(long, default_value_t = 10 * 1024 * 1024)]
        max_bytes: i64,
        #[arg(long, default_value_t = 500)]
        max_distinct_paths: i32,
    },
    List {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        #[arg(long, default_value_t = 0)]
        offset: i64,
    },
    Status {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
    Stop {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
    GenerateSpec {
        #[arg(long)]
        team: Option<String>,
        session: String,
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
        #[arg(long)]
        revision: i64,
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
    #[command(alias = "envoy-config")]
    Bootstrap {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(long, value_enum, default_value_t = DataplaneBootstrapMode::Dev)]
        mode: DataplaneBootstrapMode,
        #[arg(long, default_value = "127.0.0.1")]
        xds_host: String,
        #[arg(long, default_value_t = 18000)]
        xds_port: u16,
        #[arg(long, default_value_t = 9901)]
        admin_port: u16,
        #[arg(long)]
        cert_path: Option<String>,
        #[arg(long)]
        key_path: Option<String>,
        #[arg(long)]
        ca_path: Option<String>,
    },
    Cert {
        #[command(subcommand)]
        command: CertCommand,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum DataplaneBootstrapMode {
    /// Plaintext xDS for local FLOWPLANE_DEV_MODE runs.
    Dev,
    /// mTLS xDS for non-dev dataplanes.
    Mtls,
}

impl DataplaneBootstrapMode {
    pub(crate) fn as_query_value(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Mtls => "mtls",
        }
    }
}

#[derive(Debug, Args)]
pub struct ExposeCommand {
    pub upstream: String,
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub team: Option<String>,
    #[arg(long, default_value = "/")]
    pub path: String,
    #[arg(long)]
    pub port: Option<u16>,
}

#[derive(Debug, Args)]
pub struct UnexposeCommand {
    pub name: String,
    #[arg(long)]
    pub team: Option<String>,
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
    Trace {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        request_id: Option<String>,
        #[arg(long)]
        trace_id: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
}

#[derive(Debug, Subcommand)]
pub enum XdsCommand {
    Status {
        #[arg(long)]
        team: Option<String>,
    },
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
    /// Refuse silently ignoring prune requests; apply is additive-only until server batch support.
    #[arg(long)]
    pub prune: bool,
}
