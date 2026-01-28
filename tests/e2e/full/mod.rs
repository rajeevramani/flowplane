//! Full E2E test suite
//!
//! These tests are comprehensive and may take 5-10 minutes to run.
//! Run for PR merge to main, release branches, and nightly CI.
//!
//! ```bash
//! RUN_E2E=1 cargo test --test e2e_v2 -- --ignored --test-threads=1
//! ```

pub mod test_11_bootstrap;
pub mod test_12_header_mutation;
pub mod test_13_jwt_auth;
pub mod test_14_retry_policies;
pub mod test_15_circuit_breakers;
pub mod test_16_rate_limit;
pub mod test_17_compression;
pub mod test_18_outlier_detection;
pub mod test_19_custom_response;
pub mod test_20_ext_authz;
pub mod test_21_cors;
pub mod test_22_wasm;
pub mod test_23_oauth2;
