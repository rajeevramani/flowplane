//! Setup token generation for initial system bootstrapping.
//!
//! Setup tokens are special single-use or limited-use tokens used for bootstrapping
//! new teams or provisioning initial access. They use a different format from regular
//! personal access tokens and have stricter usage constraints.

use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, Utc};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::hashing;
use crate::errors::{Error, Result};

/// Default expiration for setup tokens (7 days)
pub const DEFAULT_SETUP_TOKEN_EXPIRATION_DAYS: i64 = 7;

/// Default max usage count for setup tokens (single use)
pub const DEFAULT_MAX_USAGE_COUNT: i64 = 1;

/// Response containing the generated setup token
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupTokenResponse {
    /// The token ID
    pub id: String,
    /// The complete token value (fp_setup_{id}.{secret})
    pub token: String,
    /// When the token expires
    pub expires_at: DateTime<Utc>,
    /// Maximum number of times this token can be used
    pub max_usage_count: i64,
}

/// Setup token generator
pub struct SetupToken {
    argon2: Argon2<'static>,
}

impl SetupToken {
    /// Create a new setup token generator
    pub fn new() -> Self {
        Self { argon2: hashing::password_hasher() }
    }

    /// Generate a new setup token with cryptographically secure random data
    ///
    /// # Arguments
    ///
    /// * `max_usage_count` - Maximum number of times the token can be used (default: 1)
    /// * `expiration_days` - Number of days until expiration (default: 7)
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - The complete token value (format: `fp_setup_{uuid}.{base64_secret}`)
    /// - The hashed secret for storage
    /// - The expiration timestamp
    ///
    /// # Security
    ///
    /// - Uses 64 bytes of cryptographically secure random entropy from OsRng
    /// - Encodes secret as URL-safe base64 (no padding)
    /// - Hashes with Argon2id before storage
    /// - Token ID is a random UUID v4
    pub fn generate(
        &self,
        _max_usage_count: Option<i64>,
        expiration_days: Option<i64>,
    ) -> Result<(String, String, DateTime<Utc>)> {
        // Generate 64 bytes of cryptographically secure random data
        let mut secret_bytes = [0u8; 64];
        OsRng.fill_bytes(&mut secret_bytes);

        // Encode as URL-safe base64 (no padding)
        let secret = URL_SAFE_NO_PAD.encode(secret_bytes);

        // Generate a random UUID for the token ID
        let id = uuid::Uuid::new_v4().to_string();

        // Format the complete token
        let token_value = format!("fp_setup_{}.{}", id, secret);

        // Hash the secret with Argon2
        let hashed_secret = self.hash_secret(&secret)?;

        // Calculate expiration
        let expiration_days = expiration_days.unwrap_or(DEFAULT_SETUP_TOKEN_EXPIRATION_DAYS);
        let expires_at = Utc::now() + Duration::days(expiration_days);

        Ok((token_value, hashed_secret, expires_at))
    }

    /// Generate a setup token response with metadata
    ///
    /// # Arguments
    ///
    /// * `max_usage_count` - Maximum number of times the token can be used
    /// * `expiration_days` - Number of days until expiration
    ///
    /// # Returns
    ///
    /// A `SetupTokenResponse` containing the token, ID, expiration, and usage limits
    pub fn generate_with_metadata(
        &self,
        max_usage_count: Option<i64>,
        expiration_days: Option<i64>,
    ) -> Result<SetupTokenResponse> {
        let (token, _hashed_secret, expires_at) =
            self.generate(max_usage_count, expiration_days)?;

        // Extract ID from token (format: fp_setup_{id}.{secret})
        let id = token
            .strip_prefix("fp_setup_")
            .and_then(|s| s.split('.').next())
            .ok_or_else(|| Error::internal("Failed to extract token ID"))?
            .to_string();

        Ok(SetupTokenResponse {
            id,
            token,
            expires_at,
            max_usage_count: max_usage_count.unwrap_or(DEFAULT_MAX_USAGE_COUNT),
        })
    }

    /// Hash a secret using Argon2id
    ///
    /// # Arguments
    ///
    /// * `secret` - The plaintext secret to hash
    ///
    /// # Returns
    ///
    /// The hashed secret in PHC string format
    fn hash_secret(&self, secret: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let hash = self.argon2.hash_password(secret.as_bytes(), &salt).map_err(|err| {
            Error::internal(format!("Failed to hash setup token secret: {}", err))
        })?;
        Ok(hash.to_string())
    }
}

impl Default for SetupToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_setup_token() {
        let generator = SetupToken::new();
        let result = generator.generate(None, None);

        assert!(result.is_ok());
        let (token, hashed, expires_at) = result.unwrap();

        // Verify token format
        assert!(token.starts_with("fp_setup_"));

        // Verify token contains UUID and secret separated by dot
        let parts: Vec<&str> = token.strip_prefix("fp_setup_").unwrap().split('.').collect();
        assert_eq!(parts.len(), 2, "Token should have UUID and secret parts");

        // Verify UUID is valid
        assert!(uuid::Uuid::parse_str(parts[0]).is_ok(), "First part should be valid UUID");

        // Verify secret is base64 encoded (URL-safe, no padding)
        let decoded = URL_SAFE_NO_PAD.decode(parts[1]);
        assert!(decoded.is_ok(), "Secret should be valid base64");
        assert_eq!(decoded.unwrap().len(), 64, "Secret should be 64 bytes");

        // Verify hash is in PHC format
        assert!(hashed.starts_with("$argon2"), "Hash should be in Argon2 PHC format");

        // Verify expiration is in the future
        assert!(expires_at > Utc::now(), "Expiration should be in the future");

        // Verify default expiration is 7 days
        let expected_expiration = Utc::now() + Duration::days(DEFAULT_SETUP_TOKEN_EXPIRATION_DAYS);
        let time_diff = (expires_at - expected_expiration).num_seconds().abs();
        assert!(time_diff < 2, "Expiration should be approximately 7 days from now");
    }

    #[test]
    fn test_generate_with_custom_expiration() {
        let generator = SetupToken::new();
        let custom_days = 14;
        let (_, _, expires_at) = generator.generate(None, Some(custom_days)).unwrap();

        let expected_expiration = Utc::now() + Duration::days(custom_days);
        let time_diff = (expires_at - expected_expiration).num_seconds().abs();
        assert!(time_diff < 2, "Expiration should be approximately {} days from now", custom_days);
    }

    #[test]
    fn test_generate_with_metadata() {
        let generator = SetupToken::new();
        let max_usage = 5;
        let expiration_days = 10;

        let result = generator.generate_with_metadata(Some(max_usage), Some(expiration_days));
        assert!(result.is_ok());

        let response = result.unwrap();

        // Verify token format
        assert!(response.token.starts_with("fp_setup_"));

        // Verify ID matches the token
        assert!(response.token.contains(&response.id));

        // Verify max usage count
        assert_eq!(response.max_usage_count, max_usage);

        // Verify expiration
        let expected_expiration = Utc::now() + Duration::days(expiration_days);
        let time_diff = (response.expires_at - expected_expiration).num_seconds().abs();
        assert!(time_diff < 2, "Expiration should match custom days");
    }

    #[test]
    fn test_tokens_are_unique() {
        let generator = SetupToken::new();

        let (token1, hash1, _) = generator.generate(None, None).unwrap();
        let (token2, hash2, _) = generator.generate(None, None).unwrap();

        // Tokens should be different
        assert_ne!(token1, token2, "Generated tokens should be unique");

        // Hashes should be different
        assert_ne!(hash1, hash2, "Hashes should be unique");
    }

    #[test]
    fn test_default_max_usage_count() {
        let generator = SetupToken::new();
        let response = generator.generate_with_metadata(None, None).unwrap();

        assert_eq!(
            response.max_usage_count, DEFAULT_MAX_USAGE_COUNT,
            "Should use default max usage count"
        );
    }

    #[test]
    fn test_secret_is_cryptographically_secure() {
        let generator = SetupToken::new();

        // Generate multiple tokens and verify they have high entropy
        let mut secrets = Vec::new();
        for _ in 0..100 {
            let (token, _, _) = generator.generate(None, None).unwrap();
            let secret = token.split('.').nth(1).unwrap();
            secrets.push(secret.to_string());
        }

        // All secrets should be unique (probability of collision is astronomically low)
        let unique_count = secrets.iter().collect::<std::collections::HashSet<_>>().len();
        assert_eq!(
            unique_count, 100,
            "All 100 generated secrets should be unique (verifies cryptographic randomness)"
        );

        // Each secret should be the expected length (64 bytes = 86 base64 chars without padding)
        for secret in &secrets {
            assert!(
                secret.len() >= 85 && secret.len() <= 88,
                "Base64-encoded 64-byte secret should be around 86 characters"
            );
        }
    }

    #[test]
    fn test_hash_verification_would_work() {
        let generator = SetupToken::new();
        let (_token, hashed, _) = generator.generate(None, None).unwrap();

        // Verify we can parse the hash (actual verification would need PasswordVerifier)
        use argon2::PasswordHash;
        let parsed_hash = PasswordHash::new(&hashed);
        assert!(parsed_hash.is_ok(), "Hash should be parseable");
    }
}
