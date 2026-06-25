//! The in-RAM policy set the RLS enforces against, plus the wire format the CP pushes.
//!
//! The push is a full snapshot (level-triggered: the CP's 60 s reconcile re-sends the whole
//! set, so a missed delta self-heals — design pillar 4). `replace` swaps the set atomically.

use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;

use fp_domain::rate_limit::{descriptors_canonical, RateLimitUnit};
use serde::{Deserialize, Serialize};

/// One policy as pushed by the CP `rls_sync` worker (S5).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PushedPolicy {
    /// CP-composed namespace `{org_id}|{team_id}|{domain}` — opaque identity to the RLS.
    pub domain: String,
    /// The flat descriptor set this policy matches (canonicalized on ingest).
    pub descriptors: BTreeMap<String, String>,
    pub requests_per_unit: u64,
    pub unit: RateLimitUnit,
}

/// The admin push body: the complete policy set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct PolicyPush {
    pub policies: Vec<PushedPolicy>,
}

/// A policy resolved for enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchedPolicy {
    pub requests_per_unit: u64,
    pub unit: RateLimitUnit,
}

/// The enforced policy set: `domain -> (descriptors_canonical -> policy)`.
#[derive(Default)]
pub struct PolicyCache {
    inner: RwLock<HashMap<String, HashMap<String, MatchedPolicy>>>,
}

impl PolicyCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Atomically replace the whole policy set. Canonicalizes each policy's descriptors with the
    /// shared `fp_domain` function (FP-DEC-0002) so the lookup key matches what the storage/CP
    /// side computed. A later duplicate `(domain, canonical)` in the push overwrites an earlier
    /// one (the CP guarantees uniqueness, so this is only defensive).
    pub fn replace(&self, push: PolicyPush) {
        let mut next: HashMap<String, HashMap<String, MatchedPolicy>> = HashMap::new();
        for policy in push.policies {
            let canonical = descriptors_canonical(&policy.descriptors);
            next.entry(policy.domain).or_default().insert(
                canonical,
                MatchedPolicy {
                    requests_per_unit: policy.requests_per_unit,
                    unit: policy.unit,
                },
            );
        }
        let mut guard = match self.inner.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = next;
    }

    /// Resolve a policy by namespace + canonical descriptor key. `None` means no policy matches
    /// (the request is not limited) — including the never-synced / unknown-namespace case.
    pub fn lookup(&self, domain: &str, descriptors_canonical: &str) -> Option<MatchedPolicy> {
        let guard = match self.inner.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .get(domain)
            .and_then(|by_desc| by_desc.get(descriptors_canonical))
            .copied()
    }

    /// Total policies currently loaded (for /readyz introspection and tests).
    pub fn policy_count(&self) -> usize {
        let guard = match self.inner.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.values().map(HashMap::len).sum()
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn policy(domain: &str, pairs: &[(&str, &str)], rpu: u64) -> PushedPolicy {
        PushedPolicy {
            domain: domain.to_string(),
            descriptors: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            requests_per_unit: rpu,
            unit: RateLimitUnit::Minute,
        }
    }

    #[test]
    fn replace_then_lookup_by_canonical() {
        let cache = PolicyCache::new();
        cache.replace(PolicyPush {
            policies: vec![policy("orgA|teamA|checkout", &[("client_id", "bob")], 100)],
        });
        let canonical = descriptors_canonical(
            &[("client_id".to_string(), "bob".to_string())]
                .into_iter()
                .collect(),
        );
        let found = cache.lookup("orgA|teamA|checkout", &canonical);
        assert_eq!(found.map(|p| p.requests_per_unit), Some(100));
        assert_eq!(cache.policy_count(), 1);
    }

    #[test]
    fn lookup_misses_on_unknown_namespace_or_descriptor() {
        let cache = PolicyCache::new();
        cache.replace(PolicyPush {
            policies: vec![policy("orgA|teamA|checkout", &[("client_id", "bob")], 100)],
        });
        let canonical = descriptors_canonical(
            &[("client_id".to_string(), "bob".to_string())]
                .into_iter()
                .collect(),
        );
        // Wrong namespace (different team) -> no match: isolation by construction.
        assert!(cache.lookup("orgA|teamB|checkout", &canonical).is_none());
        // Unknown descriptor -> no match.
        assert!(cache
            .lookup("orgA|teamA|checkout", "client_id\u{1f}alice")
            .is_none());
    }

    #[test]
    fn replace_swaps_the_whole_set() {
        let cache = PolicyCache::new();
        cache.replace(PolicyPush {
            policies: vec![policy("d1", &[("k", "v")], 1)],
        });
        cache.replace(PolicyPush {
            policies: vec![policy("d2", &[("k", "v")], 2)],
        });
        let canonical =
            descriptors_canonical(&[("k".to_string(), "v".to_string())].into_iter().collect());
        assert!(cache.lookup("d1", &canonical).is_none()); // old set gone
        assert_eq!(
            cache.lookup("d2", &canonical).map(|p| p.requests_per_unit),
            Some(2)
        );
    }
}
