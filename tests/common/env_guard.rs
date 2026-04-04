//! RAII guard that restores environment variables on drop.
//!
//! Use in test code that mutates process-wide env vars to prevent leaking
//! state into other tests running in the same process.
//!
//! # Safety
//!
//! Environment variable mutation is inherently process-global and not
//! thread-safe. Tests using `EnvGuard` should be serialized with a mutex
//! (e.g. `static ENV_MUTEX: Mutex<()>`) to prevent races.

#![allow(dead_code)]

use std::sync::Mutex;

/// Global mutex for tests that mutate environment variables.
/// Acquire this before using EnvGuard to prevent races when `cargo test`
/// runs tests in parallel within the same process.
pub static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// RAII guard that saves original env var values and restores them on drop.
pub struct EnvGuard {
    originals: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    /// Create a new empty guard.
    pub fn new() -> Self {
        Self { originals: vec![] }
    }

    /// Set an env var, saving the original value for restoration on drop.
    pub fn set(&mut self, key: &str, value: &str) {
        let original = std::env::var(key).ok();
        self.originals.push((key.to_string(), original));
        std::env::set_var(key, value);
    }

    /// Remove an env var, saving the original value for restoration on drop.
    pub fn remove(&mut self, key: &str) {
        let original = std::env::var(key).ok();
        self.originals.push((key.to_string(), original));
        std::env::remove_var(key);
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Restore in reverse order so nested mutations are unwound correctly.
        for (key, original) in self.originals.iter().rev() {
            match original {
                Some(val) => std::env::set_var(key, val),
                None => std::env::remove_var(key),
            }
        }
    }
}
