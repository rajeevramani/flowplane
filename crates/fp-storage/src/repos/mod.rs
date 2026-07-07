//! Repositories. Tenant-table queries require a [`crate::scope::TeamScope`]; identity and
//! governance tables are org-keyed with the same explicitness.

pub mod ai;
pub mod ai_trace;
pub mod api_lifecycle;
pub mod audit;
pub mod bootstrap;
pub mod clusters;
pub mod dataplanes;
pub mod discovery;
pub mod gateway;
pub mod identity;
mod observation_ingest;
pub mod rate_limit;
pub mod route_generation;
pub mod secrets;
pub mod xds_nacks;
