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
        Some(Self { issuer, project_id, audience, jwks_url })
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
// Shared middleware state
// ---------------------------------------------------------------------------

/// Shared state for the Zitadel auth middleware.
#[derive(Clone)]
pub struct ZitadelAuthState {
    pub config: Arc<ZitadelConfig>,
    pub jwks_cache: JwksCache,
    pub pool: DbPool,
    pub permission_cache: Arc<PermissionCache>,
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
}
