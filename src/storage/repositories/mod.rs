//! Repository modules for data access
//!
//! This module provides repository implementations split into focused, manageable files.
//! Each repository handles CRUD operations for a specific resource type.

pub mod aggregated_schema;
pub mod api_definition;
pub mod audit_log;
pub mod cluster;
pub mod inferred_schema;
pub mod learning_session;
pub mod listener;
pub mod reporting;
pub mod route;
pub mod team;
pub mod token;
pub mod user;

// Re-export all repository types and their associated request/response types
pub use aggregated_schema::{
    AggregatedSchemaData, AggregatedSchemaRepository, CreateAggregatedSchemaRequest,
};
pub use api_definition::{
    ApiDefinitionData, ApiDefinitionRepository, ApiRouteData, CreateApiDefinitionRequest,
    CreateApiRouteRequest, UpdateBootstrapMetadataRequest,
};
pub use audit_log::{AuditEvent, AuditLogEntry, AuditLogFilters, AuditLogRepository};
pub use cluster::{ClusterData, ClusterRepository, CreateClusterRequest, UpdateClusterRequest};
pub use inferred_schema::{InferredSchemaData, InferredSchemaRepository};
pub use learning_session::{
    CreateLearningSessionRequest, LearningSessionData, LearningSessionRepository,
    LearningSessionStatus, UpdateLearningSessionRequest,
};
pub use listener::{
    CreateListenerRequest, ListenerData, ListenerRepository, UpdateListenerRequest,
};
pub use reporting::{ReportingRepository, RouteFlowRow};
pub use route::{CreateRouteRequest, RouteData, RouteRepository, UpdateRouteRequest};
pub use team::{SqlxTeamRepository, TeamRepository};
pub use token::{SqlxTokenRepository, TokenRepository};
pub use user::{
    SqlxTeamMembershipRepository, SqlxUserRepository, TeamMembershipRepository, UserRepository,
};
