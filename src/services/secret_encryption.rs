//! Secret encryption service using AES-256-GCM
//!
//! This module provides encryption and decryption for secret data stored
//! in the database. Secrets are encrypted at rest using AES-256-GCM with
//! unique nonces per secret.
//!
//! ## Configuration
//!
//! The encryption key is loaded from the environment variable:
//! `FLOWPLANE_SECRET_ENCRYPTION_KEY` - Base64-encoded 32-byte key
//!
//! ## Key Rotation
//!
//! Key rotation is supported via the `encryption_key_id` field in the database.
//! When rotating keys, old secrets remain decryptable with the old key until
//! they are re-encrypted with the new key.

use crate::errors::{FlowplaneError, Result};
use base64::Engine;
use ring::aead::{self, Aad, BoundKey, Nonce, NonceSequence, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};
use std::sync::Arc;
use tracing::{debug, error, instrument};

/// Size of AES-256-GCM nonce in bytes
const NONCE_SIZE: usize = 12;

/// Size of AES-256-GCM tag in bytes
const TAG_SIZE: usize = 16;

/// Configuration for the secret encryption service
#[derive(Debug, Clone)]
pub struct SecretEncryptionConfig {
    /// Base64-encoded 32-byte master encryption key
    pub master_key_base64: String,
    /// Key version for rotation tracking
    pub key_version: String,
}

impl SecretEncryptionConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self> {
        let master_key_base64 = std::env::var("FLOWPLANE_SECRET_ENCRYPTION_KEY").map_err(|_| {
            FlowplaneError::config(
                "FLOWPLANE_SECRET_ENCRYPTION_KEY environment variable not set. \
                 Generate a key with: openssl rand -base64 32",
            )
        })?;

        let key_version =
            std::env::var("FLOWPLANE_SECRET_KEY_VERSION").unwrap_or_else(|_| "default".to_string());

        Ok(Self { master_key_base64, key_version })
    }

    /// Create a development/testing configuration with a fixed key
    /// WARNING: Only use this for development/testing, never in production!
    #[cfg(test)]
    pub fn for_testing() -> Self {
        // Generate a deterministic test key (NOT for production!)
        let test_key = [0x42u8; 32]; // All 0x42 bytes for testing
        Self {
            master_key_base64: base64::engine::general_purpose::STANDARD.encode(test_key),
            key_version: "test".to_string(),
        }
    }
}

/// Single-use nonce sequence for AES-GCM
struct SingleNonce {
    nonce: Option<[u8; NONCE_SIZE]>,
}

impl SingleNonce {
    fn new(nonce_bytes: [u8; NONCE_SIZE]) -> Self {
        Self { nonce: Some(nonce_bytes) }
    }
}

impl NonceSequence for SingleNonce {
    fn advance(&mut self) -> std::result::Result<Nonce, ring::error::Unspecified> {
        self.nonce.take().map(Nonce::assume_unique_for_key).ok_or(ring::error::Unspecified)
    }
}

/// Secret encryption service
#[derive(Clone)]
pub struct SecretEncryption {
    key_bytes: Arc<[u8; 32]>,
    key_version: String,
    rng: Arc<SystemRandom>,
}

impl SecretEncryption {
    /// Create a new encryption service from configuration
    pub fn new(config: &SecretEncryptionConfig) -> Result<Self> {
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&config.master_key_base64)
            .map_err(|e| {
                FlowplaneError::config(format!(
                    "Invalid base64 in FLOWPLANE_SECRET_ENCRYPTION_KEY: {}",
                    e
                ))
            })?;

        if key_bytes.len() != 32 {
            return Err(FlowplaneError::config(format!(
                "FLOWPLANE_SECRET_ENCRYPTION_KEY must be 32 bytes (256 bits), got {} bytes",
                key_bytes.len()
            )));
        }

        let mut key_array = [0u8; 32];
        key_array.copy_from_slice(&key_bytes);

        debug!(
            key_version = %config.key_version,
            "Secret encryption service initialized"
        );

        Ok(Self {
            key_bytes: Arc::new(key_array),
            key_version: config.key_version.clone(),
            rng: Arc::new(SystemRandom::new()),
        })
    }

    /// Get the current key version
    pub fn key_version(&self) -> &str {
        &self.key_version
    }

    /// Encrypt plaintext data
    ///
    /// Returns a tuple of (ciphertext, nonce) where:
    /// - ciphertext includes the authentication tag appended
    /// - nonce is 12 bytes for AES-256-GCM
    #[instrument(skip(self, plaintext), fields(plaintext_len = plaintext.len()))]
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        // Generate a random nonce
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        self.rng.fill(&mut nonce_bytes).map_err(|_| {
            error!("Failed to generate random nonce");
            FlowplaneError::internal("Failed to generate random nonce for encryption")
        })?;

        // Create the sealing key
        let unbound_key = UnboundKey::new(&AES_256_GCM, &*self.key_bytes).map_err(|_| {
            error!("Failed to create encryption key");
            FlowplaneError::internal("Failed to create encryption key")
        })?;

        let nonce_sequence = SingleNonce::new(nonce_bytes);
        let mut sealing_key = aead::SealingKey::new(unbound_key, nonce_sequence);

        // Prepare buffer with plaintext + space for tag
        let mut ciphertext = plaintext.to_vec();
        ciphertext.reserve(TAG_SIZE);

        // Encrypt in place and append tag
        sealing_key.seal_in_place_append_tag(Aad::empty(), &mut ciphertext).map_err(|_| {
            error!("Encryption failed");
            FlowplaneError::internal("Failed to encrypt secret data")
        })?;

        debug!(
            ciphertext_len = ciphertext.len(),
            nonce_len = nonce_bytes.len(),
            "Successfully encrypted secret"
        );

        Ok((ciphertext, nonce_bytes.to_vec()))
    }

    /// Decrypt ciphertext data
    ///
    /// The ciphertext must include the authentication tag appended.
    /// The nonce must be the same 12-byte value used during encryption.
    #[instrument(skip(self, ciphertext, nonce), fields(ciphertext_len = ciphertext.len()))]
    pub fn decrypt(&self, ciphertext: &[u8], nonce: &[u8]) -> Result<Vec<u8>> {
        if nonce.len() != NONCE_SIZE {
            return Err(FlowplaneError::internal(format!(
                "Invalid nonce length: expected {} bytes, got {} bytes",
                NONCE_SIZE,
                nonce.len()
            )));
        }

        if ciphertext.len() < TAG_SIZE {
            return Err(FlowplaneError::internal(
                "Ciphertext too short (missing authentication tag)",
            ));
        }

        let mut nonce_bytes = [0u8; NONCE_SIZE];
        nonce_bytes.copy_from_slice(nonce);

        // Create the opening key
        let unbound_key = UnboundKey::new(&AES_256_GCM, &*self.key_bytes).map_err(|_| {
            error!("Failed to create decryption key");
            FlowplaneError::internal("Failed to create decryption key")
        })?;

        let nonce_sequence = SingleNonce::new(nonce_bytes);
        let mut opening_key = aead::OpeningKey::new(unbound_key, nonce_sequence);

        // Prepare buffer with ciphertext
        let mut plaintext = ciphertext.to_vec();

        // Decrypt in place
        let decrypted = opening_key.open_in_place(Aad::empty(), &mut plaintext).map_err(|_| {
            error!("Decryption failed - possible tampering or wrong key");
            FlowplaneError::internal("Failed to decrypt secret data - authentication failed")
        })?;

        debug!(plaintext_len = decrypted.len(), "Successfully decrypted secret");

        Ok(decrypted.to_vec())
    }
}

impl std::fmt::Debug for SecretEncryption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretEncryption")
            .field("key_version", &self.key_version)
            .field("key_bytes", &"[REDACTED]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_encryption() -> SecretEncryption {
        let config = SecretEncryptionConfig::for_testing();
        SecretEncryption::new(&config).unwrap()
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let encryption = test_encryption();
        let plaintext = b"my-secret-oauth-token";

        let (ciphertext, nonce) = encryption.encrypt(plaintext).unwrap();

        // Ciphertext should be larger than plaintext (includes tag)
        assert!(ciphertext.len() > plaintext.len());
        assert_eq!(nonce.len(), NONCE_SIZE);

        let decrypted = encryption.decrypt(&ciphertext, &nonce).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_different_nonces_produce_different_ciphertext() {
        let encryption = test_encryption();
        let plaintext = b"same-plaintext";

        let (ciphertext1, nonce1) = encryption.encrypt(plaintext).unwrap();
        let (ciphertext2, nonce2) = encryption.encrypt(plaintext).unwrap();

        // Nonces should be different (random)
        assert_ne!(nonce1, nonce2);

        // Ciphertexts should be different due to different nonces
        assert_ne!(ciphertext1, ciphertext2);

        // Both should decrypt to same plaintext
        let decrypted1 = encryption.decrypt(&ciphertext1, &nonce1).unwrap();
        let decrypted2 = encryption.decrypt(&ciphertext2, &nonce2).unwrap();
        assert_eq!(decrypted1, plaintext);
        assert_eq!(decrypted2, plaintext);
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let encryption = test_encryption();
        let plaintext = b"sensitive-data";

        let (mut ciphertext, nonce) = encryption.encrypt(plaintext).unwrap();

        // Tamper with the ciphertext
        ciphertext[0] ^= 0xFF;

        // Decryption should fail
        let result = encryption.decrypt(&ciphertext, &nonce);
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_nonce_fails() {
        let encryption = test_encryption();
        let plaintext = b"sensitive-data";

        let (ciphertext, _nonce) = encryption.encrypt(plaintext).unwrap();

        // Use wrong nonce
        let wrong_nonce = vec![0u8; NONCE_SIZE];

        // Decryption should fail
        let result = encryption.decrypt(&ciphertext, &wrong_nonce);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_nonce_length_fails() {
        let encryption = test_encryption();
        let plaintext = b"test";

        let (ciphertext, _nonce) = encryption.encrypt(plaintext).unwrap();

        // Use wrong nonce length
        let wrong_nonce = vec![0u8; 8]; // Should be 12

        let result = encryption.decrypt(&ciphertext, &wrong_nonce);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_plaintext() {
        let encryption = test_encryption();
        let plaintext = b"";

        let (ciphertext, nonce) = encryption.encrypt(plaintext).unwrap();

        // Should have at least the tag
        assert_eq!(ciphertext.len(), TAG_SIZE);

        let decrypted = encryption.decrypt(&ciphertext, &nonce).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_large_plaintext() {
        let encryption = test_encryption();
        let plaintext = vec![0xAB; 1024 * 1024]; // 1MB

        let (ciphertext, nonce) = encryption.encrypt(&plaintext).unwrap();
        let decrypted = encryption.decrypt(&ciphertext, &nonce).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_invalid_key_length() {
        let config = SecretEncryptionConfig {
            master_key_base64: base64::engine::general_purpose::STANDARD.encode(vec![0u8; 16]), // 16 bytes instead of 32
            key_version: "test".to_string(),
        };

        let result = SecretEncryption::new(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_key_version() {
        let config = SecretEncryptionConfig {
            master_key_base64: base64::engine::general_purpose::STANDARD.encode(vec![0x42u8; 32]),
            key_version: "v2".to_string(),
        };

        let encryption = SecretEncryption::new(&config).unwrap();
        assert_eq!(encryption.key_version(), "v2");
    }
}
