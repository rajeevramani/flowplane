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
pub mod test_cli_deletes;
pub mod test_cli_filter_ops;
pub mod test_cli_ops;
pub mod test_cli_phase3;
pub mod test_cli_phase3_prod;
pub mod test_cli_phase4;
pub mod test_cli_phase5;
pub mod test_cli_subprocess;
pub mod test_dev_mode_smoke;
pub mod test_prod_mode_smoke;
pub mod test_routing;
pub mod test_sds_delivery;
