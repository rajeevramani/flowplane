// NOTE: Requires PostgreSQL - disabled until Phase 4 of PostgreSQL migration
// To run these tests: cargo test --features postgres_tests
#![cfg(feature = "postgres_tests")]

#[path = "auth/contract/mod.rs"]
mod contract;
#[path = "auth/integration/mod.rs"]
mod integration;
#[path = "auth/support.rs"]
mod support;
#[path = "auth/unit/mod.rs"]
mod unit;
