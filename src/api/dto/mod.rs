//! Data Transfer Objects (DTOs) for API layer
//!
//! This module contains DTOs that define the external API contract.
//! DTOs are separate from domain entities and infrastructure types,
//! providing a stable API interface that can evolve independently.
//!
//! ## Architecture
//!
//! ```text
//! HTTP Request → DTO → Domain Entity → Repository → Database
//! HTTP Response ← DTO ← Domain Entity ← Repository ← Database
//! ```
//!
//! ## Guidelines
//!
//! - DTOs use serde for serialization/deserialization
//! - DTOs include validation via validator crate
//! - DTOs use utoipa for OpenAPI schema generation
//! - DTOs are separate from domain types to allow API evolution
//! - Conversion layers (From/Into traits) bridge DTOs and domain types

pub mod cluster;
pub mod listener;
pub mod route;

// Re-export main DTO types
pub use cluster::{
    CircuitBreakerThresholdsDto, CircuitBreakersDto, CreateClusterDto, EndpointDto, HealthCheckDto,
    OutlierDetectionDto, UpdateClusterDto,
};
pub use listener::{
    CreateListenerDto, ListenerAccessLogDto, ListenerFilterChainDto, ListenerFilterDto,
    ListenerFilterTypeDto, ListenerResponseDto, ListenerTlsContextDto, ListenerTracingDto,
    UpdateListenerDto,
};
pub use route::{
    HeaderMatchDefinitionDto, PathMatchDefinitionDto, QueryParameterMatchDefinitionDto,
    RouteActionDefinitionDto, RouteDefinitionDto, RouteMatchDefinitionDto, RouteResponseDto,
    RouteRuleDefinitionDto, VirtualHostDefinitionDto, WeightedClusterDefinitionDto,
};
