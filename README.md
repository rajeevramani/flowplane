# Flowplane

![Version](https://img.shields.io/badge/version-0.0.11-blue)
![License](https://img.shields.io/badge/license-MIT-green)
![Rust](https://img.shields.io/badge/rust-2021_edition-orange)

## What is Flowplane?

Flowplane is a dynamic Envoy control plane that provides REST APIs for managing proxy configuration without writing raw protobuf. It translates high-level JSON resource definitions into Envoy's xDS protocol, enabling teams to configure clusters, routes, listeners, and filters through a standard API.

The platform addresses three challenges faced by teams operating Envoy proxies:

Configuration Complexity: Envoy requires protobuf-based xDS configuration that demands deep protocol knowledge. Flowplane exposes REST endpoints for clusters, routes, listeners, and filters, translating JSON payloads into the underlying xDS resources (LDS, RDS, CDS, EDS, SDS) that Envoy consumes via gRPC.

Undocumented APIs: Services in production often lack accurate schema documentation. Flowplane's learning sessions capture traffic samples through Envoy's Access Log Service and External Processor, then infer JSON schemas from observed request/response patterns—extracting type information without persisting actual
payload data.

Multi-tenant Isolation: Shared proxy infrastructure needs proper team boundaries. Flowplane scopes all resources to teams, enforces authorization through token-based access control with fine-grained scopes, and provides audit logging for security compliance.

The system provides three core capabilities: Configure (REST API and Web UI for managing xDS resources), Import (OpenAPI specifications materialized directly into routes and clusters), and Learn (traffic-based schema inference through learning sessions). These translate into Envoy configuration delivered via a Tonic-based gRPC xDS server supporting ADS, LDS, RDS, CDS, EDS, and SDS protocols.

Flowplane supports 13 HTTP filter types including JWT authentication, OAuth2, CORS, local and distributed rate limiting, header mutation, custom response handling, external authorization, RBAC, and health checks—all configurable through structured JSON rather than protobuf.

## Features

- **xDS Server** - gRPC-based configuration server for Envoy proxies (ADS, LDS, RDS, CDS, EDS, SDS)
- **REST API** - Management API for clusters, listeners, routes, filters, and secrets
- **Web UI** - SvelteKit dashboard for resource management and monitoring
- **Multi-tenant** - Team-based resource isolation with RBAC
- **HTTP Filters** - 15 filters including JWT Auth, OAuth2, Rate Limit, CORS, Header Mutation
- **API Learning** - Infer API schemas from traffic via ExtProc and Access Logs
- **Observability** - OpenTelemetry tracing, Prometheus metrics
- **Security** - OAuth2, JWT, mTLS with Vault PKI integration

## Requirements

- Rust (edition 2021)
- Node.js 18+ (for UI)
- SQLite (default) or PostgreSQL
- protoc (Protocol Buffers compiler)

## Quick Start

### Docker (Recommended)

```bash
docker run -d \
  --name flowplane \
  -p 8080:8080 \
  -p 50051:50051 \
  -v flowplane_data:/app/data \
  -e FLOWPLANE_DATABASE_URL=sqlite:///app/data/flowplane.db \
  ghcr.io/rajeevramani/flowplane:latest
```

#### Access Points

| Service    | URL                               |
|------------|-----------------------------------|
| API        | http://localhost:8080/api/v1/     |
| UI         | http://localhost:8080/            |
| Swagger UI | http://localhost:8080/swagger-ui/ |
| xDS (gRPC) | localhost:50051                   |

> **Ports are configurable**: `FLOWPLANE_API_PORT` (default: 8080), `FLOWPLANE_XDS_PORT` (default: 50051 in Docker, 18000 for local dev). See [Configuration](docs/configuration.md).


### Binary

Download from [GitHub Releases](https://github.com/flowplane-ai/flowplane/releases):

```bash
# Linux (x86_64)
curl -LO https://github.com/flowplane-ai/flowplane/releases/latest/download/flowplane-x86_64-unknown-linux-gnu.tar.gz
tar xzf flowplane-x86_64-unknown-linux-gnu.tar.gz

# macOS (Apple Silicon)
curl -LO https://github.com/flowplane-ai/flowplane/releases/latest/download/flowplane-aarch64-apple-darwin.tar.gz
tar xzf flowplane-aarch64-apple-darwin.tar.gz

# Run
./flowplane-*/flowplane
```

### Platform Setup

On first startup, a setup token appears in the logs. Use it to create your first API token:

```bash
# Initialize with setup token
curl --request POST \
  --url http://localhost:8080/api/v1/bootstrap/initialize \
  --header 'content-type: application/json' \
  --data '{
  "email": "admin@example.com",
  "password": "YOUR_PASSW0RD!",
  "name": "Admin User"
}'

# Login
curl --request POST \
  --url http://localhost:8080/api/v1/auth/login \
  --header 'content-type: application/json' \
  --data '{
  "email": "admin@example.com",
  "password": "YOUR_PASSW0RD!"
}'

# Token
curl --request POST \
  --url http://localhost:8080/api/v1/tokens \
  --header 'authorization: Bearer TOKEN_FROM_PREVIOUS_STEP' \
  --header 'content-type: application/json' \
  --header 'x-csrf-token: CSRF_TOKEN_FROM_PREVIOUS_STEP' \
  --data '{
  "name": "admin-api-token",
  "description": "Token with admin:all scope for testing",
  "scopes": [
    "admin:all"
  ]
}'

```

### Boot Envoy

```bash
# Generate Envoy bootstrap config
curl --request GET \
  --url 'http://localhost:8080/api/v1/teams/platform-admin/bootstrap' \
  --header 'authorization: Bearer API_TOKEN_FROM_PREVIOUS_STEP' \
  > envoy.yaml

# Run Envoy
envoy -c envoy.yaml
```

### Create the first API

```bash
# Create a cluster
curl --request POST \
  --url http://localhost:8080/api/v1/clusters \
  --header 'authorization: Bearer API_TOKEN_FROM_PREVIOUS_STEP' \
  --header 'content-type: application/json' \
  --data '{
  "team": "platform-admin",
  "name": "httpbin-cluster",
  "serviceName": "httpbin-service",
  "endpoints": [
    {
      "host": "httpbin.org",
      "port": 443
    }
  ],
  "connectTimeoutSeconds": 5,
  "useTls": true,
  "lbPolicy": "ROUND_ROBIN"
}'

# Create a route-config
curl --request POST \
  --url http://localhost:8080/api/v1/route-configs \
  --header 'authorization: Bearer API_TOKEN_FROM_PREVIOUS_STEP' \
  --header 'content-type: application/json' \
  --data '{
  "name": "httpbin-route",
  "team": "platform-admin",
  "virtualHosts": [
    {
      "name": "httpbin-vhost",
      "domains": ["*"],
      "routes": [
        {
          "name": "httpbin-route",
          "match": {
            "path": {
              "type": "prefix",
              "value": "/"
            }
          },
          "action": {
            "type": "forward",
            "cluster": "httpbin-cluster",
            "timeoutSeconds": 30
          }
        }
      ]
    }
  ]
}'

# Create a listener
curl --request POST \
  --url http://localhost:8080/api/v1/listeners \
  --header 'authorization: Bearer API_TOKEN_FROM_PREVIOUS_STEP' \
  --header 'content-type: application/json' \
  --data '{
  "name": "httpbin-listener",
  "address": "0.0.0.0",
  "port": 10000,
  "team": "platform-admin",
  "protocol": "HTTP",
  "filterChains": [
    {
      "name": "default",
      "filters": [
        {
          "name": "envoy.filters.network.http_connection_manager",
          "type": "httpConnectionManager",
          "routeConfigName": "httpbin-route",
          "httpFilters": [
            {
              "filter": {
                "type": "router"
              }
            }
          ]
        }
      ]
    }
  ]
}'

# Test the proxy
curl http://localhost:10000/get
```

## Architecture

```mermaid
graph TD
    Client[DevOps / Developer] -->|REST / UI| API[API Server :8080]
    API -->|Persist| DB[(SQLite/PostgreSQL)]
    DB -.->|Query| XDS[xDS Server :50051]

    subgraph Control Plane
        API
        XDS
        ALS[Access Log Service]
        ExtProc[External Processor]
    end

    Envoy[Envoy Proxy] -->|gRPC xDS| XDS
    Envoy -->|Access Logs| ALS
    Envoy -->|Request/Response| ExtProc
    ALS -->|Schema Inference| DB
    ExtProc -->|Body Capture| DB

    Envoy -->|Traffic| Upstream[Upstream Services]
```

## Documentation

- [Getting Started](docs/getting-started.md)
- [CLI](docs/cli.md)
- [Configuration](docs/configuration.md)
- [Operations](docs/operations.md)
- [HTTP Filters](docs/filters.md)
- [TLS](docs/tls.md)
- [Secrets Management (SDS)](docs/secrets-sds.md)

## Acknowledgments

Flowplane's xDS implementation is built on [envoy-types](https://github.com/flemosr/envoy-types), a Rust crate providing pre-compiled protobuf types for the Envoy Proxy. This library enables type-safe gRPC communication with Envoy without requiring manual protobuf compilation.

## License

MIT
