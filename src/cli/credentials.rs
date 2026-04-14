//! CLI credential helpers — re-exports from [`crate::auth::dev_token`].
//!
//! The core implementation lives in `src/auth/dev_token.rs`. This module
//! provides convenient access from the CLI layer.

pub use crate::auth::dev_token::{
    read_credentials_file, read_credentials_from_path, write_credentials_file,
    write_credentials_to_path,
};
