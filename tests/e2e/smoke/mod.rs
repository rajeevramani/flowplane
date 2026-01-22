//! Smoke tests for quick CI validation
//!
//! These tests run in ~30-60 seconds and validate core functionality:
//! - Bootstrap and auth flow
//! - Basic routing
//! - One filter end-to-end
//!
//! ```bash
//! RUN_E2E=1 cargo test --test smoke -- --ignored --nocapture
//! ```

pub mod test_bootstrap;
pub mod test_routing;
