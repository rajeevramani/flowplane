//! Mock external services for E2E tests
//!
//! Provides wiremock-based mocks for:
//! - Auth0/JWKS endpoints (JWT validation)
//! - httpbin (echo/test backend)
//! - ext_authz (external authorization)
//! - OAuth2 token endpoint

use std::net::SocketAddr;
use std::sync::Arc;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::json;
use wiremock::matchers::{header, method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Collection of mock services for E2E tests
pub struct MockServices {
    /// Echo/httpbin mock server
    pub echo: MockServer,
    /// Auth0/JWKS mock server (for JWT validation tests)
    pub auth: Option<MockServer>,
    /// External authorization mock server
    pub ext_authz: Option<MockServer>,
    /// JWKS for JWT signing/validation
    jwks: Option<Arc<JwksConfig>>,
}

/// JWKS configuration for JWT tests
pub struct JwksConfig {
    /// RSA public key in JWK format
    pub jwks: serde_json::Value,
    /// RSA private key for signing test JWTs
    pub private_key_pem: String,
    /// Key ID
    pub kid: String,
    /// Issuer URL
    pub issuer: String,
}

/// JWT claims for test tokens
#[derive(Debug, Serialize, Deserialize)]
pub struct TestJwtClaims {
    /// Subject (user identifier)
    pub sub: String,
    /// Issuer
    pub iss: String,
    /// Audience
    pub aud: String,
    /// Expiration time (Unix timestamp)
    pub exp: u64,
    /// Issued at time (Unix timestamp)
    pub iat: u64,
    /// Custom claims
    #[serde(flatten)]
    pub custom: std::collections::HashMap<String, serde_json::Value>,
}

impl MockServices {
    /// Start basic mock services (echo only)
    pub async fn start_basic() -> Self {
        let echo = MockServer::start().await;

        // Setup echo endpoint that returns request info
        Mock::given(method("GET"))
            .and(path_regex(r".*"))
            .respond_with(|req: &Request| {
                let body = json!({
                    "path": req.url.path(),
                    "method": req.method.to_string(),
                    "headers": req.headers.iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect::<std::collections::HashMap<_, _>>(),
                });
                ResponseTemplate::new(200)
                    .set_body_json(body)
                    .insert_header("content-type", "application/json")
            })
            .mount(&echo)
            .await;

        Mock::given(method("POST"))
            .and(path_regex(r".*"))
            .respond_with(|req: &Request| {
                let body = json!({
                    "path": req.url.path(),
                    "method": req.method.to_string(),
                    "headers": req.headers.iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect::<std::collections::HashMap<_, _>>(),
                    "body": String::from_utf8_lossy(&req.body).to_string(),
                });
                ResponseTemplate::new(200)
                    .set_body_json(body)
                    .insert_header("content-type", "application/json")
            })
            .mount(&echo)
            .await;

        Self { echo, auth: None, ext_authz: None, jwks: None }
    }

    /// Start mock services with Auth0/JWKS support
    pub async fn start_with_auth() -> Self {
        let mut services = Self::start_basic().await;
        let auth = MockServer::start().await;

        // Get the auth server URI to use as issuer
        let auth_uri = auth.uri();
        // Ensure issuer has trailing slash for JWT validation
        let issuer = format!("{}/", auth_uri.trim_end_matches('/'));

        // Generate JWKS for JWT validation with the correct issuer
        let jwks_config = generate_test_jwks_with_issuer(&issuer);

        // Setup JWKS endpoint
        let jwks_response = jwks_config.jwks.clone();
        Mock::given(method("GET"))
            .and(path("/.well-known/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(jwks_response))
            .mount(&auth)
            .await;

        // Setup OpenID configuration endpoint
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": &issuer,
                "jwks_uri": format!("{}/.well-known/jwks.json", auth_uri),
                "authorization_endpoint": format!("{}/authorize", auth_uri),
                "token_endpoint": format!("{}/oauth/token", auth_uri),
            })))
            .mount(&auth)
            .await;

        services.auth = Some(auth);
        services.jwks = Some(Arc::new(jwks_config));
        services
    }

    /// Start mock services with external authorization support
    pub async fn start_with_ext_authz() -> Self {
        let mut services = Self::start_basic().await;
        let ext_authz = MockServer::start().await;

        // ext_authz that allows requests with X-Ext-Authz-Allow header
        Mock::given(method("POST"))
            .and(path("/auth"))
            .and(header("x-ext-authz-allow", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": {"code": 0},
                "ok_response": {
                    "headers": [
                        {"header": {"key": "x-ext-authz-check-received", "value": "true"}}
                    ]
                }
            })))
            .mount(&ext_authz)
            .await;

        // ext_authz that denies requests without the header
        Mock::given(method("POST"))
            .and(path("/auth"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "status": {"code": 7, "message": "PERMISSION_DENIED"},
                "denied_response": {
                    "status": {"code": "Forbidden"},
                    "body": "Access denied by ext_authz"
                }
            })))
            .mount(&ext_authz)
            .await;

        services.ext_authz = Some(ext_authz);
        services
    }

    /// Start all mock services
    pub async fn start_all() -> Self {
        let mut services = Self::start_with_auth().await;
        let ext_authz = MockServer::start().await;

        // Setup ext_authz (same as above)
        Mock::given(method("POST"))
            .and(path("/auth"))
            .and(header("x-ext-authz-allow", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": {"code": 0}
            })))
            .mount(&ext_authz)
            .await;

        Mock::given(method("POST"))
            .and(path("/auth"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "status": {"code": 7, "message": "PERMISSION_DENIED"}
            })))
            .mount(&ext_authz)
            .await;

        services.ext_authz = Some(ext_authz);
        services
    }

    /// Get echo server address (for cluster configuration)
    pub fn echo_addr(&self) -> SocketAddr {
        *self.echo.address()
    }

    /// Get echo server endpoint string (host:port)
    pub fn echo_endpoint(&self) -> String {
        format!("127.0.0.1:{}", self.echo.address().port())
    }

    /// Get auth server URI (for JWKS configuration)
    pub fn auth_uri(&self) -> Option<String> {
        self.auth.as_ref().map(|s| s.uri())
    }

    /// Get auth server endpoint string
    pub fn auth_endpoint(&self) -> Option<String> {
        self.auth.as_ref().map(|s| format!("127.0.0.1:{}", s.address().port()))
    }

    /// Get ext_authz server endpoint string
    pub fn ext_authz_endpoint(&self) -> Option<String> {
        self.ext_authz.as_ref().map(|s| format!("127.0.0.1:{}", s.address().port()))
    }

    /// Generate a valid JWT for testing (requires auth mock to be started)
    ///
    /// # Arguments
    /// * `sub` - Subject (user identifier)
    /// * `custom_claims` - Optional custom claims to include
    pub fn generate_valid_jwt(
        &self,
        sub: &str,
        custom_claims: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> Option<String> {
        let jwks = self.jwks.as_ref()?;
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        let claims = TestJwtClaims {
            sub: sub.to_string(),
            iss: jwks.issuer.clone(),
            aud: "e2e-test-api".to_string(),
            exp: now + 3600, // 1 hour from now
            iat: now,
            custom: custom_claims.unwrap_or_default(),
        };

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(jwks.kid.clone());

        let key = EncodingKey::from_rsa_pem(jwks.private_key_pem.as_bytes()).ok()?;
        encode(&header, &claims, &key).ok()
    }

    /// Generate a valid JWT with specific audience
    pub fn generate_jwt_with_audience(&self, sub: &str, aud: &str) -> Option<String> {
        let jwks = self.jwks.as_ref()?;
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        let claims = TestJwtClaims {
            sub: sub.to_string(),
            iss: jwks.issuer.clone(),
            aud: aud.to_string(),
            exp: now + 3600,
            iat: now,
            custom: std::collections::HashMap::new(),
        };

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(jwks.kid.clone());

        let key = EncodingKey::from_rsa_pem(jwks.private_key_pem.as_bytes()).ok()?;
        encode(&header, &claims, &key).ok()
    }

    /// Generate an expired JWT for testing
    pub fn generate_expired_jwt(&self, sub: &str) -> Option<String> {
        let jwks = self.jwks.as_ref()?;
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        let claims = TestJwtClaims {
            sub: sub.to_string(),
            iss: jwks.issuer.clone(),
            aud: "e2e-test-api".to_string(),
            exp: now - 3600, // 1 hour ago (expired)
            iat: now - 7200,
            custom: std::collections::HashMap::new(),
        };

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(jwks.kid.clone());

        let key = EncodingKey::from_rsa_pem(jwks.private_key_pem.as_bytes()).ok()?;
        encode(&header, &claims, &key).ok()
    }

    /// Generate an invalid/malformed JWT for testing
    pub fn generate_invalid_jwt() -> String {
        "invalid.jwt.token".to_string()
    }

    /// Generate a JWT with wrong issuer
    pub fn generate_wrong_issuer_jwt(&self, sub: &str) -> Option<String> {
        let jwks = self.jwks.as_ref()?;
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        let claims = TestJwtClaims {
            sub: sub.to_string(),
            iss: "https://wrong-issuer.example.com/".to_string(),
            aud: "e2e-test-api".to_string(),
            exp: now + 3600,
            iat: now,
            custom: std::collections::HashMap::new(),
        };

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(jwks.kid.clone());

        let key = EncodingKey::from_rsa_pem(jwks.private_key_pem.as_bytes()).ok()?;
        encode(&header, &claims, &key).ok()
    }

    /// Get the JWKS JSON for use in filter configuration
    pub fn get_jwks_json(&self) -> Option<String> {
        self.jwks.as_ref().map(|j| j.jwks.to_string())
    }

    /// Get the issuer URL for JWT filter configuration
    pub fn get_issuer(&self) -> Option<String> {
        self.jwks.as_ref().map(|j| j.issuer.clone())
    }
}

/// Generate test JWKS configuration with RSA key pair
///
/// Uses a static 2048-bit RSA key pair for testing purposes.
/// These keys are for E2E testing only - NOT for production use.
fn generate_test_jwks() -> JwksConfig {
    let kid = "e2e-test-key-1".to_string();

    // Static RSA 2048-bit private key for testing
    // Generated specifically for E2E tests - NOT for production
    let private_key_pem = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEAu1SU1LfVLPHCozMxH2Mo4lgOEePzNm0tRgeLezV6ffAt0gun
VTLw7onLRnrq0/IzW7yWR7QkrmBL7jTKEn5u+qKhbwKfBstIs+bMY2Zkp18gnTxk
LxUq9fKHjD8BccrxIhowNnkm8d1YFjfplPvl7PoLZd7xqJpPqJ3mziGSLZ5FQy/g
mQXFJhTKdBBglmznPNOs7LXPJhYXRNa/kHlSvfq7zAN7Kj2ZlL8G0xTzHRx0DZSQ
aIkC4Vh1jbjpBxVhFqyKdG64FfYzz4SqFNiD6fXkDdU8mLHPJXuvS9jeFM7+AONX
l2/zWjIK3ofN9fwOOdO4NmPgKqFGfhT/AMmNfwIDAQABAoIBACH5qGdYkd6fP3pP
HN6r/kHVAj/v9dh6ySPgS9wLWpZhNaWr+Xsad4qxmOyN8pS2cA4oiHXuY2q4wJLf
lVJrK8QfLmqbKRHXmw5u7nEXlYqmrPHR7qT9KrRPiNlWQnkGxR0JJmJqPE7n6X8B
7IaM5n6lF7Q9xTCEVVqMDiCVJ8xfj8JzPGzjvs9YmPqaScZRHf8qPJnfk7v2cFOb
8IxqYrvfI/p9z2ryRTvkneDlpJgvchR3F8sqq0HaC1iMsmwN9TlQY7SKYwq3RPEP
bNPOGSX8H1HvJR1zFL3kVqEg6q8FVfH8Jzs4b3qUqdCMvJVxOEX7k7m8TQMZ4wSL
u7bRGAECgYEA7k3KqNwN3fNcZCqHV4PqLJNJGCNm4GRBxmPHH8rN0OWKV5Q3lKCe
k/E2xB7pKZMpJGYwP4c3ND6r1fN0zXJUlzO0oG3gJRhFbKdHC4x9hKJlGfVRmWXm
E6qJlmfmzT0yxQfxPwvHg8VrGXs0X9YLw9QRUjSMpTYYfxH7G8N5U/8CgYEAyXK0
4C0+PKdq2JZJx9hqPwV6bz8KBc8YO3sT+t8d6RixF6hVN7sAqCeXxKJ4q7amLVQp
bUXeSJZL5xHKV7xyWEk6cUNDLbnBBJQTPtb5yMR4+DiNOGRhmy0J4Kn6Pq9cVQ7D
8JXsF9J3xVJAqmH7cPPxpwpPnqvJvOPNTJ/7KgECgYEA2FDLadpGGxnLzninvNlH
iUV9x3K7hDPKg6cKpbBBAUwL6+NLl+IrURHGmnPiPYMvNGEd5+6wEPq7OF+agVYf
L5lDaSMJ7nJP9hnfNd0ZMdPBfNGBCYeC7qMqh7WCJj1n4xfK2osUQmSiUIp6BNfM
9u0C/tN7H7o6sHn7HWOJq+8CgYBKNyrGatpzWGCbNxfzn0Grv0jR8HYU0i0mjs8E
SLg7Gf4EbsJB+wt+1q+2gHFyqhLJqRI6F6s/zBFpx/v3TEEXA4D0ZzijkYX9MFE0
j6CshNPP5bBs4rN3DBvhMe9ee5ZIj0VtnrE9Kqx9TAUG4Rj0bVgMNwWvr1sFKqcq
SwfCAQKBgA5HfxBQpr3v/rK7VTl4Mk7Mx8Y4trO6OC96gNHMz8a1psCnwJYA8fKv
mThKR2lKmrYCphEhwm9nKWN6vp7gDGnWYBslCTy6P5Xr6Z8dYpSoGwN3PLnK4qul
B5kR1T7+hFNKaMJf2gBAGZKy0xHD7KNdaQ2qAQwUqKKEr4XGAiBU
-----END RSA PRIVATE KEY-----"#
        .to_string();

    // Corresponding RSA public key components in JWK format
    // n and e values derived from the private key above
    let jwks = json!({
        "keys": [{
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "kid": &kid,
            "n": "u1SU1LfVLPHCozMxH2Mo4lgOEePzNm0tRgeLezV6ffAt0gunVTLw7onLRnrq0_IzW7yWR7QkrmBL7jTKEn5u-qKhbwKfBstIs-bMY2Zkp18gnTxkLxUq9fKHjD8BccrxIhowNnkm8d1YFjfplPvl7PoLZd7xqJpPqJ3mziGSLZ5FQy_gmQXFJhTKdBBglmznPNOs7LXPJhYXRNa_kHlSvfq7zAN7Kj2ZlL8G0xTzHRx0DZSQaIkC4Vh1jbjpBxVhFqyKdG64FfYzz4SqFNiD6fXkDdU8mLHPJXuvS9jeFM7-AONXl2_zWjIK3ofN9fwOOdO4NmPgKqFGfhT_AMmNfw",
            "e": "AQAB"
        }]
    });

    // Default issuer - will be updated when auth mock starts
    let issuer = "https://e2e-test-auth.local/".to_string();

    JwksConfig { jwks, private_key_pem, kid, issuer }
}

/// Generate test JWKS with the actual mock server URI as issuer
fn generate_test_jwks_with_issuer(issuer: &str) -> JwksConfig {
    let mut config = generate_test_jwks();
    config.issuer = issuer.to_string();
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_mock_services() {
        let mocks = MockServices::start_basic().await;

        // Verify echo server is running
        let addr = mocks.echo_addr();
        assert!(addr.port() > 0);

        // Verify echo endpoint
        let endpoint = mocks.echo_endpoint();
        assert!(endpoint.starts_with("127.0.0.1:"));
    }

    #[tokio::test]
    async fn test_auth_mock_services() {
        let mocks = MockServices::start_with_auth().await;

        // Verify auth server is running
        assert!(mocks.auth_uri().is_some());

        // Test JWKS endpoint
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/.well-known/jwks.json", mocks.auth_uri().unwrap()))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["keys"].is_array());
    }

    #[tokio::test]
    async fn test_echo_returns_request_info() {
        let mocks = MockServices::start_basic().await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/test/path", mocks.echo.uri()))
            .header("X-Custom-Header", "test-value")
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["path"], "/test/path");
        assert_eq!(body["method"], "GET");
    }

    #[tokio::test]
    async fn test_jwt_generation() {
        let mocks = MockServices::start_with_auth().await;

        // Generate a valid JWT
        let jwt = mocks.generate_valid_jwt("test-user", None);
        assert!(jwt.is_some(), "Should generate a valid JWT");

        let token = jwt.unwrap();
        // JWT should have 3 parts separated by dots
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT should have 3 parts: header.payload.signature");

        // Verify the JWT can be decoded (header)
        let header_decoded =
            base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, parts[0])
                .expect("Header should be valid base64url");
        let header: serde_json::Value =
            serde_json::from_slice(&header_decoded).expect("Header should be valid JSON");
        assert_eq!(header["alg"], "RS256");
        assert_eq!(header["kid"], "e2e-test-key-1");

        // Verify issuer matches auth server
        let issuer = mocks.get_issuer().unwrap();
        assert!(issuer.starts_with("http://"), "Issuer should be HTTP URL from mock server");
    }

    #[tokio::test]
    async fn test_expired_jwt_generation() {
        let mocks = MockServices::start_with_auth().await;

        let jwt = mocks.generate_expired_jwt("test-user");
        assert!(jwt.is_some(), "Should generate an expired JWT");

        let token = jwt.unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);

        // Decode payload and verify it's expired
        let payload_decoded =
            base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, parts[1])
                .expect("Payload should be valid base64url");
        let payload: serde_json::Value =
            serde_json::from_slice(&payload_decoded).expect("Payload should be valid JSON");

        let exp = payload["exp"].as_u64().unwrap();
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert!(exp < now, "Token should be expired");
    }

    #[test]
    fn test_invalid_jwt() {
        let invalid = MockServices::generate_invalid_jwt();
        assert_eq!(invalid, "invalid.jwt.token");
    }

    #[tokio::test]
    async fn test_jwks_json_retrieval() {
        let mocks = MockServices::start_with_auth().await;

        let jwks_json = mocks.get_jwks_json();
        assert!(jwks_json.is_some());

        let jwks: serde_json::Value =
            serde_json::from_str(&jwks_json.unwrap()).expect("JWKS should be valid JSON");
        assert!(jwks["keys"].is_array());
        assert_eq!(jwks["keys"][0]["kid"], "e2e-test-key-1");
    }
}
