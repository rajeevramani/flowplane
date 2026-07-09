//! flowplane-rls: the first-party global rate-limit service (feature fpv2-4ht, slice S4).
//!
//! A standalone process — separate from the control plane (spec/14 §1.3: request-path traffic
//! must not pass through the CP). It answers Envoy's
//! `envoy.service.ratelimit.v3.RateLimitService/ShouldRateLimit` gRPC, enforcing team policies
//! against an in-memory fixed-window counter, and receives the team policy set over an HTTP
//! admin endpoint pushed by the CP `rls_sync` worker (S5).
//!
//! Isolation note: the RLS treats the request `domain` as an opaque namespace. The CP composes
//! it as `{org_id}|{team_id}|{domain}` (S7) and pushes policies keyed by the same string, so the
//! counter key — `domain` + `descriptors_canonical` + window — never crosses tenants. The RLS
//! never trusts a caller-supplied descriptor as identity (spec/14 §5).
//!
//! Documented MVP semantics (design pillar 2): in-memory counters reset on restart, and a
//! horizontally-scaled RLS over-admits by ~N. 1.1.0 ships a single instance; a Redis-backed
//! `CounterStore` is a named follow-on.

pub mod admin;
pub mod config;
pub mod counter;
pub mod grpc;
pub mod policy;
pub mod server;
