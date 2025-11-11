# Security Best Practices

This document outlines security best practices for deploying and using Flowplane's authentication system.

## Production Deployment

### HTTPS Only

**Always use HTTPS in production.** Session cookies have the `Secure` flag, which means they will only be transmitted over HTTPS.

```nginx
# Nginx configuration example
server {
    listen 443 ssl http2;
    server_name flowplane.example.com;

    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_ciphers HIGH:!aNULL:!MD5;

    location / {
        proxy_pass http://localhost:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

### Session Cookie Configuration

Flowplane sets secure cookie attributes by default:

- `HttpOnly`: Prevents JavaScript access (XSS protection)
- `Secure`: Only sent over HTTPS
- `SameSite=Strict`: Maximum CSRF protection
- `Path=/`: Available for all endpoints

**For development only**, you can disable the `Secure` flag:

```bash
FLOWPLANE_COOKIES_SECURE=false cargo run
```

**Never disable this in production.**

## Token Management

### Setup Tokens

Setup tokens are designed for first-time bootstrap only:

1. **Single-use**: Automatically revoked after creating a session
2. **Time-limited**: Default 7 days expiration
3. **Secure storage**: Hashed with Argon2id before database storage
4. **Failed attempt tracking**: Locked after 5 failed attempts

**Best Practices:**
- Generate setup tokens only when needed
- Use setup tokens immediately
- Never commit setup tokens to version control
- Treat setup tokens like passwords

### Personal Access Tokens (PATs)

For programmatic API access:

**DO:**
- ✅ Use scoped tokens with minimal required permissions
- ✅ Set expiration dates on tokens
- ✅ Rotate tokens regularly
- ✅ Revoke unused tokens immediately
- ✅ Store tokens in secure secret management systems (Vault, AWS Secrets Manager, etc.)
- ✅ Use different tokens for different environments (dev, staging, prod)

**DON'T:**
- ❌ Share tokens between users or applications
- ❌ Commit tokens to version control
- ❌ Log tokens in application logs
- ❌ Send tokens via email or chat
- ❌ Store tokens in plain text files
- ❌ Use tokens with `admin:*` scope unless absolutely necessary

### Session Tokens

**DO:**
- ✅ Store CSRF tokens in `sessionStorage`, never `localStorage`
- ✅ Clear session storage on logout
- ✅ Implement automatic logout on session expiration
- ✅ Use short session lifetimes (default: 24 hours)

**DON'T:**
- ❌ Store CSRF tokens in `localStorage` (XSS vulnerability)
- ❌ Share sessions between users
- ❌ Log CSRF tokens
- ❌ Expose CSRF tokens in URLs or error messages

## CSRF Protection

### How It Works

1. Server generates CSRF token during session creation
2. Client stores token in memory (`sessionStorage`)
3. Client includes token in `X-CSRF-Token` header for state-changing requests
4. Server validates token matches stored value

### Implementation Requirements

**Required for:**
- POST requests
- PUT requests
- PATCH requests
- DELETE requests

**Not required for:**
- GET requests (read-only)
- OPTIONS requests (CORS preflight)
- PAT authentication (immune to CSRF)

### Frontend Best Practices

```javascript
// ✅ GOOD: Store in sessionStorage
sessionStorage.setItem('csrfToken', token);

// ❌ BAD: Store in localStorage (persists across sessions)
localStorage.setItem('csrfToken', token);

// ❌ BAD: Store in cookie (CSRF vulnerability)
document.cookie = `csrf=${token}`;

// ❌ BAD: Expose in URL
window.location.href = `/api?csrf=${token}`;
```

## XSS Prevention

### Content Security Policy

Implement a strict CSP:

```html
<meta http-equiv="Content-Security-Policy"
      content="default-src 'self';
               script-src 'self';
               style-src 'self' 'unsafe-inline';
               img-src 'self' data:;
               connect-src 'self';">
```

### Input Sanitization

Always sanitize user inputs:

```javascript
// ✅ Use a library like DOMPurify
import DOMPurify from 'dompurify';

function displayUserInput(input) {
  const clean = DOMPurify.sanitize(input);
  element.innerHTML = clean;
}

// ❌ Never directly inject user input
element.innerHTML = userInput; // Dangerous!
```

### Token Storage

```javascript
// ✅ GOOD: In-memory or sessionStorage
const token = sessionStorage.getItem('csrfToken');

// ❌ BAD: In DOM attributes
<div data-csrf="${csrfToken}"></div>

// ❌ BAD: In global variables
window.csrfToken = token;
```

## Scope Management

### Principle of Least Privilege

Grant tokens the minimum scopes required:

```bash
# ✅ GOOD: Specific scopes
{
  "scopes": ["clusters:read", "routes:read"]
}

# ❌ BAD: Overly broad scopes
{
  "scopes": ["admin:*"]
}
```

### Team Isolation

Use team-scoped tokens when possible:

```bash
# ✅ Team-specific access
{
  "scopes": ["team:payments:clusters:write", "team:payments:routes:read"]
}

# ❌ Cross-team access
{
  "scopes": ["clusters:write", "routes:write"]
}
```

## Audit Logging

All authentication events are automatically logged:

- Token creation
- Token rotation
- Token revocation
- Session creation
- Session logout
- Failed authentication attempts
- Setup token generation

**Review logs regularly** for suspicious activity:

```sql
-- Check for failed login attempts
SELECT * FROM audit_log
WHERE action LIKE '%failed%'
AND created_at > datetime('now', '-7 days')
ORDER BY created_at DESC;

-- Check for token creation
SELECT * FROM audit_log
WHERE action = 'token.created'
AND created_at > datetime('now', '-30 days');
```

## Rate Limiting

Implement rate limiting to prevent brute force attacks:

```nginx
# Nginx rate limiting
limit_req_zone $binary_remote_addr zone=auth:10m rate=5r/m;

location /api/v1/auth/ {
    limit_req zone=auth burst=10;
    proxy_pass http://localhost:8080;
}
```

## Network Security

### Firewall Rules

Restrict access to the control plane:

```bash
# Only allow from trusted networks
iptables -A INPUT -p tcp --dport 8080 -s 10.0.0.0/8 -j ACCEPT
iptables -A INPUT -p tcp --dport 8080 -j DROP
```

### Database Security

1. **Encryption at Rest**: Enable database encryption
2. **Network Isolation**: Database should not be publicly accessible
3. **Strong Credentials**: Use strong database passwords
4. **Regular Backups**: Backup encryption keys and database

```bash
# SQLite encryption example
FLOWPLANE_DATABASE_URL="sqlite:/path/to/encrypted.db?key=your-encryption-key"
```

## Monitoring and Alerting

### Key Metrics to Monitor

1. **Failed Authentication Attempts**
   - Alert threshold: > 10 failures per minute

2. **Token Creation Rate**
   - Alert threshold: Unusual spikes

3. **Session Expiration Rate**
   - Alert threshold: High rate may indicate attack

4. **Database Connection Errors**
   - Alert threshold: Any error

### Example Prometheus Queries

```promql
# Failed auth rate
rate(flowplane_auth_failures_total[5m]) > 0.1

# Active sessions
flowplane_active_sessions > 1000

# Token creation anomalies
rate(flowplane_tokens_created_total[1h]) > (rate(flowplane_tokens_created_total[24h]) * 2)
```

## Incident Response

### Compromise Response Checklist

If a token or session is compromised:

1. **Immediate Actions:**
   - [ ] Revoke compromised token/session
   - [ ] Review audit logs for suspicious activity
   - [ ] Identify affected resources
   - [ ] Change affected secrets/passwords

2. **Investigation:**
   - [ ] Determine scope of compromise
   - [ ] Identify entry point
   - [ ] Check for lateral movement

3. **Recovery:**
   - [ ] Rotate all potentially affected credentials
   - [ ] Apply security patches
   - [ ] Update security policies
   - [ ] Document incident

### Revoking All Sessions

```bash
# Emergency: Revoke all active tokens
DELETE FROM personal_access_tokens WHERE status = 'active';
```

**Note:** This will force all users to re-authenticate.

## Development vs Production

### Development Settings

```bash
# Development only
FLOWPLANE_COOKIES_SECURE=false
FLOWPLANE_LOG_LEVEL=debug
FLOWPLANE_CORS_ALLOW_ALL=true
```

### Production Settings

```bash
# Production
FLOWPLANE_COOKIES_SECURE=true
FLOWPLANE_LOG_LEVEL=info
FLOWPLANE_CORS_ALLOWED_ORIGINS=https://app.example.com
FLOWPLANE_SESSION_EXPIRATION_HOURS=8
FLOWPLANE_SETUP_TOKEN_TTL_DAYS=1
```

## Compliance

### GDPR Considerations

- Personal data: Email addresses in tokens
- Right to erasure: Implement token deletion on user request
- Data retention: Set appropriate token expiration times
- Audit logging: Maintain records of access

### SOC 2 Considerations

- Access control: Scope-based authorization
- Audit logging: All auth events logged
- Encryption: TLS for data in transit, hashing for secrets
- Monitoring: Track authentication events

## Security Checklist

### Deployment Checklist

- [ ] HTTPS enabled with valid certificate
- [ ] `FLOWPLANE_COOKIES_SECURE=true`
- [ ] Strong database credentials
- [ ] Database encryption at rest
- [ ] Network firewall configured
- [ ] Rate limiting enabled
- [ ] Monitoring and alerting configured
- [ ] Regular security updates scheduled
- [ ] Incident response plan documented
- [ ] Audit log retention policy set

### Application Checklist

- [ ] CSRF tokens stored in `sessionStorage`
- [ ] No tokens in `localStorage`
- [ ] No tokens in URL parameters
- [ ] Content Security Policy configured
- [ ] Input sanitization implemented
- [ ] Proper error handling (no token leakage)
- [ ] Logout functionality implemented
- [ ] Session expiration handling
- [ ] XSS prevention measures
- [ ] Dependencies regularly updated

## Security Updates

Stay informed about security updates:

1. **Subscribe** to Flowplane security advisories
2. **Monitor** CVE databases for dependencies
3. **Update** regularly (at least monthly)
4. **Test** updates in staging before production
5. **Document** all security changes

## Reporting Security Issues

If you discover a security vulnerability:

1. **Do not** open a public GitHub issue
2. **Email** security@flowplane.dev with details
3. **Include** steps to reproduce
4. **Wait** for confirmation before public disclosure

## Further Reading

- [OWASP Top 10](https://owasp.org/www-project-top-ten/)
- [OWASP CSRF Prevention](https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html)
- [OWASP Session Management](https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html)
- [Session Management](./session-management.md)
- [Frontend Integration](./frontend-integration.md)
