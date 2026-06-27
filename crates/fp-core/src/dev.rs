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

/// Default dev-user token lifetime (24h). Long enough that a local exploration session does not
/// expire mid-use (#190); dev-only, so it never affects production token lifetimes.
const DEV_TOKEN_DEFAULT_TTL_SECS: i64 = 86_400;

/// Resolve the dev-user token lifetime from `FLOWPLANE_DEV_TOKEN_TTL` (seconds), defaulting to
/// [`DEV_TOKEN_DEFAULT_TTL_SECS`]. Reads process env; the parse/validation is factored into the
/// pure [`parse_dev_token_ttl`] so it is unit-testable without env mutation.
fn dev_token_ttl_secs() -> i64 {
    parse_dev_token_ttl(std::env::var("FLOWPLANE_DEV_TOKEN_TTL").ok().as_deref())
}

/// Pure parse: a positive integer wins; anything else (absent, non-numeric, ≤ 0) falls back to
/// the 24h default.
fn parse_dev_token_ttl(raw: Option<&str>) -> i64 {
    raw.and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEV_TOKEN_DEFAULT_TTL_SECS)
}

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

    /// Mint the standard dev-user token. Lifetime is configurable via `FLOWPLANE_DEV_TOKEN_TTL`
    /// (seconds, dev-only) and defaults to 24h, so a local exploration session no longer dies
    /// after an hour and forces a control-plane restart (#190). Dev-mode only; production builds
    /// (`--no-default-features`) never reach this path, and the expiry-enforcement path is unchanged.
    pub fn mint_dev_user(&self) -> DomainResult<String> {
        self.mint(DEV_SUBJECT, DEV_EMAIL, "Dev User", dev_token_ttl_secs())
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

    #[test]
    fn dev_token_ttl_parses_env_else_defaults_to_24h() {
        // default when absent / invalid / non-positive.
        assert_eq!(parse_dev_token_ttl(None), DEV_TOKEN_DEFAULT_TTL_SECS);
        assert_eq!(parse_dev_token_ttl(Some("abc")), DEV_TOKEN_DEFAULT_TTL_SECS);
        assert_eq!(parse_dev_token_ttl(Some("0")), DEV_TOKEN_DEFAULT_TTL_SECS);
        assert_eq!(parse_dev_token_ttl(Some("-5")), DEV_TOKEN_DEFAULT_TTL_SECS);
        // default is at least 24h (no more 1h forced restart).
        assert!(DEV_TOKEN_DEFAULT_TTL_SECS >= 86_400);
        // a valid positive override wins.
        assert_eq!(parse_dev_token_ttl(Some("3600")), 3600);
        assert_eq!(parse_dev_token_ttl(Some(" 7200 ")), 7200);
    }

    #[tokio::test]
    async fn dev_user_token_lifetime_matches_resolved_ttl() {
        // The minted dev-user token's exp-iat reflects the resolved TTL (>= 24h by default),
        // proving mint_dev_user honours dev_token_ttl_secs() rather than the old hardcoded 1h.
        let issuer = DevIssuer::generate().expect("keygen");
        let token = issuer.mint_dev_user().expect("mint");
        let payload = token.split('.').nth(1).expect("jwt payload segment");
        let decoded = base64_url_decode(payload);
        let claims: serde_json::Value = serde_json::from_slice(&decoded).expect("claims are JSON");
        let iat = claims["iat"].as_i64().expect("iat");
        let exp = claims["exp"].as_i64().expect("exp");
        assert!(
            exp - iat >= 86_400,
            "default dev token lifetime must be >= 24h, got {}s",
            exp - iat
        );
    }

    fn base64_url_decode(s: &str) -> Vec<u8> {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s)
            .expect("valid base64url")
    }

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
