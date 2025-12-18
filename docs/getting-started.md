# Getting Started

This guide walks you through setting up Flowplane and creating your first resources.

## Prerequisites

- Rust (edition 2021) with cargo
- Node.js 18+ (for UI)
- SQLite (included) or PostgreSQL
- protoc (Protocol Buffers compiler)

Optional:
- Docker and Docker Compose
- Envoy proxy (for testing xDS)

## Installation

### Option 1: Build from Source

```bash
git clone https://github.com/yourusername/flowplane.git
cd flowplane

# Build release binary
cargo build --release

# Binary is at target/release/flowplane
```

### Option 2: Docker Compose

```bash
# Clone repository
git clone https://github.com/yourusername/flowplane.git
cd flowplane

# Start with Docker Compose
docker-compose up -d

# Services start at:
# - API: http://localhost:8080
# - xDS: localhost:50051 (per docker-compose.yml)
```

## First Run

### 1. Start the Control Plane

```bash
# Create data directory for SQLite
mkdir -p data

# Run with default settings
cargo run --release
```

The server starts with:
- **API Server**: http://127.0.0.1:8080
- **xDS Server**: 0.0.0.0:18000 (gRPC)
- **Metrics**: http://0.0.0.0:9090/metrics

### 2. Bootstrap Authentication

On first startup with an empty database, Flowplane generates a setup token displayed in the logs:

```
============================================================
FLOWPLANE SETUP TOKEN
============================================================
A setup token has been generated for initial bootstrap.

Setup Token: fp_setup_xxxxx-xxxxx-xxxxx.yyyyyyyyyyyy

Use this token to create your first admin token.
============================================================
```

Exchange the setup token for an admin token:

```bash
# Set your setup token
export SETUP_TOKEN="fp_setup_YOUR_TOKEN_HERE"

# Initialize bootstrap
curl -X POST http://127.0.0.1:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{
    "setupToken": "'$SETUP_TOKEN'",
    "adminTokenName": "admin-token",
    "adminTokenDescription": "Initial admin token"
  }'

# Response:
# {
#   "adminToken": "fp_pat_...",
#   "message": "Bootstrap completed successfully"
# }

# Save the admin token
export FLOWPLANE_TOKEN="fp_pat_..."
```

The setup token is revoked after successful bootstrap. Store your admin token securely.

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
  http://127.0.0.1:8080/api/v1/teams/default/bootstrap > envoy-bootstrap.yaml
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

### Start the UI

```bash
cd ui
npm install
npm run dev
```

Access the dashboard at http://localhost:5173

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
curl -X POST "http://127.0.0.1:8080/api/v1/openapi/import?team=default" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d @your-openapi-spec.json
```

This creates clusters, routes, and optionally a listener from your API spec.

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
