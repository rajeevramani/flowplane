//! Repositories. Tenant-table queries require a [`crate::scope::TeamScope`]; identity and
//! governance tables are org-keyed with the same explicitness.

pub mod api_lifecycle;
pub mod audit;
pub mod bootstrap;
pub mod clusters;
pub mod dataplanes;
pub mod discovery;
pub mod gateway;
pub mod identity;
pub mod route_generation;
pub mod secrets;
pub mod xds_nacks;
