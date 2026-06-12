//! Flowplane xDS subsystem (spec/10 §5): domain snapshot → Envoy protos → ADS streams.
//! Translation is deterministic by construction — inputs are sorted, no HashMap iteration
//! reaches any encoded output (kills v1's version-churn class, spec/04 §8.6).

pub mod translate;
