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
        /// Static bearer token to store for the active context.
        #[arg(long)]
        token: Option<String>,
        /// Read the static bearer token from stdin.
        #[arg(long)]
        token_stdin: bool,
        /// Use the OAuth device-authorization flow.
        #[arg(long, alias = "device-code")]
        device: bool,
        /// Use the OAuth authorization-code flow with PKCE.
        #[arg(long)]
        pkce: bool,
        /// OIDC issuer URL to authenticate against.
        #[arg(long)]
        issuer: Option<String>,
        /// OAuth client ID to use for the login flow.
        #[arg(long)]
        client_id: Option<String>,
        /// Local redirect URL for the PKCE callback.
        #[arg(long)]
        callback_url: Option<String>,
        /// OAuth scopes to request.
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
        /// Name of the context to create or update.
        name: String,
        /// Control-plane server base URL for the context.
        #[arg(long)]
        server: String,
        /// Organization scope to store in the context.
        #[arg(long)]
        org: Option<String>,
        /// Team scope to store in the context.
        #[arg(long)]
        team: Option<String>,
        /// Bearer token to store in the context.
        #[arg(long)]
        token: Option<String>,
        /// Read the context's bearer token from stdin.
        #[arg(long)]
        token_stdin: bool,
    },
    /// Switch the active context.
    UseContext {
        /// Name of the context to switch to.
        name: String,
    },
    /// List the configured contexts.
    GetContexts,
}

#[derive(Debug, Subcommand)]
pub enum OrgCommand {
    /// List organizations.
    List,
    /// Show one organization.
    Get {
        /// Name of the organization to show.
        org: String,
    },
    /// Create an organization.
    Create {
        /// Name (slug) of the organization to create.
        name: String,
        /// Optional human-readable display name for the organization.
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Delete an organization.
    Delete {
        /// Name of the organization to delete.
        org: String,
    },
    /// Manage organization members.
    Member {
        #[command(subcommand)]
        command: OrgMemberCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum OrgMemberCommand {
    /// List members of an organization.
    List {
        /// Organization whose members to list.
        org: String,
    },
    /// Add a member to an organization.
    Add {
        /// Organization to add the member to.
        org: String,
        /// Email address identifying the user to add.
        #[arg(long)]
        email: Option<String>,
        /// Subject (JWT `sub`) identifying the user to add.
        #[arg(long)]
        subject: Option<String>,
        /// User ID identifying the user to add.
        #[arg(long)]
        user_id: Option<String>,
        /// Role to assign to the member.
        #[arg(long)]
        role: String,
    },
    /// Remove a member from an organization.
    Remove {
        /// Organization to remove the member from.
        org: String,
        /// User ID of the member to remove.
        user_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum TeamCommand {
    /// List teams.
    List,
    /// Create a team.
    Create {
        /// Name (slug) of the team to create.
        name: String,
        /// Optional human-readable display name for the team.
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Delete a team.
    Delete {
        /// Team to delete; defaults to the active context's team.
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
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
    /// Add a member to a team.
    Add {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Email address of the user to add.
        email: String,
    },
    /// Remove a member from a team.
    Remove {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// User ID of the member to remove.
        user_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum GrantCommand {
    /// List a team's grants.
    List {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
    /// Grant a member an action on a resource.
    Add {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Email address of the member to grant access to.
        email: String,
        /// Resource type the grant applies to.
        #[arg(long)]
        resource: String,
        /// Action the grant permits on the resource.
        #[arg(long)]
        action: String,
    },
    /// Revoke a grant.
    Remove {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Identifier of the grant to revoke.
        grant_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ResourceCommand {
    /// List the resources in this collection.
    List {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
    /// Show one resource.
    Get {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the resource to show.
        name: String,
    },
    /// Create a resource from a JSON file.
    Create {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Path to the JSON request body (use `-` for stdin).
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Update a resource from a JSON file (requires `--revision`).
    Update {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the resource to update.
        name: String,
        /// Path to the JSON request body (use `-` for stdin).
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Delete a resource.
    Delete {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the resource to delete.
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RouteCommand {
    /// List route configurations.
    List {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
    /// Show one route configuration.
    Get {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the route configuration to show.
        name: String,
    },
    /// Create a route configuration from a JSON file.
    Create {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Path to the JSON request body (use `-` for stdin).
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Update a route configuration from a JSON file (requires `--revision`).
    Update {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the route configuration to update.
        name: String,
        /// Path to the JSON request body (use `-` for stdin).
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Delete a route configuration.
    Delete {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the route configuration to delete.
        name: String,
    },
    /// Generate a route plan from a published API spec.
    Generate {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Reviewed or published learned spec version ID to generate the route plan from.
        #[arg(long)]
        from_spec: String,
        /// Listener port the generated routes bind to.
        #[arg(long)]
        listener_port: u16,
    },
    /// Apply a previously generated route plan.
    Apply {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Identifier of the previously generated route plan to apply.
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
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Filter usage records to this AI provider ID.
        #[arg(long)]
        provider_id: Option<String>,
        /// Filter usage records to this route configuration ID.
        #[arg(long)]
        route_config_id: Option<String>,
        /// Maximum number of records to return.
        #[arg(long, default_value_t = 50)]
        limit: i64,
        /// Number of records to skip for pagination.
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
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Rate-limit domain the policies belong to.
        #[arg(long)]
        domain: String,
    },
    /// Show one policy.
    Get {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Rate-limit domain the policy belongs to.
        #[arg(long)]
        domain: String,
        /// Name of the policy to show.
        name: String,
    },
    /// Create a policy from a JSON file.
    Create {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Rate-limit domain to create the policy in.
        #[arg(long)]
        domain: String,
        /// Path to the JSON request body (use `-` for stdin).
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Update a policy from a JSON file (requires `--revision`).
    Update {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Rate-limit domain the policy belongs to.
        #[arg(long)]
        domain: String,
        /// Name of the policy to update.
        name: String,
        /// Path to the JSON request body (use `-` for stdin).
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Delete a policy.
    Delete {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Rate-limit domain the policy belongs to.
        #[arg(long)]
        domain: String,
        /// Name of the policy to delete.
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RateLimitOverrideCommand {
    /// Show a team's override of a policy.
    Get {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Rate-limit domain the policy belongs to.
        #[arg(long)]
        domain: String,
        /// Policy whose team override to show.
        #[arg(long)]
        policy: String,
    },
    /// Create the override from a JSON file (`{ "spec": { "requests_per_unit": N } }`).
    Set {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Rate-limit domain the policy belongs to.
        #[arg(long)]
        domain: String,
        /// Policy to override.
        #[arg(long)]
        policy: String,
        /// Path to the JSON request body (use `-` for stdin).
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Update a policy override from a JSON file (requires `--revision`).
    Update {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Rate-limit domain the policy belongs to.
        #[arg(long)]
        domain: String,
        /// Policy whose override to update.
        #[arg(long)]
        policy: String,
        /// Path to the JSON request body (use `-` for stdin).
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Delete a policy override.
    Delete {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Rate-limit domain the policy belongs to.
        #[arg(long)]
        domain: String,
        /// Policy whose override to delete.
        #[arg(long)]
        policy: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ApiCommand {
    /// List API definitions.
    List {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
    /// Show one API definition.
    Get {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the API definition to show.
        name: String,
    },
    /// Show an API definition's lifecycle status.
    Status {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the API definition whose status to show.
        name: String,
    },
    /// Manage an API's imported spec versions.
    Spec {
        #[command(subcommand)]
        command: ApiSpecCommand,
    },
    /// Create an API definition (optionally importing an OpenAPI document).
    Create {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the API definition to create.
        name: String,
        /// Optional human-readable display name for the API.
        #[arg(long)]
        display_name: Option<String>,
        /// Optional description for the API definition.
        #[arg(long, default_value = "")]
        description: String,
        /// Path to an OpenAPI document to import.
        #[arg(long)]
        from_openapi: Option<PathBuf>,
        /// Route configuration ID to attach the API to.
        #[arg(long)]
        route_config_id: Option<String>,
        /// Listener ID to attach the API to.
        #[arg(long)]
        listener_id: Option<String>,
        /// Virtual host name to attach the API to.
        #[arg(long)]
        virtual_host: Option<String>,
        /// Route name to attach the API to.
        #[arg(long)]
        route: Option<String>,
    },
    /// Delete an API definition.
    Delete {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the API definition to delete.
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ApiSpecCommand {
    /// Reject a pending spec version.
    Reject {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the API whose spec version to reject.
        api: String,
        /// Spec version number to reject.
        version: i64,
        /// Optional human-readable reason recorded in the audit log.
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Publish a reviewed spec version.
    Publish {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the API whose spec version to publish.
        api: String,
        /// Spec version number to publish.
        version: i64,
        /// Optional human-readable reason recorded in the audit log.
        #[arg(long, default_value = "")]
        reason: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum McpCommand {
    /// Show MCP server status.
    Status {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
    /// List active MCP connections.
    Connections {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
    /// Expose an API as an MCP tool.
    Enable {
        /// Name of the API to expose as an MCP tool.
        #[arg(long = "api", alias = "tool")]
        api: String,
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
    /// Stop exposing an API as an MCP tool.
    Disable {
        /// Name of the API to stop exposing as an MCP tool.
        #[arg(long = "api", alias = "tool")]
        api: String,
        /// Team scope; defaults to the active context's team.
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
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name for the capture session.
        name: String,
        /// Name of the existing API to capture traffic for.
        #[arg(long)]
        api: Option<String>,
        /// API definition ID to capture traffic for.
        #[arg(long)]
        api_definition_id: Option<String>,
        /// Route configuration ID to scope capture to.
        #[arg(long)]
        route_config_id: Option<String>,
        /// Listener ID to scope capture to.
        #[arg(long)]
        listener_id: Option<String>,
        /// Virtual host name to scope capture to.
        #[arg(long)]
        virtual_host: Option<String>,
        /// Route name to scope capture to.
        #[arg(long)]
        route: Option<String>,
        /// Target number of request samples to collect.
        #[arg(long, default_value_t = 1000)]
        target_sample_count: i32,
        /// Maximum capture duration in seconds.
        #[arg(long)]
        max_duration_seconds: Option<i32>,
        /// Maximum total bytes to capture.
        #[arg(long, default_value_t = 10 * 1024 * 1024)]
        max_bytes: i64,
        /// Maximum number of distinct paths to track.
        #[arg(long, default_value_t = 500)]
        max_distinct_paths: i32,
    },
    /// List capture sessions.
    List {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Filter sessions by status.
        #[arg(long)]
        status: Option<String>,
        /// Maximum number of sessions to return.
        #[arg(long, default_value_t = 50)]
        limit: i64,
        /// Number of sessions to skip for pagination.
        #[arg(long, default_value_t = 0)]
        offset: i64,
    },
    /// Show one capture session.
    Get {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Identifier of the capture session to show.
        session: String,
    },
    /// Stop a capture session.
    Stop {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Identifier of the capture session to stop.
        session: String,
    },
    /// Generate an OpenAPI spec from a capture session.
    GenerateSpec {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Identifier of the capture session to generate a spec from.
        session: String,
    },
    /// Cancel a capture session.
    Cancel {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Identifier of the capture session to cancel.
        session: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum LearnDiscoverCommand {
    /// Start a discovery session against a raw upstream.
    Start {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name for the discovery session.
        name: String,
        /// Upstream address to capture traffic from.
        #[arg(long)]
        upstream: String,
        /// Listener port to bind for capture.
        #[arg(long)]
        listener_port: i32,
        /// Connect to the upstream over TLS.
        #[arg(long)]
        upstream_tls: bool,
        /// Target number of request samples to collect.
        #[arg(long, default_value_t = 1000)]
        target_sample_count: i32,
        /// Maximum capture duration in seconds.
        #[arg(long)]
        max_duration_seconds: Option<i32>,
        /// Maximum total bytes to capture.
        #[arg(long, default_value_t = 10 * 1024 * 1024)]
        max_bytes: i64,
        /// Maximum number of distinct paths to track.
        #[arg(long, default_value_t = 500)]
        max_distinct_paths: i32,
    },
    /// List discovery sessions.
    List {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Filter sessions by status.
        #[arg(long)]
        status: Option<String>,
        /// Maximum number of sessions to return.
        #[arg(long, default_value_t = 50)]
        limit: i64,
        /// Number of sessions to skip for pagination.
        #[arg(long, default_value_t = 0)]
        offset: i64,
    },
    /// Show a discovery session's status.
    Status {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Identifier of the discovery session to show.
        session: String,
    },
    /// Stop a discovery session.
    Stop {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Identifier of the discovery session to stop.
        session: String,
    },
    /// Generate an OpenAPI spec from a discovery session.
    GenerateSpec {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Identifier of the discovery session to generate a spec from.
        session: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum SecretCommand {
    /// List secrets (metadata only).
    List {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
    /// Show one secret's metadata.
    Get {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the secret to show.
        name: String,
    },
    /// Create a secret from a JSON file.
    Create {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Path to the JSON request body (use `-` for stdin).
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Rotate a secret's value from a JSON file.
    Rotate {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the secret to rotate.
        name: String,
        /// Current revision being rotated (optimistic concurrency).
        #[arg(long)]
        revision: i64,
        /// Path to the JSON request body (use `-` for stdin).
        #[arg(short, long)]
        file: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum DataplaneCommand {
    /// List dataplanes.
    List {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
    /// Show one dataplane.
    Get {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the dataplane to show.
        name: String,
    },
    /// Register a dataplane.
    Create {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the dataplane to register.
        name: String,
        /// Optional description for the dataplane.
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Submit dataplane telemetry from a JSON file.
    Telemetry {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the dataplane to submit telemetry for.
        name: String,
        /// Path to the JSON request body (use `-` for stdin).
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Generate an Envoy bootstrap config for a dataplane.
    #[command(alias = "envoy-config")]
    Bootstrap {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the dataplane to generate a bootstrap config for.
        name: String,
        /// Bootstrap mode: dev (plaintext xDS) or mtls.
        #[arg(long, value_enum, default_value_t = DataplaneBootstrapMode::Dev)]
        mode: DataplaneBootstrapMode,
        /// xDS server host the Envoy bootstrap points at.
        #[arg(long, default_value = "127.0.0.1")]
        xds_host: String,
        /// xDS server port the Envoy bootstrap points at.
        #[arg(long, default_value_t = 18000)]
        xds_port: u16,
        /// Envoy admin interface port.
        #[arg(long, default_value_t = 9901)]
        admin_port: u16,
        /// Path to the client certificate for mTLS xDS.
        #[arg(long)]
        cert_path: Option<String>,
        /// Path to the client private key for mTLS xDS.
        #[arg(long)]
        key_path: Option<String>,
        /// Path to the CA certificate for mTLS xDS.
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
    /// Upstream address to expose through the gateway.
    pub upstream: String,
    /// Name for the exposed route and listener.
    #[arg(long)]
    pub name: String,
    /// Team scope; defaults to the active context's team.
    #[arg(long)]
    pub team: Option<String>,
    /// Path prefix to route to the upstream.
    #[arg(long, default_value = "/")]
    pub path: String,
    /// Listener port to bind.
    #[arg(long)]
    pub port: Option<u16>,
    /// Public gateway base URL clients can use to reach the listener.
    #[arg(long)]
    pub public_base_url: Option<String>,
}

#[derive(Debug, Args)]
pub struct UnexposeCommand {
    /// Name of the exposed route to remove.
    pub name: String,
    /// Team scope; defaults to the active context's team.
    #[arg(long)]
    pub team: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum CertCommand {
    /// List a dataplane's proxy certificates.
    List {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
    /// Register a proxy certificate from a JSON file.
    Register {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Path to the JSON request body (use `-` for stdin).
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Issue a proxy certificate for a dataplane.
    Issue {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Name of the dataplane to issue a certificate for.
        dataplane: String,
        /// Certificate lifetime in hours.
        #[arg(long, default_value_t = 24)]
        ttl_hours: i64,
    },
    /// Revoke a proxy certificate.
    Revoke {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Serial number of the certificate to revoke.
        serial: String,
        /// Reason recorded for the revocation.
        #[arg(long)]
        reason: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum StatsCommand {
    /// Show a team's resource counts.
    Overview {
        /// Team scope; defaults to the active context's team.
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
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
        /// Filter traces by request ID.
        #[arg(long)]
        request_id: Option<String>,
        /// Filter traces by trace ID.
        #[arg(long)]
        trace_id: Option<String>,
        /// Filter traces by request path.
        #[arg(long)]
        path: Option<String>,
        /// Maximum number of traces to return.
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
}

#[derive(Debug, Subcommand)]
pub enum XdsCommand {
    /// Show xDS stream status.
    Status {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
    /// Show recent xDS NACKs.
    Nacks {
        /// Team scope; defaults to the active context's team.
        #[arg(long)]
        team: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct ApplyCommand {
    /// Path to the JSON document of resources to apply (use `-` for stdin).
    #[arg(short, long)]
    pub file: PathBuf,
    /// Show the diff without applying it.
    #[arg(long)]
    pub diff: bool,
    /// Refuse silently ignoring prune requests; apply is additive-only until server batch support.
    #[arg(long)]
    pub prune: bool,
}
