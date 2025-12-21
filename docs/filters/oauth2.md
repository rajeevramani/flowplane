# OAuth2 Filter

The OAuth2 filter enables OAuth2 authentication for HTTP requests using the authorization code flow. It handles the complete OAuth2 dance including redirecting unauthenticated users to the authorization server, exchanging authorization codes for tokens, and managing session cookies.

## Envoy Documentation

- [OAuth2 Filter Reference](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/oauth2_filter)
- [OAuth2 Filter API](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/filters/http/oauth2/v3/oauth.proto)

## How It Works in Envoy

The OAuth2 filter implements the OAuth2 Authorization Code Grant flow:

```
┌─────────┐     ┌─────────┐     ┌──────────────┐     ┌─────────────┐
│  User   │     │  Envoy  │     │ Auth Server  │     │  Upstream   │
└────┬────┘     └────┬────┘     └──────┬───────┘     └──────┬──────┘
     │               │                 │                    │
     │ 1. Request    │                 │                    │
     ├──────────────►│                 │                    │
     │               │                 │                    │
     │ 2. No token   │                 │                    │
     │◄──────────────┤                 │                    │
     │   Redirect    │                 │                    │
     │               │                 │                    │
     │ 3. Login at auth server         │                    │
     ├────────────────────────────────►│                    │
     │                                 │                    │
     │ 4. Redirect with code           │                    │
     │◄────────────────────────────────┤                    │
     │                                 │                    │
     │ 5. Callback with code           │                    │
     ├──────────────►│                 │                    │
     │               │                 │                    │
     │               │ 6. Exchange code│                    │
     │               ├────────────────►│                    │
     │               │                 │                    │
     │               │ 7. Access token │                    │
     │               │◄────────────────┤                    │
     │               │                 │                    │
     │ 8. Set cookie │                 │                    │
     │◄──────────────┤                 │                    │
     │   Redirect    │                 │                    │
     │               │                 │                    │
     │ 9. Request    │                 │                    │
     ├──────────────►│                 │                    │
     │               │                 │                    │
     │               │ 10. Forward with token               │
     │               ├─────────────────────────────────────►│
     │               │                 │                    │
     │ 11. Response  │◄────────────────────────────────────┤
     │◄──────────────┤                 │                    │
```

### Key Behaviors

1. **Token Storage**: Tokens are stored in encrypted cookies on the client
2. **Token Forwarding**: Access tokens can be forwarded to upstream services in the `Authorization` header
3. **Refresh Tokens**: Automatic token refresh when enabled
4. **HMAC Verification**: Cookies are HMAC-signed to prevent tampering

### Important Limitation

**The OAuth2 filter does NOT support per-route configuration.** Unlike most Envoy filters, you cannot use `typed_per_filter_config` to disable or configure OAuth2 on specific routes. Envoy will reject such configurations with:

```
The filter envoy.filters.http.oauth2 doesn't support virtual host or route specific configurations
```

To bypass OAuth2 for specific paths (e.g., health checks, public endpoints), use the `pass_through_matcher` configuration.

## Flowplane Configuration

### Configuration Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `token_endpoint` | object | Yes | - | Token endpoint configuration |
| `token_endpoint.uri` | string | Yes | - | Full URI to the token endpoint |
| `token_endpoint.cluster` | string | Yes | - | Cluster name for token endpoint |
| `token_endpoint.timeout_ms` | integer | No | `5000` | Request timeout in milliseconds |
| `authorization_endpoint` | string | Yes | - | Full URI to the authorization endpoint |
| `credentials` | object | Yes | - | OAuth2 credentials configuration |
| `credentials.client_id` | string | Yes | - | OAuth2 client ID |
| `credentials.token_secret` | object | No | - | SDS secret for client secret |
| `credentials.cookie_domain` | string | No | - | Domain for OAuth cookies |
| `credentials.cookie_names` | object | No | - | Custom cookie names |
| `redirect_uri` | string | Yes | - | Full callback URL (must match OAuth app config) |
| `redirect_path` | string | No | `/oauth2/callback` | Path that handles OAuth callback |
| `signout_path` | string | No | - | Path to clear OAuth cookies |
| `auth_scopes` | array | No | `["openid", "profile", "email"]` | OAuth2 scopes to request |
| `auth_type` | string | No | `url_encoded_body` | Auth type: `url_encoded_body` or `basic_auth` |
| `forward_bearer_token` | boolean | No | `true` | Forward token to upstream |
| `preserve_authorization_header` | boolean | No | `false` | Keep existing auth header |
| `use_refresh_token` | boolean | No | `false` | Enable refresh token flow |
| `default_expires_in_seconds` | integer | No | - | Default token expiry if not provided |
| `stat_prefix` | string | No | - | Stats prefix for metrics |
| `pass_through_matcher` | array | No | `[]` | Paths/headers to bypass OAuth2 |

### Pass-Through Matcher

Since OAuth2 doesn't support per-route configuration, use `pass_through_matcher` to bypass authentication for specific paths:

| Field | Type | Description |
|-------|------|-------------|
| `path_exact` | string | Exact path match (e.g., `/healthz`) |
| `path_prefix` | string | Path prefix match (e.g., `/api/public/`) |
| `path_regex` | string | Regex path match (e.g., `^/static/.*`) |
| `header_name` | string | Header name to match (requires `header_value`) |
| `header_value` | string | Header value to match (requires `header_name`) |

## Prerequisites

Before configuring OAuth2, you need:

### 1. Enable Secrets Feature (Required)

The OAuth2 filter requires **two secrets** delivered via SDS (Secret Discovery Service):
- **Client Secret**: Your OAuth2 provider's client secret
- **HMAC Secret**: A 32-byte key for signing cookies (prevents tampering)

**Step 1: Set the encryption key (required for ALL secret backends) when booting the CP**

```bash
# Generate a 32-byte encryption key
openssl rand -base64 32

# Set before starting Flowplane
export FLOWPLANE_SECRET_ENCRYPTION_KEY="your-generated-key-here"
```

**For Docker Compose:**

A complete example with Vault and tracing is available at [`docker-compose-secrets-tracing.yml`](../docker-compose-secrets-tracing.yml):

```bash
# Start Flowplane with Vault and Jaeger
docker-compose -f docker-compose-secrets-tracing.yml up

# Access:
# - Flowplane API: http://localhost:8080
# - Vault UI: http://localhost:8200 (token: flowplane-dev-token)
# - Jaeger UI: http://localhost:16686
```

Key environment variables in the compose file:
```yaml
services:
  control-plane:
    environment:
      # Required for secrets/SDS
      FLOWPLANE_SECRET_ENCRYPTION_KEY: "d2t71S8xKUQqhaWbj1VofrH/Z8Dq4qR+hAcgXpP6Udg="
      # Vault integration
      VAULT_ADDR: "http://vault:8200"
      VAULT_TOKEN: "flowplane-dev-token"
```

> **Important:** Without `FLOWPLANE_SECRET_ENCRYPTION_KEY`, the secrets API returns 503 errors and SDS cannot deliver secrets to Envoy. This key is required even when using Vault, as it enables the database that stores secret metadata.

**Step 2: For Vault-backed secrets (production), also configure:**

```bash
# Enable the external_secrets feature via Admin API
curl -X PUT "http://localhost:8080/api/v1/admin/apps/external_secrets" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -d '{
    "enabled": true,
    "config": {
      "backendType": "vault",
      "vaultAddr": "http://127.0.0.1:8200",
      "vaultKvMount": "secret",
      "cacheTtlSeconds": 300
    }
  }'
```

### 2. OAuth2 Provider Cluster

Create a cluster for your OAuth2 provider:

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "my-team",
    "name": "oauth-provider",
    "serviceName": "oauth-provider-service",
    "endpoints": [
      {"host": "auth.example.com", "port": 443}
    ],
    "useTls": true,
    "lbPolicy": "ROUND_ROBIN"
  }'
```

### 3. Secrets (Client Secret and HMAC)

OAuth2 requires two secrets:
- **Client Secret**: The OAuth2 client secret from your identity provider
- **HMAC Secret**: A 32-byte key used to sign cookies (prevents tampering)

You can store these secrets either in Flowplane's database (simple) or in HashiCorp Vault (production).

#### Option A: Database-Backed Secrets (Simple)

```bash
# Base64 encode your client secret
CLIENT_SECRET=$(echo -n "your-oauth2-client-secret-from-provider" | base64)

# Create OAuth2 client secret
curl -X POST "http://localhost:8080/api/v1/teams/my-team/secrets" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"name\": \"oauth2-client-secret\",
    \"secret_type\": \"generic_secret\",
    \"description\": \"OAuth2 client secret\",
    \"configuration\": {
      \"type\": \"generic_secret\",
      \"secret\": \"${CLIENT_SECRET}\"
    }
  }"

# Generate a random 32-byte HMAC secret and base64 encode it
HMAC_SECRET=$(openssl rand -base64 32)

# Create HMAC secret for cookie signing
curl -X POST "http://localhost:8080/api/v1/teams/my-team/secrets" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"name\": \"hmac-secret\",
    \"secret_type\": \"generic_secret\",
    \"description\": \"HMAC secret for OAuth2 cookie signing\",
    \"configuration\": {
      \"type\": \"generic_secret\",
      \"secret\": \"${HMAC_SECRET}\"
    }
  }"
```

#### Option B: Vault-Backed Secrets (Production)

For production deployments, store secrets in HashiCorp Vault and reference them in Flowplane.

> **Note:** Ensure you have enabled the external_secrets feature as described in [Prerequisites Step 1](#1-enable-secrets-feature).

**Step 1: Store secrets in Vault**

```bash
# Set Vault environment
export VAULT_ADDR='http://127.0.0.1:8200'
export VAULT_TOKEN='your-vault-token'

# Base64 encode the client secret
CLIENT_SECRET=$(echo -n "your-oauth2-client-secret-from-provider" | base64)

# Store OAuth2 client secret in Vault
vault kv put secret/teams/my-team/oauth2-client-secret \
  type="generic_secret" \
  secret="${CLIENT_SECRET}"

# Generate and store HMAC secret (32 bytes, base64 encoded)
HMAC_SECRET=$(openssl rand -base64 32)

vault kv put secret/teams/my-team/hmac-secret \
  type="generic_secret" \
  secret="${HMAC_SECRET}"

# Verify secrets are stored
vault kv get secret/teams/my-team/oauth2-client-secret
vault kv get secret/teams/my-team/hmac-secret
```

**Step 2: Create secret references in Flowplane**

```bash
# Create reference to OAuth2 client secret
curl -X POST "http://localhost:8080/api/v1/teams/my-team/secrets/reference" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "name": "oauth2-client-secret",
    "secret_type": "generic_secret",
    "description": "OAuth2 client secret (stored in Vault)",
    "backend": "vault",
    "reference": "teams/my-team/oauth2-client-secret"
  }'

# Create reference to HMAC secret
curl -X POST "http://localhost:8080/api/v1/teams/my-team/secrets/reference" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "name": "hmac-secret",
    "secret_type": "generic_secret",
    "description": "HMAC secret for OAuth2 cookie signing (stored in Vault)",
    "backend": "vault",
    "reference": "teams/my-team/hmac-secret"
  }'
```

For complete Vault integration details, see [Secrets Management (SDS)](../secrets-sds.md).

## Complete Example

### Basic OAuth2 with Google

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "oauth2-listener",
    "address": "0.0.0.0",
    "port": 8080,
    "team": "my-team",
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "my-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.oauth2",
            "filter": {
              "type": "oauth2",
              "token_endpoint": {
                "uri": "https://oauth2.googleapis.com/token",
                "cluster": "google-oauth",
                "timeout_ms": 5000
              },
              "authorization_endpoint": "https://accounts.google.com/o/oauth2/v2/auth",
              "credentials": {
                "client_id": "your-client-id.apps.googleusercontent.com",
                "token_secret": {
                  "name": "oauth2-client-secret"
                },
                "cookie_domain": "example.com"
              },
              "redirect_uri": "https://app.example.com/oauth2/callback",
              "redirect_path": "/oauth2/callback",
              "signout_path": "/logout",
              "auth_scopes": ["openid", "profile", "email"],
              "forward_bearer_token": true,
              "use_refresh_token": true,
              "pass_through_matcher": [
                {"path_exact": "/healthz"},
                {"path_exact": "/readyz"},
                {"path_prefix": "/api/public/"}
              ]
            }
          },
          {
            "name": "envoy.filters.http.router",
            "filter": {"type": "router"}
          }
        ]
      }]
    }]
  }'
```

### OAuth2 with Keycloak

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "keycloak-oauth2-listener",
    "address": "0.0.0.0",
    "port": 8080,
    "team": "my-team",
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "my-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.oauth2",
            "filter": {
              "type": "oauth2",
              "token_endpoint": {
                "uri": "https://keycloak.example.com/realms/myrealm/protocol/openid-connect/token",
                "cluster": "keycloak",
                "timeout_ms": 5000
              },
              "authorization_endpoint": "https://keycloak.example.com/realms/myrealm/protocol/openid-connect/auth",
              "credentials": {
                "client_id": "my-app",
                "token_secret": {
                  "name": "keycloak-client-secret"
                }
              },
              "redirect_uri": "https://app.example.com/oauth2/callback",
              "auth_scopes": ["openid", "profile"],
              "auth_type": "basic_auth",
              "forward_bearer_token": true
            }
          },
          {
            "name": "envoy.filters.http.router",
            "filter": {"type": "router"}
          }
        ]
      }]
    }]
  }'
```

### OAuth2 with Azure AD

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "azure-oauth2-listener",
    "address": "0.0.0.0",
    "port": 8080,
    "team": "my-team",
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "my-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.oauth2",
            "filter": {
              "type": "oauth2",
              "token_endpoint": {
                "uri": "https://login.microsoftonline.com/{tenant-id}/oauth2/v2.0/token",
                "cluster": "azure-oauth",
                "timeout_ms": 5000
              },
              "authorization_endpoint": "https://login.microsoftonline.com/{tenant-id}/oauth2/v2.0/authorize",
              "credentials": {
                "client_id": "your-azure-app-id",
                "token_secret": {
                  "name": "azure-client-secret"
                }
              },
              "redirect_uri": "https://app.example.com/oauth2/callback",
              "auth_scopes": ["openid", "profile", "email", "User.Read"],
              "forward_bearer_token": true
            }
          },
          {
            "name": "envoy.filters.http.router",
            "filter": {"type": "router"}
          }
        ]
      }]
    }]
  }'
```

## Bypassing OAuth2 for Specific Paths

Since OAuth2 doesn't support per-route disable, use `pass_through_matcher`:

```json
{
  "pass_through_matcher": [
    {"path_exact": "/healthz"},
    {"path_exact": "/readyz"},
    {"path_exact": "/.well-known/openid-configuration"},
    {"path_prefix": "/api/public/"},
    {"path_prefix": "/static/"},
    {"path_regex": "^/assets/.*\\.(js|css|png|jpg)$"},
    {"header_name": "X-Internal-Request", "header_value": "true"}
  ]
}
```

## Cookie Configuration

Customize OAuth2 cookie names:

```json
{
  "credentials": {
    "client_id": "my-app",
    "cookie_domain": "example.com",
    "cookie_names": {
      "bearer_token": "my_app_token",
      "oauth_hmac": "my_app_hmac",
      "oauth_expires": "my_app_expires",
      "id_token": "my_app_id",
      "refresh_token": "my_app_refresh"
    }
  }
}
```

## Troubleshooting

### Common Issues

1. **"OAuth flow failed" After Successful Login**

   You're redirected to the OAuth provider, login succeeds, but the callback fails.

   **Most likely cause:** Secrets are not being delivered to Envoy via SDS.

   **Check Envoy's secret status:**
   ```bash
   curl -s "http://localhost:9902/config_dump?resource=secrets" | jq '.configs[].dynamic_warming_secrets'
   ```

   If secrets show `"version_info": "uninitialized"`, they're not being delivered.

   **Fix:** Ensure `FLOWPLANE_SECRET_ENCRYPTION_KEY` is set and restart the control plane. See [Secrets SDS Troubleshooting](../secrets-sds.md#troubleshooting).

2. **401 Errors from Token Endpoint**

   Check Envoy stats for 401 responses:
   ```bash
   curl -s "http://localhost:9902/stats" | grep "oauth2.*401"
   # Look for: cluster.oauth2-auth-cluster.internal.upstream_rq_401
   ```

   **Cause:** The OAuth2 filter is sending token requests without the client secret because SDS hasn't delivered it.

   **Fix:** Verify secrets are in `dynamic_active_secrets` (not `dynamic_warming_secrets`):
   ```bash
   curl -s "http://localhost:9902/config_dump?resource=secrets" | jq '.configs[].dynamic_active_secrets[].name'
   # Should show: oauth2-client-secret, hmac-secret
   ```

3. **"Secret repository unavailable" (503)**

   The control plane's secrets API returns 503.

   **Cause:** `FLOWPLANE_SECRET_ENCRYPTION_KEY` environment variable is not set.

   **Fix:** Set the key and restart:
   ```bash
   export FLOWPLANE_SECRET_ENCRYPTION_KEY="$(openssl rand -base64 32)"
   docker-compose restart control-plane
   ```

4. **Redirect Loop**
   - Ensure `redirect_path` matches the path in `redirect_uri`
   - Verify the OAuth provider's allowed redirect URIs include your callback URL

5. **Token Exchange Fails**
   - Check the `token_endpoint.cluster` exists and can reach the auth server
   - Verify TLS is properly configured for the cluster
   - Check the client secret is correct and base64-encoded in Vault/DB

6. **Cookie Not Set**
   - Ensure `cookie_domain` matches your application domain
   - For HTTPS, cookies require secure context

7. **Public Endpoints Still Require Auth**
   - Verify `pass_through_matcher` patterns are correct
   - Path matchers are case-sensitive

### Debug Checklist

```bash
# 1. Is encryption key set?
docker exec flowplane-control-plane env | grep FLOWPLANE_SECRET

# 2. Can you list secrets via API?
curl -s "http://localhost:8080/api/v1/teams/my-team/secrets" \
  -H "Authorization: Bearer $TOKEN"
# If 503 → encryption key not set

# 3. Are secrets delivered to Envoy (active, not warming)?
curl -s "http://localhost:9902/config_dump?resource=secrets" | \
  jq '.configs[] | {active: .dynamic_active_secrets, warming: .dynamic_warming_secrets}'

# 4. Is the OAuth cluster healthy?
curl -s "http://localhost:9902/clusters" | grep oauth2

# 5. Check for 401s from token endpoint
curl -s "http://localhost:9902/stats" | grep -E "oauth2.*rq_4"
```

### Metrics

With `stat_prefix` configured, Envoy emits metrics:

| Metric | Description |
|--------|-------------|
| `oauth2.{prefix}.oauth_success` | Successful OAuth flows |
| `oauth2.{prefix}.oauth_failure` | Failed OAuth flows |
| `oauth2.{prefix}.token_fetch_failed` | Token exchange failures |

## Security Considerations

1. **HMAC Secret**: Use a cryptographically random 32-byte secret for cookie signing
2. **Client Secret**: Store securely using Flowplane's secrets management
3. **TLS**: Always use HTTPS for OAuth2 flows in production
4. **Cookie Domain**: Set appropriately to prevent cookie leakage
5. **Scopes**: Request only the scopes your application needs

## See Also

- [Secrets Management](../secrets-sds.md) - Managing OAuth2 secrets
- [TLS Configuration](../tls.md) - Configuring TLS for OAuth2 providers
- [Filters Overview](../filters.md) - All available filters
