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
use wiremock::matchers::{method, path, path_regex};
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
    ///
    /// The echo mock is "smart" and supports several path patterns:
    /// - `/status/{code}` - returns that status code (like httpbin)
    /// - Path ending in `/fail` - returns 500
    /// - Path ending in `/error` - returns 500
    /// - Path containing `/503` - returns 503
    /// - All other paths return 200 with request info
    pub async fn start_basic() -> Self {
        let echo = MockServer::start().await;

        // Setup smart echo endpoint that returns different status codes based on path
        Mock::given(method("GET"))
            .and(path_regex(r".*"))
            .respond_with(|req: &Request| {
                let path = req.url.path();
                let status_code = determine_status_code_from_path(path);

                let body = json!({
                    "path": path,
                    "method": req.method.to_string(),
                    "headers": req.headers.iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect::<std::collections::HashMap<_, _>>(),
                    "status": status_code,
                });
                ResponseTemplate::new(status_code)
                    .set_body_json(body)
                    .insert_header("content-type", "application/json")
            })
            .mount(&echo)
            .await;

        Mock::given(method("POST"))
            .and(path_regex(r".*"))
            .respond_with(|req: &Request| {
                let path = req.url.path();
                let status_code = determine_status_code_from_path(path);

                let body = json!({
                    "path": path,
                    "method": req.method.to_string(),
                    "headers": req.headers.iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect::<std::collections::HashMap<_, _>>(),
                    "body": String::from_utf8_lossy(&req.body).to_string(),
                    "status": status_code,
                });
                ResponseTemplate::new(status_code)
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

        // Debug mock: Returns what headers were received
        // This helps debug whether headers are being forwarded correctly
        // Note: Envoy's HTTP ext_authz uses the same method as the original request
        // So we need to accept both GET and POST (any method starting with /auth)
        Mock::given(path_regex("^/auth.*"))
            .respond_with(|req: &Request| {
                // Log all received headers for debugging
                let header_names: Vec<String> = req
                    .headers
                    .iter()
                    .map(|(name, value)| format!("{}={}", name, value.to_str().unwrap_or("")))
                    .collect();
                println!("[ext_authz mock] Received headers: {:?}", header_names);
                println!("[ext_authz mock] Request path: {}", req.url.path());

                // Check if x-ext-authz-allow header is present
                let allow_header = req
                    .headers
                    .iter()
                    .find(|(name, _)| name.as_str().eq_ignore_ascii_case("x-ext-authz-allow"));

                if allow_header.is_some() {
                    println!("[ext_authz mock] ALLOW - header found");
                    ResponseTemplate::new(200).set_body_json(json!({
                        "status": {"code": 0},
                        "ok_response": {
                            "headers": [
                                {"header": {"key": "x-ext-authz-check-received", "value": "true"}}
                            ]
                        }
                    }))
                } else {
                    println!("[ext_authz mock] DENY - header not found");
                    ResponseTemplate::new(403).set_body_json(json!({
                        "status": {"code": 7, "message": "PERMISSION_DENIED"},
                        "denied_response": {
                            "status": {"code": "Forbidden"},
                            "body": "Access denied by ext_authz"
                        }
                    }))
                }
            })
            .mount(&ext_authz)
            .await;

        services.ext_authz = Some(ext_authz);
        services
    }

    /// Start all mock services
    pub async fn start_all() -> Self {
        let mut services = Self::start_with_auth().await;
        let ext_authz = MockServer::start().await;

        // ext_authz mock that checks for x-ext-authz-allow header
        // - If header is present: return 200 (allow)
        // - If header is missing: return 403 (deny)
        // Note: Envoy's HTTP ext_authz uses the same method as the original request
        Mock::given(path_regex("^/auth.*"))
            .respond_with(|req: &Request| {
                // Log all received headers for debugging
                let header_names: Vec<String> = req
                    .headers
                    .iter()
                    .map(|(name, value)| format!("{}={}", name, value.to_str().unwrap_or("")))
                    .collect();
                println!("[ext_authz mock] Received headers: {:?}", header_names);
                println!("[ext_authz mock] Request path: {}", req.url.path());

                // Check if x-ext-authz-allow header is present
                let allow_header = req
                    .headers
                    .iter()
                    .find(|(name, _)| name.as_str().eq_ignore_ascii_case("x-ext-authz-allow"));

                if allow_header.is_some() {
                    println!("[ext_authz mock] ALLOW - header found");
                    ResponseTemplate::new(200).set_body_json(json!({
                        "status": {"code": 0},
                        "ok_response": {
                            "headers": [
                                {"header": {"key": "x-ext-authz-check-received", "value": "true"}}
                            ]
                        }
                    }))
                } else {
                    println!("[ext_authz mock] DENY - header not found");
                    ResponseTemplate::new(403).set_body_json(json!({
                        "status": {"code": 7, "message": "PERMISSION_DENIED"},
                        "denied_response": {
                            "status": {"code": "Forbidden"},
                            "body": "Access denied by ext_authz"
                        }
                    }))
                }
            })
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

/// Determine HTTP status code based on path patterns
///
/// Supports:
/// - `/status/{code}` - returns that status code (like httpbin)
/// - Path ending in `/fail` - returns 500
/// - Path ending in `/error` - returns 500
/// - Path containing `/503` - returns 503
/// - Path containing `/500` - returns 500
/// - Path containing `/endpoint1` with `/multi` - returns 500 (for multi-endpoint tests)
/// - All other paths return 200
fn determine_status_code_from_path(path: &str) -> u16 {
    // Check for /status/{code} pattern (like httpbin)
    if let Some(status_part) = path.strip_prefix("/status/") {
        if let Ok(code) = status_part.split('/').next().unwrap_or("200").parse::<u16>() {
            if (100..=599).contains(&code) {
                return code;
            }
        }
    }

    // Check for paths ending in /fail or /error
    if path.ends_with("/fail") || path.ends_with("/error") {
        return 500;
    }

    // Check for paths containing specific status codes
    if path.contains("/503") {
        return 503;
    }
    if path.contains("/500") {
        return 500;
    }
    if path.contains("/502") {
        return 502;
    }
    if path.contains("/504") {
        return 504;
    }
    if path.contains("/429") {
        return 429;
    }
    if path.contains("/401") {
        return 401;
    }
    if path.contains("/403") {
        return 403;
    }
    if path.contains("/404") {
        return 404;
    }

    // Special case for multi-endpoint outlier detection tests
    // /testing/multi/endpoint1 should return 500, endpoint2 returns 200
    if path.contains("/multi/endpoint1") {
        return 500;
    }

    // Default to 200
    200
}

/// Generate test JWKS configuration with RSA key pair
///
/// Uses a static 2048-bit RSA key pair for testing purposes.
/// These keys are for E2E testing only - NOT for production use.
fn generate_test_jwks() -> JwksConfig {
    let kid = "e2e-test-key-1".to_string();

    // Static RSA 2048-bit private key for testing (PKCS#1 format)
    // Generated specifically for E2E tests - NOT for production
    let private_key_pem = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEogIBAAKCAQEA0hM+IgJX+PIXXcPUnx5lNyTPlXbll++juKxgsL6iEYjy25uD
J8up+m5NxKeiE4WqYm5oQ1dcCfKX/8tKAclF1bjLPd75j0ckvixHa3hgT9VXPTx5
1zFkYmiVNeNoWF9mT0VlDZuujf82XGkyIDzSFS0C4nzDxXxTqPiRmYzflrDqTCmF
TnDIkamht4kgaeflqA7kn7Xu+e3j9Lo8zJn0YiddmOOCOpFbYCFvxJuFrHwBCdQn
IBIcqDwpnAzJG7LHBhxsgahSyaUQx/z9IUrRotIq0pc0xJX6Fp0kLokBAhklKGs1
wPZvAD3Y/DgZj4+UuSGvflXhU/qrBJLsuOMBrQIDAQABAoIBAAP3whOBXikBED5A
oyTy6DAqR/4c4J7wiJ6bKY5dDhGXt8KRxYs8YQmuXgCqC+POLAGE7/+JnbRAZUI7
FDruvXIZIJmauwr2PzQ/q4T/oTi7dlTdQ2M0THRBYRlt91Fn/TY1FiteITtkXSH8
s1TW4T7uJTZNlfhbw0wXOc91JjSh5QfdetbcT1fU9EKpUI55KDSVcN1YpFfACs87
la0lYgoOcimJ2XJXzoPZKkXMTlPbM0Hz5U87RGF3vN+Re4ZHKIEXKxrfAQy6cbPZ
zO1o+x0D/sILNJGY0rInzszrMpoP223+vngB5UKAxB5GhnqaMjDnc1HsDGweEQbj
F6Tl5c8CgYEA9Iz0fDOeEpwrYmQW6WQYOuJApjXPhTPngeuWjyE7i1nFl1KrN3Mn
/Zk5dQdVaiV4I3t81NgPsLOpvWVXvk4yR/G6Vc+aDmGboMaA8g6sRVK0LEbwSNJ+
8xyOFdcEPKK1y/79ZmjRlvRy+iIp8n154ylFrQebZOwKlUCdfN89nW8CgYEA2+kV
qT7YvDP1SJvQrvL7RIXV8Ocfuu968ba/DFeQE3WMOraV+0EIvr1+TraJQuRIMY80
oEIHz2JDAUM2+HIn/bN9c3FndFbE7YPGrf9wbIiN/vN60BEZjUWUSYMw5nFse/iw
RffvXKsrxZPY2H1ibfKm/BQ1kmWzHUE6b+L6fKMCgYBjFBiZmXAZqhwJqPN/a4ZF
lRUMQhDprrXE9WXyZ0xwkNZ1EJE9zfIN1N5qg6Yfcz7RYV6Z/U+eD6xdh4mdGKFW
dKFB0vJfkTw0Tzg+2aMCExfcOIFxf5bfeFo4jvywdFujYpPXwe/ocPGEVgMYs62G
U1pfWA2lPdyry5oC1Y9pEQKBgBUYnCJbTBFp7prjj7Zoyt/88tQkZ+/X73Rmspct
gz3KpgQv5d1vlLYvmYFVk39eROq0MTk6fGNRqtnhJ9HXqax13pAHjgQkGsoqPRIO
EivnQa/2jY6ORWQ/C4Wt1zAUK3MNHWPo8AZ0yUMv9rp19M5VW92M1sLPjMo+qqt3
G85/AoGAM6Wc9OlsZ9lZWQgS0FeCUGQctMOvfLArd/Lxr2LPy2VUEEDGZzRjm8bh
0PS0AbYJ3P6hR1ZIt5wC+as5pm0ZKnuDxydFeSea1Mv38u9osDguG5Kh+EG2epX3
QIdRZxu7xYgyFnbR+TKBq55ejJr03C5A5Q74s3se2K9qAoMiss4=
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
            "n": "0hM-IgJX-PIXXcPUnx5lNyTPlXbll--juKxgsL6iEYjy25uDJ8up-m5NxKeiE4WqYm5oQ1dcCfKX_8tKAclF1bjLPd75j0ckvixHa3hgT9VXPTx51zFkYmiVNeNoWF9mT0VlDZuujf82XGkyIDzSFS0C4nzDxXxTqPiRmYzflrDqTCmFTnDIkamht4kgaeflqA7kn7Xu-e3j9Lo8zJn0YiddmOOCOpFbYCFvxJuFrHwBCdQnIBIcqDwpnAzJG7LHBhxsgahSyaUQx_z9IUrRotIq0pc0xJX6Fp0kLokBAhklKGs1wPZvAD3Y_DgZj4-UuSGvflXhU_qrBJLsuOMBrQ",
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
