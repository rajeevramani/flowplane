//! First-party global rate-limit policy model (spec/03 §3.7, feature fpv2-4ht, slice S1).
//!
//! Pure types only — no IO, no SQL (spec/10 §2). Three team-owned entities form a chain:
//! a [`RateLimitDomain`] groups [`RateLimitPolicy`] rows (each a descriptor-set ->
//! `requests_per_unit` + [`RateLimitUnit`]), and a [`RateLimitTeamOverride`] replaces a
//! policy's limit for the team.
//!
//! The descriptor match key is canonicalized here ([`descriptors_canonical`]) — the
//! deterministic sorted-key form (spec/03:485) — so RLS lookup and the stored unique
//! constraint agree. Computing it in Rust (not a SQL trigger) keeps the rule unit-testable
//! and the storage layer a dumb writer (decision recorded in FP-DEC for this feature).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DomainError, DomainResult};
use crate::id::{RateLimitDomainId, RateLimitPolicyId, RateLimitTeamOverrideId, TeamId};
use crate::identity::validate_name;

/// The window a limit is measured over. Serialized lowercase; matches the DB CHECK and the
/// Envoy `RateLimitUnit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RateLimitUnit {
    Second,
    Minute,
    Hour,
    Day,
}

impl RateLimitUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Second => "second",
            Self::Minute => "minute",
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }

    /// Window length in seconds — the fixed-window size the RLS counter store uses.
    pub fn window_seconds(self) -> u64 {
        match self {
            Self::Second => 1,
            Self::Minute => 60,
            Self::Hour => 3_600,
            Self::Day => 86_400,
        }
    }
}

impl std::str::FromStr for RateLimitUnit {
    type Err = DomainError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "second" => Ok(Self::Second),
            "minute" => Ok(Self::Minute),
            "hour" => Ok(Self::Hour),
            "day" => Ok(Self::Day),
            _ => Err(DomainError::validation(format!(
                "\"{raw}\" is not a rate-limit unit (expected second|minute|hour|day)"
            ))),
        }
    }
}

// ---- Domain -----------------------------------------------------------------------------

/// A rate-limit domain: the user-facing group an operator names their limits under. The
/// `name` carries the policy `domain` string (1–253 chars, spec/02:329).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimitDomain {
    pub id: RateLimitDomainId,
    pub team_id: TeamId,
    pub name: String,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Max length of a policy `domain` (spec/02:329). NB: this is the *user* domain on the row,
/// not the CP-composed Envoy filter domain (S7), which is longer.
pub const MAX_POLICY_DOMAIN_LEN: usize = 253;

/// Validate a domain `name`: a non-empty user string up to 253 chars (spec/02:329). Looser
/// than [`validate_name`] because a domain is free-text the operator chooses, not a slug.
pub fn validate_rate_limit_domain_name(name: &str) -> DomainResult<()> {
    if name.is_empty() {
        return Err(DomainError::validation(
            "rate-limit domain must not be empty",
        ));
    }
    if name.chars().count() > MAX_POLICY_DOMAIN_LEN {
        return Err(DomainError::validation(format!(
            "rate-limit domain must be at most {MAX_POLICY_DOMAIN_LEN} characters"
        )));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(DomainError::validation(
            "rate-limit domain must not contain control characters",
        ));
    }
    Ok(())
}

// ---- Policy -----------------------------------------------------------------------------

/// One policy: a descriptor-set maps to `requests_per_unit` over a [`RateLimitUnit`] window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimitPolicy {
    pub id: RateLimitPolicyId,
    pub team_id: TeamId,
    pub domain_id: RateLimitDomainId,
    pub name: String,
    pub spec: RateLimitPolicySpec,
    /// Deterministic sorted-key form of `spec.descriptors` (spec/03:485); the match key.
    pub descriptors_canonical: String,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The mutable policy payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RateLimitPolicySpec {
    /// Flat string->string descriptor set the Envoy route emits and this policy matches.
    pub descriptors: BTreeMap<String, String>,
    pub requests_per_unit: u64,
    pub unit: RateLimitUnit,
}

/// Max descriptor entries in one policy — bounds the canonical key and matches the route-side
/// cap (`route_config.rs` allows up to 16 actions per definition).
pub const MAX_DESCRIPTOR_ENTRIES: usize = 16;

impl RateLimitPolicySpec {
    pub fn validate(&self) -> DomainResult<()> {
        if self.requests_per_unit == 0 {
            return Err(DomainError::validation(
                "rate-limit requests_per_unit must be greater than zero",
            ));
        }
        if self.descriptors.is_empty() {
            return Err(DomainError::validation(
                "rate-limit policy must carry at least one descriptor",
            ));
        }
        if self.descriptors.len() > MAX_DESCRIPTOR_ENTRIES {
            return Err(DomainError::validation(format!(
                "rate-limit policy must carry at most {MAX_DESCRIPTOR_ENTRIES} descriptors"
            )));
        }
        for (key, value) in &self.descriptors {
            if key.is_empty() || value.is_empty() {
                return Err(DomainError::validation(
                    "rate-limit descriptor keys and values must be non-empty",
                ));
            }
            if key.contains(CANONICAL_PAIR_SEP)
                || key.contains(CANONICAL_KV_SEP)
                || value.contains(CANONICAL_PAIR_SEP)
                || value.contains(CANONICAL_KV_SEP)
            {
                return Err(DomainError::validation(
                    "rate-limit descriptor keys/values must not contain control separators",
                ));
            }
            if key.chars().any(|c| c.is_control()) || value.chars().any(|c| c.is_control()) {
                return Err(DomainError::validation(
                    "rate-limit descriptor keys/values must not contain control characters",
                ));
            }
        }
        Ok(())
    }

    /// The canonical match key for this descriptor set. Stable across insertion order because
    /// [`BTreeMap`] iterates sorted; safe to use as a unique key and an RLS lookup key.
    pub fn descriptors_canonical(&self) -> String {
        descriptors_canonical(&self.descriptors)
    }
}

/// Validate a policy `name` (the CRUD handle within a domain) — a slug, [`validate_name`].
pub fn validate_rate_limit_policy_name(name: &str) -> DomainResult<()> {
    validate_name(name)
}

// Separators chosen to be control characters that descriptor keys/values are forbidden from
// containing (validated above), so the canonical form is unambiguous.
const CANONICAL_KV_SEP: char = '\u{1f}'; // unit separator, between key and value
const CANONICAL_PAIR_SEP: char = '\u{1e}'; // record separator, between pairs

/// Deterministic sorted-key serialization of a descriptor map (spec/03:485). Two maps with
/// the same entries produce the same string regardless of insertion order.
pub fn descriptors_canonical(descriptors: &BTreeMap<String, String>) -> String {
    descriptors
        .iter()
        .map(|(k, v)| format!("{k}{CANONICAL_KV_SEP}{v}"))
        .collect::<Vec<_>>()
        .join(&CANONICAL_PAIR_SEP.to_string())
}

// ---- Team override ----------------------------------------------------------------------

/// A per-team override of a policy's limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimitTeamOverride {
    pub id: RateLimitTeamOverrideId,
    pub team_id: TeamId,
    pub policy_id: RateLimitPolicyId,
    pub spec: RateLimitTeamOverrideSpec,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The mutable override payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RateLimitTeamOverrideSpec {
    pub requests_per_unit: u64,
}

impl RateLimitTeamOverrideSpec {
    pub fn validate(&self) -> DomainResult<()> {
        if self.requests_per_unit == 0 {
            return Err(DomainError::validation(
                "rate-limit override requests_per_unit must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn descriptors(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn unit_round_trips_and_window_is_correct() {
        for (unit, secs) in [
            (RateLimitUnit::Second, 1),
            (RateLimitUnit::Minute, 60),
            (RateLimitUnit::Hour, 3_600),
            (RateLimitUnit::Day, 86_400),
        ] {
            assert_eq!(RateLimitUnit::from_str(unit.as_str()).unwrap(), unit);
            assert_eq!(unit.window_seconds(), secs);
        }
        assert!(RateLimitUnit::from_str("week").is_err());
    }

    #[test]
    fn canonical_is_insertion_order_independent() {
        let a = descriptors(&[("client_id", "bob"), ("path", "/x")]);
        let mut b = BTreeMap::new();
        b.insert("path".to_string(), "/x".to_string());
        b.insert("client_id".to_string(), "bob".to_string());
        assert_eq!(descriptors_canonical(&a), descriptors_canonical(&b));
    }

    #[test]
    fn canonical_distinguishes_different_sets() {
        let a = descriptors(&[("client_id", "bob")]);
        let b = descriptors(&[("client_id", "alice")]);
        let c = descriptors(&[("client_id", "bob"), ("path", "/x")]);
        assert_ne!(descriptors_canonical(&a), descriptors_canonical(&b));
        assert_ne!(descriptors_canonical(&a), descriptors_canonical(&c));
    }

    #[test]
    fn policy_spec_rejects_zero_and_empty() {
        let zero = RateLimitPolicySpec {
            descriptors: descriptors(&[("k", "v")]),
            requests_per_unit: 0,
            unit: RateLimitUnit::Minute,
        };
        assert!(zero.validate().is_err());

        let empty = RateLimitPolicySpec {
            descriptors: BTreeMap::new(),
            requests_per_unit: 10,
            unit: RateLimitUnit::Minute,
        };
        assert!(empty.validate().is_err());
    }

    #[test]
    fn policy_spec_rejects_too_many_and_blank_descriptors() {
        let too_many: BTreeMap<String, String> = (0..=MAX_DESCRIPTOR_ENTRIES)
            .map(|i| (format!("k{i}"), "v".to_string()))
            .collect();
        let spec = RateLimitPolicySpec {
            descriptors: too_many,
            requests_per_unit: 1,
            unit: RateLimitUnit::Second,
        };
        assert!(spec.validate().is_err());

        let blank = RateLimitPolicySpec {
            descriptors: descriptors(&[("", "v")]),
            requests_per_unit: 1,
            unit: RateLimitUnit::Second,
        };
        assert!(blank.validate().is_err());
    }

    #[test]
    fn policy_spec_rejects_separator_injection() {
        let sneaky = RateLimitPolicySpec {
            descriptors: descriptors(&[("k", "a\u{1f}b")]),
            requests_per_unit: 1,
            unit: RateLimitUnit::Second,
        };
        assert!(sneaky.validate().is_err());
    }

    #[test]
    fn valid_policy_spec_passes() {
        let spec = RateLimitPolicySpec {
            descriptors: descriptors(&[("client_id", "bob")]),
            requests_per_unit: 100,
            unit: RateLimitUnit::Minute,
        };
        assert!(spec.validate().is_ok());
        assert_eq!(spec.descriptors_canonical(), "client_id\u{1f}bob");
    }

    #[test]
    fn domain_name_bounds() {
        assert!(validate_rate_limit_domain_name("checkout").is_ok());
        assert!(validate_rate_limit_domain_name("").is_err());
        assert!(validate_rate_limit_domain_name(&"x".repeat(254)).is_err());
        assert!(validate_rate_limit_domain_name(&"x".repeat(253)).is_ok());
        assert!(validate_rate_limit_domain_name("bad\nname").is_err());
    }

    #[test]
    fn override_spec_rejects_zero() {
        assert!(RateLimitTeamOverrideSpec {
            requests_per_unit: 0
        }
        .validate()
        .is_err());
        assert!(RateLimitTeamOverrideSpec {
            requests_per_unit: 5
        }
        .validate()
        .is_ok());
    }
}
