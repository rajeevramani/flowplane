//! Dev-only support modules.
//!
//! This whole module is gated behind the `dev-oidc` cargo feature. It exists so
//! the control plane can stand up a local mock OIDC issuer in `AuthMode::Dev`
//! without requiring a real Zitadel. Production release builds that do not
//! enable `dev-oidc` should not pay any compile cost for anything in here.

pub mod oidc_server;
