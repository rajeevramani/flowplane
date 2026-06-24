//! Dev-mode identity issuer (feature `dev-oidc`; spec/10 §4a).
//!
//! Mints RS256 tokens from a key generated at boot and exposes the matching JWKS so the
//! production [`crate::oidc::OidcValidator`] validates them through the IDENTICAL code path
//! — there is no skip-auth branch. This module does not exist in builds without the
//! `dev-oidc` feature, and callers must additionally pass the runtime gate (see the server's
//! dev-mode ack check).

use fp_domain::{DomainError, DomainResult};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use serde_json::json;

pub const DEV_ISSUER: &str = "https://dev.flowplane.local";
pub const DEV_AUDIENCE: &str = "flowplane";
pub const DEV_SUBJECT: &str = "dev-user";
pub const DEV_EMAIL: &str = "dev@flowplane.local";
const DEV_KID: &str = "flowplane-dev-key";

/// In-process token issuer for dev mode. The key is generated per boot — dev tokens never
/// survive a restart and can never validate against another instance.
pub struct DevIssuer {
    encoding: EncodingKey,
    jwks_json: String,
}

impl DevIssuer {
    /// Generate a fresh 2048-bit RSA key. Takes ~1s; called once at dev-mode boot.
    pub fn generate() -> DomainResult<Self> {
        let mut rng = rsa::rand_core::OsRng;
        let private = RsaPrivateKey::new(&mut rng, 2048)
            .map_err(|e| DomainError::internal(format!("dev key generation failed: {e}")))?;
        let public = private.to_public_key();
        let jwks_json = json!({
            "keys": [{
                "kty": "RSA",
                "kid": DEV_KID,
                "use": "sig",
                "alg": "RS256",
                "n": base64url(&public.n().to_bytes_be()),
                "e": base64url(&public.e().to_bytes_be()),
            }]
        })
        .to_string();
        let der = private
            .to_pkcs1_der()
            .map_err(|e| DomainError::internal(format!("dev key encoding failed: {e}")))?;
        Ok(Self {
            encoding: EncodingKey::from_rsa_der(der.as_bytes()),
            jwks_json,
        })
    }

    /// JWKS document for loading into the validator (or serving from a dev endpoint).
    pub fn jwks_json(&self) -> &str {
        &self.jwks_json
    }

    /// Mint a token for an arbitrary subject (tests) with the given lifetime.
    pub fn mint(
        &self,
        subject: &str,
        email: &str,
        name: &str,
        lifetime_secs: i64,
    ) -> DomainResult<String> {
        let now = chrono::Utc::now().timestamp();
        let claims = json!({
            "iss": DEV_ISSUER,
            "aud": DEV_AUDIENCE,
            "sub": subject,
            "email": email,
            "name": name,
            "iat": now,
            "exp": now + lifetime_secs,
        });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(DEV_KID.to_string());
        encode(&header, &claims, &self.encoding)
            .map_err(|e| DomainError::internal(format!("dev token mint failed: {e}")))
    }

    /// Mint the standard dev-user token (1 hour).
    pub fn mint_dev_user(&self) -> DomainResult<String> {
        self.mint(DEV_SUBJECT, DEV_EMAIL, "Dev User", 3600)
    }

    /// The validator configuration matching this issuer.
    pub fn oidc_config(&self) -> crate::oidc::OidcConfig {
        crate::oidc::OidcConfig {
            issuer: DEV_ISSUER.to_string(),
            audience: DEV_AUDIENCE.to_string(),
            // JWKS is loaded directly via load_jwks_json; no network fetch in dev.
            jwks_uri: Some("https://dev.flowplane.local/jwks".to_string()),
            // Dev mode never intercepts its in-process issuer.
            ca_bundle_path: None,
        }
    }
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let quad = [
            ALPHABET[(n >> 18) as usize & 63],
            ALPHABET[(n >> 12) as usize & 63],
            ALPHABET[(n >> 6) as usize & 63],
            ALPHABET[n as usize & 63],
        ];
        out.extend_from_slice(&quad[..chunk.len() + 1]);
    }
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::oidc::OidcValidator;

    #[tokio::test]
    async fn dev_tokens_validate_through_the_production_path() {
        let issuer = DevIssuer::generate().expect("keygen");
        let validator = OidcValidator::new(issuer.oidc_config());
        validator
            .load_jwks_json(issuer.jwks_json())
            .await
            .expect("jwks");

        let claims = validator
            .validate(&issuer.mint_dev_user().expect("mint"))
            .await
            .expect("dev token must validate via the standard RS256 path");
        assert_eq!(claims.subject, DEV_SUBJECT);
        assert_eq!(claims.email.as_deref(), Some(DEV_EMAIL));
    }

    #[tokio::test]
    async fn dev_tokens_do_not_survive_a_restart() {
        let boot1 = DevIssuer::generate().expect("keygen");
        let boot2 = DevIssuer::generate().expect("keygen");
        let validator = OidcValidator::new(boot2.oidc_config());
        validator
            .load_jwks_json(boot2.jwks_json())
            .await
            .expect("jwks");
        // Token minted by the previous boot's key must be rejected by the new boot.
        let stale = boot1.mint_dev_user().expect("mint");
        assert!(validator.validate(&stale).await.is_err());
    }

    #[tokio::test]
    async fn expired_dev_token_rejected() {
        let issuer = DevIssuer::generate().expect("keygen");
        let validator = OidcValidator::new(issuer.oidc_config());
        validator
            .load_jwks_json(issuer.jwks_json())
            .await
            .expect("jwks");
        let expired = issuer
            .mint(DEV_SUBJECT, DEV_EMAIL, "Dev", -120)
            .expect("mint");
        assert!(validator.validate(&expired).await.is_err());
    }
}
