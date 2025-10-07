# HTTP API Reference

Flowplane exposes a REST API on the configured bind address (defaults to `127.0.0.1:8080`). Every
request must carry a valid bearer token with scopes that match the requested resource. See
[`docs/authentication.md`](authentication.md) for a detailed overview of personal access tokens
and scope assignments.

The OpenAPI document is always available at `/api-docs/openapi.json`; an interactive Swagger UI is
served from `/swagger-ui`.

## Quick Start

### First-Time Setup

On first startup, Flowplane generates a bootstrap admin token and displays it in a **prominent ASCII art banner**:

```bash
# Start the control plane
DATABASE_URL=sqlite://./data/flowplane.db \
FLOWPLANE_API_BIND_ADDRESS=0.0.0.0 \
cargo run --bin flowplane

# Look for banner output:
================================================================================
🔐 BOOTSTRAP ADMIN TOKEN GENERATED
================================================================================
This token has FULL ACCESS to the control plane.
Store it securely - it will not be shown again!

Token: fp_pat_a1b2c3d4-e5f6-7890-abcd-ef1234567890.xJ9k...
================================================================================
```

Extract and store the token:

```bash
# From Docker logs
docker-compose logs control-plane 2>&1 | grep -oP 'token: \Kfp_pat_[^\s]+'

# Export for use in requests
export FLOWPLANE_TOKEN="fp_pat_..."
```

Use the bootstrap token to create scoped service tokens for your applications. See [token-management.md](token-management.md) for detailed workflows.

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

## Core Resource Endpoints (Native API)

The Native API provides direct access to Envoy configuration primitives. All endpoints require appropriate scopes:

### Clusters

| Endpoint | Method | Scope | Description |
|----------|--------|-------|-------------|
| `/api/v1/clusters` | `GET` | `clusters:read` | List all clusters |
| `/api/v1/clusters` | `POST` | `clusters:write` | Create a new cluster |
| `/api/v1/clusters/{name}` | `GET` | `clusters:read` | Get cluster by name |
| `/api/v1/clusters/{name}` | `PUT` | `clusters:write` | Update existing cluster |
| `/api/v1/clusters/{name}` | `DELETE` | `clusters:write` | Delete cluster |

#### Create Cluster Example

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "backend-api",
    "serviceName": "api",
    "endpoints": [
      { "host": "api.example.com", "port": 443 }
    ],
    "connectTimeoutSeconds": 5,
    "useTls": true,
    "tlsServerName": "api.example.com",
    "healthCheck": {
      "path": "/health",
      "intervalSeconds": 10,
      "timeoutSeconds": 2,
      "unhealthyThreshold": 3,
      "healthyThreshold": 2
    }
  }'
```

#### Update Cluster Example

```bash
curl -sS -X PUT http://127.0.0.1:8080/api/v1/clusters/backend-api \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "backend-api",
    "serviceName": "api",
    "endpoints": [
      { "host": "api.example.com", "port": 443 },
      { "host": "api2.example.com", "port": 443 }
    ],
    "connectTimeoutSeconds": 5,
    "useTls": true,
    "tlsServerName": "api.example.com"
  }'
```

### Routes

| Endpoint | Method | Scope | Description |
|----------|--------|-------|-------------|
| `/api/v1/routes` | `GET` | `routes:read` | List all route configurations |
| `/api/v1/routes` | `POST` | `routes:write` | Create a new route configuration |
| `/api/v1/routes/{name}` | `GET` | `routes:read` | Get route configuration by name |
| `/api/v1/routes/{name}` | `PUT` | `routes:write` | Update existing route configuration |
| `/api/v1/routes/{name}` | `DELETE` | `routes:write` | Delete route configuration |

#### Create Route Example

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/routes \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "api-routes",
    "virtualHosts": [
      {
        "name": "api-host",
        "domains": ["api.example.com", "*.api.example.com"],
        "routes": [
          {
            "name": "users-api",
            "match": {
              "path": {"type": "prefix", "value": "/users"},
              "methods": ["GET", "POST"]
            },
            "action": {
              "type": "forward",
              "cluster": "backend-api",
              "timeoutSeconds": 30
            }
          },
          {
            "name": "catch-all",
            "match": {
              "path": {"type": "prefix", "value": "/"}
            },
            "action": {
              "type": "forward",
              "cluster": "backend-api",
              "timeoutSeconds": 10
            }
          }
        ]
      }
    ]
  }'
```

### Listeners

| Endpoint | Method | Scope | Description |
|----------|--------|-------|-------------|
| `/api/v1/listeners` | `GET` | `listeners:read` | List all listeners |
| `/api/v1/listeners` | `POST` | `listeners:write` | Create a new listener |
| `/api/v1/listeners/{name}` | `GET` | `listeners:read` | Get listener by name |
| `/api/v1/listeners/{name}` | `PUT` | `listeners:write` | Update existing listener |
| `/api/v1/listeners/{name}` | `DELETE` | `listeners:write` | Delete listener |

#### Create Listener Example

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "api-listener",
    "address": "0.0.0.0",
    "port": 8080,
    "protocol": "HTTP",
    "filterChains": [
      {
        "name": "default",
        "filters": [
          {
            "name": "envoy.filters.network.http_connection_manager",
            "type": "httpConnectionManager",
            "routeConfigName": "api-routes",
            "httpFilters": [
              {
                "name": "envoy.filters.http.cors",
                "filter": {
                  "type": "cors",
                  "allow_origin_string_match": [{"exact": "https://app.example.com"}],
                  "allow_methods": ["GET", "POST", "PUT", "DELETE"],
                  "allow_headers": ["Content-Type", "Authorization"],
                  "max_age": "86400"
                }
              },
              {
                "name": "envoy.filters.http.local_ratelimit",
                "filter": {
                  "type": "local_rate_limit",
                  "stat_prefix": "listener_global",
                  "token_bucket": {
                    "max_tokens": 1000,
                    "tokens_per_fill": 1000,
                    "fill_interval_ms": 1000
                  }
                }
              },
              {
                "name": "envoy.filters.http.router",
                "filter": {"type": "router"}
              }
            ]
          }
        ]
      }
    ]
  }'
```

See [filters.md](filters.md) for complete HTTP filter reference and configuration options.

## Platform API (API Definitions)

The Platform API provides higher-level abstractions for API gateway workflows, supporting team-based multi-tenancy and OpenAPI-driven gateway creation.

### API Definitions (BFF Pattern)

| Endpoint | Method | Scope | Description |
|----------|--------|-------|-------------|
| `/api/v1/api-definitions` | `POST` | `routes:write` | Create a new API definition (BFF) |
| `/api/v1/api-definitions` | `GET` | `routes:read` | List all API definitions |
| `/api/v1/api-definitions/{id}` | `GET` | `routes:read` | Get API definition by ID |
| `/api/v1/api-definitions/from-openapi` | `POST` | `routes:write` | Import OpenAPI 3.0 spec and generate gateway resources |
| `/api/v1/api-definitions/{id}/routes` | `POST` | `routes:write` | Append a route to existing API definition |
| `/api/v1/api-definitions/{id}/bootstrap` | `GET` | `routes:read` | Get Envoy bootstrap configuration for API |

### Import OpenAPI Specification

The OpenAPI import endpoint generates complete gateway configurations (clusters, routes, listener) from an OpenAPI 3.0 document:

```bash
curl -sS \
  -X POST "http://127.0.0.1:8080/api/v1/api-definitions/from-openapi?team=example" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  --data-binary @openapi.json
```

**Query Parameters:**

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `team` | Yes | - | Team namespace for generated resources (e.g., `myteam`) |
| `listener` | No | `default-gateway-listener` | Listener name (creates new if specified) |
| `port` | No | `10000` | Listener port (only used with custom listener) |
| `bind_address` | No | `0.0.0.0` | Listener bind address (only used with custom listener) |
| `protocol` | No | `HTTP` | Listener protocol (only used with custom listener) |

**Behavior:**

- **Default (Shared Listener Mode)**: Generated routes join the pre-seeded `default-gateway-listener` on port `10000`, allowing multiple teams to share infrastructure without port conflicts
- **Dedicated Listener Mode**: Specify `listener` query parameter to create isolated listener with custom port, bind address, and protocol

**OpenAPI Extensions:**

Flowplane supports custom extensions for filter configuration:

```yaml
x-flowplane-filters:
  cors:
    allow_origin: ["*"]
    allow_methods: ["GET", "POST"]
  local_rate_limit:
    stat_prefix: "api_ratelimit"
    token_bucket:
      max_tokens: 100
      tokens_per_fill: 100
      fill_interval_ms: 1000

paths:
  /users:
    get:
      x-flowplane-custom-response:
        status_code: 429
        body: "Rate limit exceeded"
```

See [examples/README-x-flowplane-extensions.md](../examples/README-x-flowplane-extensions.md) for complete extension reference.

Each request returns a structured error payload on validation or authorization failure, and logs an
audit entry for traceability.

## Observability Endpoints

| Endpoint | Method | Auth Required | Description |
|----------|--------|---------------|-------------|
| `/healthz` | `GET` | No | Control plane health check (returns 200 OK when ready) |
| `/metrics` | `GET` | No | Prometheus metrics endpoint (if `FLOWPLANE_ENABLE_METRICS=true`) |
| `/api-docs/openapi.json` | `GET` | No | OpenAPI 3.0 specification |
| `/swagger-ui` | `GET` | No | Interactive API documentation |

## Response Formats

### Success Responses

**201 Created** (POST operations):
```json
{
  "id": "resource-id",
  "name": "resource-name",
  "createdAt": "2025-10-07T10:30:00Z"
}
```

**200 OK** (GET, PUT operations):
```json
{
  "name": "resource-name",
  "config": { ... }
}
```

**204 No Content** (DELETE operations):
```
(empty body)
```

### Error Responses

**400 Bad Request** (validation errors):
```json
{
  "error": "validation_failed",
  "message": "Invalid cluster configuration",
  "details": [
    "endpoints: must contain at least one endpoint",
    "connectTimeoutSeconds: must be positive"
  ]
}
```

**401 Unauthorized** (missing/invalid token):
```json
{
  "error": "unauthorized",
  "message": "missing or invalid bearer token"
}
```

**403 Forbidden** (insufficient scopes):
```json
{
  "error": "forbidden",
  "message": "insufficient permissions: requires clusters:write"
}
```

**404 Not Found** (resource doesn't exist):
```json
{
  "error": "not_found",
  "message": "cluster 'backend-api' not found"
}
```

**409 Conflict** (resource already exists):
```json
{
  "error": "conflict",
  "message": "cluster 'backend-api' already exists"
}
```

**500 Internal Server Error** (unexpected failures):
```json
{
  "error": "internal_error",
  "message": "database connection failed",
  "requestId": "req_a1b2c3d4"
}
```

## Common Patterns

### Listing Resources with Pagination

```bash
# List clusters with pagination
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/clusters?limit=20&offset=0"

# List routes
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/routes"
```

### Idempotent Updates

PUT operations are idempotent - sending the same request multiple times produces the same result:

```bash
# First update: creates/updates resource
curl -sS -X PUT http://127.0.0.1:8080/api/v1/clusters/backend-api \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{...}'

# Second update with same data: no-op, returns 200 OK
curl -sS -X PUT http://127.0.0.1:8080/api/v1/clusters/backend-api \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{...}'
```

### Resource Verification

After creating resources, verify they were persisted correctly:

```bash
# Create cluster
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters -H "..." -d '{...}'

# Verify cluster exists
curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/clusters/backend-api"
```

### Cleanup

Delete resources in reverse order of dependencies:

```bash
# 1. Delete listener (depends on routes)
curl -sS -X DELETE http://127.0.0.1:8080/api/v1/listeners/api-listener \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN"

# 2. Delete routes (depends on clusters)
curl -sS -X DELETE http://127.0.0.1:8080/api/v1/routes/api-routes \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN"

# 3. Delete cluster (no dependencies)
curl -sS -X DELETE http://127.0.0.1:8080/api/v1/clusters/backend-api \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN"
```

## CLI vs API

The CLI uses the same service layer as the HTTP API. If you prefer terminal workflows, run
`flowplane auth --help` for usage examples or see [`token-management.md`](token-management.md).

## Additional Resources

- **Getting Started**: [getting-started.md](getting-started.md) - Step-by-step tutorial
- **Authentication**: [authentication.md](authentication.md) - Token management and scopes
- **Filters**: [filters.md](filters.md) - HTTP filter reference
- **Architecture**: [architecture.md](architecture.md) - System design and components
- **Interactive Docs**: http://127.0.0.1:8080/swagger-ui - Live API exploration
