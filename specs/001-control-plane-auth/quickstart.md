# Quickstart: Control Plane API Authentication

## Overview
This guide walks through setting up and using personal access tokens for Flowplane API authentication.

## Prerequisites
- Flowplane control plane running
- Database migrations applied (includes auth tables)
- Admin access to create initial tokens

## Step 1: Create Your First Token

### 1.1 Generate Admin Token (Bootstrap)
```bash
# During initial setup, create bootstrap admin token via CLI
flowplane auth create-token \
  --name "Bootstrap Admin" \
  --scopes "admin:read,admin:write,clusters:read,clusters:write,routes:read,routes:write,listeners:read,listeners:write" \
  --expires-in "90d"

# Output:
# Token created successfully!
# ID: 550e8400-e29b-41d4-a716-446655440000
# Name: Bootstrap Admin
# Token: fp_abc123def456ghi789jkl012mno345pq (SAVE THIS - shown only once)
# Scopes: admin:read, admin:write, clusters:read, clusters:write, routes:read, routes:write, listeners:read, listeners:write
# Expires: 2025-12-25T12:00:00Z
```

### 1.2 Verify Bootstrap Token Creation
```bash
# Verify the token creation was audited
curl -H "Authorization: Bearer fp_abc123def456ghi789jkl012mno345pq" \
  "http://localhost:8080/api/v1/audit/events?event_type=auth.token.seeded&limit=1"

# Expected output:
# [
#   {
#     "event_type": "auth.token.seeded",
#     "timestamp": "2025-09-26T12:00:00Z",
#     "correlation_id": "bootstrap_550e8400",
#     "metadata": {
#       "token_id": "550e8400-e29b-41d4-a716-446655440000",
#       "token_name": "Bootstrap Admin",
#       "granted_scopes": ["admin:read", "admin:write", "clusters:read", "clusters:write", "routes:read", "routes:write", "listeners:read", "listeners:write"],
#       "bootstrap": true,
#       "created_by": "system"
#     }
#   }
# ]
```

### 1.3 Verify Token Works
```bash
# Test the token with an API call
curl -H "Authorization: Bearer fp_abc123def456ghi789jkl012mno345pq" \
  http://localhost:8080/api/v1/clusters

# Should return list of clusters (or empty array if none exist)
```

## Step 2: Create Application-Specific Tokens via API

### 2.1 Create CI/CD Token
```bash
# Create token for continuous integration with limited scopes
curl -X POST http://localhost:8080/api/v1/tokens \
  -H "Authorization: Bearer fp_abc123def456ghi789jkl012mno345pq" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "CI/CD Pipeline",
    "scopes": ["clusters:read", "routes:write", "listeners:read"],
    "expires_at": "2025-06-30T23:59:59Z"
  }'

# Response:
# {
#   "id": "550e8400-e29b-41d4-a716-446655440001",
#   "name": "CI/CD Pipeline",
#   "token": "fp_xyz789abc123def456ghi012jkl345mn",
#   "scopes": ["clusters:read", "routes:write", "listeners:read"],
#   "created_at": "2025-09-26T12:00:00Z",
#   "expires_at": "2025-06-30T23:59:59Z"
# }
```

### 2.2 Create Read-Only Token
```bash
# Create token for monitoring/observability tools
curl -X POST http://localhost:8080/api/v1/tokens \
  -H "Authorization: Bearer fp_abc123def456ghi789jkl012mno345pq" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Monitoring Dashboard",
    "scopes": ["clusters:read", "routes:read", "listeners:read"]
  }'

# Response includes new read-only token
```

## Step 3: Use Tokens for API Access

### 3.1 Cluster Operations
```bash
# List clusters (requires clusters:read)
curl -H "Authorization: Bearer fp_xyz789abc123def456ghi012jkl345mn" \
  http://localhost:8080/api/v1/clusters

# Create cluster (requires clusters:write)
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer fp_xyz789abc123def456ghi012jkl345mn" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-service-cluster",
    "endpoints": [{"address": "service.example.com", "port": 8080}],
    "load_balancing_policy": "round_robin"
  }'
```

### 3.2 Route Configuration
```bash
# Create route (requires routes:write)
curl -X POST http://localhost:8080/api/v1/routes \
  -H "Authorization: Bearer fp_xyz789abc123def456ghi012jkl345mn" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-service-routes",
    "virtual_hosts": [{
      "name": "my-service",
      "domains": ["api.example.com"],
      "routes": [{
        "match": {"prefix": "/api/"},
        "route": {"cluster": "my-service-cluster"}
      }]
    }]
  }'
```

### 3.3 Error Handling Examples
```bash
# Insufficient permissions (will return 403)
curl -H "Authorization: Bearer fp_xyz789abc123def456ghi012jkl345mn" \
  -X POST http://localhost:8080/api/v1/tokens \
  -d '{"name": "test", "scopes": ["admin:read"]}'

# Response:
# {
#   "error": "insufficient_permissions",
#   "message": "Required scope 'admin:write' not granted",
#   "details": {
#     "required_scopes": ["admin:write"],
#     "granted_scopes": ["clusters:read", "routes:write", "listeners:read"]
#   },
#   "correlation_id": "req_abc123def456"
# }

# Invalid token (will return 401)
curl -H "Authorization: Bearer invalid_token" \
  http://localhost:8080/api/v1/clusters

# Response:
# {
#   "error": "invalid_token",
#   "message": "Token is invalid or expired",
#   "correlation_id": "req_def456ghi789"
# }
```

## Step 4: Token Management

### 4.1 List Your Tokens
```bash
# View all active tokens
curl -H "Authorization: Bearer fp_abc123def456ghi789jkl012mno345pq" \
  http://localhost:8080/api/v1/tokens

# View tokens by status
curl -H "Authorization: Bearer fp_abc123def456ghi789jkl012mno345pq" \
  "http://localhost:8080/api/v1/tokens?status=active"
```

### 4.2 Update Token Metadata
```bash
# Update token name and expiration
curl -X PATCH http://localhost:8080/api/v1/tokens/550e8400-e29b-41d4-a716-446655440001 \
  -H "Authorization: Bearer fp_abc123def456ghi789jkl012mno345pq" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "CI/CD Pipeline v2",
    "expires_at": "2025-12-31T23:59:59Z"
  }'
```

### 4.3 Rotate Token
```bash
# Generate new token value while keeping metadata
curl -X POST http://localhost:8080/api/v1/tokens/550e8400-e29b-41d4-a716-446655440001/rotate \
  -H "Authorization: Bearer fp_abc123def456ghi789jkl012mno345pq"

# Response includes new token value (old token immediately invalid)
# {
#   "id": "550e8400-e29b-41d4-a716-446655440001",
#   "name": "CI/CD Pipeline v2",
#   "token": "fp_new123token456here789abc012def345",
#   "scopes": ["clusters:read", "routes:write", "listeners:read"],
#   "rotated_at": "2025-09-26T14:30:00Z"
# }
```

### 4.4 Revoke Token
```bash
# Immediately invalidate a token
curl -X DELETE http://localhost:8080/api/v1/tokens/550e8400-e29b-41d4-a716-446655440001 \
  -H "Authorization: Bearer fp_abc123def456ghi789jkl012mno345pq"

# Returns 204 No Content on success
# Token is immediately unusable
```

## Step 5: Integration with CI/CD

### 5.1 GitHub Actions Example
```yaml
# .github/workflows/deploy.yml
name: Deploy Configuration
on: [push]

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Deploy Cluster Config
        run: |
          curl -X POST ${{ vars.FLOWPLANE_URL }}/api/v1/clusters \
            -H "Authorization: Bearer ${{ secrets.FLOWPLANE_TOKEN }}" \
            -H "Content-Type: application/json" \
            -d @cluster-config.json
        env:
          FLOWPLANE_TOKEN: ${{ secrets.FLOWPLANE_TOKEN }}
```

### 5.2 Environment-Specific Tokens
```bash
# Development environment (broader permissions for testing)
curl -X POST http://localhost:8080/api/v1/tokens \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -d '{
    "name": "Development Environment",
    "scopes": ["clusters:write", "routes:write", "listeners:write"]
  }'

# Production environment (restricted permissions)
curl -X POST http://localhost:8080/api/v1/tokens \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -d '{
    "name": "Production Deployment",
    "scopes": ["clusters:write", "routes:read"],
    "expires_at": "2025-03-31T23:59:59Z"
  }'
```

## Step 6: Monitoring and Security

### 6.1 Audit Log Review
```bash
# Check authentication events (requires admin access to audit logs)
curl -H "Authorization: Bearer fp_abc123def456ghi789jkl012mno345pq" \
  "http://localhost:8080/api/v1/audit/events?event_type=auth.request.authenticated&limit=100"

# Monitor failed authentication attempts
curl -H "Authorization: Bearer fp_abc123def456ghi789jkl012mno345pq" \
  "http://localhost:8080/api/v1/audit/events?event_type=auth.request.failed&since=24h"
```

### 6.2 Token Security Best Practices
```bash
# Set reasonable expiration dates
# Short-lived for automation: 30-90 days
# Long-lived for services: 6 months - 1 year

# Use minimal required scopes
# Don't grant admin:write unless necessary
# Prefer resource-specific permissions

# Regular token rotation
# Automate rotation for long-lived tokens
# Rotate immediately if compromise suspected

# Monitor token usage
# Review audit logs regularly
# Alert on unusual access patterns
```

## Troubleshooting

### Common Error Responses

**401 Unauthorized**: Token missing, invalid, expired, or revoked
- Check Authorization header format: `Bearer fp_...`
- Verify token is active and not expired
- Confirm token hasn't been revoked

**403 Forbidden**: Valid token but insufficient permissions
- Check required scopes for the endpoint
- Verify token has necessary scopes granted
- May need to create new token with broader permissions

**400 Bad Request**: Invalid request format
- Check JSON syntax in request body
- Verify required fields are present
- Ensure scope names match valid patterns

### Token Validation
```bash
# Debug token validation by checking metadata
curl -H "Authorization: Bearer fp_your_token_here" \
  http://localhost:8080/api/v1/tokens/your-token-id

# Check if token appears in active token list
curl -H "Authorization: Bearer fp_admin_token" \
  "http://localhost:8080/api/v1/tokens?status=active" | grep "your-token-name"
```

## Next Steps

1. **Configure Application**: Set `FLOWPLANE_AUTH_REQUIRED=true` to enforce authentication on all endpoints
2. **Set Up Monitoring**: Implement alerts for authentication failures and suspicious activity
3. **Document Workflows**: Create team guidelines for token creation and rotation
4. **Plan JWT/OIDC**: Prepare for future integration with corporate identity providers

For more details, see the full API documentation and security guidelines.