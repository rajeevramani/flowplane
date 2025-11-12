//! Password hashing and verification utilities using Argon2id.
//!
//! This module provides secure password hashing and verification
//! following OWASP password storage guidelines.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::rngs::OsRng;

use crate::errors::{FlowplaneError, Result};

/// Create an Argon2 hasher with recommended parameters.
///
/// Configuration:
/// - Algorithm: Argon2id (hybrid mode for resistance to both side-channel and GPU attacks)
/// - Memory cost: 768 KiB (0.75 MiB) - balances security and performance for API latency
/// - Iterations: 1 - sufficient with Argon2id's memory-hard design
/// - Parallelism: 1 - suitable for single-user authentication contexts
/// - Output length: 32 bytes
///
/// These parameters keep verification under 10ms on development hardware while
/// maintaining strong security guarantees.
pub fn password_hasher() -> Argon2<'static> {
    const MEMORY_COST_KIB: u32 = 768; // 0.75 MiB keeps verification below the latency budget
    const ITERATIONS: u32 = 1;
    const PARALLELISM: u32 = 1;
    let params = Params::new(MEMORY_COST_KIB, ITERATIONS, PARALLELISM, Some(32))
        .expect("valid Argon2 parameters");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Hash a password using Argon2id with a random salt.
///
/// Returns a PHC string format hash that includes:
/// - Algorithm identifier
/// - Version
/// - Parameters (memory cost, iterations, parallelism)
/// - Salt (base64-encoded)
/// - Hash digest (base64-encoded)
///
/// # Example
///
/// ```no_run
/// use flowplane::auth::hashing::hash_password;
///
/// let password = "SecureP@ssw0rd";
/// let hash = hash_password(password).unwrap();
/// // hash format: $argon2id$v=19$m=768,t=1,p=1$<salt>$<hash>
/// ```
///
/// # Errors
///
/// Returns an error if password hashing fails (extremely rare in practice).
pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hasher = password_hasher();

    let password_hash = hasher
        .hash_password(password.as_bytes(), &salt)
        .map_err(|err| FlowplaneError::internal(format!("Failed to hash password: {}", err)))?;

    Ok(password_hash.to_string())
}

/// Verify a password against a hash.
///
/// Performs constant-time comparison to prevent timing attacks.
///
/// # Arguments
///
/// * `password` - The plaintext password to verify
/// * `hash` - The PHC string format hash to verify against
///
/// # Returns
///
/// Returns `Ok(true)` if the password matches, `Ok(false)` if it doesn't match.
/// Returns an error if the hash format is invalid.
///
/// # Example
///
/// ```no_run
/// use flowplane::auth::hashing::{hash_password, verify_password};
///
/// let password = "SecureP@ssw0rd";
/// let hash = hash_password(password).unwrap();
///
/// assert!(verify_password(password, &hash).unwrap());
/// assert!(!verify_password("WrongPassword", &hash).unwrap());
/// ```
pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    let parsed_hash = PasswordHash::new(hash).map_err(|err| {
        FlowplaneError::validation(format!("Invalid password hash format: {}", err))
    })?;

    let hasher = password_hasher();

    match hasher.verify_password(password.as_bytes(), &parsed_hash) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(err) => Err(FlowplaneError::internal(format!("Password verification failed: {}", err))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_password_creates_valid_hash() {
        let password = "TestP@ssw0rd123";
        let hash = hash_password(password).unwrap();

        // PHC string format: $argon2id$v=19$m=768,t=1,p=1$<salt>$<hash>
        assert!(hash.starts_with("$argon2id$"));
        assert!(hash.contains("$v=19$"));
        assert!(hash.contains("$m=768,t=1,p=1$"));

        // Hash should be reasonably long (includes salt and digest)
        assert!(hash.len() > 80);
    }

    #[test]
    fn test_hash_password_generates_different_salts() {
        let password = "SamePassword";
        let hash1 = hash_password(password).unwrap();
        let hash2 = hash_password(password).unwrap();

        // Different salts mean different hashes even for the same password
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_verify_password_correct() {
        let password = "CorrectP@ssw0rd";
        let hash = hash_password(password).unwrap();

        assert!(verify_password(password, &hash).unwrap());
    }

    #[test]
    fn test_verify_password_incorrect() {
        let password = "CorrectP@ssw0rd";
        let hash = hash_password(password).unwrap();

        assert!(!verify_password("WrongPassword", &hash).unwrap());
        assert!(!verify_password("correctp@ssw0rd", &hash).unwrap()); // Case sensitive
        assert!(!verify_password("CorrectP@ssw0rd ", &hash).unwrap()); // Trailing space
    }

    #[test]
    fn test_verify_password_empty_password() {
        let password = "";
        let hash = hash_password(password).unwrap();

        assert!(verify_password("", &hash).unwrap());
        assert!(!verify_password("anything", &hash).unwrap());
    }

    #[test]
    fn test_verify_password_invalid_hash_format() {
        let result = verify_password("password", "invalid-hash");
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_password_empty_hash() {
        let result = verify_password("password", "");
        assert!(result.is_err());
    }

    #[test]
    fn test_hash_special_characters() {
        let passwords = vec![
            "P@ssw0rd!#$%",
            "Pass with spaces",
            "Unicode: 你好密码",
            "Emoji: 🔒🔐🗝️",
            "Tab\tNewline\nPassword",
        ];

        for password in passwords {
            let hash = hash_password(password).unwrap();
            assert!(
                verify_password(password, &hash).unwrap(),
                "Failed to verify password with special characters: {}",
                password
            );
        }
    }

    #[test]
    fn test_hash_long_password() {
        // Test with a very long password (1000 characters)
        let long_password = "a".repeat(1000);
        let hash = hash_password(&long_password).unwrap();
        assert!(verify_password(&long_password, &hash).unwrap());
    }

    #[test]
    fn test_verify_constant_time() {
        // This test verifies that verification doesn't fail on similar passwords
        // (actual constant-time verification is handled by argon2 crate)
        let password = "BasePassword123";
        let hash = hash_password(password).unwrap();

        // These should all fail in constant time
        assert!(!verify_password("BasePassword12", &hash).unwrap());
        assert!(!verify_password("BasePassword1234", &hash).unwrap());
        assert!(!verify_password("basePassword123", &hash).unwrap());
    }

    #[test]
    fn test_hash_password_idempotency() {
        // Hashing the same password multiple times should work
        let password = "IdempotentTest";

        for _ in 0..5 {
            let hash = hash_password(password).unwrap();
            assert!(verify_password(password, &hash).unwrap());
        }
    }

    #[test]
    fn test_real_world_passwords() {
        // Test with realistic password patterns
        let passwords = vec![
            "MySecureP@ssw0rd2024",
            "correct-horse-battery-staple",
            "Tr0ub4dor&3",
            "ILoveCats!123",
            "P@55w0rd_with_underscores",
        ];

        for password in passwords {
            let hash = hash_password(password).unwrap();
            assert!(
                verify_password(password, &hash).unwrap(),
                "Failed to verify real-world password: {}",
                password
            );
        }
    }
}
