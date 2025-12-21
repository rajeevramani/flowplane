# Getting Started

This guide walks you through setting up Flowplane and creating your first resources.

## Prerequisites

**For Docker deployment:**
- Docker and Docker Compose

**For binary releases:**
- No prerequisites (UI included)

**For building from source:**
- Rust (edition 2021) with cargo
- Node.js 18+ (for building UI)
- SQLite (included) or PostgreSQL
- protoc (Protocol Buffers compiler)

Optional:
- Envoy proxy (for testing xDS)

## Installation

### Option 1: Build from Source

```bash
git clone https://github.com/flowplane-ai/flowplane.git
cd flowplane

# Build UI (required for web dashboard)
cd ui && npm install && npm run build && cd ..

# Build release binary
cargo build --release

# Binary is at target/release/flowplane
```

The server automatically serves the UI from `./ui/build` when present. You can override this path with `FLOWPLANE_UI_DIR`.

### Option 2: Docker Compose

```bash
# Clone repository
git clone https://github.com/flowplane-ai/flowplane.git
cd flowplane

# Start with Docker Compose
docker-compose up -d
```

Services start at:
- **API & UI**: http://localhost:8080
- **Swagger UI**: http://localhost:8080/swagger-ui/
- **xDS**: localhost:50051 (gRPC)

## First Run

### 1. Start the Control Plane

```bash
# Create data directory for SQLite
mkdir -p data

# Run with default settings
cargo run --release
```

The server starts with:
- **Web UI**: http://127.0.0.1:8080
- **API Server**: http://127.0.0.1:8080/api/v1/
- **Swagger UI**: http://127.0.0.1:8080/swagger-ui/
- **xDS Server**: 0.0.0.0:18000 (gRPC)
- **Metrics**: http://0.0.0.0:9090/metrics (when enabled)

### 2. Bootstrap Authentication

On first startup with an empty database, Flowplane displays setup instructions. Initialize the system by creating your admin account:

```bash
# Initialize with your admin credentials
curl -X POST http://127.0.0.1:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{
    "email": "admin@example.com",
    "password": "YOUR_SECURE_PASSWORD",
    "name": "Admin User"
  }'

# Response includes a setup token (for optional session creation):
# {
#   "setupToken": "fp_setup_...",
#   "expiresAt": "...",
#   "message": "Bootstrap complete! Admin user 'Admin User' created successfully."
# }
```

Now login to get a session:

```bash
# Login with your credentials
curl -X POST http://127.0.0.1:8080/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{
    "email": "admin@example.com",
    "password": "YOUR_SECURE_PASSWORD"
  }'

# Response includes session cookie and CSRF token
```

Create an API token for programmatic access:

```bash
# Create an API token (use Authorization header and CSRF token from login response)
curl -X POST http://127.0.0.1:8080/api/v1/tokens \
  -H "Authorization: Bearer SESSION_TOKEN" \
  -H "X-CSRF-Token: CSRF_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "admin-api-token",
    "description": "Token with admin:all scope",
    "scopes": ["admin:all"]
  }'

# Save the returned token
export FLOWPLANE_TOKEN="fp_pat_..."
```

### 3. Verify API Access

```bash
# Check health
curl http://127.0.0.1:8080/health

# List clusters (should be empty)
curl -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/clusters
```

## Creating Resources

### Create a Cluster

Clusters define upstream backends:

```bash
curl -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "platform-admin",
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

### Create a Route Configuration

Route configs map requests to clusters:

```bash
curl -X POST http://127.0.0.1:8080/api/v1/route-configs \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "platform-admin",
    "name": "demo-routes",
    "virtualHosts": [
      {
        "name": "default",
        "domains": ["*"],
        "routes": [
          {
            "name": "to-httpbin",
            "match": {
              "path": {"type": "prefix", "value": "/"}
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

### Create a Listener

Listeners bind to ports and reference route configs:

```bash
curl -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "platform-admin",
    "name": "demo-listener",
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
            "routeConfigName": "demo-routes",
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

## Connecting Envoy

### Generate Bootstrap Configuration

```bash
# Get bootstrap config for your team
curl -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/teams/platform-admin/bootstrap > envoy-bootstrap.yaml
```

### Start Envoy

```bash
# Using func-e
func-e run -c envoy-bootstrap.yaml

# Or using envoy directly
envoy -c envoy-bootstrap.yaml
```

### Verify Traffic

```bash
# Send request through Envoy (port 10000)
curl -i http://127.0.0.1:10000/status/200
```

## Using the Web UI

The Web UI is integrated with the Flowplane server and served from the same port as the API. No separate UI process is required.

Access the dashboard at http://localhost:8080

### UI Features

- **Dashboard** - Overview of resources and quick actions
- **Clusters** - Manage upstream backends with health checks, circuit breakers
- **Listeners** - Configure ports and TLS
- **Route Configs** - Define routing rules and virtual hosts
- **Filters** - Install and configure HTTP filters
- **Secrets** - Manage TLS certificates and credentials
- **Tokens** - Create and rotate API tokens
- **Stats** - Real-time Envoy metrics (when enabled)

## OpenAPI Import

Import an OpenAPI specification to auto-generate resources:

```bash
curl -X POST "http://127.0.0.1:8080/api/v1/openapi/import?team=platform-admin" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d @your-openapi-spec.json
```

This creates clusters, routes, and optionally a listener from your API spec. The `platform-admin` team is created during bootstrap.

## Environment Configuration

Create a `.env` file for custom settings:

```bash
# Copy example
cp .env.example .env

# Edit as needed
# Key variables:
# FLOWPLANE_DATABASE_URL=sqlite://./data/flowplane.db
# FLOWPLANE_API_PORT=8080
# FLOWPLANE_XDS_PORT=18000
```

See the [README](../README.md) for the full list of environment variables.

## Next Steps

- [API Reference](api.md) - Complete API documentation
- [Authentication](authentication.md) - Token management and RBAC
- [HTTP Filters](filters.md) - Configure rate limiting, JWT, OAuth2, CORS
- [Architecture](architecture.md) - System design overview
- [Operations](operations.md) - Deployment and monitoring

### Cookbooks

- [Cluster Cookbook](cluster-cookbook.md) - Health checks, circuit breakers, TLS
- [Routing Cookbook](routing-cookbook.md) - Weighted routes, redirects, rewrites
- [Listener Cookbook](listener-cookbook.md) - TLS termination, filter chains
