//! Flowplane xDS subsystem (spec/10 §5): domain snapshot → Envoy protos → ADS streams.
//! Translation is deterministic by construction — inputs are sorted, no HashMap iteration
//! reaches any encoded output (kills v1's version-churn class, spec/04 §8.6).

pub mod ads;
pub mod capture;
pub mod diagnostics;
pub mod server;
pub mod snapshot;
pub mod translate;

// Implementors of fp-xds async boundaries should not need a direct tonic dependency merely for
// the proc macro that defines those boundaries.
pub use tonic::async_trait;
