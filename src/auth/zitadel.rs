//! Zitadel JWT authentication middleware for the Flowplane auth spike.
//!
//! When `FLOWPLANE_ZITADEL_ISSUER` is set, this module activates a parallel
//! auth path under `/api/v1/zitadel/` that validates Zitadel JWTs and maps
//! role claims into Flowplane's `AuthContext`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::State,
    http::{header::AUTHORIZATION, Method, Request},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{
    decode, decode_header,
    jwk::{AlgorithmParameters, JwkSet},
    Algorithm, DecodingKey, Validation,
};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::api::error::ApiError;
use crate::auth::cache::PermissionCache;
use crate::auth::models::AuthContext;
use crate::domain::{OrgId, TokenId};
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
// Role-claim mapping
// ---------------------------------------------------------------------------

/// Map Zitadel project role claims into Flowplane scope strings.
///
/// Zitadel encodes roles under the key:
/// `urn:zitadel:iam:org:project:{project_id}:roles`
///
/// Each role key has the shape `{team}:{resource}:{action}` (fine-grained) or
/// `{team}:admin` (coarse admin). The *value* is a map of org-ID → org domain
/// which we use to extract org context.
///
/// Mapping rules:
///   `team-01:clusters:read`  → `team:team-01:clusters:read`
///   `team-01:admin`          → `team:team-01:*:*`
///   malformed keys           → skipped
pub fn parse_role_claims(claims_map: &Value, project_id: &str) -> Vec<String> {
    let claim_key = format!("urn:zitadel:iam:org:project:{project_id}:roles");
    let roles = match claims_map.get(&claim_key).and_then(|v| v.as_object()) {
        Some(obj) => obj,
        None => return Vec::new(),
    };

    let mut scopes = Vec::new();
    for role_key in roles.keys() {
        let parts: Vec<&str> = role_key.splitn(3, ':').collect();
        match parts.len() {
            3 => {
                // Fine-grained: team:resource:action
                let team = parts[0];
                let resource = parts[1];
                let action = parts[2];
                scopes.push(format!("team:{team}:{resource}:{action}"));
            }
            2 if parts[0] == "admin" && parts[1] == "all" => {
                // Platform admin: admin:all → admin:all
                scopes.push("admin:all".to_string());
            }
            2 if parts[1] == "admin" => {
                // Coarse admin: team:admin → team:{team}:*:*
                let team = parts[0];
                scopes.push(format!("team:{team}:*:*"));
            }
            _ => {
                tracing::debug!(role_key, "skipping malformed Zitadel role key");
            }
        }
    }
    scopes
}

/// Extract the first org ID from role claim values.
///
/// Role values look like `{ "org-id-123": { "primaryDomain": "example.com" } }`.
/// We take the first org ID found across all role entries.
fn extract_org_id(claims_map: &Value, project_id: &str) -> Option<String> {
    let claim_key = format!("urn:zitadel:iam:org:project:{project_id}:roles");
    let roles = claims_map.get(&claim_key)?.as_object()?;
    for role_value in roles.values() {
        if let Some(obj) = role_value.as_object() {
            if let Some(org_id) = obj.keys().next() {
                return Some(org_id.clone());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// JWT validation
// ---------------------------------------------------------------------------

/// Validate a Zitadel JWT and return a Flowplane `AuthContext`.
pub async fn validate_zitadel_jwt(
    token: &str,
    config: &ZitadelConfig,
    jwks_cache: &JwksCache,
) -> Result<AuthContext, ApiError> {
    // Decode header to find the key ID (kid)
    let header = decode_header(token)
        .map_err(|e| ApiError::unauthorized(format!("invalid JWT header: {e}")))?;

    let kid =
        header.kid.as_deref().ok_or_else(|| ApiError::unauthorized("JWT missing kid header"))?;

    // Fetch JWKS and find the matching key
    let jwks = jwks_cache.get().await?;
    let jwk = jwks
        .find(kid)
        .ok_or_else(|| ApiError::unauthorized(format!("no JWKS key for kid={kid}")))?;

    // Build decoding key from the JWK
    let decoding_key = match &jwk.algorithm {
        AlgorithmParameters::RSA(rsa) => DecodingKey::from_rsa_components(&rsa.n, &rsa.e)
            .map_err(|e| ApiError::Internal(format!("RSA key decode failed: {e}")))?,
        other => {
            return Err(ApiError::Internal(format!("unsupported JWK algorithm: {other:?}")));
        }
    };

    // Validate the token
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[&config.issuer]);
    validation.set_audience(&[&config.audience]);

    let token_data = decode::<Value>(token, &decoding_key, &validation)
        .map_err(|e| ApiError::unauthorized(format!("JWT validation failed: {e}")))?;

    let claims = token_data.claims;

    // Extract subject for token name
    let sub = claims.get("sub").and_then(|v| v.as_str()).unwrap_or("zitadel-user");

    // Map role claims → Flowplane scopes
    let scopes = parse_role_claims(&claims, &config.project_id);

    // Build AuthContext with a synthetic token ID
    let token_id = TokenId::from_string(format!("zitadel:{sub}"));
    let mut ctx = AuthContext::new(token_id, format!("zitadel/{sub}"), scopes);

    // Set org context if available from role grant values
    if let Some(org_id_str) = extract_org_id(&claims, &config.project_id) {
        ctx = ctx.with_org(OrgId::from_string(org_id_str.clone()), org_id_str);
    }

    Ok(ctx)
}

// ---------------------------------------------------------------------------
// Axum middleware
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// JWT sub-only extraction (A.4)
// ---------------------------------------------------------------------------

/// Minimal decoded JWT claims for auth middleware use.
pub struct JwtClaims {
    pub sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
}

/// Validate a Zitadel JWT and return only the core identity claims.
///
/// Unlike [`validate_zitadel_jwt`], this function does **not** parse role
/// claims or build an [`AuthContext`]. Permissions are loaded separately from
/// the database after the `sub` is extracted.
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

/// Axum middleware that validates Zitadel JWTs and inserts an `AuthContext`.
pub async fn authenticate_zitadel(
    State(state): State<ZitadelAuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    // Pass through OPTIONS (CORS preflight)
    if request.method() == Method::OPTIONS {
        return Ok(next.run(request).await);
    }

    let header = request.headers().get(AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("");

    let token = header
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::unauthorized("bearer token required"))?;

    let auth_context = validate_zitadel_jwt(token, &state.config, &state.jwks_cache).await?;

    tracing::debug!(
        sub = %auth_context.token_name,
        scopes = ?auth_context.scopes().collect::<Vec<_>>(),
        "Zitadel auth: JWT validated"
    );

    request.extensions_mut().insert(auth_context);
    Ok(next.run(request).await)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TEST_PROJECT_ID: &str = "123456789";

    fn make_claims(roles: Value) -> Value {
        let claim_key = format!("urn:zitadel:iam:org:project:{TEST_PROJECT_ID}:roles");
        json!({
            "sub": "user-1",
            "iss": "http://localhost:8080",
            claim_key: roles
        })
    }

    #[test]
    fn parse_role_claims_fine_grained() {
        let claims = make_claims(json!({
            "team-01:clusters:read": { "org-abc": { "primaryDomain": "example.com" } },
            "team-01:clusters:write": { "org-abc": { "primaryDomain": "example.com" } },
            "team-02:routes:read": { "org-abc": { "primaryDomain": "example.com" } }
        }));

        let mut scopes = parse_role_claims(&claims, TEST_PROJECT_ID);
        scopes.sort();

        assert_eq!(
            scopes,
            vec![
                "team:team-01:clusters:read",
                "team:team-01:clusters:write",
                "team:team-02:routes:read",
            ]
        );
    }

    #[test]
    fn parse_role_claims_empty() {
        let claims = json!({ "sub": "user-1" });
        let scopes = parse_role_claims(&claims, TEST_PROJECT_ID);
        assert!(scopes.is_empty());
    }

    #[test]
    fn parse_role_claims_coarse_admin() {
        let claims = make_claims(json!({
            "team-01:admin": { "org-abc": {} }
        }));

        let scopes = parse_role_claims(&claims, TEST_PROJECT_ID);
        assert_eq!(scopes, vec!["team:team-01:*:*"]);
    }

    #[test]
    fn parse_role_claims_malformed_skipped() {
        let claims = make_claims(json!({
            "just-a-bare-key": { "org-abc": {} },
            "team-01:clusters:read": { "org-abc": {} }
        }));

        let scopes = parse_role_claims(&claims, TEST_PROJECT_ID);
        assert_eq!(scopes, vec!["team:team-01:clusters:read"]);
    }

    #[test]
    fn parse_role_claims_platform_admin() {
        let claims = make_claims(json!({
            "admin:all": { "org-abc": {} }
        }));

        let scopes = parse_role_claims(&claims, TEST_PROJECT_ID);
        assert_eq!(scopes, vec!["admin:all"]);
    }

    #[test]
    fn parse_role_claims_platform_admin_with_team_roles() {
        let claims = make_claims(json!({
            "admin:all": { "org-abc": {} },
            "team-01:clusters:read": { "org-abc": {} }
        }));

        let mut scopes = parse_role_claims(&claims, TEST_PROJECT_ID);
        scopes.sort();
        assert_eq!(scopes, vec!["admin:all", "team:team-01:clusters:read"]);
    }

    #[test]
    fn parse_role_claims_two_part_non_admin_skipped() {
        let claims = make_claims(json!({
            "team-01:viewer": { "org-abc": {} },
            "team-01:admin": { "org-abc": {} }
        }));

        let scopes = parse_role_claims(&claims, TEST_PROJECT_ID);
        // "team-01:viewer" is 2 parts but not "admin" → skipped
        assert_eq!(scopes, vec!["team:team-01:*:*"]);
    }

    #[test]
    fn extract_org_id_from_claims() {
        let claims = make_claims(json!({
            "team-01:clusters:read": { "org-abc-123": { "primaryDomain": "example.com" } }
        }));

        let org_id = extract_org_id(&claims, TEST_PROJECT_ID);
        assert_eq!(org_id.as_deref(), Some("org-abc-123"));
    }

    #[test]
    fn extract_org_id_missing_claims() {
        let claims = json!({ "sub": "user-1" });
        let org_id = extract_org_id(&claims, TEST_PROJECT_ID);
        assert!(org_id.is_none());
    }

    #[test]
    fn zitadel_config_from_env_returns_none_when_unset() {
        // Ensure the env var is not set
        std::env::remove_var("FLOWPLANE_ZITADEL_ISSUER");
        assert!(ZitadelConfig::from_env().is_none());
    }
}
