# Flowplane - Envoy Control Plane

Flowplane is a modern Envoy control plane that makes it simple to configure and manage Envoy proxies in non-Kubernetes environments. It provides structured JSON/YAML configuration models that translate automatically into Envoy protobufs, eliminating the need to hand-craft complex `Any` blobs.

**Key Features:**
- 🚀 Simple REST API for managing clusters, routes, and listeners
- 📝 OpenAPI 3.0 spec import - automatically generate gateway configuration
- 🔐 Built-in JWT authentication, rate limiting, and header manipulation
- 🎯 Team-based multi-tenancy with fine-grained access control
- 📊 Learning mode - capture real traffic to generate API schemas
- 🔄 Dynamic xDS updates - changes propagate instantly to Envoy
- 🛡️ Comprehensive security - TLS/mTLS support for both admin API and xDS

## Table of Contents

- [Quick Start](#quick-start)
- [Token Management and Scopes](#token-management-and-scopes)
- [Import an OpenAPI Specification](#import-an-openapi-specification)
- [Learning Gateway](#learning-gateway)
- [Cookbook](#cookbook)
  - [JWT Authentication](#jwt-authentication)
  - [Rate Limiting (Quota)](#rate-limiting-quota)
  - [Local Rate Limiting](#local-rate-limiting)
  - [Header Manipulation](#header-manipulation)
  - [Circuit Breaking](#circuit-breaking)
  - [External Processor (ext_proc)](#external-processor-ext_proc)
  - [Clusters](#clusters)
  - [Listeners](#listeners)
  - [Routes](#routes)
- [Examples](#examples)
  - [Docker Compose](#docker-compose)
  - [HTTPBin Example](#httpbin-example)
- [Documentation](#documentation)
- [Contributing](#contributing)

---

## Quick Start

Get Flowplane running with Envoy in under 5 minutes.

### Prerequisites

- Docker
- curl
- [Envoy proxy](https://www.envoyproxy.io/docs/envoy/latest/start/install) (optional - can use Docker)

### 1. Pull and Start the Control Plane

```bash
# Pull the latest Flowplane image
docker pull ghcr.io/rajeevramani/flowplane:latest

# Run the control plane
docker run -d \
  --name flowplane \
  -p 8080:8080 \
  -p 50051:50051 \
  -e DATABASE_URL=sqlite:///app/data/flowplane.db \
  -e FLOWPLANE_API_BIND_ADDRESS=0.0.0.0 \
  -e RUST_LOG=info \
  -v flowplane_data:/app/data \
  ghcr.io/rajeevramani/flowplane:latest
```

### 2. Bootstrap and Create Admin Token

Flowplane uses a secure three-step bootstrap process for initial setup:

#### Step 2a: Generate Setup Token

When the system has no active tokens, call the bootstrap endpoint to generate a one-time setup token:

```bash
# Bootstrap initialization - generate setup token
curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{
    "adminEmail": "admin@example.com"
  }'

# Response:
# {
#   "setupToken": "fp_setup_a1b2c3d4...",
#   "expiresAt": "2025-01-24T12:00:00Z",
#   "maxUsageCount": 1,
#   "message": "Setup token generated successfully...",
#   "nextSteps": [...]
# }

# Export the setup token
export SETUP_TOKEN="fp_setup_YOUR_TOKEN_HERE"
```

**Note:** The bootstrap endpoint only works when the system is uninitialized (no active tokens exist).

#### Step 2b: Create Session from Setup Token

Exchange the setup token for an authenticated session:

```bash
# Create session
curl -X POST http://localhost:8080/api/v1/auth/sessions \
  -H "Content-Type: application/json" \
  -d "{
    \"setupToken\": \"$SETUP_TOKEN\"
  }"

# Response includes session cookie and CSRF token:
# {
#   "sessionId": "...",
#   "csrfToken": "csrf-token-here",
#   "expiresAt": "2025-01-18T12:00:00Z",
#   "teams": ["admin"],
#   "scopes": ["admin:*"]
# }
# Headers: Set-Cookie: fp_session=fp_session_...; HttpOnly; Secure; SameSite=Strict

# Save the session cookie and CSRF token
export SESSION_COOKIE="fp_session_YOUR_SESSION_HERE"
export CSRF_TOKEN="YOUR_CSRF_TOKEN_HERE"
```

#### Step 2c: Create Admin Token from Session

Now use your session to create a Personal Access Token (PAT) for API automation:

```bash
# Create admin PAT
curl -X POST http://localhost:8080/api/v1/tokens \
  -H "Cookie: $SESSION_COOKIE" \
  -H "X-CSRF-Token: $CSRF_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "admin-token",
    "description": "Initial administrator token",
    "scopes": ["admin:*"],
    "expiresAt": "2026-12-31T23:59:59Z"
  }'

# Response:
# {
#   "id": "...",
#   "token": "fp_pat_...",
#   "name": "admin-token",
#   "scopes": ["admin:*"],
#   ...
# }

# Export the admin token from the response
export ADMIN_TOKEN="fp_pat_YOUR_ADMIN_TOKEN_HERE"
```

**Security Notes:**
- Setup tokens are **single-use** and automatically revoked after creating a session
- Sessions are **cookie-based** with CSRF protection for web applications
- Personal Access Tokens (PATs) are for **API automation** and long-lived access
- Store tokens securely (password manager, secrets vault)

**Quick Setup Script:**

```bash
# One-liner to bootstrap and get admin token
SETUP_RESP=$(curl -s -X POST http://localhost:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{"adminEmail":"admin@example.com"}')

SETUP_TOKEN=$(echo $SETUP_RESP | jq -r '.setupToken')

SESSION_RESP=$(curl -s -X POST http://localhost:8080/api/v1/auth/sessions \
  -H "Content-Type: application/json" \
  -c cookies.txt \
  -d "{\"setupToken\":\"$SETUP_TOKEN\"}")

CSRF_TOKEN=$(echo $SESSION_RESP | jq -r '.csrfToken')

TOKEN_RESP=$(curl -s -X POST http://localhost:8080/api/v1/tokens \
  -b cookies.txt \
  -H "X-CSRF-Token: $CSRF_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"name":"admin","scopes":["admin:*"]}')

export ADMIN_TOKEN=$(echo $TOKEN_RESP | jq -r '.token')
echo "Admin token: $ADMIN_TOKEN"
```

For complete session management documentation, see [docs/session-management.md](docs/session-management.md).

### 3. Create Resources Using Native API

Let's create a simple gateway manually using the Native API:

**Create a Cluster:**

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "httpbin-cluster",
    "serviceName": "httpbin",
    "endpoints": [
      { "host": "httpbin.org", "port": 443 }
    ],
    "connectTimeoutSeconds": 5,
    "useTls": true,
    "tlsServerName": "httpbin.org"
  }'
```

**Create a Route:**

```bash
curl -X POST http://localhost:8080/api/v1/routes \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "httpbin-routes",
    "virtualHosts": [
      {
        "name": "default",
        "domains": ["*"],
        "routes": [
          {
            "name": "catch-all",
            "match": {
              "path": { "type": "prefix", "value": "/" }
            },
            "action": {
              "type": "forward",
              "cluster": "httpbin-cluster",
              "timeoutSeconds": 10
            }
          }
        ]
      }
    ]
  }'
```

**Create a Listener:**

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "edge-listener",
    "address": "0.0.0.0",
    "port": 10000,
    "protocol": "HTTP",
    "filterChains": [
      {
        "name": "default",
        "filters": [
          {
            "name": "envoy.filters.network.http_connection_manager",
            "type": "httpConnectionManager",
            "routeConfigName": "httpbin-routes",
            "httpFilters": [
              {
                "name": "envoy.filters.http.router",
                "filter": { "type": "router" }
              }
            ]
          }
        ]
      }
    ]
  }'
```

### 4. Generate Bootstrap Configuration

Flowplane can generate an Envoy bootstrap configuration with all your resources:

```bash
# Get bootstrap config (using team-scoped endpoint)
curl -H "Authorization: Bearer $ADMIN_TOKEN" \
  "http://localhost:8080/api/v1/teams/admin/bootstrap" \
  > envoy-bootstrap.yaml

# Verify the configuration
cat envoy-bootstrap.yaml
```

### 5. Start Envoy

Launch Envoy with the generated bootstrap configuration:

```bash
# Using Envoy binary
envoy -c envoy-bootstrap.yaml

# OR using Docker
docker run -d \
  --name envoy-gateway \
  --network host \
  -v $(pwd)/envoy-bootstrap.yaml:/etc/envoy/envoy.yaml \
  envoyproxy/envoy:v1.31-latest \
  -c /etc/envoy/envoy.yaml
```

Envoy will:
1. Load the bootstrap configuration
2. Connect to Flowplane's xDS server (port 50051)
3. Start listening on port 10000

### 6. Test the Gateway

```bash
# Make a request through the gateway
curl http://localhost:10000/get -H "Host: httpbin.org"

# You should see a JSON response from httpbin with your request details
```

🎉 **Congratulations!** You now have a working Envoy gateway managed by Flowplane.

**Next Steps:**
- Import an OpenAPI spec (see below) for faster setup
- Configure JWT authentication, rate limiting, and more (see [Cookbook](#cookbook))
- Explore the interactive API docs at http://localhost:8080/swagger-ui

---

## Token Management and Scopes

Flowplane uses a hierarchical token system with three levels of scopes:

### Token Scope Hierarchy

1. **`admin:all`** - Full administrative access (bypasses all permission checks)
   - Can manage all resources across all teams
   - Can create and manage other tokens
   - Required for bootstrap and initial setup

2. **`{resource}:{action}`** - Resource-level permissions
   - Examples: `clusters:read`, `routes:write`, `listeners:delete`
   - Grants access to all resources of that type across all teams
   - Resources: `clusters`, `routes`, `listeners`, `api-definitions`, `tokens`
   - Actions: `read`, `write`, `delete`

3. **`team:{team_name}:{resource}:{action}`** - Team-scoped permissions
   - Examples: `team:demo:routes:read`, `team:acme:clusters:write`
   - Grants access only to resources within the specified team
   - Most granular and recommended for production use

### Creating Additional Tokens

After bootstrapping, you can create additional tokens with specific scopes:

#### Admin Token (Full Access)

```bash
curl -X POST http://localhost:8080/api/v1/tokens \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "second-admin",
    "description": "Another admin token",
    "scopes": ["admin:all"],
    "createdBy": "admin@example.com"
  }'
```

#### Team-Scoped Token (Recommended)

```bash
curl -X POST http://localhost:8080/api/v1/tokens \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "team-demo-token",
    "description": "Token for demo team resources",
    "scopes": [
      "team:demo:routes:read",
      "team:demo:routes:write",
      "team:demo:clusters:read",
      "team:demo:clusters:write",
      "team:demo:listeners:read",
      "team:demo:listeners:write"
    ],
    "createdBy": "admin@example.com",
    "expiresAt": "2025-12-31T23:59:59Z"
  }'
```

#### Multi-Team Token

A single token can have permissions across multiple teams:

```bash
curl -X POST http://localhost:8080/api/v1/tokens \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "multi-team-token",
    "scopes": [
      "team:demo:routes:read",
      "team:demo:routes:write",
      "team:production:routes:read",
      "team:staging:clusters:read"
    ]
  }'
```

### Token Operations

**List all tokens:**
```bash
curl -H "Authorization: Bearer $ADMIN_TOKEN" \
  http://localhost:8080/api/v1/tokens
```

**Rotate a token (generate new secret):**
```bash
curl -X POST -H "Authorization: Bearer $ADMIN_TOKEN" \
  http://localhost:8080/api/v1/tokens/{token_id}/rotate
```

**Revoke a token:**
```bash
curl -X DELETE -H "Authorization: Bearer $ADMIN_TOKEN" \
  http://localhost:8080/api/v1/tokens/{token_id}
```

For complete token management documentation, see [docs/token-management.md](docs/token-management.md).

---

## Import an OpenAPI Specification

The fastest way to create a complete gateway is to import an existing OpenAPI 3.0 specification. Flowplane automatically generates clusters, routes, and listeners from your spec.

### Basic Import

```bash
curl -X POST "http://localhost:8080/api/v1/api-definitions/from-openapi?team=demo&listenerIsolation=false" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/yaml" \
  --data-binary @examples/httpbin-basic.yaml
```

**Response:**
```json
{
  "id": "api_def_abc123",
  "team": "demo",
  "domain": "httpbin-org",
  "bootstrapUri": "/api/v1/teams/demo/bootstrap?scope=all",
  "routes": ["httpbin-org-get", "httpbin-org-post", ...],
  "listeners": ["default-gateway-listener"]
}
```

**Save the API ID:**
```bash
export API_ID="api_def_abc123"  # Use the actual ID from the response
```

### Listener Isolation Modes

Flowplane supports two listener isolation modes:

#### Shared Listener Mode (`listenerIsolation=false`)

Multiple APIs share a single listener on port 10000. This is the default and recommended for most use cases.

```bash
curl -X POST "http://localhost:8080/api/v1/api-definitions/from-openapi?team=demo&listenerIsolation=false" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/yaml" \
  --data-binary @your-api.yaml
```

**Benefits:**
- Simple port management (only port 10000)
- Multiple APIs can coexist
- Easier firewall configuration

#### Isolated Listener Mode (`listenerIsolation=true`)

Each API gets its own dedicated listener on a deterministic port (based on domain hash).

```bash
curl -X POST "http://localhost:8080/api/v1/api-definitions/from-openapi?team=demo&listenerIsolation=true" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/yaml" \
  --data-binary @your-api.yaml
```

**Benefits:**
- Complete traffic isolation per API
- Independent filter configurations
- Useful for multi-tenant scenarios

### Get Bootstrap Configuration

After importing, generate the Envoy bootstrap configuration:

```bash
# For team-scoped bootstrap (all APIs for a team)
curl -H "Authorization: Bearer $ADMIN_TOKEN" \
  "http://localhost:8080/api/v1/teams/demo/bootstrap" \
  > envoy-bootstrap.yaml
```

### Start Envoy (if not already running)

If Envoy is already running, it will automatically pick up the new configuration via xDS. If not:

```bash
docker run -d \
  --name envoy-gateway \
  --network host \
  -v $(pwd)/envoy-bootstrap.yaml:/etc/envoy/envoy.yaml \
  envoyproxy/envoy:v1.31-latest \
  -c /etc/envoy/envoy.yaml
```

### Make API Calls

```bash
# For shared listener mode (port 10000)
curl http://localhost:10000/get -H "Host: httpbin.org"
curl http://localhost:10000/headers -H "Host: httpbin.org"
curl -X POST http://localhost:10000/post -H "Host: httpbin.org" -d '{"test":"data"}'

# For isolated listener mode, find your port first:
PORT=$(curl -s -H "Authorization: Bearer $ADMIN_TOKEN" \
  "http://localhost:8080/api/v1/teams/demo/bootstrap" | \
  yq '.static_resources.listeners[] | select(.name | contains("platform")) | .address.socket_address.port_value')

curl http://localhost:${PORT}/get -H "Host: httpbin.org"
```

**Important:** Always include the `Host` header matching your OpenAPI spec's server URL domain.

### Adding Filters to OpenAPI Specs

You can add HTTP filters directly in your OpenAPI spec using `x-flowplane-filters` extensions:

**Example: Global Filters**
```yaml
openapi: 3.0.0
info:
  title: My API
  version: 1.0.0

servers:
  - url: https://api.example.com

# Global filters applied to all routes
x-flowplane-filters:
  - filter:
      type: header_mutation
      request_headers_to_add:
        - key: x-gateway
          value: "flowplane"
  - filter:
      type: local_rate_limit
      stat_prefix: global_rl
      token_bucket:
        max_tokens: 100
        tokens_per_fill: 100
        fill_interval_ms: 60000

paths:
  /api/users:
    get:
      summary: List users
      # Route-specific overrides
      x-flowplane-route-overrides:
        rate_limit:
          stat_prefix: users_rl
          token_bucket:
            max_tokens: 10
            tokens_per_fill: 10
            fill_interval_ms: 60000
```

See [examples/README-x-flowplane-extensions.md](examples/README-x-flowplane-extensions.md) for complete filter reference.

---

## Learning Gateway

Flowplane's Learning mode captures real traffic flowing through your gateway to automatically generate API schemas. This is useful for:

- **API Discovery** - Document undocumented APIs
- **Contract Testing** - Validate actual API behavior
- **Schema Generation** - Create OpenAPI specs from live traffic

### How Learning Mode Works

1. **Capture Traffic**: Envoy access logs are sent to Flowplane's External Processor
2. **Aggregate Schemas**: Request/response pairs are analyzed and aggregated
3. **Export OpenAPI**: Generate OpenAPI 3.1 specs from learned patterns

### Setting Up Learning Mode

#### 1. Create a Team Token with Learning Permissions

```bash
curl -X POST http://localhost:8080/api/v1/tokens \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "learning-token",
    "scopes": [
      "team:demo:clusters:read",
      "team:demo:clusters:write",
      "team:demo:routes:read",
      "team:demo:routes:write",
      "team:demo:listeners:read",
      "team:demo:listeners:write",
      "team:demo:learning-sessions:read",
      "team:demo:learning-sessions:write",
      "team:demo:aggregated-schemas:read"
    ],
    "expiresIn": 2592000
  }'

export LEARNING_TOKEN="<token from response>"
```

#### 2. Create Resources with External Processor Filter

**Create Cluster:**
```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $LEARNING_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "httpbin-cluster",
    "endpoints": [{"host": "httpbin.org", "port": 80}],
    "connectTimeoutSeconds": 5
  }'
```

**Create Route:**
```bash
curl -X POST http://localhost:8080/api/v1/routes \
  -H "Authorization: Bearer $LEARNING_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "users-route",
    "virtualHosts": [{
      "name": "default",
      "domains": ["*"],
      "routes": [{
        "name": "users",
        "match": {"path": {"type": "prefix", "value": "/users"}},
        "action": {
          "type": "forward",
          "cluster": "httpbin-cluster",
          "prefixRewrite": "/anything/users"
        }
      }]
    }]
  }'
```

**Create Listener with External Processor:**
```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $LEARNING_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "learning-listener",
    "address": "0.0.0.0",
    "port": 10001,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "users-route",
        "httpFilters": [
          {
            "filter": {
              "type": "ext_proc",
              "grpc_service": {
                "target_uri": "flowplane_ext_proc_service",
                "timeout_seconds": 5
              },
              "failure_mode_allow": true,
              "processing_mode": {
                "request_header_mode": "SEND",
                "response_header_mode": "SEND",
                "request_body_mode": "BUFFERED",
                "response_body_mode": "BUFFERED"
              }
            }
          },
          { "filter": { "type": "router" } }
        ]
      }]
    }]
  }'
```

#### 3. Start a Learning Session

```bash
curl -X POST http://localhost:8080/api/v1/learning-sessions \
  -H "Authorization: Bearer $LEARNING_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "routePattern": "^(/anything)?/users",
    "targetSampleCount": 5,
    "maxDurationSeconds": 300
  }'

# Save the session ID from response
export SESSION_ID="<session_id>"
```

#### 4. Send Traffic Through the Gateway

```bash
# Send sample requests (at least 5 to complete the session)
for i in {1..5}; do
  curl http://localhost:10001/users/$i
  sleep 1
done
```

#### 5. Check Learning Progress

```bash
curl -H "Authorization: Bearer $LEARNING_TOKEN" \
  "http://localhost:8080/api/v1/learning-sessions/$SESSION_ID"
```

The session status will show:
- `active` - Still collecting samples
- `completed` - Target sample count reached
- `expired` - Max duration exceeded

#### 6. View Learned Schemas

Once the session completes, schemas are automatically aggregated:

```bash
# List all aggregated schemas for your team
curl -H "Authorization: Bearer $LEARNING_TOKEN" \
  "http://localhost:8080/api/v1/aggregated-schemas"

# Filter by path
curl -H "Authorization: Bearer $LEARNING_TOKEN" \
  "http://localhost:8080/api/v1/aggregated-schemas?path=users"

# Get specific schema details
curl -H "Authorization: Bearer $LEARNING_TOKEN" \
  "http://localhost:8080/api/v1/aggregated-schemas/<schema_id>"
```

#### 7. Export as OpenAPI

```bash
# Export with Flowplane metadata
curl -H "Authorization: Bearer $LEARNING_TOKEN" \
  "http://localhost:8080/api/v1/aggregated-schemas/<schema_id>/export?include_metadata=true" \
  > learned-api.yaml

# Export clean OpenAPI (without metadata)
curl -H "Authorization: Bearer $LEARNING_TOKEN" \
  "http://localhost:8080/api/v1/aggregated-schemas/<schema_id>/export?include_metadata=false" \
  > clean-api.yaml
```

### Learning Session Best Practices

- **Sample Size**: Start with 5-10 samples, increase for complex APIs
- **Route Patterns**: Use regex patterns to match route rewrites
- **Session Duration**: Set appropriate `maxDurationSeconds` (300s = 5 min is typical)
- **Traffic Variety**: Send requests with different parameters, headers, and bodies
- **Team Isolation**: Schemas are team-scoped and never leak across teams

---

## Cookbook

Practical recipes for common gateway patterns.

### JWT Authentication

JWT authentication validates bearer tokens against JWKS providers.

#### Global JWT Authentication (All Routes)

Add the JWT filter to your listener to protect all routes:

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "secure-listener",
    "address": "0.0.0.0",
    "port": 10000,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "my-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.jwt_authn",
            "filter": {
              "type": "jwt_authn",
              "providers": {
                "auth0": {
                  "issuer": "https://your-tenant.auth0.com/",
                  "audiences": ["your-api-identifier"],
                  "jwks": {
                    "remote": {
                      "httpUri": {
                        "uri": "https://your-tenant.auth0.com/.well-known/jwks.json",
                        "cluster": "jwks-cluster",
                        "timeoutMs": 5000
                      },
                      "cacheDuration": "300s"
                    }
                  },
                  "payloadInMetadata": "jwt_payload"
                }
              },
              "rules": [
                {
                  "match": {"path": {"type": "prefix", "value": "/"}},
                  "requires": {
                    "type": "provider_name",
                    "providerName": "auth0"
                  }
                }
              ]
            }
          },
          { "filter": { "type": "router" } }
        ]
      }]
    }]
  }'
```

**Don't forget to create the JWKS cluster:**

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "jwks-cluster",
    "endpoints": [{"host": "your-tenant.auth0.com", "port": 443}],
    "useTls": true,
    "tlsServerName": "your-tenant.auth0.com",
    "connectTimeoutSeconds": 5
  }'
```

#### Route-Specific JWT Authentication

Require JWT only for specific routes using `typedPerFilterConfig`:

```bash
{
  "name": "api-routes",
  "virtualHosts": [{
    "name": "default",
    "domains": ["*"],
    "routes": [
      {
        "name": "public-health",
        "match": {"path": {"type": "exact", "value": "/health"}},
        "action": {"type": "forward", "cluster": "backend"},
        "typedPerFilterConfig": {
          "envoy.filters.http.jwt_authn": {
            "requirement_name": "allow_missing"
          }
        }
      },
      {
        "name": "protected-api",
        "match": {"path": {"type": "prefix", "value": "/api"}},
        "action": {"type": "forward", "cluster": "backend"},
        "typedPerFilterConfig": {
          "envoy.filters.http.jwt_authn": {
            "requirement_name": "auth0"
          }
        }
      }
    ]
  }]
}
```

#### Using OpenAPI Extensions

Add JWT authentication in your OpenAPI spec:

```yaml
x-flowplane-filters:
  - filter:
      type: jwt_authn
      providers:
        auth0:
          issuer: https://your-tenant.auth0.com/
          audiences: ["your-api-identifier"]
          jwks:
            remote:
              httpUri:
                uri: https://your-tenant.auth0.com/.well-known/jwks.json
                cluster: jwks-cluster
      rules:
        - match:
            path:
              type: prefix
              value: /api
          requires:
            type: provider_name
            providerName: auth0
```

---

### Rate Limiting (Quota)

Quota-based rate limiting uses an external Rate Limit Quota Service (RLQS) for sophisticated rate limiting across multiple Envoy instances.

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "quota-listener",
    "address": "0.0.0.0",
    "port": 10000,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "my-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.rate_limit_quota",
            "filter": {
              "type": "rate_limit_quota",
              "rlqs_server": {
                "cluster_name": "rlqs-cluster",
                "timeout_ms": 500
              },
              "bucket_id_builder": {
                "bucket_id_type": "client_id",
                "header_name": "x-client-id"
              }
            }
          },
          { "filter": { "type": "router" } }
        ]
      }]
    }]
  }'
```

**Create RLQS cluster:**

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "rlqs-cluster",
    "endpoints": [{"host": "rlqs-server.internal", "port": 8081}],
    "connectTimeoutSeconds": 5,
    "type": "STRICT_DNS"
  }'
```

---

### Local Rate Limiting

Local rate limiting uses Envoy's built-in token bucket algorithm (no external service required).

#### Global Rate Limit (Listener Level)

Apply a rate limit to all traffic through the listener:

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "rate-limited-listener",
    "address": "0.0.0.0",
    "port": 10000,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "my-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.local_ratelimit",
            "filter": {
              "type": "local_rate_limit",
              "stat_prefix": "global_rate_limit",
              "token_bucket": {
                "max_tokens": 100,
                "tokens_per_fill": 100,
                "fill_interval_ms": 1000
              },
              "status_code": 429
            }
          },
          { "filter": { "type": "router" } }
        ]
      }]
    }]
  }'
```

This allows 100 requests per second globally.

#### Per-Route Rate Limit

Apply different rate limits to specific routes:

```bash
{
  "name": "api-routes",
  "virtualHosts": [{
    "name": "default",
    "domains": ["*"],
    "routes": [
      {
        "name": "expensive-api",
        "match": {"path": {"type": "prefix", "value": "/api/search"}},
        "action": {"type": "forward", "cluster": "backend"},
        "typedPerFilterConfig": {
          "envoy.filters.http.local_ratelimit": {
            "stat_prefix": "search_rate_limit",
            "token_bucket": {
              "max_tokens": 10,
              "tokens_per_fill": 10,
              "fill_interval_ms": 1000
            },
            "status_code": 429
          }
        }
      },
      {
        "name": "normal-api",
        "match": {"path": {"type": "prefix", "value": "/api"}},
        "action": {"type": "forward", "cluster": "backend"}
      }
    ]
  }]
}
```

#### Using OpenAPI Extensions

```yaml
paths:
  /api/search:
    get:
      x-flowplane-route-overrides:
        rate_limit:
          stat_prefix: search_rl
          token_bucket:
            max_tokens: 10
            tokens_per_fill: 10
            fill_interval_ms: 1000
```

---

### Header Manipulation

Add, modify, or remove headers on requests and responses.

#### Add Headers to Request/Response

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "header-mutation-listener",
    "address": "0.0.0.0",
    "port": 10000,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "my-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.header_mutation",
            "filter": {
              "type": "header_mutation",
              "request_headers_to_add": [
                {"key": "x-gateway", "value": "flowplane", "append": false},
                {"key": "x-request-id", "value": "%REQ(x-request-id)%", "append": false}
              ],
              "request_headers_to_remove": ["x-internal-header"],
              "response_headers_to_add": [
                {"key": "x-served-by", "value": "flowplane-gateway", "append": false},
                {"key": "x-response-time", "value": "%RESPONSE_DURATION%", "append": false}
              ],
              "response_headers_to_remove": ["server", "x-powered-by"]
            }
          },
          { "filter": { "type": "router" } }
        ]
      }]
    }]
  }'
```

#### Per-Route Header Manipulation

Apply different header rules per route:

```bash
{
  "routes": [
    {
      "name": "api-v2",
      "match": {"path": {"type": "prefix", "value": "/api/v2"}},
      "action": {"type": "forward", "cluster": "backend-v2"},
      "typedPerFilterConfig": {
        "envoy.filters.http.header_mutation": {
          "request_headers_to_add": [
            {"key": "x-api-version", "value": "v2"}
          ]
        }
      }
    }
  ]
}
```

#### Using OpenAPI Extensions

```yaml
x-flowplane-filters:
  - filter:
      type: header_mutation
      request_headers_to_add:
        - key: x-gateway
          value: flowplane
      response_headers_to_add:
        - key: x-served-by
          value: flowplane-gateway
```

---

### Circuit Breaking

Protect upstream services from overload with circuit breakers and outlier detection.

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "protected-cluster",
    "endpoints": [
      {"host": "backend-1.internal", "port": 8080},
      {"host": "backend-2.internal", "port": 8080}
    ],
    "circuitBreakers": {
      "default": {
        "maxConnections": 1000,
        "maxPendingRequests": 100,
        "maxRequests": 1000,
        "maxRetries": 3
      }
    },
    "outlierDetection": {
      "consecutive5xx": 5,
      "intervalSeconds": 30,
      "baseEjectionTimeSeconds": 30,
      "maxEjectionPercent": 50,
      "enforcingSuccessRate": 100
    }
  }'
```

**Circuit Breaker Thresholds:**
- `maxConnections`: Maximum concurrent connections
- `maxPendingRequests`: Maximum pending requests (waiting for connection)
- `maxRequests`: Maximum active requests
- `maxRetries`: Maximum concurrent retry requests

**Outlier Detection:**
- `consecutive5xx`: Eject endpoint after N consecutive 5xx errors
- `intervalSeconds`: Analysis interval
- `baseEjectionTimeSeconds`: Minimum ejection duration
- `maxEjectionPercent`: Maximum % of endpoints that can be ejected

---

### External Processor (ext_proc)

Use external processors for custom request/response processing, including custom error responses.

#### Basic External Processor

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "ext-proc-listener",
    "address": "0.0.0.0",
    "port": 10000,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "my-routes",
        "httpFilters": [
          {
            "filter": {
              "type": "ext_proc",
              "grpc_service": {
                "target_uri": "ext-proc.internal:9002",
                "timeout_seconds": 5
              },
              "failure_mode_allow": true,
              "processing_mode": {
                "request_header_mode": "SEND",
                "response_header_mode": "SEND",
                "request_body_mode": "BUFFERED",
                "response_body_mode": "BUFFERED"
              }
            }
          },
          { "filter": { "type": "router" } }
        ]
      }]
    }]
  }'
```

#### Custom Response (Per Route)

Use external processor to generate custom responses for specific routes:

```yaml
paths:
  /api/custom:
    get:
      summary: Returns custom response
      x-flowplane-route-overrides:
        custom_response:
          status_code: 403
          body: |
            {
              "error": "Forbidden",
              "message": "You don't have permission to access this resource"
            }
          headers:
            - key: content-type
              value: application/json
            - key: x-custom-header
              value: custom-value
```

---

### Clusters

Clusters define upstream services and their configuration.

#### Static Cluster (HTTP)

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "basic-cluster",
    "serviceName": "backend",
    "endpoints": [
      {"host": "backend-1.local", "port": 8080},
      {"host": "backend-2.local", "port": 8080}
    ],
    "connectTimeoutSeconds": 5
  }'
```

#### HTTPS Cluster (with TLS)

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "tls-cluster",
    "endpoints": [{"host": "api.example.com", "port": 443}],
    "useTls": true,
    "tlsServerName": "api.example.com",
    "connectTimeoutSeconds": 5
  }'
```

#### DNS Discovery Cluster (STRICT_DNS)

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "dns-cluster",
    "serviceName": "backend.internal",
    "type": "STRICT_DNS",
    "dnsLookupFamily": "AUTO",
    "endpoints": [{"host": "backend.internal", "port": 8080}]
  }'
```

**Cluster Types:**
- `STATIC` (default): Fixed list of endpoints
- `STRICT_DNS`: Periodic DNS resolution
- `LOGICAL_DNS`: Single DNS resolution
- `EDS`: Endpoint Discovery Service

#### Health Checked Cluster

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "health-checked-cluster",
    "endpoints": [
      {"host": "backend-1", "port": 8080},
      {"host": "backend-2", "port": 8080}
    ],
    "healthChecks": [
      {
        "type": "http",
        "path": "/health",
        "intervalSeconds": 10,
        "timeoutSeconds": 3,
        "healthyThreshold": 2,
        "unhealthyThreshold": 3
      }
    ]
  }'
```

**Health Check Types:**
- `http`: HTTP health checks (GET request to path)
- `tcp`: TCP connection checks
- `grpc`: gRPC health checks

---

### Listeners

Listeners bind Envoy to ports and configure filter chains.

#### HTTP Listener (Minimal)

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "http-listener",
    "address": "0.0.0.0",
    "port": 10000,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "my-routes",
        "httpFilters": [
          {"filter": {"type": "router"}}
        ]
      }]
    }]
  }'
```

#### HTTPS Listener (TLS Termination)

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "https-listener",
    "address": "0.0.0.0",
    "port": 443,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "tls-chain",
      "tlsContext": {
        "certChainFile": "/etc/envoy/certs/server.crt",
        "privateKeyFile": "/etc/envoy/certs/server.key",
        "caCertFile": "/etc/envoy/certs/ca.crt",
        "requireClientCertificate": false
      },
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "secure-routes",
        "httpFilters": [
          {"filter": {"type": "router"}}
        ]
      }]
    }]
  }'
```

#### TCP Listener (Layer 4)

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "tcp-listener",
    "address": "0.0.0.0",
    "port": 9000,
    "protocol": "TCP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.tcp_proxy",
        "type": "tcpProxy",
        "cluster": "tcp-backend-cluster"
      }]
    }]
  }'
```

---

### Routes

Routes map incoming requests to upstream clusters.

#### Prefix Match Route

```bash
{
  "name": "api-routes",
  "virtualHosts": [{
    "name": "default",
    "domains": ["*"],
    "routes": [
      {
        "name": "api-prefix",
        "match": {
          "path": {"type": "prefix", "value": "/api"}
        },
        "action": {
          "type": "forward",
          "cluster": "backend-cluster",
          "timeoutSeconds": 30
        }
      }
    ]
  }]
}
```

#### Exact Path Match

```bash
{
  "routes": [
    {
      "name": "health-check",
      "match": {
        "path": {"type": "exact", "value": "/health"}
      },
      "action": {
        "type": "forward",
        "cluster": "backend-cluster",
        "timeoutSeconds": 5
      }
    }
  ]
}
```

#### Regex Match Route

```bash
{
  "routes": [
    {
      "name": "versioned-api",
      "match": {
        "path": {"type": "regex", "value": "^/api/v[0-9]+/"}
      },
      "action": {
        "type": "forward",
        "cluster": "backend-cluster"
      }
    }
  ]
}
```

#### Weighted Routing (Blue/Green)

```bash
{
  "routes": [
    {
      "name": "blue-green",
      "match": {
        "path": {"type": "prefix", "value": "/"}
      },
      "action": {
        "type": "weighted",
        "totalWeight": 100,
        "clusters": [
          {"name": "app-blue", "weight": 80},
          {"name": "app-green", "weight": 20}
        ]
      }
    }
  ]
}
```

#### Redirect Route

```bash
{
  "routes": [
    {
      "name": "redirect-to-https",
      "match": {
        "path": {"type": "prefix", "value": "/"}
      },
      "action": {
        "type": "redirect",
        "hostRedirect": "secure.example.com",
        "responseCode": 301
      }
    }
  ]
}
```

---

## Examples

### Docker Compose

Flowplane includes several Docker Compose configurations for different use cases.

#### Basic Setup (docker-compose.yml)

The basic setup runs just the Flowplane control plane:

```bash
cd /path/to/flowplane
docker-compose up -d
```

**What it includes:**
- Flowplane control plane (ports 8080, 50051)
- SQLite database (persisted in volume)
- Health checks

**Accessing the control plane:**
```bash
# API
curl http://localhost:8080/swagger-ui

# Bootstrap to get admin token (see Quick Start section for full instructions)
# Step 1: Generate setup token
curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{"adminEmail": "admin@example.com"}'

# Step 2: Create session from setup token
# Step 3: Create admin PAT from session
# (See "Bootstrap and Create Admin Token" section above for complete flow)
```

#### With Zipkin Tracing (docker-compose-zipkin.yml)

Includes distributed tracing with Zipkin:

```bash
docker-compose -f docker-compose-zipkin.yml up -d
```

**What it includes:**
- Flowplane control plane
- Zipkin server (port 9411)
- Automatic trace collection

**Accessing Zipkin:**
```bash
# Open Zipkin UI
open http://localhost:9411

# Traces appear automatically when traffic flows through Envoy
```

**Configuration:**
The control plane automatically sends traces when `FLOWPLANE_ENABLE_TRACING=true` is set.

#### With Monitoring (docker-compose-monitoring.yml)

Includes Prometheus and Grafana for metrics:

```bash
docker-compose -f docker-compose-monitoring.yml up -d
```

**What it includes:**
- Flowplane control plane
- Prometheus (port 9090)
- Grafana (port 3000)
- Pre-configured dashboards

**Accessing monitoring:**
```bash
# Prometheus
open http://localhost:9090

# Grafana (default credentials: admin/admin)
open http://localhost:3000

# Envoy metrics are scraped automatically
```

**Available Metrics:**
- Request rates and latencies
- xDS update statistics
- Control plane health
- Envoy resource counts

#### Stopping and Cleanup

```bash
# Stop services
docker-compose down

# Stop and remove volumes (WARNING: deletes database)
docker-compose down -v
```

---

### HTTPBin Example

The `examples/` directory contains working OpenAPI specifications using httpbin.org as the backend.

#### Available Example Files

| File | Description |
|------|-------------|
| `httpbin-basic.yaml` | Simple working example with global filters and rate limiting |
| `httpbin-full.yaml` | Complete feature demonstration with all filter types |
| `httpbin-with-filters.yaml` | Shows various filter combinations |
| `httpbin-custom-response.yaml` | Custom response examples using ext_proc |
| `httpbin-method-extraction.yaml` | Route-specific method handling patterns |

#### Using httpbin-basic.yaml

This is the recommended starting point. It demonstrates:
- Global header mutation filters
- Global rate limiting (100 req/min)
- Per-route rate limit overrides
- Multiple HTTP methods (GET, POST, PUT)

**Import the spec:**

```bash
cd examples

curl -X POST "http://localhost:8080/api/v1/api-definitions/from-openapi?team=demo&listenerIsolation=false" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/yaml" \
  --data-binary @httpbin-basic.yaml
```

**Expected response:**
```json
{
  "id": "api_def_xyz789",
  "team": "demo",
  "domain": "httpbin-org",
  "bootstrapUri": "/api/v1/teams/demo/bootstrap?scope=all",
  "routes": [
    "httpbin-org-get",
    "httpbin-org-headers",
    "httpbin-org-post",
    "httpbin-org-put",
    "httpbin-org-status-200",
    "httpbin-org-json",
    "httpbin-org-uuid"
  ],
  "listeners": ["default-gateway-listener"]
}
```

**Get bootstrap config:**

```bash
curl -H "Authorization: Bearer $ADMIN_TOKEN" \
  "http://localhost:8080/api/v1/teams/demo/bootstrap" \
  > envoy-bootstrap.yaml
```

**Start Envoy:**

```bash
docker run -d \
  --name envoy \
  --network host \
  -v $(pwd)/envoy-bootstrap.yaml:/etc/envoy/envoy.yaml \
  envoyproxy/envoy:v1.31-latest \
  -c /etc/envoy/envoy.yaml
```

**Test the endpoints:**

```bash
# Simple GET (with added headers)
curl http://localhost:10000/get -H "Host: httpbin.org"

# View headers added by gateway
curl http://localhost:10000/headers -H "Host: httpbin.org" | jq '.headers'
# Look for: x-gateway: flowplane

# POST with stricter rate limit (5/min)
curl -X POST http://localhost:10000/post \
  -H "Host: httpbin.org" \
  -H "Content-Type: application/json" \
  -d '{"test": "data"}'

# PUT with very strict rate limit (3/min)
curl -X PUT http://localhost:10000/put \
  -H "Host: httpbin.org" \
  -H "Content-Type: application/json" \
  -d '{"update": "data"}'

# Test rate limiting
for i in {1..10}; do
  echo "Request $i:"
  curl -s -X POST http://localhost:10000/post \
    -H "Host: httpbin.org" \
    -w "\nHTTP: %{http_code}\n" \
    -o /dev/null
done
# After 5 requests, you'll see HTTP: 429 (rate limited)
```

#### Testing Other Examples

**httpbin-full.yaml** - Complete feature set:

```bash
curl -X POST "http://localhost:8080/api/v1/api-definitions/from-openapi?team=demo&listenerIsolation=true" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/yaml" \
  --data-binary @httpbin-full.yaml

# Find the assigned port
curl -H "Authorization: Bearer $ADMIN_TOKEN" \
  "http://localhost:8080/api/v1/teams/demo/bootstrap" | \
  yq '.static_resources.listeners[] | select(.name | contains("platform")) | .address.socket_address.port_value'

# Test with the assigned port
curl http://localhost:<PORT>/get -H "Host: httpbin.org"
```

#### HTTPBin Example Features

These examples demonstrate:

**Global Filters:**
- ✅ Header mutation (add tracking headers)
- ✅ Rate limiting (global limits)
- ✅ CORS configuration

**Route-Level Overrides:**
- ✅ Per-route rate limits
- ✅ Per-route header manipulation
- ✅ Method-specific handling

**Rate Limiting Tiers:**
- GET endpoints: 100/min (global)
- POST endpoints: 5/min
- PUT endpoints: 3/min
- Status endpoints: 3/min

**Testing Tips:**
- Always include `Host: httpbin.org` header
- Use `-v` flag to see response headers
- Test rate limits with rapid requests
- Check Envoy stats at `localhost:9901/stats` (if admin interface enabled)

See [examples/HTTPBIN-TESTING.md](examples/HTTPBIN-TESTING.md) for comprehensive testing guide.

---

## Documentation

Comprehensive guides are available in the `docs/` directory:

### Core Documentation

- **[Getting Started](docs/getting-started.md)** - Step-by-step tutorial from zero to working gateway
- **[Platform API](docs/platform-api.md)** - Team-based multi-tenancy and OpenAPI import
- **[CLI Usage](docs/cli-usage.md)** - Command-line interface documentation
- **[Filters](docs/filters.md)** - Complete HTTP filter reference and configuration

### Authentication & Security

- **[Session Management](docs/session-management.md)** - Cookie-based authentication, CSRF protection, and logout
- **[Token Management](docs/token-management.md)** - Creating and managing Personal Access Tokens (PATs)
- **[Authentication Overview](docs/authentication.md)** - Token types and access control concepts
- **[Frontend Integration](docs/frontend-integration.md)** - Web application integration with React, Vue.js examples
- **[Security Best Practices](docs/security-best-practices.md)** - Production security guidelines and compliance

### Cookbooks

- **[Cluster Cookbook](docs/cluster-cookbook.md)** - TLS, health checks, circuit breakers, DNS discovery
- **[Routing Cookbook](docs/routing-cookbook.md)** - Route patterns, weighted routing, redirects
- **[Listener Cookbook](docs/listener-cookbook.md)** - HTTP/HTTPS/TCP listeners, global filters, TLS termination
- **[Gateway Recipes](docs/gateway-recipes.md)** - End-to-end gateway scenarios

### Advanced Topics

- **[TLS Configuration](docs/tls.md)** - Secure xDS and API endpoints with TLS/mTLS
- **[Architecture](docs/architecture.md)** - System design and module layout
- **[Configuration Model](docs/config-model.md)** - Schema reference for resources
- **[Testing](docs/testing.md)** - Test suite commands and validation

### API Reference

- **Swagger UI:** http://localhost:8080/swagger-ui
- **OpenAPI JSON:** http://localhost:8080/api-docs/openapi.json

---

## Contributing

We welcome contributions! Here's how to get started:

### Development Setup

```bash
# Clone the repository
git clone https://github.com/rajeevramani/flowplane.git
cd flowplane

# Build
cargo build

# Run tests
cargo test

# Run linting
cargo clippy

# Format code
cargo fmt

# Start development server
DATABASE_URL=sqlite://./data/flowplane.db \
BOOTSTRAP_TOKEN=$(openssl rand -base64 32) \
FLOWPLANE_API_BIND_ADDRESS=0.0.0.0 \
cargo run --bin flowplane
```

### Code Quality Standards

All code must pass:
- ✅ `cargo fmt` - Code formatting
- ✅ `cargo clippy` - Linting (zero warnings)
- ✅ `cargo test` - All tests passing

### Development Guidelines

See [CLAUDE.md](CLAUDE.md) for:
- Rust best practices (error handling, testing, async/await)
- Architectural patterns (DRY, code reuse, multi-tenancy)
- Database guidelines (repository pattern, team isolation)
- xDS integration patterns

See [docs/contributing.md](docs/contributing.md) for:
- Pull request process
- Code review checklist
- Testing requirements

### Reporting Issues

- 🐛 **Bug Reports:** https://github.com/rajeevramani/flowplane/issues
- 💡 **Feature Requests:** https://github.com/rajeevramani/flowplane/issues
- 📖 **Documentation:** https://github.com/rajeevramani/flowplane/tree/main/docs

---

## License

Flowplane is open source software. See LICENSE file for details.

---

## Support

- **Documentation:** https://github.com/rajeevramani/flowplane/tree/main/docs
- **Issues:** https://github.com/rajeevramani/flowplane/issues
- **Discussions:** https://github.com/rajeevramani/flowplane/discussions

---

**Built with ❤️ using Rust, Axum, Envoy-Types and Envoy**
