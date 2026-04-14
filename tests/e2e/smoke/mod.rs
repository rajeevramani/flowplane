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
pub mod test_cli_admin;
pub mod test_cli_dataplane;
pub mod test_cli_deletes;
pub mod test_cli_expose;
pub mod test_cli_expose_prod;
pub mod test_cli_filter_ops;
pub mod test_cli_import;
pub mod test_cli_learn;
pub mod test_cli_local;
pub mod test_cli_mcp;
pub mod test_cli_negatives;
pub mod test_cli_ops;
pub mod test_cli_ops_envoy;
pub mod test_cli_reads;
pub mod test_cli_scaffold;
pub mod test_cli_secrets;
pub mod test_cli_security_reads;
pub mod test_cli_subprocess;
pub mod test_cli_team_org;
pub mod test_cli_views;
pub mod test_cli_wasm;
pub mod test_dev_mode_smoke;
pub mod test_dev_mtls_chain;
pub mod test_dev_mtls_docker;
pub mod test_prod_mode_smoke;
pub mod test_routing;
pub mod test_sds_delivery;
