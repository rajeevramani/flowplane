//! Business logic services
//!
//! This module contains service layer components that encapsulate
//! business logic, separated from HTTP concerns.

pub mod access_log_processor;
pub mod cluster_service;
pub mod filter_service;
pub mod learning_session_service;
pub mod listener_filter_chain;
pub mod listener_service;
pub mod path_normalizer;
pub mod route_service;
pub mod schema_aggregator;
pub mod schema_diff;
pub mod webhook_service;

pub use access_log_processor::{AccessLogProcessor, ProcessorConfig, ProcessorHandle};
pub use cluster_service::ClusterService;
pub use filter_service::FilterService;
pub use learning_session_service::LearningSessionService;
pub use listener_service::ListenerService;
pub use path_normalizer::{normalize_path, PathNormalizationConfig};
pub use route_service::RouteService;
pub use schema_aggregator::SchemaAggregator;
pub use schema_diff::{detect_breaking_changes, BreakingChange, BreakingChangeType, SchemaDiff};
pub use webhook_service::{LearningSessionWebhookEvent, WebhookEndpoint, WebhookService};
