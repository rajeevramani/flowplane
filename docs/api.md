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

## Learning Sessions

Learning sessions enable automatic API schema discovery by observing live traffic. They capture request/response patterns, infer JSON schemas, and build API documentation from actual usage. Learning sessions are team-scoped and track progress towards a configurable sample target.

| Endpoint | Method | Scope | Description |
|----------|--------|-------|-------------|
| `/api/v1/learning-sessions` | `POST` | `learning-sessions:write` | Create a new learning session to capture traffic |
| `/api/v1/learning-sessions` | `GET` | `learning-sessions:read` | List all learning sessions for your team |
| `/api/v1/learning-sessions/{id}` | `GET` | `learning-sessions:read` | Get learning session details and progress |
| `/api/v1/learning-sessions/{id}` | `DELETE` | `learning-sessions:write` | Cancel an active learning session |

### Create a Learning Session

```bash
curl -sS \
  -X POST http://127.0.0.1:8080/api/v1/learning-sessions \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "routePattern": "^/api/v2/payments/.*",
    "clusterName": "payments-api-prod",
    "httpMethods": ["POST", "PUT"],
    "targetSampleCount": 1000,
    "maxDurationSeconds": 7200,
    "triggeredBy": "deploy-pipeline-v2.3.4",
    "deploymentVersion": "v2.3.4"
  }'
```

**Request Fields:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `routePattern` | string | Yes | Regex pattern to match request paths (1-500 chars) |
| `clusterName` | string | No | Filter by cluster name |
| `httpMethods` | string[] | No | Filter by HTTP methods (e.g., `["GET", "POST"]`) |
| `targetSampleCount` | integer | Yes | Number of samples to collect (1-100000) |
| `maxDurationSeconds` | integer | No | Maximum session duration in seconds |
| `triggeredBy` | string | No | Who/what triggered this session (e.g., pipeline ID) |
| `deploymentVersion` | string | No | Version being deployed/learned |
| `configurationSnapshot` | object | No | Optional JSON snapshot of configuration |

**Response (201 Created):**

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "team": "payments-team",
  "routePattern": "^/api/v2/payments/.*",
  "clusterName": "payments-api-prod",
  "httpMethods": ["POST", "PUT"],
  "status": "active",
  "createdAt": "2025-10-26T12:00:00Z",
  "startedAt": "2025-10-26T12:00:01Z",
  "endsAt": "2025-10-26T14:00:01Z",
  "completedAt": null,
  "targetSampleCount": 1000,
  "currentSampleCount": 0,
  "progressPercentage": 0.0,
  "triggeredBy": "deploy-pipeline-v2.3.4",
  "deploymentVersion": "v2.3.4",
  "errorMessage": null
}
```

**Status Values:**

- `pending` - Session created but not yet active
- `active` - Currently capturing traffic
- `completed` - Reached target sample count
- `failed` - Encountered an error
- `cancelled` - Manually cancelled by user

### List Learning Sessions

```bash
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/learning-sessions?status=active&limit=20&offset=0"
```

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `status` | string | Filter by status (`pending`, `active`, `completed`, `failed`, `cancelled`) |
| `limit` | integer | Maximum number of results (default: 100) |
| `offset` | integer | Pagination offset (default: 0) |

Returns an array of learning session objects (same structure as create response).

### Get Learning Session

```bash
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/learning-sessions/550e8400-e29b-41d4-a716-446655440000"
```

Returns a single learning session object with current progress.

### Cancel Learning Session

```bash
curl -sS \
  -X DELETE \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/learning-sessions/550e8400-e29b-41d4-a716-446655440000"
```

Returns `204 No Content` on success. Cancellation stops traffic observation and marks the session as `cancelled`.

## Aggregated Schemas (API Catalog)

After learning sessions collect traffic samples, Flowplane automatically aggregates the observed request/response patterns into versioned JSON schemas. These aggregated schemas represent the inferred API contract based on actual usage, including field presence, type consistency, and confidence scores. Schemas support version tracking, breaking change detection, and OpenAPI export.

| Endpoint | Method | Scope | Description |
|----------|--------|-------|-------------|
| `/api/v1/aggregated-schemas` | `GET` | `aggregated-schemas:read` | List all aggregated schemas for your team |
| `/api/v1/aggregated-schemas/{id}` | `GET` | `aggregated-schemas:read` | Get detailed schema by ID |
| `/api/v1/aggregated-schemas/{id}/compare` | `GET` | `aggregated-schemas:read` | Compare schema versions to detect changes |
| `/api/v1/aggregated-schemas/{id}/export` | `GET` | `aggregated-schemas:read` | Export schema as OpenAPI 3.1 specification |

### List Aggregated Schemas

```bash
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/aggregated-schemas?path=users&http_method=GET&min_confidence=0.8"
```

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `path` | string | Filter by API path (substring match, e.g., `users`) |
| `http_method` | string | Filter by HTTP method (exact match: `GET`, `POST`, etc.) |
| `min_confidence` | number | Filter by minimum confidence score (0.0-1.0) |

**Response (200 OK):**

```json
[
  {
    "id": 42,
    "team": "payments-team",
    "path": "/api/v2/users/{id}",
    "httpMethod": "GET",
    "version": 3,
    "previousVersionId": 38,
    "requestSchema": null,
    "responseSchemas": {
      "200": {
        "type": "object",
        "properties": {
          "id": { "type": "string" },
          "email": { "type": "string" },
          "name": { "type": "string" }
        },
        "required": ["id", "email"]
      }
    },
    "sampleCount": 1247,
    "confidenceScore": 0.94,
    "breakingChanges": null,
    "firstObserved": "2025-10-20T08:30:00Z",
    "lastObserved": "2025-10-26T12:00:00Z",
    "createdAt": "2025-10-26T12:05:00Z",
    "updatedAt": "2025-10-26T12:05:00Z"
  }
]
```

**Schema Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `id` | integer | Unique schema identifier |
| `team` | string | Team that owns this schema |
| `path` | string | API path pattern |
| `httpMethod` | string | HTTP method (GET, POST, etc.) |
| `version` | integer | Version number (increments on schema changes) |
| `previousVersionId` | integer | ID of previous version (null for v1) |
| `requestSchema` | object | JSON Schema for request body (null for GET) |
| `responseSchemas` | object | JSON Schemas keyed by status code |
| `sampleCount` | integer | Number of observations aggregated |
| `confidenceScore` | number | Quality score (0.0-1.0) based on consistency |
| `breakingChanges` | array | List of breaking changes from previous version |
| `firstObserved` | string | Timestamp of first observation |
| `lastObserved` | string | Timestamp of most recent observation |

### Get Aggregated Schema

```bash
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/aggregated-schemas/42"
```

Returns a single aggregated schema object (same structure as list response).

### Compare Schema Versions

Compare two versions of the same endpoint schema to detect changes:

```bash
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/aggregated-schemas/42/compare?with_version=2"
```

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `with_version` | integer | Yes | Version number to compare against current schema |

**Response (200 OK):**

```json
{
  "currentSchema": {
    "id": 42,
    "version": 3,
    "sampleCount": 1247,
    "confidenceScore": 0.94
  },
  "comparedSchema": {
    "id": 38,
    "version": 2,
    "sampleCount": 856,
    "confidenceScore": 0.89
  },
  "differences": {
    "versionChange": 1,
    "sampleCountChange": 391,
    "confidenceChange": 0.05,
    "hasBreakingChanges": true,
    "breakingChanges": [
      {
        "type": "field_removed",
        "field": "deprecated_field",
        "message": "Field 'deprecated_field' was removed"
      },
      {
        "type": "required_added",
        "field": "email",
        "message": "Field 'email' is now required"
      }
    ]
  }
}
```

**Breaking Change Types:**

- `field_removed` - A field that existed in the old schema is gone
- `field_type_changed` - A field's type changed (e.g., string → integer)
- `required_added` - An optional field became required
- `required_removed` - A required field became optional (usually safe)

### Export Schema as OpenAPI

Export an aggregated schema as an OpenAPI 3.1 specification for use with API tools:

```bash
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/aggregated-schemas/42/export?includeMetadata=true"
```

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `includeMetadata` | boolean | `true` | Include Flowplane-specific extensions (x-flowplane-*) |

**Response (200 OK):**

```json
{
  "openapi": "3.1.0",
  "info": {
    "title": "Learned API Schema",
    "version": "3",
    "description": "API schema learned from 1247 traffic samples"
  },
  "paths": {
    "/api/v2/users/{id}": {
      "get": {
        "summary": "GET /api/v2/users/{id}",
        "responses": {
          "200": {
            "description": "Successful response",
            "content": {
              "application/json": {
                "schema": {
                  "type": "object",
                  "properties": {
                    "id": { "type": "string" },
                    "email": { "type": "string" },
                    "name": { "type": "string" }
                  },
                  "required": ["id", "email"]
                }
              }
            }
          }
        },
        "x-flowplane-confidence": 0.94,
        "x-flowplane-samples": 1247
      }
    }
  },
  "components": {
    "schemas": {}
  }
}
```

The exported OpenAPI spec can be imported into tools like Swagger UI, Postman, or used to generate client SDKs.

## Reporting API

The Reporting API provides comprehensive visibility into your API gateway infrastructure. These endpoints allow you to view the current state of routes, clusters, listeners, and understand how requests flow through the system.

All reporting endpoints respect team-based isolation:
- Team-scoped tokens (`team:{name}:reports:read`) only see their team's resources
- Resource-level tokens (`reports:read`) see all resources
- `admin:all` scope grants full access across all teams

| Endpoint | Method | Scope | Description |
|----------|--------|-------|-------------|
| `/api/v1/reports/route-flows` | `GET` | `reports:read` | List route flows showing end-to-end request routing |

### List Route Flows

View how requests flow through the system from listener → route → cluster → endpoint. This provides a comprehensive overview of your API gateway routing configuration.

```bash
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/reports/route-flows"
```

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `limit` | integer | `50` | Maximum number of items to return (1-1000) |
| `offset` | integer | `0` | Number of items to skip for pagination |
| `team` | string | - | Filter by team name (admin tokens only) |

**Examples:**

```bash
# List first 10 route flows
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/reports/route-flows?limit=10&offset=0"

# Filter by team (requires admin:all or reports:read scope)
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/reports/route-flows?team=platform"
```

**Response:**

```json
{
  "routeFlows": [
    {
      "routeName": "api-gateway-route",
      "path": "/api/v1/*",
      "cluster": "backend-cluster",
      "endpoints": ["10.0.1.5:8080", "10.0.1.6:8080"],
      "listener": {
        "name": "public-https",
        "port": 443,
        "address": "0.0.0.0"
      },
      "team": "platform"
    },
    {
      "routeName": "health-check-route",
      "path": "/health",
      "cluster": "health-cluster",
      "endpoints": ["localhost:8081"],
      "listener": {
        "name": "public-https",
        "port": 443,
        "address": "0.0.0.0"
      },
      "team": null
    }
  ],
  "total": 42,
  "offset": 0,
  "limit": 50
}
```

**Use Cases:**

- **Infrastructure Audit**: Verify routing configuration matches intended architecture
- **Troubleshooting**: Understand request flow when debugging routing issues
- **Capacity Planning**: Identify clusters with many routes for load distribution
- **Security Review**: Audit which endpoints are exposed through which listeners
- **Documentation**: Generate up-to-date routing diagrams from actual configuration

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
