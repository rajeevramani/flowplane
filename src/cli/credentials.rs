//! CLI credential helpers — re-exports from [`crate::auth::dev_token`].
//!
//! The core implementation lives in `src/auth/dev_token.rs`. This module
//! provides convenient access from the CLI layer.

pub use crate::auth::dev_token::{
    read_credentials_file, resolve_or_generate_dev_token, write_credentials_file,
};
