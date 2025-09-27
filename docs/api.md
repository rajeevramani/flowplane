# HTTP API Reference

Flowplane exposes a REST API on the configured bind address (defaults to `127.0.0.1:8080`). Every
request must carry a valid bearer token with scopes that match the requested resource. See
[`docs/authentication.md`](docs/authentication.md) for a detailed overview of personal access tokens
and scope assignments.

The OpenAPI document is always available at `/api-docs/openapi.json`; an interactive Swagger UI is
served from `/swagger-ui`.

## Authentication Header

```
Authorization: Bearer fp_pat_<token-id>.<secret>
```

Tokens are checked for scope membership before handlers execute. Failure to present a credential or
attempting an operation without the matching scope yields a `401`/`403` error with a sanitized body:

```json
{
  "error": "unauthorized",
  "message": "missing or invalid bearer"
}
```

## Token Management

| Endpoint | Method | Scope | Description |
|----------|--------|-------|-------------|
| `/api/v1/tokens` | `POST` | `tokens:write` | Issue a new personal access token. Returns the token once. |
| `/api/v1/tokens` | `GET` | `tokens:read` | List tokens with pagination. |
| `/api/v1/tokens/{id}` | `GET` | `tokens:read` | Retrieve token metadata. Secret is never returned. |
| `/api/v1/tokens/{id}` | `PATCH` | `tokens:write` | Update name, description, status, scopes, or expiration. |
| `/api/v1/tokens/{id}` | `DELETE` | `tokens:write` | Revoke a token (status becomes `revoked`). |
| `/api/v1/tokens/{id}/rotate` | `POST` | `tokens:write` | Rotate the secret. Response contains the new token once. |

### Create a Token

```bash
curl -sS \
  -X POST http://127.0.0.1:8080/api/v1/tokens \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
        "name": "ci-pipeline",
        "description": "Token used by CI deployments",
        "scopes": ["clusters:write", "routes:write", "listeners:read"],
        "expiresAt": null
      }'
```

Successful responses return `201 Created` and the new token in the body:

```json
{
  "id": "8a6f9d37-9a4c-4dbe-a494-9bd924dbd1b1",
  "token": "fp_pat_8a6f9d37-9a4c-4dbe-a494-9bd924dbd1b1.CJ7p..."
}
```

### List Tokens

```bash
curl -sS \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  "http://127.0.0.1:8080/api/v1/tokens?limit=20&offset=0"
```

Returns a JSON array of token records (without secrets):

```json
[
  {
    "id": "8a6f9d37-9a4c-4dbe-a494-9bd924dbd1b1",
    "name": "ci-pipeline",
    "status": "active",
    "scopes": ["clusters:write", "routes:write", "listeners:read"],
    "expiresAt": null,
    "lastUsedAt": "2025-01-05T17:10:22Z"
  }
]
```

### Rotate a Token

```bash
curl -sS \
  -X POST http://127.0.0.1:8080/api/v1/tokens/8a6f9d37-9a4c-4dbe-a494-9bd924dbd1b1/rotate \
  -H "Authorization: Bearer $ADMIN_TOKEN"
```

Response contains the new token value. Update dependent systems immediately—previous secrets stop
working as soon as the rotation succeeds.

## Core Configuration Endpoints

The legacy configuration APIs remain unchanged, but now require scopes:

| Endpoint | Method | Scope |
|----------|--------|-------|
| `/api/v1/clusters` | `GET` | `clusters:read` |
| `/api/v1/clusters` | `POST` | `clusters:write` |
| `/api/v1/clusters/{name}` | `GET` | `clusters:read` |
| `/api/v1/clusters/{name}` | `PUT`/`DELETE` | `clusters:write` |
| `/api/v1/routes` | `GET` | `routes:read` |
| `/api/v1/routes` | `POST` | `routes:write` |
| `/api/v1/routes/{name}` | `GET` | `routes:read` |
| `/api/v1/routes/{name}` | `PUT`/`DELETE` | `routes:write` |
| `/api/v1/listeners` | `GET` | `listeners:read` |
| `/api/v1/listeners` | `POST` | `listeners:write` |
| `/api/v1/listeners/{name}` | `GET` | `listeners:read` |
| `/api/v1/listeners/{name}` | `PUT`/`DELETE` | `listeners:write` |
| `/api/v1/gateways/openapi` | `POST` | `gateways:import` |

Each request returns a structured error payload on validation or authorization failure, and logs an
audit entry for traceability.

## Observability Endpoints

- `/healthz` – control plane readiness (no auth required).
- `/metrics` – Prometheus exporter. Requires metrics to be enabled via configuration. The exporter is
  bound to the address returned by `ObservabilityConfig::metrics_bind_address`.

## CLI vs API

The CLI uses the same service layer as the HTTP API. If you prefer terminal workflows, run
`flowplane auth --help` for usage examples or see [`docs/token-management.md`](docs/token-management.md).
