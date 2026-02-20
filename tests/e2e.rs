//! E2E Test Suite
//!
//! Main entry point for the E2E test suite.
//!
//! ## Running Tests
//!
//! ```bash
//! # Run all e2e tests (requires RUN_E2E=1 and Envoy binary)
//! RUN_E2E=1 cargo test --test e2e -- --ignored --test-threads=1 --nocapture
//!
//! # Run specific test phase
//! RUN_E2E=1 cargo test --test e2e test_11 -- --ignored --nocapture
//!
//! # Run smoke tests only (faster)
//! RUN_E2E=1 cargo test --test e2e smoke -- --ignored --nocapture
//! ```
//!
//! ## Test Organization
//!
//! - `smoke/` - Fast tests (~30s-1min) for every commit
//! - `full/` - Comprehensive tests (5-10min) for PR/release
//! - `common/` - Shared test infrastructure
//!
//! ## Requirements
//!
//! - Set `RUN_E2E=1` environment variable
//! - Envoy binary on PATH (for proxy tests)
//! - wiremock dependency for mock services

// TLS support utilities (shared with tls tests)
#[path = "tls/support.rs"]
pub mod tls_support;

// Re-export as tls module for consistency with harness imports
pub mod tls {
    pub use super::tls_support as support;
}

// Shared test infrastructure
#[path = "e2e/common/mod.rs"]
pub mod common;

// Smoke tests (fast, run on every commit)
#[path = "e2e/smoke/mod.rs"]
mod smoke;

// Full test suite (comprehensive, run on PR/release)
#[path = "e2e/full/mod.rs"]
mod full;
