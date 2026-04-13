//! Deterministic dedup hash for diagnostic reports.
//!
//! The hash inputs are `(dataplane_id, resource_type, resource_name, error_details)`.
//! Envoy's `last_update_attempt` is **deliberately excluded** — Envoy retries
//! the same failing config on its own cadence and would otherwise bust dedup
//! every poll cycle, producing duplicate rows on the CP.

use sha2::{Digest, Sha256};

/// Compute the stable dedup hash for a diagnostic report.
///
/// The hash is SHA-256 over `dataplane_id | '\0' | kind | '\0' | name | '\0' | details`,
/// hex-encoded. NUL separators prevent the trivial collision
/// `("a","bc","","")` vs `("a","b","c","")`.
pub fn compute_dedup_hash(dataplane_id: &str, kind: &str, name: &str, details: &str) -> String {
    let mut h = Sha256::new();
    h.update(dataplane_id.as_bytes());
    h.update([0u8]);
    h.update(kind.as_bytes());
    h.update([0u8]);
    h.update(name.as_bytes());
    h.update([0u8]);
    h.update(details.as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_across_calls() {
        let a = compute_dedup_hash("dp-1", "listener", "my-l", "err");
        let b = compute_dedup_hash("dp-1", "listener", "my-l", "err");
        assert_eq!(a, b);
    }

    #[test]
    fn changing_dataplane_id_changes_hash() {
        let a = compute_dedup_hash("dp-1", "listener", "my-l", "err");
        let b = compute_dedup_hash("dp-2", "listener", "my-l", "err");
        assert_ne!(a, b);
    }

    #[test]
    fn changing_kind_changes_hash() {
        let a = compute_dedup_hash("dp-1", "listener", "my-l", "err");
        let b = compute_dedup_hash("dp-1", "cluster", "my-l", "err");
        assert_ne!(a, b);
    }

    #[test]
    fn changing_name_changes_hash() {
        let a = compute_dedup_hash("dp-1", "listener", "my-l", "err");
        let b = compute_dedup_hash("dp-1", "listener", "other-l", "err");
        assert_ne!(a, b);
    }

    #[test]
    fn changing_details_changes_hash() {
        let a = compute_dedup_hash("dp-1", "listener", "my-l", "err1");
        let b = compute_dedup_hash("dp-1", "listener", "my-l", "err2");
        assert_ne!(a, b);
    }

    #[test]
    fn null_byte_separators_prevent_boundary_collisions() {
        // Without NUL separators, ("ab","") and ("a","b") as the last two
        // fields would collide. We assert they do NOT.
        let a = compute_dedup_hash("dp", "listener", "ab", "");
        let b = compute_dedup_hash("dp", "listener", "a", "b");
        assert_ne!(a, b);
    }

    #[test]
    fn last_update_attempt_has_no_input_so_no_effect() {
        // Sanity: the function does not accept last_update_attempt at all,
        // so different attempt timestamps for the same failing config
        // produce the same hash by construction. This test documents the
        // design intent.
        let a = compute_dedup_hash("dp-1", "listener", "my-l", "err");
        let b = compute_dedup_hash("dp-1", "listener", "my-l", "err");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_is_hex_encoded_64_chars() {
        let h = compute_dedup_hash("dp", "listener", "n", "d");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
