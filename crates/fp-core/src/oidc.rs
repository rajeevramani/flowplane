//! Provider-agnostic OIDC JWT validation (spec/05 §2, Q-004).
//!
//! Works against any compliant IdP: the issuer's JWKS is fetched (directly or via
//! `.well-known/openid-configuration` discovery), cached, and refreshed at most once per
//! cooldown window when an unknown `kid` appears — so a flood of bogus tokens cannot hammer
//! the IdP (v1's JWKS cold-start brownout, spec/05 gap 15).
//!
//! Algorithms are pinned to RS256: `alg=none` and HS256 confusion attacks are rejected at
//! the validation config, not by string checks. Dev mode uses this same code path against
//! an in-process issuer — there is no skip-auth branch in the codebase.

use fp_domain::{DomainError, DomainResult, ErrorCode};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Issuer configuration. Provider-agnostic: Zitadel, Keycloak, Okta, Entra, or the dev mock.
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// Expected `iss` claim, exact match.
    pub issuer: String,
    /// Expected `aud` claim.
    pub audience: String,
    /// JWKS endpoint. When `None`, discovered from `{issuer}/.well-known/openid-configuration`.
    pub jwks_uri: Option<String>,
}

/// Identity claims extracted from a validated token. Authorization context (memberships,
/// grants) is loaded from the database by the auth middleware — the token carries identity
/// only (spec/05 §2: JWTs are identity-only).
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedClaims {
    pub subject: String,
    pub email: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawClaims {
    sub: String,
    email: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DiscoveryDocument {
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct JwkSet {
    keys: Vec<serde_json::Value>,
}

struct KeyCache {
    keys: HashMap<String, DecodingKey>,
    last_refresh: Option<Instant>,
}

/// Validates bearer tokens against one OIDC issuer.
pub struct OidcValidator {
    config: OidcConfig,
    http: reqwest::Client,
    cache: Arc<RwLock<KeyCache>>,
    refresh_cooldown: Duration,
}

impl OidcValidator {
    pub fn new(config: OidcConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
            cache: Arc::new(RwLock::new(KeyCache {
                keys: HashMap::new(),
                last_refresh: None,
            })),
            refresh_cooldown: Duration::from_secs(30),
        }
    }

    /// Test/dev hook: load keys from a JWKS JSON document directly (no network).
    pub async fn load_jwks_json(&self, jwks: &str) -> DomainResult<usize> {
        let set: JwkSet = serde_json::from_str(jwks)
            .map_err(|e| DomainError::internal(format!("invalid JWKS document: {e}")))?;
        let mut cache = self.cache.write().await;
        cache.keys.clear();
        for key in &set.keys {
            let kid = key
                .get("kid")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if kid.is_empty() {
                continue;
            }
            let jwk: jsonwebtoken::jwk::Jwk = match serde_json::from_value(key.clone()) {
                Ok(jwk) => jwk,
                Err(e) => {
                    tracing::warn!(kid, "skipping unparseable JWK: {e}");
                    continue;
                }
            };
            match DecodingKey::from_jwk(&jwk) {
                Ok(decoding) => {
                    cache.keys.insert(kid, decoding);
                }
                Err(e) => tracing::warn!(kid, "skipping unusable JWK: {e}"),
            }
        }
        cache.last_refresh = Some(Instant::now());
        Ok(cache.keys.len())
    }

    async fn refresh_keys(&self) -> DomainResult<()> {
        // Single-flight with cooldown: take the write lock first so concurrent unknown-kid
        // requests don't stampede the IdP.
        let mut cache = self.cache.write().await;
        if let Some(last) = cache.last_refresh {
            if last.elapsed() < self.refresh_cooldown {
                return Ok(()); // someone else refreshed recently; keys are as fresh as allowed
            }
        }

        let jwks_uri = match &self.config.jwks_uri {
            Some(uri) => uri.clone(),
            None => {
                let discovery_url = format!(
                    "{}/.well-known/openid-configuration",
                    self.config.issuer.trim_end_matches('/')
                );
                let doc: DiscoveryDocument = self
                    .http
                    .get(&discovery_url)
                    .send()
                    .await
                    .map_err(|e| unavailable_idp(&discovery_url, e))?
                    .error_for_status()
                    .map_err(|e| unavailable_idp(&discovery_url, e))?
                    .json()
                    .await
                    .map_err(|e| unavailable_idp(&discovery_url, e))?;
                doc.jwks_uri
            }
        };

        let set: JwkSet = self
            .http
            .get(&jwks_uri)
            .send()
            .await
            .map_err(|e| unavailable_idp(&jwks_uri, e))?
            .error_for_status()
            .map_err(|e| unavailable_idp(&jwks_uri, e))?
            .json()
            .await
            .map_err(|e| unavailable_idp(&jwks_uri, e))?;

        cache.keys.clear();
        for key in &set.keys {
            let kid = key
                .get("kid")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if kid.is_empty() {
                continue;
            }
            if let Ok(jwk) = serde_json::from_value::<jsonwebtoken::jwk::Jwk>(key.clone()) {
                if let Ok(decoding) = DecodingKey::from_jwk(&jwk) {
                    cache.keys.insert(kid, decoding);
                }
            }
        }
        cache.last_refresh = Some(Instant::now());
        tracing::info!(key_count = cache.keys.len(), "JWKS refreshed");
        Ok(())
    }

    /// Validate a bearer token: signature (RS256 only), `iss`, `aud`, `exp`, `nbf`.
    pub async fn validate(&self, token: &str) -> DomainResult<ValidatedClaims> {
        let header = decode_header(token).map_err(|_| unauthorized("malformed token"))?;
        if header.alg != Algorithm::RS256 {
            // Pinned algorithm: rejects alg=none and HS256-confusion outright.
            return Err(unauthorized("unsupported token algorithm"));
        }
        let kid = header
            .kid
            .ok_or_else(|| unauthorized("token has no key id"))?;

        // Fast path under the read lock.
        let key = { self.cache.read().await.keys.get(&kid).cloned() };
        let key = match key {
            Some(k) => k,
            None => {
                self.refresh_keys().await?;
                self.cache
                    .read()
                    .await
                    .keys
                    .get(&kid)
                    .cloned()
                    .ok_or_else(|| unauthorized("token signed by unknown key"))?
            }
        };

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[&self.config.audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_nbf = true;

        let data = decode::<RawClaims>(token, &key, &validation).map_err(|e| match e.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => {
                unauthorized("token expired").with_hint("re-authenticate: flowplane auth login")
            }
            _ => unauthorized("token validation failed"),
        })?;

        Ok(ValidatedClaims {
            subject: data.claims.sub,
            email: data.claims.email,
            name: data.claims.name,
        })
    }
}

fn unauthorized(message: &str) -> DomainError {
    DomainError::new(ErrorCode::Unauthorized, message)
}

fn unavailable_idp(url: &str, e: impl std::fmt::Display) -> DomainError {
    DomainError::unavailable(format!("identity provider unreachable ({url}): {e}"))
        .with_hint("check the OIDC issuer configuration and IdP availability")
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::traits::PublicKeyParts;
    use rsa::RsaPrivateKey;
    use serde_json::json;

    struct TestIssuer {
        encoding: EncodingKey,
        jwks: String,
        kid: &'static str,
    }

    fn base64url(bytes: &[u8]) -> String {
        // Minimal base64url (no padding) for test JWK construction.
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
            // 1 input byte -> 2 output chars, 2 -> 3, 3 -> 4.
            out.extend_from_slice(&quad[..chunk.len() + 1]);
        }
        String::from_utf8(out).expect("alphabet is ASCII")
    }

    fn test_issuer() -> TestIssuer {
        let mut rng = rsa::rand_core::OsRng;
        let private = RsaPrivateKey::new(&mut rng, 2048).expect("generate test RSA key");
        let public = private.to_public_key();
        let n = base64url(&public.n().to_bytes_be());
        let e = base64url(&public.e().to_bytes_be());
        let kid = "test-key-1";
        let jwks = json!({
            "keys": [{ "kty": "RSA", "kid": kid, "use": "sig", "alg": "RS256", "n": n, "e": e }]
        })
        .to_string();
        let der = private.to_pkcs1_der().expect("der");
        TestIssuer {
            encoding: EncodingKey::from_rsa_der(der.as_bytes()),
            jwks,
            kid,
        }
    }

    fn token(issuer: &TestIssuer, claims: serde_json::Value, kid: Option<&str>) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = kid.map(str::to_owned);
        encode(&header, &claims, &issuer.encoding).expect("encode")
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64
    }

    fn validator() -> OidcValidator {
        OidcValidator::new(OidcConfig {
            issuer: "https://idp.test".into(),
            audience: "flowplane".into(),
            jwks_uri: Some("https://unreachable.invalid/jwks".into()),
        })
    }

    fn good_claims() -> serde_json::Value {
        json!({
            "iss": "https://idp.test",
            "aud": "flowplane",
            "sub": "user-123",
            "email": "a@b.c",
            "name": "Alice",
            "exp": now() + 600,
        })
    }

    #[tokio::test]
    async fn valid_token_yields_identity_claims() {
        let issuer = test_issuer();
        let v = validator();
        v.load_jwks_json(&issuer.jwks).await.expect("load jwks");
        let claims = v
            .validate(&token(&issuer, good_claims(), Some(issuer.kid)))
            .await
            .expect("valid");
        assert_eq!(claims.subject, "user-123");
        assert_eq!(claims.email.as_deref(), Some("a@b.c"));
    }

    #[tokio::test]
    async fn adversarial_tokens_rejected() {
        let issuer = test_issuer();
        let v = validator();
        v.load_jwks_json(&issuer.jwks).await.expect("load jwks");

        let cases: Vec<(&str, String)> = vec![
            ("garbage", "not.a.jwt".into()),
            (
                "wrong issuer",
                token(
                    &issuer,
                    {
                        let mut c = good_claims();
                        c["iss"] = json!("https://evil.test");
                        c
                    },
                    Some(issuer.kid),
                ),
            ),
            (
                "wrong audience",
                token(
                    &issuer,
                    {
                        let mut c = good_claims();
                        c["aud"] = json!("other-app");
                        c
                    },
                    Some(issuer.kid),
                ),
            ),
            (
                "expired",
                token(
                    &issuer,
                    {
                        let mut c = good_claims();
                        c["exp"] = json!(now() - 60);
                        c
                    },
                    Some(issuer.kid),
                ),
            ),
            (
                "missing exp",
                token(
                    &issuer,
                    {
                        let mut c = good_claims();
                        c.as_object_mut().expect("obj").remove("exp");
                        c
                    },
                    Some(issuer.kid),
                ),
            ),
            ("no kid", token(&issuer, good_claims(), None)),
            (
                "unknown kid",
                token(&issuer, good_claims(), Some("other-key")),
            ),
            (
                "nbf in future",
                token(
                    &issuer,
                    {
                        let mut c = good_claims();
                        c["nbf"] = json!(now() + 600);
                        c
                    },
                    Some(issuer.kid),
                ),
            ),
        ];
        for (label, tok) in cases {
            let result = v.validate(&tok).await;
            assert!(result.is_err(), "{label}: must be rejected");
        }
    }

    #[tokio::test]
    async fn hs256_confusion_attack_rejected() {
        // Token signed with HMAC using... anything; alg header says HS256. Pinned-algorithm
        // validation must reject before any key lookup.
        let issuer = test_issuer();
        let v = validator();
        v.load_jwks_json(&issuer.jwks).await.expect("load jwks");
        let hs_token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &good_claims(),
            &EncodingKey::from_secret(b"guessable"),
        )
        .expect("encode");
        let err = v
            .validate(&hs_token)
            .await
            .expect_err("HS256 must be rejected");
        assert_eq!(err.code, ErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn token_from_different_key_rejected() {
        let issuer_a = test_issuer();
        let issuer_b = test_issuer();
        let v = validator();
        v.load_jwks_json(&issuer_a.jwks).await.expect("load jwks");
        // Signed by B's key but claims A's kid: signature check must fail.
        let forged = token(&issuer_b, good_claims(), Some(issuer_a.kid));
        assert!(v.validate(&forged).await.is_err());
    }

    #[tokio::test]
    async fn unknown_kid_refresh_is_rate_limited() {
        let issuer = test_issuer();
        let v = validator(); // jwks_uri points at unreachable.invalid
        v.load_jwks_json(&issuer.jwks).await.expect("load jwks");
        // First unknown-kid validation: inside cooldown (just loaded), so NO network refresh
        // is attempted and the error is unauthorized, not unavailable.
        let unknown = token(&issuer, good_claims(), Some("brand-new-kid"));
        let err = v.validate(&unknown).await.expect_err("unknown kid");
        assert_eq!(
            err.code,
            ErrorCode::Unauthorized,
            "cooldown prevents IdP hammering"
        );
    }
}
