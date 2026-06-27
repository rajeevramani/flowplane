use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Print the identity and scopes of the active credential.
    Whoami,
    /// Print the raw bearer token for the active context.
    Token,
    /// Acquire and store a bearer token (static token, PKCE, or device flow).
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
    /// Clear the stored credential for the active context.
    Logout,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the path to the CLI config file.
    Path,
    /// Print the merged CLI configuration.
    Show,
    /// Create or update a named context (server, org, team, token).
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
    /// Switch the active context.
    UseContext { name: String },
    /// List the configured contexts.
    GetContexts,
}

#[derive(Debug, Subcommand)]
pub enum OrgCommand {
    /// List organizations.
    List,
    /// Show one organization.
    Get { org: String },
    /// Create an organization.
    Create {
        name: String,
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Delete an organization.
    Delete { org: String },
    /// Manage organization members.
    Member {
        #[command(subcommand)]
        command: OrgMemberCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum OrgMemberCommand {
    /// List members of an organization.
    List { org: String },
    /// Add a member to an organization.
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
    /// Remove a member from an organization.
    Remove { org: String, user_id: String },
}

#[derive(Debug, Subcommand)]
pub enum TeamCommand {
    /// List teams.
    List,
    /// Create a team.
    Create {
        name: String,
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Delete a team.
    Delete {
        #[arg(long)]
        team: Option<String>,
    },
    /// Manage team members.
    Member {
        #[command(subcommand)]
        command: TeamMemberCommand,
    },
    /// Manage a team's resource grants.
    Grant {
        #[command(subcommand)]
        command: GrantCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum TeamMemberCommand {
    /// List members of a team.
    List {
        #[arg(long)]
        team: Option<String>,
    },
    /// Add a member to a team.
    Add {
        #[arg(long)]
        team: Option<String>,
        email: String,
    },
    /// Remove a member from a team.
    Remove {
        #[arg(long)]
        team: Option<String>,
        user_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum GrantCommand {
    /// List a team's grants.
    List {
        #[arg(long)]
        team: Option<String>,
    },
    /// Grant a member an action on a resource.
    Add {
        #[arg(long)]
        team: Option<String>,
        email: String,
        #[arg(long)]
        resource: String,
        #[arg(long)]
        action: String,
    },
    /// Revoke a grant.
    Remove {
        #[arg(long)]
        team: Option<String>,
        grant_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ResourceCommand {
    /// List the resources in this collection.
    List {
        #[arg(long)]
        team: Option<String>,
    },
    /// Show one resource.
    Get {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
    /// Create a resource from a JSON file.
    Create {
        #[arg(long)]
        team: Option<String>,
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Update a resource from a JSON file (requires `--revision`).
    Update {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Delete a resource.
    Delete {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RouteCommand {
    /// List route configurations.
    List {
        #[arg(long)]
        team: Option<String>,
    },
    /// Show one route configuration.
    Get {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
    /// Create a route configuration from a JSON file.
    Create {
        #[arg(long)]
        team: Option<String>,
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Update a route configuration from a JSON file (requires `--revision`).
    Update {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Delete a route configuration.
    Delete {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
    /// Generate a route plan from a published API spec.
    Generate {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        from_spec: String,
        #[arg(long)]
        listener_port: u16,
    },
    /// Apply a previously generated route plan.
    Apply {
        #[arg(long)]
        team: Option<String>,
        plan_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum AiCommand {
    /// Manage AI providers.
    Providers {
        #[command(subcommand)]
        command: ResourceCommand,
    },
    /// Manage AI routes.
    Routes {
        #[command(subcommand)]
        command: ResourceCommand,
    },
    /// Manage AI budgets.
    Budgets {
        #[command(subcommand)]
        command: ResourceCommand,
    },
    /// Show AI token-usage records.
    Usage {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        provider_id: Option<String>,
        #[arg(long)]
        route_config_id: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        #[arg(long, default_value_t = 0)]
        offset: i64,
    },
}

#[derive(Debug, Subcommand)]
pub enum RateLimitCommand {
    /// Rate-limit domains (the limit groups).
    Domain {
        #[command(subcommand)]
        command: ResourceCommand,
    },
    /// Policies within a domain.
    Policy {
        #[command(subcommand)]
        command: RateLimitPolicyCommand,
    },
    /// Per-team override of a policy's limit.
    Override {
        #[command(subcommand)]
        command: RateLimitOverrideCommand,
    },
    /// Force an immediate CP→RLS policy reconcile (platform admin).
    ForceRepush,
}

#[derive(Debug, Subcommand)]
pub enum RateLimitPolicyCommand {
    /// List the policies in a domain.
    List {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        domain: String,
    },
    /// Show one policy.
    Get {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        domain: String,
        name: String,
    },
    /// Create a policy from a JSON file.
    Create {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        domain: String,
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Update a policy from a JSON file (requires `--revision`).
    Update {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        domain: String,
        name: String,
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Delete a policy.
    Delete {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        domain: String,
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RateLimitOverrideCommand {
    /// Show a team's override of a policy.
    Get {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        domain: String,
        #[arg(long)]
        policy: String,
    },
    /// Create the override from a JSON file (`{ "spec": { "requests_per_unit": N } }`).
    Set {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        domain: String,
        #[arg(long)]
        policy: String,
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Update a policy override from a JSON file (requires `--revision`).
    Update {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        domain: String,
        #[arg(long)]
        policy: String,
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Delete a policy override.
    Delete {
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        domain: String,
        #[arg(long)]
        policy: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ApiCommand {
    /// List API definitions.
    List {
        #[arg(long)]
        team: Option<String>,
    },
    /// Show one API definition.
    Get {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
    /// Show an API definition's lifecycle status.
    Status {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
    /// Manage an API's imported spec versions.
    Spec {
        #[command(subcommand)]
        command: ApiSpecCommand,
    },
    /// Create an API definition (optionally importing an OpenAPI document).
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
    /// Delete an API definition.
    Delete {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ApiSpecCommand {
    /// Reject a pending spec version.
    Reject {
        #[arg(long)]
        team: Option<String>,
        api: String,
        version: i64,
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Publish a reviewed spec version.
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
pub enum McpCommand {
    /// Show MCP server status.
    Status {
        #[arg(long)]
        team: Option<String>,
    },
    /// List active MCP connections.
    Connections {
        #[arg(long)]
        team: Option<String>,
    },
    /// Expose an API as an MCP tool.
    Enable {
        #[arg(long = "api", alias = "tool")]
        api: String,
        #[arg(long)]
        team: Option<String>,
    },
    /// Stop exposing an API as an MCP tool.
    Disable {
        #[arg(long = "api", alias = "tool")]
        api: String,
        #[arg(long)]
        team: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum LearnCommand {
    /// Schema-free traffic discovery sessions.
    Discover {
        #[command(subcommand)]
        command: LearnDiscoverCommand,
    },
    /// Start a capture session against an existing API.
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
    /// List capture sessions.
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
    /// Show one capture session.
    Get {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
    /// Stop a capture session.
    Stop {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
    /// Generate an OpenAPI spec from a capture session.
    GenerateSpec {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
    /// Cancel a capture session.
    Cancel {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum LearnDiscoverCommand {
    /// Start a discovery session against a raw upstream.
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
    /// List discovery sessions.
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
    /// Show a discovery session's status.
    Status {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
    /// Stop a discovery session.
    Stop {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
    /// Generate an OpenAPI spec from a discovery session.
    GenerateSpec {
        #[arg(long)]
        team: Option<String>,
        session: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum SecretCommand {
    /// List secrets (metadata only).
    List {
        #[arg(long)]
        team: Option<String>,
    },
    /// Show one secret's metadata.
    Get {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
    /// Create a secret from a JSON file.
    Create {
        #[arg(long)]
        team: Option<String>,
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Rotate a secret's value from a JSON file.
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
    /// List dataplanes.
    List {
        #[arg(long)]
        team: Option<String>,
    },
    /// Show one dataplane.
    Get {
        #[arg(long)]
        team: Option<String>,
        name: String,
    },
    /// Register a dataplane.
    Create {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Submit dataplane telemetry from a JSON file.
    Telemetry {
        #[arg(long)]
        team: Option<String>,
        name: String,
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Generate an Envoy bootstrap config for a dataplane.
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
    /// Manage dataplane proxy certificates.
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
    /// Public gateway base URL clients can use to reach the listener.
    #[arg(long)]
    pub public_base_url: Option<String>,
}

#[derive(Debug, Args)]
pub struct UnexposeCommand {
    pub name: String,
    #[arg(long)]
    pub team: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum CertCommand {
    /// List a dataplane's proxy certificates.
    List {
        #[arg(long)]
        team: Option<String>,
    },
    /// Register a proxy certificate from a JSON file.
    Register {
        #[arg(long)]
        team: Option<String>,
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Issue a proxy certificate for a dataplane.
    Issue {
        #[arg(long)]
        team: Option<String>,
        dataplane: String,
        #[arg(long, default_value_t = 24)]
        ttl_hours: i64,
    },
    /// Revoke a proxy certificate.
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
    /// Show a team's resource counts.
    Overview {
        #[arg(long)]
        team: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum OpsCommand {
    /// xDS delivery diagnostics.
    Xds {
        #[command(subcommand)]
        command: XdsCommand,
    },
    /// Search request traces.
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
    /// Show xDS stream status.
    Status {
        #[arg(long)]
        team: Option<String>,
    },
    /// Show recent xDS NACKs.
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
