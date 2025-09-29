// Thin wrapper to expose the nested e2e no-NACKs test as a Cargo integration test target.
// This allows running: cargo test --test smoke_no_nacks

#[path = "e2e/smoke_no_nacks.rs"]
mod e2e_no_nacks;
