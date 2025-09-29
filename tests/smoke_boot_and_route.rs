// Thin wrapper to expose the nested e2e smoke test as a Cargo integration test target.
// This allows running: cargo test --test smoke_boot_and_route

#[path = "e2e/smoke_boot_and_route.rs"]
mod e2e_smoke;
