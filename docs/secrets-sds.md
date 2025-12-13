# Secrets and SDS (Secret Discovery Service)

Flowplane provides secure secret management for Envoy proxies via the **Secret Discovery Service (SDS)**. Secrets such as OAuth2 tokens, TLS certificates, and API keys are delivered to Envoy on-demand without embedding them in static configuration.

## Overview

Flowplane supports two secret storage backends:

| Backend | Description | Use Case |
|---------|-------------|----------|
| **Database** | Encrypted secrets stored in Flowplane's database | Simple deployments, development |
| **Vault** | References stored in Flowplane, values fetched from HashiCorp Vault | Production, enterprise, compliance |

## Feature Enablement

The Secrets feature must be enabled before use. This is an admin-level operation.

### Enable Database Backend (Default)

Database-backed secrets are available by default when the encryption key is configured:

```bash
export FLOWPLANE_SECRET_ENCRYPTION_KEY="your-32-byte-encryption-key-here"
```

### Enable External Secrets (Vault)

To use HashiCorp Vault as the secret backend:

**Step 1: Set Vault Environment Variables**

```bash
export FLOWPLANE_VAULT_ADDR="http://127.0.0.1:8200"
export FLOWPLANE_VAULT_TOKEN="your-vault-token"
export FLOWPLANE_VAULT_KV_MOUNT="secret"  # Optional, defaults to "secret"
export FLOWPLANE_VAULT_NAMESPACE=""        # Optional, for Vault Enterprise
```

**Step 2: Enable the Feature via Admin API**

```bash
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

**Step 3: Verify Feature Status**

```bash
curl -s "http://localhost:8080/api/v1/admin/apps/external_secrets" \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" | jq
```

Expected response:
```json
{
  "appId": "external_secrets",
  "enabled": true,
  "config": {
    "backendType": "vault",
    "vaultAddr": "http://127.0.0.1:8200",
    "vaultKvMount": "secret",
    "cacheTtlSeconds": 300
  }
}
```

## Secret Types

Flowplane supports four secret types matching Envoy's SDS specification:

| Type | Use Case | Configuration Fields |
|------|----------|---------------------|
| `generic_secret` | OAuth2 tokens, API keys, HMAC secrets | `secret` (base64-encoded) |
| `tls_certificate` | TLS certificates with private keys | `certificate_chain`, `private_key` |
| `certificate_validation_context` | CA certificates for peer verification | `trusted_ca` |
| `session_ticket_keys` | TLS session resumption keys | `keys[]` (array of 80-byte keys) |

## API Reference

### Create Secret (Database Backend)

Store an encrypted secret directly in Flowplane's database.

```bash
POST /api/v1/teams/{team}/secrets
```

**Request Body:**
```json
{
  "name": "my-oauth-secret",
  "secret_type": "generic_secret",
  "description": "OAuth2 client secret",
  "configuration": {
    "type": "generic_secret",
    "secret": "BASE64_ENCODED_SECRET_VALUE"
  }
}
```

**Example - Create OAuth2 Client Secret:**
```bash
# Base64 encode the secret value
SECRET=$(echo -n "your-client-secret-value" | base64)

curl -X POST "http://localhost:8080/api/v1/teams/my-team/secrets" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d "{
    \"name\": \"oauth2-client-secret\",
    \"secret_type\": \"generic_secret\",
    \"description\": \"Auth0 client secret\",
    \"configuration\": {
      \"type\": \"generic_secret\",
      \"secret\": \"${SECRET}\"
    }
  }"
```

### Create Secret Reference (Vault Backend)

Store a reference to a secret in HashiCorp Vault. The actual secret value is fetched on-demand.

```bash
POST /api/v1/teams/{team}/secrets/reference
```

**Request Body:**
```json
{
  "name": "my-vault-secret",
  "secret_type": "generic_secret",
  "description": "OAuth2 client secret (stored in Vault)",
  "backend": "vault",
  "reference": "path/to/secret/in/vault"
}
```

**Example - Create Vault Reference:**
```bash
curl -X POST "http://localhost:8080/api/v1/teams/my-team/secrets/reference" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{
    "name": "oauth2-client-secret",
    "secret_type": "generic_secret",
    "description": "Auth0 client secret (stored in Vault)",
    "backend": "vault",
    "reference": "teams/my-team/oauth2-client-secret"
  }'
```

### List Secrets

```bash
GET /api/v1/teams/{team}/secrets
```

Response includes metadata only (secret values are never exposed via API):
```json
[
  {
    "id": "sec_01HXYZ...",
    "name": "oauth2-client-secret",
    "secret_type": "generic_secret",
    "source": "vault",
    "backend": "vault",
    "reference": "teams/my-team/oauth2-client-secret",
    "team": "my-team",
    "version": 1
  }
]
```

### Get Secret

```bash
GET /api/v1/teams/{team}/secrets/{secret_id}
```

### Update Secret

```bash
PUT /api/v1/teams/{team}/secrets/{secret_id}
```

### Delete Secret

```bash
DELETE /api/v1/teams/{team}/secrets/{secret_id}
```

## HashiCorp Vault Integration

### Prerequisites

1. HashiCorp Vault running and accessible
2. KV v2 secrets engine enabled
3. Vault token with read permissions on the secret paths

### Vault Secret Format

Secrets in Vault must follow one of these formats:

**Generic Secret:**
```json
{
  "type": "generic_secret",
  "secret": "BASE64_ENCODED_VALUE"
}
```

Or simplified (type is inferred):
```json
{
  "secret": "BASE64_ENCODED_VALUE"
}
```

**TLS Certificate:**
```json
{
  "type": "tls_certificate",
  "certificate_chain": "-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----",
  "private_key": "-----BEGIN PRIVATE KEY-----\n...\n-----END PRIVATE KEY-----"
}
```

**Certificate Validation Context:**
```json
{
  "type": "certificate_validation_context",
  "trusted_ca": "-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----"
}
```

### Storing Secrets in Vault

```bash
# Set Vault environment
export VAULT_ADDR='http://127.0.0.1:8200'
export VAULT_TOKEN='your-token'

# Store a generic secret (e.g., OAuth2 client secret)
SECRET_VALUE=$(echo -n "your-client-secret" | base64)
vault kv put secret/teams/my-team/oauth2-client-secret \
  type="generic_secret" \
  secret="${SECRET_VALUE}"

# Store a TLS certificate
vault kv put secret/teams/my-team/server-cert \
  type="tls_certificate" \
  certificate_chain="$(cat server.crt)" \
  private_key="$(cat server.key)"

# Verify
vault kv get secret/teams/my-team/oauth2-client-secret
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `FLOWPLANE_VAULT_ADDR` | Vault server address | - |
| `FLOWPLANE_VAULT_TOKEN` | Vault authentication token | - |
| `FLOWPLANE_VAULT_KV_MOUNT` | KV v2 mount path | `secret` |
| `FLOWPLANE_VAULT_NAMESPACE` | Vault namespace (Enterprise) | - |
| `FLOWPLANE_SECRET_CACHE_TTL_SECS` | Cache TTL for fetched secrets | `300` |

## Using Secrets with Filters

### OAuth2 Filter

The OAuth2 filter uses SDS to securely deliver the client secret:

```json
{
  "name": "oauth2-filter",
  "team": "my-team",
  "filterType": "oauth2",
  "config": {
    "type": "oauth2",
    "config": {
      "token_endpoint": {
        "uri": "https://auth.example.com/oauth/token",
        "cluster": "oauth2-auth-cluster",
        "timeout_ms": 5000
      },
      "authorization_endpoint": "https://auth.example.com/authorize",
      "credentials": {
        "client_id": "my-client-id",
        "token_secret": {
          "type": "sds",
          "name": "oauth2-client-secret"
        }
      },
      "redirect_uri": "https://app.example.com/callback"
    }
  }
}
```

The `token_secret.name` references a secret created via the Secrets API. Whether stored in the database or Vault, the SDS mechanism handles retrieval transparently.

## Complete Setup Example: OAuth2 with Vault

### Step 1: Start Vault (Development)

```bash
vault server -dev -dev-root-token-id="root" -dev-listen-address="0.0.0.0:8200"
```

### Step 2: Store Secret in Vault

```bash
export VAULT_ADDR='http://127.0.0.1:8200'
export VAULT_TOKEN='root' #flowplane-dev-token

SECRET_VALUE=$(echo -n "your-oauth2-client-secret" | base64)
vault kv put secret/teams/engineering/oauth2-client-secret \
  type="generic_secret" \
  secret="${CLIENT_SECRET_VALUE}"
```

### Step 3: Configure Flowplane

```bash
export FLOWPLANE_VAULT_ADDR="http://127.0.0.1:8200"
export FLOWPLANE_VAULT_TOKEN="root"
# Restart Flowplane control plane
```

### Step 4: Enable External Secrets Feature

```bash
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

### Step 5: Create Secret Reference in Flowplane

```bash
curl -X POST "http://localhost:8080/api/v1/teams/engineering/secrets/reference" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{
    "name": "oauth2-client-secret",
    "secret_type": "generic_secret",
    "description": "OAuth2 client secret stored in Vault",
    "backend": "vault",
    "reference": "teams/engineering/oauth2-client-secret"
  }'
```

### Step 6: Create OAuth2 Filter

```bash
curl -X POST "http://localhost:8080/api/v1/filters" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{
    "name": "oauth2-auth-filter",
    "team": "my-team",
    "filterType": "oauth2",
    "config": {
      "type": "oauth2",
      "config": {
        "token_endpoint": {
          "uri": "https://auth.example.com/oauth/token",
          "cluster": "oauth2-auth-cluster",
          "timeout_ms": 5000
        },
        "authorization_endpoint": "https://auth.example.com/authorize",
        "credentials": {
          "client_id": "my-client-id",
          "token_secret": {
            "type": "sds",
            "name": "oauth2-client-secret"
          }
        },
        "redirect_uri": "https://app.example.com/callback"
      }
    }
  }'
```

### Step 7: Attach Filter to Route

Create clusters, routes, and listeners, then attach the OAuth2 filter to protect your routes.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              SETUP PHASE                                    │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  1. Store secret in Vault                                                   │
│     vault kv put secret/teams/my-team/oauth2-secret ...                     │
│                                                                             │
│  2. Enable external_secrets feature in Flowplane                            │
│     PUT /api/v1/admin/apps/external_secrets                                 │
│                                                                             │
│  3. Create reference in Flowplane                                           │
│     POST /api/v1/teams/{team}/secrets/reference                             │
│     {name: "oauth2-secret", backend: "vault", reference: "..."}             │
│                                                                             │
│  4. Create filter referencing the secret                                    │
│     {token_secret: {type: "sds", name: "oauth2-secret"}}                    │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│                              RUNTIME PHASE                                  │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│   Envoy                    Flowplane                      Vault             │
│     │                          │                            │               │
│     │──SDS Request────────────►│                            │               │
│     │  "oauth2-secret"         │                            │               │
│     │                          │──Fetch by reference───────►│               │
│     │                          │  secret/teams/.../...      │               │
│     │                          │                            │               │
│     │                          │◄──Secret value─────────────│               │
│     │                          │                            │               │
│     │◄──SDS Response───────────│                            │               │
│     │   (cached 5min)          │                            │               │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Caching

Secrets fetched from external backends are cached in memory:

- **Default TTL:** 300 seconds (5 minutes)
- **Configuration:** `FLOWPLANE_SECRET_CACHE_TTL_SECS` environment variable
- **Behavior:** Cache is per-reference, automatically invalidated after TTL
- **Cache Clearing:** Restart control plane or wait for TTL expiry

## Security Considerations

1. **Secret Encryption:** Database-stored secrets are encrypted at rest using AES-256-GCM
2. **No API Exposure:** Secret values are never returned via the REST API
3. **Team Isolation:** Secrets are scoped to teams; cross-team access is denied
4. **Audit Logging:** Secret access is logged for compliance
5. **Vault Best Practices:** Use short-lived tokens, enable audit logging in Vault

## Troubleshooting

### "Backend type 'vault' not registered"

- Ensure `FLOWPLANE_VAULT_ADDR` is set before starting Flowplane
- Check Flowplane logs for: `"Registered Vault secret backend"`

### "Secret not found in Vault"

- Verify Vault path: `vault kv get secret/path/to/secret`
- Ensure `reference` field matches Vault path (without mount prefix)

### "Invalid secret format in Vault"

- Vault secret must include `type` field or standard field names
- For generic secrets: use `secret` or `value` field

### "Feature not enabled"

- Enable via admin API: `PUT /api/v1/admin/apps/external_secrets`
- Verify: `GET /api/v1/admin/apps/external_secrets`

### Cache Issues

- Secrets are cached for 5 minutes by default
- To force refresh: delete and recreate the secret reference
- Or restart the control plane
