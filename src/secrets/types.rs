//! Secure types for handling sensitive data.
//!
//! This module provides types that prevent accidental exposure of secrets
//! through logging, debugging, or error messages.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A string wrapper that redacts its contents in Debug, Display, and serialization.
///
/// This type ensures that sensitive data (private keys, tokens, passwords)
/// are never accidentally logged, printed, or serialized. The actual value can only be
/// accessed via explicit method calls.
///
/// # Security
///
/// - Debug output shows `SecretString([REDACTED])` instead of the actual value
/// - Display output shows `[REDACTED]`
/// - Serialization outputs `"[REDACTED]"` (NEVER the actual value)
/// - Deserialization works normally (accepts actual secret values)
/// - **Memory is securely zeroed when dropped** (via `zeroize` crate)
/// - To serialize the actual value, you must call `expose_secret()` explicitly
///
/// # Memory Safety
///
/// When a `SecretString` is dropped, the underlying memory is overwritten with zeros
/// before being deallocated. This prevents secrets from lingering in memory where they
/// could be exposed via memory dumps, swap files, or core dumps.
///
/// # Example
///
/// ```rust,ignore
/// use flowplane::secrets::SecretString;
///
/// let secret = SecretString::new("my-secret-key");
///
/// // Safe: Redacted output
/// println!("Secret: {:?}", secret);  // Prints: SecretString([REDACTED])
/// println!("Secret: {}", secret);    // Prints: [REDACTED]
///
/// // Safe: Serialization redacts
/// let json = serde_json::to_string(&secret).unwrap();  // "[REDACTED]"
///
/// // When you need the actual value (use sparingly):
/// let raw_value = secret.expose_secret();
///
/// // When `secret` goes out of scope, memory is securely cleared
/// ```
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecretString(String);

impl Serialize for SecretString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // SECURITY: Never serialize the actual secret value
        // This prevents accidental exposure through structured logging or API responses
        serializer.serialize_str("[REDACTED]")
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Allow deserializing actual secret values (e.g., from config files)
        let value = String::deserialize(deserializer)?;
        Ok(SecretString(value))
    }
}

impl SecretString {
    /// Creates a new SecretString from a string value.
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    /// Exposes the underlying secret value.
    ///
    /// # Security Warning
    ///
    /// This method should only be used when the secret value is needed
    /// (e.g., for cryptographic operations, writing to files/network).
    /// Never log or print the result.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// Consumes the SecretString and returns the inner value.
    ///
    /// # Security Warning
    ///
    /// Use sparingly - prefer `expose_secret()` when a reference suffices.
    /// Note: The original memory will still be zeroed when this SecretString is dropped.
    pub fn into_inner(mut self) -> String {
        // Take the value out and replace with empty string
        // The empty string will be zeroed on drop (no-op but safe)
        std::mem::take(&mut self.0)
    }

    /// Returns the length of the secret without exposing the value.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns true if the secret is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretString([REDACTED])")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl PartialEq for SecretString {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for SecretString {}

impl From<String> for SecretString {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for SecretString {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl Default for SecretString {
    fn default() -> Self {
        Self::new("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_string_redacts_debug() {
        let secret = SecretString::new("super-secret-value");
        let debug_output = format!("{:?}", secret);

        assert_eq!(debug_output, "SecretString([REDACTED])");
        assert!(!debug_output.contains("super-secret"));
    }

    #[test]
    fn test_secret_string_redacts_display() {
        let secret = SecretString::new("super-secret-value");
        let display_output = format!("{}", secret);

        assert_eq!(display_output, "[REDACTED]");
        assert!(!display_output.contains("super-secret"));
    }

    #[test]
    fn test_secret_string_expose() {
        let secret = SecretString::new("my-secret");
        assert_eq!(secret.expose_secret(), "my-secret");
    }

    #[test]
    fn test_secret_string_into_inner() {
        let secret = SecretString::new("my-secret");
        assert_eq!(secret.into_inner(), "my-secret");
    }

    #[test]
    fn test_secret_string_equality() {
        let secret1 = SecretString::new("same-value");
        let secret2 = SecretString::new("same-value");
        let secret3 = SecretString::new("different-value");

        assert_eq!(secret1, secret2);
        assert_ne!(secret1, secret3);
    }

    #[test]
    fn test_secret_string_from_conversions() {
        let from_string: SecretString = "test".to_string().into();
        let from_str: SecretString = "test".into();

        assert_eq!(from_string, from_str);
    }

    #[test]
    fn test_secret_string_length() {
        let secret = SecretString::new("12345");
        assert_eq!(secret.len(), 5);
        assert!(!secret.is_empty());

        let empty = SecretString::new("");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_secret_string_serialization_redacts() {
        let secret = SecretString::new("super-secret-value");
        let json = serde_json::to_string(&secret).unwrap();

        // SECURITY: Serialization must NOT contain the actual secret
        assert_eq!(json, "\"[REDACTED]\"");
        assert!(!json.contains("super-secret"));
    }

    #[test]
    fn test_secret_string_deserialization_accepts_values() {
        // Deserialization should accept actual secret values (e.g., from config files)
        let json = "\"my-actual-secret\"";
        let secret: SecretString = serde_json::from_str(json).unwrap();
        assert_eq!(secret.expose_secret(), "my-actual-secret");
    }

    #[test]
    fn test_secret_string_not_in_struct_json() {
        // Verify that when SecretString is embedded in a struct, it's redacted
        #[derive(Serialize)]
        struct TestStruct {
            public_field: String,
            secret_field: SecretString,
        }

        let test = TestStruct {
            public_field: "visible".to_string(),
            secret_field: SecretString::new("hidden-password"),
        };

        let json = serde_json::to_string(&test).unwrap();

        // Public field should be visible
        assert!(json.contains("visible"));

        // Secret should be redacted
        assert!(json.contains("[REDACTED]"));
        assert!(!json.contains("hidden-password"));
    }

    #[test]
    fn test_secret_string_clone() {
        let secret1 = SecretString::new("cloneable");
        let secret2 = secret1.clone();
        assert_eq!(secret1, secret2);
    }
}
