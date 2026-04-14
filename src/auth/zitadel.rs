//! Zitadel JWT validation for Flowplane authentication.
//!
//! When `FLOWPLANE_ZITADEL_ISSUER` is set, this module validates Zitadel JWTs
//! and extracts the `sub` claim for identity. Permissions are loaded from the
//! Flowplane database — JWT carries no authorization information.

use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{
    decode, decode_header,
    jwk::{AlgorithmParameters, JwkSet},
    Algorithm, DecodingKey, Validation,
};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::api::error::ApiError;
use crate::auth::cache::PermissionCache;
use crate::storage::DbPool;

/// How long to cache the JWKS before re-fetching.
const JWKS_CACHE_TTL: Duration = Duration::from_secs(3600);

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Zitadel integration configuration, read from environment variables.
#[derive(Debug, Clone)]
pub struct ZitadelConfig {
    /// Zitadel issuer URL — must match the `iss` claim in JWTs (e.g. `http://localhost:8080`).
    pub issuer: String,
    /// Zitadel project ID — used to locate role claims in the JWT.
    pub project_id: String,
    /// Expected audience value. Defaults to `project_id` if unset.
    pub audience: String,
    /// JWKS endpoint URL. Defaults to `{issuer}/oauth/v2/keys`.
    /// Override with `FLOWPLANE_ZITADEL_JWKS_URL` when Zitadel is reachable at a
    /// different address than the issuer (e.g. container-to-container networking).
    pub jwks_url: String,
    /// Userinfo endpoint URL. Defaults to same base as `jwks_url` + `/oidc/v1/userinfo`.
    /// Used as fallback when JWT access token doesn't include email claim.
    pub userinfo_url: String,
}

impl ZitadelConfig {
    /// Read configuration from environment variables.
    /// Returns `None` when `FLOWPLANE_ZITADEL_ISSUER` is not set (opt-in).
    pub fn from_env() -> Option<Self> {
        let issuer = std::env::var("FLOWPLANE_ZITADEL_ISSUER").ok()?;
        let project_id = match std::env::var("FLOWPLANE_ZITADEL_PROJECT_ID") {
            Ok(id) if !id.is_empty() => id,
            _ => {
                tracing::error!(
                    "FLOWPLANE_ZITADEL_PROJECT_ID is required when FLOWPLANE_ZITADEL_ISSUER is set"
                );
                return None;
            }
        };
        let audience =
            std::env::var("FLOWPLANE_ZITADEL_AUDIENCE").unwrap_or_else(|_| project_id.clone());
        let jwks_url = std::env::var("FLOWPLANE_ZITADEL_JWKS_URL").unwrap_or_else(|_| {
            let base = issuer.trim_end_matches('/');
            format!("{base}/oauth/v2/keys")
        });
        // Derive userinfo URL from the same base as JWKS (handles container networking)
        let userinfo_url = std::env::var("FLOWPLANE_ZITADEL_USERINFO_URL").unwrap_or_else(|_| {
            // Strip the path from jwks_url to get the base
            if let Ok(url) = url::Url::parse(&jwks_url) {
                let base = format!("{}://{}", url.scheme(), url.authority());
                format!("{base}/oidc/v1/userinfo")
            } else {
                let base = issuer.trim_end_matches('/');
                format!("{base}/oidc/v1/userinfo")
            }
        });
        Some(Self { issuer, project_id, audience, jwks_url, userinfo_url })
    }

    /// Build a `ZitadelConfig` from a running in-process mock OIDC server.
    ///
    /// Used by `AuthMode::Dev` to wire the Zitadel middleware against the
    /// embedded mock issuer. The mock must already have bound its ephemeral
    /// port so `jwks_url()` returns a usable URL.
    #[cfg(feature = "dev-oidc")]
    pub fn from_mock(mock: &crate::dev::oidc_server::MockOidcServer) -> Self {
        Self {
            issuer: mock.issuer.clone(),
            project_id: mock.project_id.clone(),
            audience: mock.audience.clone(),
            jwks_url: mock.jwks_url(),
            userinfo_url: mock.userinfo_endpoint(),
        }
    }
}

// ---------------------------------------------------------------------------
// JWKS cache
// ---------------------------------------------------------------------------

struct CachedJwks {
    jwks: JwkSet,
    fetched_at: Instant,
}

/// Thread-safe, TTL-based JWKS cache.
#[derive(Clone)]
pub struct JwksCache {
    inner: Arc<RwLock<Option<CachedJwks>>>,
    jwks_url: String,
    /// Host header value derived from the issuer URL.
    /// Zitadel resolves instances by hostname, so when the JWKS URL uses a
    /// different address (e.g. container name), we must send the issuer's host.
    issuer_host: Option<String>,
}

impl JwksCache {
    /// The issuer's host (used for Host header in container networking).
    pub fn issuer_host(&self) -> Option<&str> {
        self.issuer_host.as_deref()
    }

    pub fn new(config: &ZitadelConfig) -> Self {
        // Extract "host:port" from the issuer for the Host header
        let issuer_host = url::Url::parse(&config.issuer).ok().and_then(|u| {
            u.host_str().map(|h| match u.port() {
                Some(p) => format!("{h}:{p}"),
                None => h.to_string(),
            })
        });
        Self { inner: Arc::new(RwLock::new(None)), jwks_url: config.jwks_url.clone(), issuer_host }
    }

    /// Return cached JWKS or fetch fresh if expired / missing.
    async fn get(&self) -> Result<JwkSet, ApiError> {
        // Fast path — cache hit
        {
            let guard = self.inner.read().await;
            if let Some(cached) = guard.as_ref() {
                if cached.fetched_at.elapsed() < JWKS_CACHE_TTL {
                    return Ok(cached.jwks.clone());
                }
            }
        }

        // Slow path — fetch & update
        let jwks = self.fetch_jwks().await?;
        let mut guard = self.inner.write().await;
        *guard = Some(CachedJwks { jwks: jwks.clone(), fetched_at: Instant::now() });
        Ok(jwks)
    }

    async fn fetch_jwks(&self) -> Result<JwkSet, ApiError> {
        let client = reqwest::Client::new();
        let mut req = client.get(&self.jwks_url);
        // Zitadel uses Host-based instance resolution; when the JWKS URL
        // differs from the issuer (e.g. container networking), we must
        // forward the issuer's host so Zitadel resolves the correct instance.
        if let Some(host) = &self.issuer_host {
            req = req.header("Host", host);
        }
        let resp =
            req.send().await.map_err(|e| ApiError::Internal(format!("JWKS fetch failed: {e}")))?;
        let jwks: JwkSet =
            resp.json().await.map_err(|e| ApiError::Internal(format!("JWKS parse failed: {e}")))?;
        Ok(jwks)
    }
}

// ---------------------------------------------------------------------------
// JWT validation (sub-only extraction)
// ---------------------------------------------------------------------------

/// Minimal decoded JWT claims for auth middleware use.
pub struct JwtClaims {
    pub sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
}

/// Validate a Zitadel JWT and return only the core identity claims.
///
/// Permissions are loaded separately from the database after the `sub` is
/// extracted.
pub async fn validate_jwt_extract_sub(
    token: &str,
    config: &ZitadelConfig,
    jwks_cache: &JwksCache,
) -> Result<JwtClaims, ApiError> {
    let header = decode_header(token)
        .map_err(|e| ApiError::unauthorized(format!("invalid JWT header: {e}")))?;

    let kid =
        header.kid.as_deref().ok_or_else(|| ApiError::unauthorized("JWT missing kid header"))?;

    let jwks = jwks_cache.get().await?;
    let jwk = jwks
        .find(kid)
        .ok_or_else(|| ApiError::unauthorized(format!("no JWKS key for kid={kid}")))?;

    let decoding_key = match &jwk.algorithm {
        AlgorithmParameters::RSA(rsa) => DecodingKey::from_rsa_components(&rsa.n, &rsa.e)
            .map_err(|e| ApiError::Internal(format!("RSA key decode failed: {e}")))?,
        other => {
            return Err(ApiError::Internal(format!("unsupported JWK algorithm: {other:?}")));
        }
    };

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[&config.issuer]);
    validation.set_audience(&[&config.audience]);

    let token_data = decode::<Value>(token, &decoding_key, &validation)
        .map_err(|e| ApiError::unauthorized(format!("JWT validation failed: {e}")))?;

    let claims = token_data.claims;

    let sub = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::unauthorized("JWT missing sub claim"))?
        .to_string();

    let email = claims.get("email").and_then(|v| v.as_str()).map(|s| s.to_string());
    let name = claims.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());

    Ok(JwtClaims { sub, email, name })
}

// ---------------------------------------------------------------------------
// Userinfo fallback
// ---------------------------------------------------------------------------

/// Fetch the user's email from Zitadel's userinfo endpoint.
///
/// Called when the JWT access token doesn't include an `email` claim (default
/// Zitadel behaviour). Returns `None` on any failure — callers should fall
/// back gracefully.
pub async fn fetch_user_email(
    token: &str,
    config: &ZitadelConfig,
    issuer_host: Option<&str>,
) -> Option<(String, Option<String>)> {
    let client = reqwest::Client::new();
    let mut req = client.get(&config.userinfo_url).bearer_auth(token);
    if let Some(host) = issuer_host {
        req = req.header("Host", host);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        tracing::debug!(
            status = %resp.status(),
            "userinfo request failed"
        );
        return None;
    }
    let body: Value = resp.json().await.ok()?;
    let email = body.get("email").and_then(|v| v.as_str()).map(|s| s.to_string())?;
    let name = body.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());
    Some((email, name))
}

// ---------------------------------------------------------------------------
// Shared middleware state
// ---------------------------------------------------------------------------

/// Shared state for the Zitadel auth middleware.
#[derive(Clone)]
pub struct ZitadelAuthState {
    pub config: Arc<ZitadelConfig>,
    pub jwks_cache: JwksCache,
    pub pool: DbPool,
    pub permission_cache: Arc<PermissionCache>,
    /// Rate limiter for authentication attempts (keyed by client IP).
    pub auth_rate_limiter: Arc<crate::api::rate_limit::RateLimiter>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zitadel_config_from_env_returns_none_when_unset() {
        // Ensure the env var is not set
        std::env::remove_var("FLOWPLANE_ZITADEL_ISSUER");
        assert!(ZitadelConfig::from_env().is_none());
    }

    /// Helper: extract sub from a serde_json::Value using the same logic as
    /// validate_jwt_extract_sub (lines 201-207). This lets us unit-test the
    /// claim extraction without needing real JWTs or JWKS.
    fn extract_sub(claims: &Value) -> Result<String, ApiError> {
        let sub = claims
            .get("sub")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::unauthorized("JWT missing sub claim"))?
            .to_string();
        Ok(sub)
    }

    fn extract_claims(claims: &Value) -> Result<JwtClaims, ApiError> {
        let sub = extract_sub(claims)?;
        let email = claims.get("email").and_then(|v| v.as_str()).map(|s| s.to_string());
        let name = claims.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());
        Ok(JwtClaims { sub, email, name })
    }

    #[test]
    fn test_valid_token_extracts_sub() {
        let claims = serde_json::json!({
            "sub": "user-12345",
            "iss": "https://auth.example.com",
            "aud": "project-id",
            "email": "alice@example.com",
            "name": "Alice"
        });
        let result = extract_claims(&claims).expect("should extract claims");
        assert_eq!(result.sub, "user-12345");
        assert_eq!(result.email.as_deref(), Some("alice@example.com"));
        assert_eq!(result.name.as_deref(), Some("Alice"));
    }

    #[test]
    fn test_extra_claims_ignored() {
        // Role claims, scopes, and other Zitadel-specific fields must not
        // affect identity extraction — permissions come from the DB.
        let claims = serde_json::json!({
            "sub": "user-99",
            "iss": "https://auth.example.com",
            "aud": "project-id",
            "urn:zitadel:iam:org:project:id:12345:roles": {
                "engineering:admin": { "org-abc": "org-abc" }
            },
            "scope": "openid profile email",
            "custom_claim": "should-be-ignored"
        });
        let result = extract_claims(&claims).expect("should extract claims");
        assert_eq!(result.sub, "user-99");
        // No email in this token — should be None, not extracted from role claims
        assert!(result.email.is_none());
    }

    #[test]
    fn test_malformed_sub_fails_closed() {
        // Missing sub entirely
        let no_sub = serde_json::json!({
            "iss": "https://auth.example.com",
            "aud": "project-id"
        });
        assert!(extract_sub(&no_sub).is_err());

        // Sub is null
        let null_sub = serde_json::json!({
            "sub": null,
            "iss": "https://auth.example.com"
        });
        assert!(extract_sub(&null_sub).is_err());

        // Sub is a number (not a string)
        let numeric_sub = serde_json::json!({
            "sub": 12345,
            "iss": "https://auth.example.com"
        });
        assert!(extract_sub(&numeric_sub).is_err());
    }

    // Adversarial: ZitadelConfig::from_mock must faithfully propagate the
    // mock's post-bind fields. The critical invariant is jwks_url pointing
    // at the ephemeral bind URL (architect's day-1 warning in
    // specs/decisions/2026-04-14-fp-4n5-pre-implementation.md §G1): if
    // from_mock captures a pre-configured constant instead of the live
    // mock.jwks_url(), token validation silently fails against a wrong
    // endpoint. This test forces a non-default audience so the check is
    // not a tautology on Default.
    #[cfg(feature = "dev-oidc")]
    #[tokio::test]
    async fn from_mock_propagates_post_bind_fields() {
        use crate::dev::oidc_server::{MockOidcConfig, MockOidcServer};

        let config = MockOidcConfig {
            project_id: "from-mock-project-xyz".to_string(),
            audience: "from-mock-audience-42".to_string(),
            ..Default::default()
        };
        let mock = MockOidcServer::start(config).await.expect("mock start");

        let cfg = ZitadelConfig::from_mock(&mock);

        // Issuer is derived from the ephemeral bind URL, so we assert
        // field equality with the mock's own value rather than a constant.
        assert_eq!(
            cfg.issuer, mock.issuer,
            "ZitadelConfig.issuer must mirror mock.issuer (post-bind, ephemeral port)"
        );
        assert_eq!(
            cfg.project_id, "from-mock-project-xyz",
            "project_id must propagate from MockOidcConfig"
        );
        assert_eq!(
            cfg.audience, "from-mock-audience-42",
            "audience must propagate from MockOidcConfig"
        );
        assert_eq!(
            cfg.jwks_url,
            mock.jwks_url(),
            "jwks_url MUST equal mock.jwks_url() — capturing a pre-bind constant \
             here silently breaks JWT validation against the ephemeral port"
        );

        // Sanity: the jwks_url must contain the ephemeral port (not 0 and
        // not any default). The architect's G1 warning was specifically
        // about constructing from_mock before the listener bound. A URL
        // without a numeric port after the colon would indicate we
        // captured a placeholder.
        assert!(
            cfg.jwks_url.starts_with("http://127.0.0.1:")
                || cfg.jwks_url.starts_with("http://localhost:"),
            "jwks_url should target loopback mock server, got: {}",
            cfg.jwks_url
        );
        let port_part = cfg
            .jwks_url
            .trim_start_matches("http://127.0.0.1:")
            .trim_start_matches("http://localhost:");
        let port_end = port_part.find('/').unwrap_or(port_part.len());
        let port_str = &port_part[..port_end];
        let port: u16 = port_str.parse().expect("jwks_url must carry a numeric port");
        assert!(port > 0, "ephemeral port must be non-zero");
    }

    // Adversarial: two sequential mocks must produce distinct jwks_urls
    // (distinct ephemeral ports). Catches regressions where from_mock
    // would cache or memoize the jwks_url and reuse a stale port across
    // mock instances.
    #[cfg(feature = "dev-oidc")]
    #[tokio::test]
    async fn from_mock_distinct_instances_have_distinct_jwks_urls() {
        use crate::dev::oidc_server::{MockOidcConfig, MockOidcServer};

        let mock_a = MockOidcServer::start(MockOidcConfig::default()).await.unwrap();
        let mock_b = MockOidcServer::start(MockOidcConfig::default()).await.unwrap();

        let cfg_a = ZitadelConfig::from_mock(&mock_a);
        let cfg_b = ZitadelConfig::from_mock(&mock_b);

        assert_ne!(
            cfg_a.jwks_url, cfg_b.jwks_url,
            "two distinct mock servers must yield distinct jwks_urls"
        );
        assert_ne!(
            cfg_a.issuer, cfg_b.issuer,
            "two distinct mock servers must yield distinct issuers"
        );
    }
}
