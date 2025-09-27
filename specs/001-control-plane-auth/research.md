# Research: Control Plane API Authentication System

## Technical Decisions & Research Findings

### Token Storage & Security

**Decision**: Use Argon2id for token hashing with secure random token generation
**Rationale**:
- Argon2id is the current OWASP recommended password hashing algorithm
- Provides protection against timing attacks and GPU-based attacks
- Memory-hard function makes brute force attacks expensive
- Rust `argon2` crate is well-maintained and follows security best practices

**Alternatives Considered**:
- bcrypt: Older algorithm, less secure against modern attacks
- PBKDF2: More vulnerable to specialized hardware attacks
- Plain SHA-256: Completely insecure for password/token storage

### Token Generation Strategy

**Decision**: Use cryptographically secure random bytes with Base64URL encoding
**Rationale**:
- 32 bytes of entropy = 256 bits of security (sufficient for enterprise use)
- Base64URL encoding produces URL-safe tokens without padding
- `rand::thread_rng()` provides cryptographically secure randomness
- Prefixed tokens (fp_xxxx) for easy identification and rotation

**Alternatives Considered**:
- UUID v4: Only 122 bits of entropy, insufficient for high-security tokens
- Custom encoding: Higher risk of implementation errors
- JWT as opaque tokens: Unnecessary complexity for simple bearer tokens

### Database Schema Design

**Decision**: Dedicated auth tables with foreign key relationships to existing audit_log
**Rationale**:
- `personal_access_tokens` table for token storage (id, name, hash, scopes, metadata)
- `token_scopes` junction table for flexible scope assignment
- Leverages existing `audit_log` table for authentication events
- Supports token metadata (creation date, last used, creator identity)

**Schema Design**:
```sql
CREATE TABLE personal_access_tokens (
    id UUID PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    token_hash VARCHAR(255) NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_by VARCHAR(255),
    status VARCHAR(50) NOT NULL DEFAULT 'active',
    INDEX idx_token_hash (token_hash),
    INDEX idx_status (status)
);

CREATE TABLE token_scopes (
    token_id UUID REFERENCES personal_access_tokens(id) ON DELETE CASCADE,
    scope VARCHAR(100) NOT NULL,
    PRIMARY KEY (token_id, scope)
);
```

### Scope Permission Model

**Decision**: Hierarchical resource:action pattern (e.g., "clusters:read", "listeners:write")
**Rationale**:
- Clear, intuitive naming convention
- Easy to extend with new resources and actions
- Simple string matching for authorization checks
- Supports both read and write permissions granularly

**Scope Definitions**:
- `clusters:read` - View cluster configurations
- `clusters:write` - Create, update, delete clusters
- `routes:read` - View route configurations
- `routes:write` - Create, update, delete routes
- `listeners:read` - View listener configurations
- `listeners:write` - Create, update, delete listeners
- `admin:read` - View system information
- `admin:write` - System administration (token management)

### Axum Middleware Integration

**Decision**: Custom authentication middleware using Axum's middleware system
**Rationale**:
- Integrates seamlessly with existing Axum application structure
- Provides request-level authentication context
- Supports both optional and required authentication per endpoint
- Easy to test and maintain with clear separation of concerns

**Implementation Pattern**:
```rust
pub struct AuthMiddleware<S> {
    inner: S,
    auth_service: Arc<AuthService>,
}

impl<S> Service<Request<Body>> for AuthMiddleware<S> {
    // Extract bearer token, validate, inject auth context
}
```

### JWT/OIDC Extension Path

**Decision**: Use `jsonwebtoken` crate with pluggable authenticator trait
**Rationale**:
- Well-maintained crate with good security track record
- Supports all standard JWT algorithms (RS256, ES256, HS256)
- Easy to extend current personal access token system
- Allows gradual migration from opaque tokens to JWT/OIDC

**Extension Architecture**:
```rust
#[async_trait]
pub trait Authenticator {
    async fn authenticate(&self, token: &str) -> Result<AuthContext, AuthError>;
}

pub struct PersonalAccessTokenAuth { /* ... */ }
pub struct JwtAuth { /* ... */ }  // Future implementation
```

### Audit Logging Integration

**Decision**: Extend existing audit_log table with authentication events
**Rationale**:
- Reuses existing audit infrastructure
- Maintains consistent logging format across the application
- Supports correlation IDs for request tracing
- Easy to query and analyze authentication patterns

**Additional Audit Event Types**:
- `auth.token.created` - Token creation events via API
- `auth.token.seeded` - Bootstrap token creation and system-level provisioning
- `auth.token.revoked` - Token revocation events
- `auth.request.authenticated` - Successful authentication
- `auth.request.failed` - Authentication failures
- `auth.request.forbidden` - Authorization failures

### Performance Considerations

**Decision**: In-memory token hash cache with TTL for high-frequency validation
**Rationale**:
- Database queries for every request would create performance bottleneck
- Cache frequently used token hashes to reduce database load
- Short TTL (5 minutes) ensures revoked tokens are quickly invalidated
- Memory usage stays bounded with LRU eviction

**Cache Strategy**:
- Use `moka` crate for async-aware LRU cache
- Cache token validation results, not raw tokens
- Implement cache warming for active tokens
- Monitor hit rates and adjust TTL based on usage patterns

### Error Handling & Security

**Decision**: Structured error types with careful information disclosure
**Rationale**:
- Use `thiserror` for comprehensive error modeling
- Different error types for authentication vs authorization failures
- Avoid timing attacks by using constant-time comparisons
- Log security events without exposing sensitive data to API responses

**Error Categories**:
- `AuthError::InvalidToken` - Malformed or invalid token (401)
- `AuthError::ExpiredToken` - Token past expiration (401)
- `AuthError::InsufficientScopes` - Missing required permissions (403)
- `AuthError::RevokedToken` - Token has been revoked (401)
- `AuthError::Internal` - System errors (500, logged but not exposed)

## Research Validation

All technical decisions support constitutional requirements:
- ✅ Structured configs with strong typing
- ✅ Early validation with descriptive errors
- ✅ Comprehensive test coverage planned
- ✅ Idempotent operations with atomic database transactions
- ✅ Security by default with industry best practices
- ✅ No breaking changes to existing functionality
- ✅ DRY principles with reusable auth traits and proper error handling