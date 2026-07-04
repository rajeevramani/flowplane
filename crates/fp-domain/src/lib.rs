//! Flowplane domain model.
//!
//! Pure types only: no async, no IO, no SQL (spec/10 §2). Everything observable by other
//! crates — errors, identifiers, lifecycle states, event types — originates here so all
//! surfaces speak one language.

pub mod ai;
pub mod api_lifecycle;
pub mod authz;
pub mod dataplane;
pub mod discovery;
pub mod error;
pub mod event;
pub mod gateway;
pub mod id;
pub mod identity;
pub mod learning;
pub mod rate_limit;
pub mod route_generation;
pub mod secret;

pub use ai::{
    openai_usage_from_json, prepare_openai_chat_request, rewrite_openai_chat_request_model,
    strip_synthetic_openai_usage_sse, validate_ai_budget_name, validate_ai_provider_name,
    validate_ai_route_name, validate_trace_ttl_days, AiBudget, AiBudgetMode, AiBudgetSpec,
    AiProvider, AiProviderKind, AiProviderSpec, AiRetentionPolicy, AiRoute, AiRouteBackend,
    AiRouteMaterializedResources, AiRouteSpec, AiRouteStatus, AiTraceEvent, AiUsageSummary,
    OpenAiChatRequest, OpenAiTokenUsage, AI_MODEL_HEADER, DEFAULT_AI_ROUTE_TIMEOUT_SECS,
    MAX_AI_REQUEST_BODY_BYTES, MAX_AI_TRACE_TTL_DAYS,
};
pub use dataplane::{validate_spiffe_uri, Dataplane, ProxyCertificate, TeamStatsOverview};
pub use discovery::{
    cluster_discovery_observations, DiscoveryCandidateCluster, DiscoveryObservation,
    DiscoveryObservationKey, DiscoveryObservationProvenance, DiscoverySession,
    DiscoverySessionSpec, DiscoverySessionStatus,
};
pub use error::{DomainError, DomainResult, ErrorCode};
pub use id::{
    AgentId, AiBudgetId, AiProviderId, AiRouteId, ApiDefinitionId, ApiRouteBindingId, ApiToolId,
    AuditEntryId, CaptureSessionId, ClusterId, DataplaneId, DiscoverySessionId, GrantId,
    ListenerId, MembershipId, OrgId, ProxyCertificateId, RateLimitDomainId, RateLimitPolicyId,
    RateLimitTeamOverrideId, RawObservationId, RequestId, RetentionPolicyId, RouteConfigId,
    RouteGenerationPlanId, SecretId, SpecVersionId, SpecVersionReviewEventId, TeamId, UserId,
};
pub use identity::{
    validate_name, Agent, AgentKind, EntityStatus, OrgRole, Organization, Team, User,
};
pub use rate_limit::{
    descriptors_canonical, validate_rate_limit_domain_name, validate_rate_limit_policy_name,
    RateLimitDomain, RateLimitPolicy, RateLimitPolicySpec, RateLimitTeamOverride,
    RateLimitTeamOverrideSpec, RateLimitUnit,
};
pub use route_generation::{
    RouteGenerationPlan, RouteGenerationPlanSpec, RouteGenerationPlanStatus,
};
pub use secret::{Secret, SecretSpec, SecretType};
