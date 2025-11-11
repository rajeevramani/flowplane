# Session Management

Flowplane provides secure session-based authentication for web applications and interactive workflows, complementing Personal Access Tokens (PATs) for programmatic API access.

## Overview

The session management system provides:

- **Cookie-based authentication** for web applications
- **CSRF protection** for state-changing operations
- **Setup token bootstrap** for initial system configuration
- **Secure session lifecycle** with automatic expiration
- **Logout functionality** with session revocation

## Authentication Methods Comparison

| Feature | Sessions | Personal Access Tokens |
|---------|----------|----------------------|
| **Use Case** | Web UIs, interactive workflows | API automation, scripts |
| **Authentication** | Cookie + CSRF token | Bearer token |
| **CSRF Protection** | Required for POST/PUT/PATCH/DELETE | Not required |
| **Lifetime** | 24 hours (default) | Configurable (can be long-lived) |
| **Revocation** | Logout endpoint | Token revoke endpoint |

## Bootstrap Flow

### 1. Generate Setup Token

When the system is uninitialized (no active tokens exist), call the bootstrap endpoint:

```bash
POST /api/v1/bootstrap/initialize
Content-Type: application/json

{
  "adminEmail": "admin@example.com"
}
```

**Response:**
```json
{
  "setupToken": "fp_setup_abc123...",
  "expiresAt": "2025-01-18T12:00:00Z",
  "maxUsageCount": 1,
  "message": "Setup token generated successfully...",
  "nextSteps": [...]
}
```

**Important:** This endpoint can ONLY be called when the system is uninitialized.

### 2. Exchange Setup Token for Session

Use the setup token to create an authenticated session:

```bash
POST /api/v1/auth/sessions
Content-Type: application/json

{
  "setupToken": "fp_setup_abc123..."
}
```

**Response:**
```json
{
  "sessionId": "uuid-here",
  "csrfToken": "csrf-token-here",
  "expiresAt": "2025-01-18T12:00:00Z",
  "teams": ["team1", "team2"],
  "scopes": ["bootstrap:initialize", "admin:*"]
}
```

**Response Headers:**
- `Set-Cookie: fp_session=fp_session_uuid.secret; HttpOnly; Secure; SameSite=Strict`
- `X-CSRF-Token: csrf-token-here`

### 3. Use Session to Create PATs

Once you have a session, you can create Personal Access Tokens:

```bash
POST /api/v1/tokens
Cookie: fp_session=fp_session_uuid.secret
X-CSRF-Token: <csrf-token-from-session>
Content-Type: application/json

{
  "name": "my-api-token",
  "description": "My first PAT",
  "scopes": ["clusters:write", "routes:read"],
  "expiresAt": "2026-01-17T00:00:00Z"
}
```

## Session Usage

### GET Requests (No CSRF Required)

For read-only operations, only the session cookie is needed:

```bash
GET /api/v1/auth/sessions/me
Cookie: fp_session=fp_session_uuid.secret
```

**Response:**
```json
{
  "sessionId": "uuid",
  "teams": ["team1"],
  "scopes": ["admin:*"],
  "expiresAt": "2025-01-18T12:00:00Z"
}
```

### POST/PUT/PATCH/DELETE Requests (CSRF Required)

For state-changing operations, both cookie and CSRF token are required:

```bash
POST /api/v1/clusters
Cookie: fp_session=fp_session_uuid.secret
X-CSRF-Token: <csrf-token>
Content-Type: application/json

{
  "name": "my-cluster",
  "endpoints": [...]
}
```

**If CSRF token is missing or invalid:** `403 Forbidden`

## Logout

To terminate a session and clear credentials:

```bash
POST /api/v1/auth/sessions/logout
Cookie: fp_session=fp_session_uuid.secret
X-CSRF-Token: <csrf-token>
```

**Response:** `204 No Content`

**Response Headers:**
- `Set-Cookie: fp_session=; Max-Age=0` (clears cookie)

After logout, the session is revoked and cannot be used for further requests.

## Security Features

### Session Token Format

Session tokens use the format: `fp_session_{uuid}.{secret}`

- **UUID:** Random session identifier
- **Secret:** 64 bytes of cryptographically secure random data, hashed with Argon2id before storage
- **Prefix:** `fp_session_` to distinguish from PATs (`fp_pat_`)

### CSRF Protection

CSRF tokens are required for all state-changing requests (POST/PUT/PATCH/DELETE) when using session authentication:

1. **Token Generation:** 32 bytes of cryptographically secure random data (256 bits of entropy)
2. **Storage:** Stored in database, associated with session
3. **Validation:** Server validates token on each state-changing request
4. **Header:** Must be sent via `X-CSRF-Token` header

### Cookie Security

Session cookies are configured with maximum security:

- `HttpOnly`: Cannot be accessed by JavaScript
- `Secure`: Only sent over HTTPS (configurable for development)
- `SameSite=Strict`: Maximum CSRF protection at the browser level
- `Path=/`: Available for all endpoints

### Session Expiration

- **Default Lifetime:** 24 hours
- **Automatic Expiration:** Sessions are validated on each request
- **Manual Revocation:** Via logout endpoint
- **Setup Token Expiration:** 7 days (default), single-use

## Error Handling

| Status Code | Meaning | Resolution |
|------------|---------|------------|
| `401 Unauthorized` | Missing or invalid session token | Login again |
| `403 Forbidden` | Missing or invalid CSRF token | Include valid X-CSRF-Token header |
| `403 Forbidden` | System already initialized | Use existing credentials |
| `404 Not Found` | Setup token not found | Generate new setup token |

## Configuration

Environment variables for session management:

```bash
# Setup token configuration
FLOWPLANE_SETUP_TOKEN_TTL_DAYS=7          # Setup token expiration (days)
FLOWPLANE_SETUP_TOKEN_MAX_USAGE=1         # Maximum uses per setup token

# Session configuration
FLOWPLANE_SESSION_EXPIRATION_HOURS=24     # Session lifetime (hours)
```

## Best Practices

1. **Use Sessions for Web UIs:** Sessions with cookies and CSRF protection are ideal for browser-based applications
2. **Use PATs for APIs:** Programmatic access should use long-lived Personal Access Tokens
3. **Protect CSRF Tokens:** Never log or expose CSRF tokens in client-side code
4. **Implement Logout:** Always provide a logout button that calls the logout endpoint
5. **Handle Expiration:** Implement token refresh or re-authentication when sessions expire
6. **Secure Cookies:** Always use HTTPS in production to protect session cookies
7. **Store Tokens Securely:** Save CSRF tokens in memory, not localStorage (XSS vulnerability)

## Troubleshooting

### "System already initialized" Error

**Problem:** Bootstrap endpoint returns 403 Forbidden

**Solution:** The system already has tokens. Use existing credentials or access the database directly to reset.

### "CSRF token required" Error

**Problem:** POST/PUT/PATCH/DELETE request returns 403 Forbidden

**Solution:** Include the `X-CSRF-Token` header with the token received during session creation.

### Session Expired

**Problem:** Requests return 401 Unauthorized

**Solution:** Session has expired (default: 24 hours). Create a new session by logging in again.

### Setup Token Already Used

**Problem:** Setup token returns "exceeded usage limit"

**Solution:** Setup tokens are single-use. If bootstrap failed, generate a new setup token by restarting with an empty database.

## API Reference

### Bootstrap Endpoint

- **Path:** `POST /api/v1/bootstrap/initialize`
- **Authentication:** None (only works on uninitialized system)
- **Request:** `{ "adminEmail": "admin@example.com" }`
- **Response:** Setup token details

### Session Creation

- **Path:** `POST /api/v1/auth/sessions`
- **Authentication:** None
- **Request:** `{ "setupToken": "fp_setup_..." }`
- **Response:** Session details with CSRF token
- **Headers:** `Set-Cookie`, `X-CSRF-Token`

### Session Info

- **Path:** `GET /api/v1/auth/sessions/me`
- **Authentication:** Session cookie
- **Response:** Current session information

### Logout

- **Path:** `POST /api/v1/auth/sessions/logout`
- **Authentication:** Session cookie + CSRF token
- **Response:** 204 No Content
- **Headers:** Cookie clearing directive

## See Also

- [Token Management](./token-management.md) - Personal Access Token (PAT) documentation
- [Authentication](./authentication.md) - General authentication overview
- [Security Best Practices](#) - Security guidelines
- [Frontend Integration Guide](#) - Web application integration examples
