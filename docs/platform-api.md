# Platform API Documentation

The Platform API provides a higher-level abstraction over Flowplane's Native API, designed for team-based multi-tenancy and OpenAPI-driven gateway creation workflows. This API simplifies the process of creating API gateways by managing clusters, routes, and listeners automatically from a single API definition.

## Overview

The Platform API allows you to:

- **Create API gateways from JSON or OpenAPI specs** - Define your entire API in one request
- **Import OpenAPI with filter extensions** - Use `x-flowplane-filters` and `x-flowplane-route-overrides` for declarative filter configuration
- **Choose listener isolation modes** - Use dedicated listeners per API or route traffic through shared listeners
- **Generate Envoy bootstrap configs** - Download ready-to-use bootstrap configurations for your data planes
- **Manage API lifecycles** - Update routes, change domains, and modify filter configurations dynamically

## Authentication & Authorization

All Platform API endpoints require authentication via Bearer token in the `Authorization` header:

```bash
Authorization: Bearer YOUR_TOKEN
```

### Required Scopes

| Scope | Operations |
|-------|-----------|
| `generate-envoy-config:read` | Download Envoy bootstrap configuration |
| `openapi-import:read` | List OpenAPI imports |
| `openapi-import:write` | Import OpenAPI specifications |

See [Authentication Documentation](authentication.md) for token management.

## API Reference

All endpoints are also documented interactively in the Swagger UI at `http://127.0.0.1:8080/swagger-ui`.

---

## Endpoints

### 1. List API Definitions

```http
GET /api/v1/api-definitions
```

Retrieve a list of API definitions, optionally filtered by team or domain.

#### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `team` | string | No | Filter by team name |
| `domain` | string | No | Filter by domain |
| `limit` | integer | No | Maximum number of results |
| `offset` | integer | No | Number of results to skip |

#### Response

**Status**: `200 OK`

**Body**: Array of API definition summaries

```json
[
  {
    "id": "api-def-abc123",
    "team": "payments",
    "domain": "payments.example.com",
    "listenerIsolation": false,
    "bootstrapUri": "/api/v1/api-definitions/api-def-abc123/bootstrap",
    "version": 1,
    "createdAt": "2025-10-06T09:00:00Z",
    "updatedAt": "2025-10-06T09:00:00Z"
  }
]
```

#### Example

```bash
curl -X GET "http://localhost:8080/api/v1/api-definitions?team=payments&limit=10" \
  -H "Authorization: Bearer YOUR_TOKEN"
```

---

### 2. Get API Definition by ID

```http
GET /api/v1/api-definitions/{id}
```

Retrieve a specific API definition by its ID.

#### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | string | API definition ID |

#### Response

**Status**: `200 OK`

**Body**: API definition summary

```json
{
  "id": "api-def-abc123",
  "team": "payments",
  "domain": "payments.example.com",
  "listenerIsolation": false,
  "bootstrapUri": "/api/v1/api-definitions/api-def-abc123/bootstrap",
  "version": 1,
  "createdAt": "2025-10-06T09:00:00Z",
  "updatedAt": "2025-10-06T09:00:00Z"
}
```

#### Error Responses

| Status | Description |
|--------|-------------|
| `404` | API definition not found |
| `503` | API definition repository not configured |

#### Example

```bash
curl -X GET "http://localhost:8080/api/v1/api-definitions/api-def-abc123" \
  -H "Authorization: Bearer YOUR_TOKEN"
```

---

### 3. Get Bootstrap Configuration

```http
GET /api/v1/api-definitions/{id}/bootstrap
```

Download the Envoy bootstrap configuration for an API definition. The bootstrap includes listener information and ADS configuration for connecting data planes.

#### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | string | API definition ID |

#### Query Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `format` | string | `yaml` | Output format: `yaml` or `json` |
| `scope` | string | `all` | Resource scope: `all`, `team`, or `allowlist` |
| `includeDefault` | boolean | `false` | Include default resources in team scope |
| `allowlist` | array[string] | `[]` | Listener names when `scope=allowlist` |

#### Response

**Status**: `200 OK`

**Content-Type**: `application/yaml` or `application/json`

**Body**: Envoy bootstrap configuration

```yaml
listeners:
  - name: "payments-shared-listener"
    address: "0.0.0.0"
    port: 10010
    protocol: "HTTP"
admin:
  access_log_path: "/tmp/envoy_admin.log"
  address:
    socket_address:
      address: "127.0.0.1"
      port_value: 9901
node:
  id: "team=payments/dp-550e8400-e29b-41d4-a716-446655440000"
  cluster: "payments-cluster"
  metadata:
    team: "payments"
dynamic_resources:
  lds_config:
    ads: {}
  cds_config:
    ads: {}
  ads_config:
    api_type: "GRPC"
    transport_api_version: "V3"
    grpc_services:
      - envoy_grpc:
          cluster_name: "xds_cluster"
static_resources:
  clusters:
    - name: "xds_cluster"
      type: "LOGICAL_DNS"
      dns_lookup_family: "V4_ONLY"
      connect_timeout: "1s"
      http2_protocol_options: {}
      load_assignment:
        cluster_name: "xds_cluster"
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: "127.0.0.1"
                      port_value: 18000
```

#### Listener Information

The `listeners` array in the bootstrap contains the listener details for the API definition:

- **Isolated Listener Mode** (`listenerIsolation: true`): Includes the generated dedicated listener
- **Shared Listener Mode** (`listenerIsolation: false`): Includes all target listeners (or `default-gateway-listener` if none specified)

#### Example

```bash
# Download YAML bootstrap
curl -X GET "http://localhost:8080/api/v1/api-definitions/api-def-abc123/bootstrap?format=yaml" \
  -H "Authorization: Bearer YOUR_TOKEN" > bootstrap.yaml

# Download JSON with team scope
curl -X GET "http://localhost:8080/api/v1/api-definitions/api-def-abc123/bootstrap?format=json&scope=team" \
  -H "Authorization: Bearer YOUR_TOKEN" > bootstrap.json
```

---

### 4. Create API Definition

```http
POST /api/v1/api-definitions
```

Create a new API definition with routes and automatically generated clusters.

#### Request Body

**Content-Type**: `application/json`

```json
{
  "team": "payments",
  "domain": "payments.example.com",
  "listenerIsolation": false,
  "targetListeners": ["default-gateway-listener"],
  "routes": [
    {
      "match": {
        "prefix": "/api/v1/payments"
      },
      "cluster": {
        "name": "payments-backend",
        "endpoint": "payments-backend.svc.cluster.local:8080"
      },
      "timeoutSeconds": 30,
      "rewrite": {
        "prefix": "/internal/v1"
      },
      "filters": {
        "cors": {
          "allowOrigin": ["https://example.com"],
          "allowMethods": ["GET", "POST", "PUT"],
          "allowCredentials": true
        },
        "rateLimit": {
          "requestsPerUnit": 100,
          "unit": "minute"
        }
      }
    }
  ]
}
```

#### Request Schema

##### CreateApiDefinitionBody

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `team` | string | Yes | Team name (max 100 characters) |
| `domain` | string | Yes | Domain name (max 253 characters, valid hostname) |
| `listenerIsolation` | boolean | No | Create dedicated listener (default: `false`) |
| `listener` | IsolationListenerBody | Required if `listenerIsolation=true` | Isolated listener configuration |
| `targetListeners` | array[string] | No | Target listener names for shared mode (mutually exclusive with `listenerIsolation`) |
| `tls` | object | No | TLS configuration |
| `routes` | array[RouteBody] | Yes | Route definitions (1-50 routes) |

##### IsolationListenerBody

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | No | Listener name (auto-generated if omitted) |
| `bindAddress` | string | Yes | Bind address (e.g., `0.0.0.0`) |
| `port` | integer | Yes | Port number (1-65535) |
| `protocol` | string | No | Protocol: `HTTP` or `HTTPS` (default: `HTTP`) |

##### RouteBody

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `match` | RouteMatchBody | Yes | Route match criteria |
| `cluster` | RouteClusterBody | Yes | Upstream cluster configuration |
| `timeoutSeconds` | integer | No | Request timeout (1-3600 seconds) |
| `rewrite` | RouteRewriteBody | No | Path rewrite configuration |
| `filters` | object | No | Filter overrides (see [Filter Configuration](#filter-configuration)) |

##### RouteMatchBody

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `prefix` | string | Conditional | Path prefix match (mutually exclusive with `path`) |
| `path` | string | Conditional | Exact path match (mutually exclusive with `prefix`) |

At least one of `prefix` or `path` must be provided.

##### RouteClusterBody

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Cluster name (max 100 characters) |
| `endpoint` | string | Yes | Upstream endpoint in `host:port` format (max 255 characters) |

##### RouteRewriteBody

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `prefix` | string | No | Replace path prefix |
| `regex` | string | Conditional | Regex pattern (must be provided with `substitution`) |
| `substitution` | string | Conditional | Regex substitution (must be provided with `regex`) |

#### Response

**Status**: `201 Created`

**Body**: Creation response with generated IDs

```json
{
  "id": "api-def-abc123",
  "bootstrapUri": "/api/v1/api-definitions/api-def-abc123/bootstrap",
  "routes": ["route-xyz789", "route-uvw456"]
}
```

#### Validation Rules

1. **Team & Domain**
   - Team and domain cannot be empty and must not exceed max length
   - Domain must be a valid hostname (alphanumeric, `.`, `-`)

2. **Routes**
   - At least 1 route required, maximum 50 routes
   - Each route must have valid match criteria
   - Cluster endpoints must be in `host:port` format with valid port (1-65535)

3. **Listener Isolation**
   - When `listenerIsolation=true`, `listener` field is required
   - `targetListeners` and `listenerIsolation` are mutually exclusive
   - `targetListeners` cannot be an empty array

4. **Path Rewrites**
   - All paths must start with `/`
   - Paths cannot contain consecutive `/` characters
   - `regex` and `substitution` must be provided together

#### Error Responses

| Status | Description |
|--------|-------------|
| `400` | Validation error (see response body for details) |
| `409` | Domain already registered or route collision |
| `503` | API definition repository not configured |

#### Example

```bash
curl -X POST "http://localhost:8080/api/v1/api-definitions" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "payments",
    "domain": "payments.example.com",
    "listenerIsolation": false,
    "routes": [{
      "match": {"prefix": "/api/v1/payments"},
      "cluster": {
        "name": "payments-backend",
        "endpoint": "payments.svc.cluster.local:8080"
      },
      "timeoutSeconds": 30
    }]
  }'
```

---

### 5. Update API Definition

```http
PATCH /api/v1/api-definitions/{id}
```

Update an existing API definition. The version number is incremented and xDS caches are refreshed automatically.

**Note**: When `routes` are provided, existing routes are deleted and replaced atomically (cascade update), triggering cleanup of orphaned native resources.

#### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | string | API definition ID to update |

#### Request Body

**Content-Type**: `application/json`

```json
{
  "domain": "updated-payments.example.com",
  "targetListeners": ["listener-1", "listener-2"],
  "routes": [
    {
      "match": {"prefix": "/api/v2/payments"},
      "cluster": {
        "name": "payments-backend-v2",
        "endpoint": "payments-v2.svc.cluster.local:8080"
      },
      "timeoutSeconds": 45
    }
  ]
}
```

#### Request Schema

##### UpdateApiDefinitionBody

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `domain` | string | No | Updated domain name (max 253 characters, valid hostname) |
| `targetListeners` | array[string] | No | Updated target listener names (cannot be empty if provided) |
| `tls` | object | No | Updated TLS configuration |
| `routes` | array[RouteBody] | No | Complete route replacement (1-50 routes, triggers cascade update) |

**At least one field must be provided for update.**

#### Response

**Status**: `200 OK`

**Body**: Updated API definition summary

```json
{
  "id": "api-def-abc123",
  "team": "payments",
  "domain": "updated-payments.example.com",
  "listenerIsolation": false,
  "bootstrapUri": "/api/v1/api-definitions/api-def-abc123/bootstrap",
  "version": 2,
  "createdAt": "2025-10-06T09:00:00Z",
  "updatedAt": "2025-10-06T10:30:00Z"
}
```

#### Cascade Update Behavior

When `routes` are provided in the update request:

1. **Existing routes are deleted** - All current routes for the API definition are removed
2. **New routes are created** - Routes from the request body are created with fresh IDs
3. **Native resources cleaned up** - Orphaned clusters, routes, and listeners are removed
4. **Version incremented** - The API definition version number increases
5. **xDS refresh triggered** - All connected Envoy proxies receive updated configuration

#### Validation Rules

1. **Domain Validation**
   - Must be a valid hostname if provided
   - Cannot be empty string

2. **Routes Validation**
   - Cannot be empty array if provided
   - Maximum 50 routes
   - Each route must pass validation (see RouteBody schema)

3. **Target Listeners**
   - Cannot be empty array if provided
   - Each listener name must be non-empty

4. **Update Requirements**
   - At least one field must be provided
   - Cannot send empty update request

#### Error Responses

| Status | Description |
|--------|-------------|
| `400` | Validation error (invalid domain, empty routes array, etc.) |
| `404` | API definition not found |
| `409` | Updated domain conflicts with another API definition |
| `500` | Internal error during update or xDS refresh |
| `503` | API definition repository not configured |

#### Example

```bash
# Update domain only
curl -X PATCH "http://localhost:8080/api/v1/api-definitions/api-def-abc123" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "domain": "new-payments.example.com"
  }'

# Replace all routes (cascade update)
curl -X PATCH "http://localhost:8080/api/v1/api-definitions/api-def-abc123" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "routes": [
      {
        "match": {"prefix": "/api/v2"},
        "cluster": {
          "name": "backend-v2",
          "endpoint": "backend-v2.svc:8080"
        }
      }
    ]
  }'
```

---

### 6. Append Route to API Definition

```http
POST /api/v1/api-definitions/{id}/routes
```

Append a new route to an existing API definition without affecting existing routes.

#### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | string | API definition ID |

#### Request Body

**Content-Type**: `application/json`

```json
{
  "route": {
    "match": {"prefix": "/api/v2/checkout"},
    "cluster": {
      "name": "checkout-service",
      "endpoint": "checkout.svc.cluster.local:8080"
    },
    "timeoutSeconds": 15,
    "filters": {
      "headerMutation": {
        "requestAdd": [
          {
            "header": {"key": "X-Service-Version"},
            "value": "v2"
          }
        ]
      }
    }
  },
  "deploymentNote": "Rolling out new checkout API v2"
}
```

#### Request Schema

##### AppendRouteBody

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `route` | RouteBody | Yes | Route configuration (see RouteBody schema) |
| `deploymentNote` | string | No | Deployment note (max 500 characters) |

#### Response

**Status**: `202 Accepted`

**Body**: Append response with created route ID

```json
{
  "apiId": "api-def-abc123",
  "routeId": "route-new999",
  "revision": 2,
  "bootstrapUri": "/api/v1/api-definitions/api-def-abc123/bootstrap"
}
```

#### Error Responses

| Status | Description |
|--------|-------------|
| `400` | Validation error in route payload |
| `404` | API definition not found |
| `409` | Route with same match pattern already exists |
| `503` | API definition repository not configured |

#### Example

```bash
curl -X POST "http://localhost:8080/api/v1/api-definitions/api-def-abc123/routes" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "route": {
      "match": {"prefix": "/api/v1/new-endpoint"},
      "cluster": {
        "name": "new-backend",
        "endpoint": "new.svc.cluster.local:8080"
      }
    },
    "deploymentNote": "Adding new endpoint"
  }'
```

---

### 7. Import OpenAPI Specification

```http
POST /api/v1/api-definitions/from-openapi
```

Create an API definition by importing an OpenAPI 3.0 specification with optional `x-flowplane` extensions for filter configuration.

#### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `team` | string | Yes | Team name for the API definition |
| `listenerIsolation` | boolean | No | Create dedicated listener (default: `false`) |
| `port` | integer | No | Port for isolated listener (only used when `listenerIsolation=true`) |

#### Request Body

**Content-Type**: `application/yaml`, `application/x-yaml`, or `application/json`

**Body**: OpenAPI 3.0 document (JSON or YAML)

```yaml
openapi: 3.0.0
info:
  title: Payments API
  version: 1.0.0
  x-flowplane-domain: payments.example.com
servers:
  - url: https://payments.example.com

x-flowplane-filters:
  - filter:
      type: cors
      policy:
        allow_origin:
          - type: exact
            value: "https://example.com"
        allow_methods: ["GET", "POST"]
        allow_credentials: true
  - filter:
      type: local_rate_limit
      stat_prefix: global_rl
      token_bucket:
        max_tokens: 1000
        tokens_per_fill: 1000
        fill_interval_ms: 60000

paths:
  /api/v1/payments:
    get:
      summary: List payments
      x-flowplane-upstream: payments-backend.svc.cluster.local:8080
      x-flowplane-route-overrides:
        rate_limit:
          stat_prefix: list_payments
          token_bucket:
            max_tokens: 100
            tokens_per_fill: 100
            fill_interval_ms: 60000
      responses:
        '200':
          description: Success
    post:
      summary: Create payment
      x-flowplane-upstream: payments-backend.svc.cluster.local:8080
      responses:
        '201':
          description: Created
```

#### OpenAPI Extensions

See [x-flowplane OpenAPI Extensions Guide](../examples/README-x-flowplane-extensions.md) for complete documentation of available extensions and filter configuration.

**Key Extensions:**

- `x-flowplane-domain`: API domain name (in `info` section)
- `x-flowplane-filters`: Global filters applied to all routes (root level)
- `x-flowplane-upstream`: Upstream endpoint for route (in operation)
- `x-flowplane-route-overrides`: Per-route filter overrides (in operation)

#### Response

**Status**: `201 Created`

**Body**: Creation response with generated IDs

```json
{
  "id": "api-def-xyz456",
  "bootstrapUri": "/api/v1/api-definitions/api-def-xyz456/bootstrap",
  "routes": ["route-aaa111", "route-bbb222"]
}
```

#### Validation Rules

1. **OpenAPI Specification**
   - Must be valid OpenAPI 3.0 format
   - Must include `info` section with `x-flowplane-domain`
   - Must have at least one path with an operation
   - Each operation must have `x-flowplane-upstream`

2. **Filter Configuration**
   - Global filters must use valid filter type names
   - Route overrides must use valid filter aliases
   - All filter configurations must pass validation

3. **Listener Isolation**
   - `listenerIsolation=true` requires `port` parameter
   - Generated listener will bind to specified port

#### Error Responses

| Status | Description |
|--------|-------------|
| `400` | Invalid OpenAPI spec, unsupported version, invalid x-flowplane extensions, or missing required fields |
| `409` | Domain already registered or route paths conflict |
| `500` | Internal error during OpenAPI parsing or materialization |
| `503` | API definition repository not configured |

#### Example

```bash
# Import YAML OpenAPI spec with listener isolation
curl -X POST "http://localhost:8080/api/v1/api-definitions/from-openapi?team=payments&listenerIsolation=true&port=10010" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/yaml" \
  --data-binary @openapi-spec.yaml

# Import JSON OpenAPI spec with shared listener
curl -X POST "http://localhost:8080/api/v1/api-definitions/from-openapi?team=payments" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  --data-binary @openapi-spec.json
```

---

## Filter Configuration

Routes can include filter overrides to customize behavior per endpoint. Filters are specified in the `filters` field of a RouteBody.

### Available Filters

The Platform API supports a subset of HTTP filters for route-level configuration:

- **CORS** - Cross-Origin Resource Sharing
- **JWT Authentication** - JSON Web Token validation
- **Header Mutation** - Request/response header manipulation
- **Custom Response** - Custom error responses
- **Local Rate Limit** - Token bucket rate limiting
- **Distributed Rate Limit** - Enterprise distributed rate limiting
- **Rate Limit Quota** - Quota-based rate limiting

For complete filter documentation including all configuration options, see:

- [Native API Filter Documentation](filters.md) - Complete filter reference
- [x-flowplane Extensions Guide](../examples/README-x-flowplane-extensions.md) - OpenAPI filter configuration

### Filter Syntax

```json
{
  "filters": {
    "cors": {
      "allowOrigin": ["https://example.com"],
      "allowMethods": ["GET", "POST"],
      "allowCredentials": true
    },
    "rateLimit": {
      "requestsPerUnit": 100,
      "unit": "minute"
    },
    "headerMutation": {
      "requestAdd": [
        {"header": {"key": "X-Custom"}, "value": "value"}
      ]
    }
  }
}
```

---

## Listener Isolation Modes

The Platform API supports two listener modes:

### Shared Listener Mode

**Default behavior** (`listenerIsolation: false`)

- Routes traffic through existing shared listeners
- Use `targetListeners` to specify which listeners to route through
- If `targetListeners` is omitted, uses `default-gateway-listener`
- Multiple API definitions can share the same listener
- Efficient for high-density deployments

**Example:**

```json
{
  "team": "payments",
  "domain": "payments.example.com",
  "listenerIsolation": false,
  "targetListeners": ["gateway-listener-1", "gateway-listener-2"],
  "routes": [...]
}
```

### Isolated Listener Mode

**Dedicated listener per API** (`listenerIsolation: true`)

- Creates a new listener specifically for this API definition
- Listener configuration must be provided in `listener` field
- Full control over listener port, bind address, and protocol
- Required when using global filters via `x-flowplane-filters`
- Ideal for tenant isolation or custom filter requirements

**Example:**

```json
{
  "team": "payments",
  "domain": "payments.example.com",
  "listenerIsolation": true,
  "listener": {
    "name": "payments-dedicated-listener",
    "bindAddress": "0.0.0.0",
    "port": 10010,
    "protocol": "HTTP"
  },
  "routes": [...]
}
```

---

## Common Workflows

### Workflow 1: Create API from JSON

1. **Define your API structure** in JSON with routes and clusters
2. **POST to `/api/v1/api-definitions`** to create the definition
3. **Download bootstrap** from the returned `bootstrapUri`
4. **Start Envoy** with the bootstrap configuration

```bash
# Step 1 & 2: Create API
curl -X POST "http://localhost:8080/api/v1/api-definitions" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d @api-definition.json > response.json

# Step 3: Get API ID and download bootstrap
API_ID=$(jq -r '.id' response.json)
curl "http://localhost:8080/api/v1/api-definitions/$API_ID/bootstrap?format=yaml" \
  -H "Authorization: Bearer $TOKEN" > bootstrap.yaml

# Step 4: Start Envoy
envoy -c bootstrap.yaml
```

### Workflow 2: Import from OpenAPI

1. **Prepare OpenAPI spec** with `x-flowplane` extensions
2. **POST to `/api/v1/api-definitions/from-openapi`** with query parameters
3. **Download bootstrap** and start Envoy

```bash
# Import with listener isolation
curl -X POST "http://localhost:8080/api/v1/api-definitions/from-openapi?team=payments&listenerIsolation=true&port=10010" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/yaml" \
  --data-binary @openapi-spec.yaml > response.json

# Download bootstrap and start
API_ID=$(jq -r '.id' response.json)
curl "http://localhost:8080/api/v1/api-definitions/$API_ID/bootstrap" \
  -H "Authorization: Bearer $TOKEN" > bootstrap.yaml
envoy -c bootstrap.yaml
```

### Workflow 3: Update API Routes

1. **PATCH `/api/v1/api-definitions/{id}`** with new routes array
2. **Existing routes are replaced** atomically
3. **xDS refreshes automatically** - connected Envoys receive updates

```bash
curl -X PATCH "http://localhost:8080/api/v1/api-definitions/$API_ID" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "routes": [
      {
        "match": {"prefix": "/api/v2"},
        "cluster": {
          "name": "backend-v2",
          "endpoint": "backend-v2.svc:8080"
        }
      }
    ]
  }'
```

### Workflow 4: Incremental Route Addition

1. **POST to `/api/v1/api-definitions/{id}/routes`** to append a route
2. **Existing routes remain unchanged**
3. **New route is activated** via xDS

```bash
curl -X POST "http://localhost:8080/api/v1/api-definitions/$API_ID/routes" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "route": {
      "match": {"prefix": "/api/v1/new"},
      "cluster": {
        "name": "new-service",
        "endpoint": "new.svc:8080"
      }
    },
    "deploymentNote": "Adding new service endpoint"
  }'
```

---

## Best Practices

### Team Organization

- Use consistent team naming across API definitions
- Filter API definitions by team using query parameters
- Leverage team-scoped bootstrap for multi-tenant deployments

### Domain Management

- Use fully qualified domain names (FQDNs)
- Ensure domains are unique across the platform
- Update domains using PATCH when migrating services

### Listener Strategy

- **Use shared listeners** for most APIs to reduce resource overhead
- **Use isolated listeners** when you need:
  - Global filters applied to all routes
  - Custom port bindings
  - Strict tenant isolation
  - Different protocol configurations

### Route Organization

- Keep route count under 50 per API definition
- Use prefix matching for broad namespaces
- Use exact path matching for specific endpoints
- Set appropriate timeouts based on backend behavior

### Filter Configuration

- Apply global filters via `x-flowplane-filters` in OpenAPI import
- Use route-level overrides sparingly for exceptions
- See [Native API Filter Documentation](filters.md) for detailed configuration options
- Reference [x-flowplane Extensions Guide](../examples/README-x-flowplane-extensions.md) for OpenAPI filter syntax

### Bootstrap Management

- Download bootstrap in YAML for readability
- Use JSON format for programmatic processing
- Leverage team scope for multi-tenant scenarios
- Version control your bootstrap configurations

---

## Troubleshooting

### API Definition Creation Fails

**Problem**: `400 Bad Request` during creation

**Solutions**:
- Verify all required fields are present
- Check domain format (must be valid hostname)
- Ensure at least one route is provided
- Validate cluster endpoints are in `host:port` format
- Check that `listenerIsolation` and `targetListeners` aren't both set

### OpenAPI Import Fails

**Problem**: `400 Bad Request` during OpenAPI import

**Solutions**:
- Ensure OpenAPI version is 3.0.x
- Verify `x-flowplane-domain` is set in `info` section
- Check all operations have `x-flowplane-upstream`
- Validate filter configurations match expected schemas
- Use correct filter type names in `x-flowplane-filters`
- Use correct filter aliases in `x-flowplane-route-overrides`

### Bootstrap Download Fails

**Problem**: `404 Not Found` when downloading bootstrap

**Solutions**:
- Verify API definition ID is correct
- Check that the API definition was created successfully
- Ensure you have `generate-envoy-config:read` scope

### Routes Not Updating

**Problem**: Changes to routes don't appear in Envoy

**Solutions**:
- Check API definition version incremented after update
- Verify Envoy is connected to control plane
- Review Envoy logs for xDS update errors
- Ensure xDS refresh completed successfully
- For isolated listeners, verify listener refresh occurred

### Domain Conflict

**Problem**: `409 Conflict` when creating/updating API definition

**Solutions**:
- Check if domain is already registered: `GET /api/v1/api-definitions?domain=example.com`
- Use a different domain or update the existing API definition
- Consider using subdomains for team isolation

---

## Related Documentation

- [Native API Documentation](api.md) - Direct cluster/route/listener management
- [Filter Reference](filters.md) - Complete filter configuration guide
- [x-flowplane Extensions Guide](../examples/README-x-flowplane-extensions.md) - OpenAPI filter syntax
- [Getting Started Guide](getting-started.md) - Quick start tutorial
- [Authentication Documentation](authentication.md) - Token management
- [Swagger UI](http://127.0.0.1:8080/swagger-ui) - Interactive API explorer

---

**API Version**: v1
**Last Updated**: 2025-10-13
**Minimum Flowplane Version**: v0.0.2
