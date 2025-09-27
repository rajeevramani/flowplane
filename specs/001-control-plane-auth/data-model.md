# Data Model: Control Plane API Authentication System

## Core Entities

### PersonalAccessToken
**Purpose**: Represents a secure authentication credential for API access
**Lifecycle**: Created by admin → Active → Revoked/Expired → Archived

**Attributes**:
- `id: UUID` - Unique identifier (primary key)
- `name: String` - Human-readable name/description
- `token_hash: String` - Argon2id hashed token (unique)
- `scopes: Vec<String>` - Granted permissions (e.g., ["clusters:read", "listeners:write"])
- `created_at: DateTime<Utc>` - Creation timestamp
- `expires_at: Option<DateTime<Utc>>` - Optional expiration date
- `last_used_at: Option<DateTime<Utc>>` - Last successful authentication
- `created_by: String` - Identity of creator
- `status: TokenStatus` - Current state (Active, Revoked, Expired)

**Validation Rules**:
- Name must be 1-255 characters, alphanumeric with spaces and hyphens
- Token hash must be valid Argon2id format
- Scopes must be from valid scope list (no custom scopes)
- Expires_at must be in the future if specified
- Created_by must be non-empty string

**State Transitions**:
```
Created → Active (default initial state)
Active → Revoked (explicit admin action)
Active → Expired (automatic when expires_at reached)
Revoked → Revoked (no state change)
Expired → Expired (no state change)
```

### TokenScope
**Purpose**: Defines granular permissions for API operations
**Pattern**: `{resource}:{action}` (e.g., "clusters:read")

**Valid Scopes**:
- `clusters:read` - View cluster configurations and status
- `clusters:write` - Create, update, delete cluster configurations
- `routes:read` - View route configurations and status
- `routes:write` - Create, update, delete route configurations
- `listeners:read` - View listener configurations and status
- `listeners:write` - Create, update, delete listener configurations
- `admin:read` - View system information and token metadata
- `admin:write` - Create, revoke tokens and system administration

**Validation Rules**:
- Must match pattern: `^(clusters|routes|listeners|admin):(read|write)$`
- Write permission implies read permission for same resource
- Admin scopes required for token management operations

### AuthContext
**Purpose**: Represents validated authentication session for a request
**Lifetime**: Single HTTP request

**Attributes**:
- `token_id: UUID` - Reference to authenticated token
- `scopes: HashSet<String>` - Available permissions
- `correlation_id: String` - Request tracing identifier
- `authenticated_at: DateTime<Utc>` - Authentication timestamp

**Methods**:
- `has_scope(&self, required: &str) -> bool` - Check permission
- `has_any_scope(&self, required: &[String]) -> bool` - Check any permission
- `has_all_scopes(&self, required: &[String]) -> bool` - Check all permissions

### AuditLogEntry
**Purpose**: Records authentication and authorization events
**Extension**: Builds on existing audit_log table structure

**Authentication Event Types**:
- `auth.token.created` - Token creation with metadata
- `auth.token.revoked` - Token revocation event
- `auth.token.expired` - Token expiration (automatic)
- `auth.request.authenticated` - Successful authentication
- `auth.request.failed` - Authentication failure
- `auth.request.forbidden` - Authorization failure (insufficient scopes)

**Event Metadata**:
```json
{
  "token_id": "uuid",
  "token_name": "string",
  "requested_scopes": ["scope1", "scope2"],
  "granted_scopes": ["scope1"],
  "user_agent": "string",
  "source_ip": "ip_address",
  "endpoint": "string",
  "method": "string"
}
```

## Database Schema

### personal_access_tokens Table
```sql
CREATE TABLE personal_access_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(255) NOT NULL,
    token_hash VARCHAR(255) NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_by VARCHAR(255) NOT NULL,
    status VARCHAR(50) NOT NULL DEFAULT 'active',

    -- Indexes for performance
    CONSTRAINT valid_status CHECK (status IN ('active', 'revoked', 'expired')),
    INDEX idx_token_hash (token_hash),
    INDEX idx_status (status),
    INDEX idx_created_by (created_by),
    INDEX idx_expires_at (expires_at)
);
```

### token_scopes Junction Table
```sql
CREATE TABLE token_scopes (
    token_id UUID NOT NULL REFERENCES personal_access_tokens(id) ON DELETE CASCADE,
    scope VARCHAR(100) NOT NULL,

    PRIMARY KEY (token_id, scope),

    -- Validate scope format
    CONSTRAINT valid_scope CHECK (
        scope ~ '^(clusters|routes|listeners|admin):(read|write)$'
    ),

    INDEX idx_scope (scope)
);
```

### Audit Log Extension
```sql
-- Extend existing audit_log table with auth-specific indexes
CREATE INDEX IF NOT EXISTS idx_audit_log_event_type_auth
ON audit_log (event_type)
WHERE event_type LIKE 'auth.%';

CREATE INDEX IF NOT EXISTS idx_audit_log_correlation_id
ON audit_log (correlation_id)
WHERE correlation_id IS NOT NULL;
```

## Entity Relationships

```
PersonalAccessToken (1) → (N) TokenScope
PersonalAccessToken (1) → (N) AuditLogEntry
AuthContext → PersonalAccessToken (references via token_id)
AuthContext → TokenScope (contains granted scopes)
```

## Validation & Business Rules

### Token Creation Rules
1. Token name must be unique per creator
2. Maximum 10 active tokens per creator (configurable)
3. Expiration date cannot exceed 1 year from creation
4. Creator cannot grant scopes they don't possess
5. Admin scopes require existing admin privileges

### Scope Authorization Rules
1. Read operations require corresponding :read scope
2. Write operations require corresponding :write scope
3. Write scope automatically includes read scope for same resource
4. Token management requires admin:write scope
5. Cross-resource operations require scopes for all affected resources

### Security Constraints
1. Token values never stored in plaintext
2. Failed authentication attempts are rate-limited
3. Expired tokens are automatically marked as expired (daily cleanup job)
4. Revoked tokens are immediately unusable
5. All authentication events are audited with correlation IDs

## Usage Patterns

### Token Creation Flow
```
1. Admin specifies token name, scopes, expiration
2. System validates admin has required scopes to delegate
3. Generate secure random token (32 bytes)
4. Hash token with Argon2id
5. Store token record with metadata
6. Return raw token ONE TIME ONLY
7. Log creation event to audit log
```

### Authentication Flow
```
1. Extract Bearer token from Authorization header
2. Hash provided token with timing-attack protection
3. Query database for matching token hash
4. Validate token is active and not expired
5. Load granted scopes for token
6. Create AuthContext with token info
7. Update last_used_at timestamp
8. Log successful authentication
```

### Authorization Flow
```
1. Extract required scopes from endpoint definition
2. Check AuthContext has all required scopes
3. If authorized: proceed with request
4. If unauthorized: return 403 Forbidden
5. Log authorization decision to audit log
```