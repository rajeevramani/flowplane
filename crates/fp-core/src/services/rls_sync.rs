//! CP→RLS policy sync (feature fpv2-4ht, slice S5). Ports the v1 `rls_translator` model
//! (spec/04:345): the CP pushes the full team policy set to the RLS admin endpoint over HTTP on
//! a 60 s reconcile (level-triggered, self-healing — a missed change converges within the
//! window). Policies are namespaced `{org}|{team}|{domain}` with deterministic SHA-256-derived
//! org/team UUIDs so the RLS never trusts caller-supplied identity (spec/14 §5).
//!
//! [`namespace_uuid`] / [`compose_domain`] are the single source of the namespace: S7 composes
//! the Envoy filter `domain` with the SAME functions, so the pushed key and what Envoy sends
//! agree byte-for-byte (a divergence would silently disable enforcement).

use crate::authz::{check_resource_access, Decision, PrincipalCtx};
use crate::services::deny_to_error;
use fp_domain::authz::{Action, Resource};
use fp_domain::rate_limit::RateLimitUnit;
use fp_domain::{DomainError, DomainResult, OrgId, TeamId};
use fp_storage::repos::rate_limit;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCredential {
    token: String,
}

impl AdminCredential {
    pub fn new(token: String) -> DomainResult<Self> {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(DomainError::invalid_config(
                "RLS admin credential token must not be empty",
            ));
        }
        Ok(Self { token })
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

/// Deterministic SHA-256-derived UUID for an org/team id (spec/04:345). Stable, opaque, and
/// identical wherever it is computed — the namespace contract between the push (S5) and the
/// Envoy-domain composition (S7).
pub fn namespace_uuid(id: Uuid) -> Uuid {
    let digest = Sha256::digest(id.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Stamp a well-formed UUID: version 8 (custom) + RFC-4122 variant.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// The CP-owned Envoy/RLS namespace for a team's domain: `{org_uuid}|{team_uuid}|{domain}`.
pub fn compose_domain(org_id: OrgId, team_id: TeamId, domain: &str) -> String {
    format!(
        "{}|{}|{}",
        namespace_uuid(org_id.as_uuid()),
        namespace_uuid(team_id.as_uuid()),
        domain
    )
}

/// One policy in the push body. JSON shape MUST match `flowplane_rls::policy::PushedPolicy`
/// (the RLS receiver) — this is the HTTP contract boundary between the two crates.
#[derive(Debug, Clone, Serialize)]
pub struct PushPolicy {
    pub domain: String,
    pub descriptors: BTreeMap<String, String>,
    pub requests_per_unit: u64,
    pub unit: RateLimitUnit,
}

/// The full policy-set push body (`flowplane_rls::policy::PolicyPush`).
#[derive(Debug, Clone, Serialize)]
pub struct PushBody {
    pub policies: Vec<PushPolicy>,
}

/// Build the full namespaced policy set from the live rate-limit tables (all teams).
pub async fn build_push(pool: &PgPool) -> DomainResult<PushBody> {
    let rows = rate_limit::list_all_for_sync(pool).await?;
    let policies = rows
        .into_iter()
        .map(|row| PushPolicy {
            domain: compose_domain(row.org_id, row.team_id, &row.domain),
            descriptors: row.descriptors,
            requests_per_unit: row.requests_per_unit,
            unit: row.unit,
        })
        .collect();
    Ok(PushBody { policies })
}

/// Build and push the full set to the RLS admin endpoint. Returns the number of policies pushed.
pub async fn reconcile_once(
    pool: &PgPool,
    admin_url: &str,
    credential: Option<&AdminCredential>,
    client: &reqwest::Client,
) -> DomainResult<usize> {
    let body = build_push(pool).await?;
    push_policy_set(&body, admin_url, credential, client).await
}

/// Push an already-built full policy set to the RLS admin endpoint.
pub async fn push_policy_set(
    body: &PushBody,
    admin_url: &str,
    credential: Option<&AdminCredential>,
    client: &reqwest::Client,
) -> DomainResult<usize> {
    let count = body.policies.len();
    let url = format!(
        "{}/api/v1/admin/rls/policies",
        admin_url.trim_end_matches('/')
    );
    let mut request = client.post(&url).json(&body);
    if let Some(credential) = credential {
        request = request.bearer_auth(credential.token());
    }
    let resp = request
        .send()
        .await
        .map_err(|e| DomainError::unavailable(format!("rls policy push to {url} failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(DomainError::unavailable(format!(
            "rls policy push to {url} returned HTTP {}",
            resp.status()
        )));
    }
    Ok(count)
}

/// Authorize a force-repush trigger: platform-governance only (admin:all). The repush itself is
/// just an immediate reconcile kick; the 60 s loop is the correctness backstop.
pub fn authorize_repush(ctx: &PrincipalCtx) -> DomainResult<()> {
    match check_resource_access(ctx, Resource::Platform, Action::Execute, None) {
        Decision::Allow(_) => Ok(()),
        Decision::Deny(reason) => Err(deny_to_error(Resource::Platform, Action::Execute, reason)),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn namespace_uuid_is_deterministic_and_distinct() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        assert_eq!(namespace_uuid(a), namespace_uuid(a), "deterministic");
        assert_ne!(
            namespace_uuid(a),
            namespace_uuid(b),
            "distinct inputs differ"
        );
        // Derived, not the identity function — must not leak the raw id.
        assert_ne!(namespace_uuid(a), a);
    }

    #[test]
    fn namespace_uuid_is_well_formed_v8_rfc4122() {
        let u = namespace_uuid(Uuid::from_u128(42));
        let bytes = u.as_bytes();
        assert_eq!(bytes[6] & 0xf0, 0x80, "version 8");
        assert_eq!(bytes[8] & 0xc0, 0x80, "RFC-4122 variant");
    }

    #[test]
    fn compose_domain_has_two_separators_and_trailing_domain() {
        let org = OrgId::from(Uuid::from_u128(1));
        let team = TeamId::from(Uuid::from_u128(2));
        let composed = compose_domain(org, team, "checkout");
        let parts: Vec<&str> = composed.split('|').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[2], "checkout");
        assert_eq!(parts[0], namespace_uuid(org.as_uuid()).to_string());
        assert_eq!(parts[1], namespace_uuid(team.as_uuid()).to_string());
        // Two teams in the same org get distinct prefixes.
        let team2 = TeamId::from(Uuid::from_u128(3));
        assert_ne!(composed, compose_domain(org, team2, "checkout"));
    }
}
