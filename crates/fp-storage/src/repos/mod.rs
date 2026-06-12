//! Repositories. Tenant-table queries require a [`crate::scope::TeamScope`]; identity and
//! governance tables are org-keyed with the same explicitness.

pub mod audit;
pub mod bootstrap;
pub mod clusters;
pub mod gateway;
pub mod identity;
