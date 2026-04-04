---
name: flowplane-secrets
description: Manage secrets for Flowplane's Envoy gateway — create, rotate, and deliver secrets via SDS. Covers CLI commands, MCP tools, REST API, encryption key management, and filter integration. Use when working with secrets, encryption keys, SDS, oauth2 tokens, TLS certificates, credential injection, or secret rotation.
license: Apache-2.0
metadata:
  author: rajeevramani
  version: "0.1.0"
compatibility: Requires Flowplane control plane with FLOWPLANE_SECRET_ENCRYPTION_KEY configured
---

# Flowplane Secrets

Manage encrypted secrets that are delivered to Envoy via SDS (Secret Discovery Service). Secrets store OAuth2 tokens, TLS certificates, API keys, and other credentials — encrypted at rest with AES-256-GCM, delivered to Envoy over the same ADS gRPC stream as CDS/LDS/RDS.

## 1. How Secrets Work

```
Create secret (CLI/MCP/REST)
  → Encrypt with AES-256-GCM → Store in PostgreSQL
  → SDS pushes to Envoy over ADS gRPC
  → Filter references secret by name
```

Secrets are team-scoped. Secret values are never exposed via API — only metadata (name, type, version, timestamps).

## 2. Enabling Secrets

### Dev Mode (automatic)

`flowplane init` generates a random 32-byte encryption key on first run and saves it to `~/.flowplane/encryption.key`. The key is reused on subsequent `flowplane init` calls, so secrets survive stack restarts. Delete this file to force a new key (existing secrets become unreadable).

### Prod Mode

Set `FLOWPLANE_SECRET_ENCRYPTION_KEY` environment variable (base64-encoded 32-byte key):

```bash
export FLOWPLANE_SECRET_ENCRYPTION_KEY=$(openssl rand -base64 32)
```

Pass it in your compose file or deployment config. Use a stable, backed-up key — changing it makes existing secrets unreadable.

## 3. Secret Types

| Type | Use Case | Configuration Fields |
|---|---|---|
| `generic_secret` | OAuth2 tokens, API keys, HMAC keys | `{ "type": "generic_secret", "secret": "<base64>" }` |
| `tls_certificate` | mTLS client/server certs | `{ "type": "tls_certificate", "certificate_chain": "<PEM>", "private_key": "<PEM>" }` |
| `certificate_validation_context` | CA trust chains | `{ "type": "certificate_validation_context", "trusted_ca": "<PEM>" }` |
| `session_ticket_keys` | TLS session resumption | `{ "type": "session_ticket_keys", "keys": [...] }` |

Note: `tls_certificate` requires PEM-formatted certificate and key data.

## 4. CLI Commands

### Create
```bash
flowplane secret create --name <NAME> --type <TYPE> --config '<JSON>' \
  [--description <DESC>] [--expires-at <ISO8601>]
```

Examples:
```bash
# Generic secret (OAuth2 client secret)
flowplane secret create --name oauth-secret --type generic_secret \
  --config '{"type":"generic_secret","secret":"dGVzdC1zZWNyZXQ="}'

# TLS certificate
flowplane secret create --name my-cert --type tls_certificate \
  --config '{"type":"tls_certificate","certificate_chain":"-----BEGIN CERTIFICATE-----\n...","private_key":"-----BEGIN PRIVATE KEY-----\n..."}'
```

### List
```bash
flowplane secret list                    # Table view
flowplane secret list --type generic_secret  # Filter by type
```

### Get
```bash
flowplane secret get <SECRET_ID> -o json
```

### Delete
```bash
flowplane secret delete <SECRET_ID> --yes    # --yes required in scripts
```

## 5. MCP Tools

| Tool | Action | Key Parameters |
|---|---|---|
| `cp_create_secret` | Create | `name`, `secretType`, `configuration`, `team`, `expiresAt` (optional, ISO 8601) |
| `cp_list_secrets` | List | `team`, `secretType` (optional filter) |
| `cp_get_secret` | Get | `id`, `team` |
| `cp_delete_secret` | Delete | `id` |

MCP parameters use camelCase. The `team` parameter is required for all tools in dev mode.

Example MCP create:
```json
{
  "name": "cp_create_secret",
  "arguments": {
    "name": "oauth-secret",
    "secretType": "generic_secret",
    "configuration": {"type": "generic_secret", "secret": "dGVzdA=="},
    "team": "default"
  }
}
```

## 6. REST API

```
POST   /api/v1/teams/{team}/secrets                    — Create secret
GET    /api/v1/teams/{team}/secrets                    — List secrets
GET    /api/v1/teams/{team}/secrets/{secret_id}        — Get secret
PUT    /api/v1/teams/{team}/secrets/{secret_id}        — Update secret
DELETE /api/v1/teams/{team}/secrets/{secret_id}        — Delete secret
POST   /api/v1/teams/{team}/secrets/{secret_id}/rotate — Rotate secret
POST   /api/v1/teams/{team}/secrets/reference          — Create reference-based secret (Vault/AWS/GCP)
```

Request body (camelCase):
```json
{
  "name": "oauth-secret",
  "secretType": "generic_secret",
  "configuration": {"type": "generic_secret", "secret": "dGVzdC1zZWNyZXQ="},
  "description": "OAuth2 client secret for upstream auth"
}
```

## 7. Secret Rotation

Rotate a secret's value without changing its name or ID. Filters referencing the secret automatically receive the new value via SDS.

```bash
# Via REST API
curl -X POST http://localhost:8080/api/v1/teams/default/secrets/{id}/rotate \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"configuration":{"type":"generic_secret","secret":"bmV3LXZhbHVl"}}'
```

Rotation increments the `version` field and updates `updatedAt`. The SDS watcher detects the change and pushes the new value to Envoy.

Reference-based secrets (Vault/AWS/GCP) cannot be rotated via this endpoint — update the value in the external backend directly.

## 8. Filter Integration

Filters reference secrets by name. The secret must exist before creating the filter.

### Which Filters Use Secrets

| Filter | Secret Field | Secret Type | Purpose |
|---|---|---|---|
| `oauth2` | `token_secret.name` | `generic_secret` | Client secret for OAuth2 flow |
| `jwt_auth` | `jwks` (Sds variant) | `generic_secret` | JWKS key material via SDS |
| `ext_authz` | `auth_secret.name` | `generic_secret` | Auth service API key/token |
| `ext_authz` | `tls_secret.name` | `tls_certificate` | mTLS cert for auth service |
| ~~`credential_injector`~~ | `secret_ref.name` | `generic_secret` | **NOT a FilterType** — XDS module exists but cannot be created via API. Use `header_mutation` instead. |

### Workflow: Secret + Filter

```bash
# 1. Create the secret
flowplane secret create --name oauth-secret --type generic_secret \
  --config '{"type":"generic_secret","secret":"c2VjcmV0LXZhbHVl"}'

# 2. Create filter referencing it (via MCP)
# Tool: cp_create_filter
# Args: { "filterType": "oauth2", "config": { "type": "oauth2", "config": {
#   "token_endpoint": "https://auth.example.com/oauth/token",
#   "authorization_endpoint": "https://auth.example.com/authorize",
#   "redirect_uri": "https://app.example.com/callback",
#   "credentials": { "client_id": "my-app" },
#   "token_secret": { "name": "oauth-secret" },
#   "forward_bearer_token": true
# }}}

# 3. Rotate when needed
curl -X POST .../secrets/{id}/rotate -d '{"configuration":{"type":"generic_secret","secret":"bmV3LXNlY3JldA=="}}'
# Envoy receives updated secret automatically via SDS
```

## 9. External Backends (Vault/AWS/GCP)

Reference-based secrets fetch values from external backends instead of storing encrypted data in PostgreSQL.

```
POST /api/v1/teams/{team}/secrets/reference
{
  "name": "vault-secret",
  "secretType": "generic_secret",
  "backend": "vault",
  "reference": "secret/data/my-app/api-key",
  "referenceVersion": "1"
}
```

Requires backend configuration via environment variables:
- **Vault**: `FLOWPLANE_VAULT_ADDR`, `FLOWPLANE_VAULT_TOKEN`
- **AWS/GCP**: Configured via `SecretBackendRegistry` (see source)

If no backends are configured, reference creation returns 503.

## 10. Source Files

| Component | File |
|---|---|
| Domain types | `src/domain/secret.rs` |
| Encryption | `src/services/secret_encryption.rs` |
| Repository | `src/storage/repositories/secret.rs` |
| REST handlers | `src/api/handlers/secrets/` |
| CLI commands | `src/cli/secrets.rs` |
| MCP tools | `src/mcp/tools/secrets.rs` |
| SDS delivery | `src/xds/secret.rs`, `src/xds/services/database.rs` |
| Backend registry | `src/secrets/backends/registry.rs` |
| Vault backend | `src/secrets/backends/vault.rs` |
| Key generation | `src/cli/compose.rs` (`get_or_create_dev_encryption_key`) |
| OAuth2 SDS ref | `src/xds/filters/http/oauth2.rs` |
| jwt_auth SDS ref | `src/xds/filters/http/jwt_auth.rs` |
| ext_authz SDS ref | `src/xds/filters/http/ext_authz.rs` |
| credential_injector SDS ref | `src/xds/filters/http/credential_injector.rs` |
