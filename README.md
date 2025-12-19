# Flowplane

![Version](https://img.shields.io/badge/version-0.0.10-blue)
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

```
docker run -d \
--name flowplane \
-p 8080:8080 \
-p 50051:50051 \
-v flowplane_data:/app/data \
-e FLOW_PLANE_DATABASE_URL=sqlite:///app/data/flowplane.db \
ghcr.io/rajeevramani/flowplane:latest
```

#### Access Points

  | Service    | URL                               |
  |------------|-----------------------------------|
  | API        | http://localhost:8080/api/v1/     |
  | UI | http://localhost:8080/                    |
  | Swagger UI | http://localhost:8080/swagger-ui/ |
  | xDS (gRPC) | localhost:50051                   |


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

### First Steps

On first startup, a setup token appears in the logs. Use it to create your first API token:

```bash
# Initialize with setup token
curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{"setupToken": "<token-from-logs>", "teamName": "default"}'

# Create a cluster
curl -X POST http://localhost:8080/api/v1/clusters?team=default \
  -H "Authorization: Bearer <your-token>" \
  -H "Content-Type: application/json" \
  -d '{"name": "my-service", "endpoints": [{"address": "127.0.0.1", "port": 3000}]}'

# Connect Envoy
curl http://localhost:8080/api/v1/teams/default/bootstrap > envoy.yaml
envoy -c envoy.yaml
```

## Environment Variables

All environment variables use the `FLOWPLANE_` prefix.

### Core Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `FLOWPLANE_DATABASE_URL` | `sqlite://./data/flowplane.db` | Database connection URL |
| `FLOWPLANE_API_PORT` | `8080` | REST API port |
| `FLOWPLANE_API_BIND_ADDRESS` | `127.0.0.1` | REST API bind address |
| `FLOWPLANE_XDS_PORT` | `18000` | xDS gRPC port |
| `FLOWPLANE_XDS_BIND_ADDRESS` | `0.0.0.0` | xDS bind address |
| `FLOWPLANE_UI_ORIGIN` | `http://localhost:3000` | CORS allowed origin for UI |

### Database

| Variable | Default | Description |
|----------|---------|-------------|
| `FLOWPLANE_DATABASE_MAX_CONNECTIONS` | `10` | Max connection pool size |
| `FLOWPLANE_DATABASE_MIN_CONNECTIONS` | `0` | Min connection pool size |
| `FLOWPLANE_DATABASE_CONNECT_TIMEOUT_SECONDS` | `10` | Connection timeout |
| `FLOWPLANE_DATABASE_IDLE_TIMEOUT_SECONDS` | `600` | Idle connection timeout |
| `FLOWPLANE_DATABASE_AUTO_MIGRATE` | `true` | Run migrations on startup |

### Observability

| Variable | Default | Description |
|----------|---------|-------------|
| `FLOWPLANE_ENABLE_METRICS` | `true` | Enable Prometheus metrics |
| `FLOWPLANE_METRICS_PORT` | `9090` | Metrics endpoint port |
| `FLOWPLANE_ENABLE_TRACING` | `true` | Enable distributed tracing |
| `FLOWPLANE_OTLP_ENDPOINT` | `http://localhost:4317` | OpenTelemetry collector endpoint |
| `FLOWPLANE_TRACE_SAMPLING_RATIO` | `1.0` | Trace sampling ratio (0.0-1.0) |
| `FLOWPLANE_SERVICE_NAME` | `flowplane` | Service name for traces |
| `FLOWPLANE_LOG_LEVEL` | `info` | Log level (trace, debug, info, warn, error) |
| `FLOWPLANE_JSON_LOGGING` | `false` | JSON structured logging |

### xDS TLS (Optional)

| Variable | Description |
|----------|-------------|
| `FLOWPLANE_XDS_TLS_CERT_PATH` | Server certificate path |
| `FLOWPLANE_XDS_TLS_KEY_PATH` | Server private key path |
| `FLOWPLANE_XDS_TLS_CLIENT_CA_PATH` | Client CA for mTLS verification |
| `FLOWPLANE_XDS_TLS_REQUIRE_CLIENT_CERT` | Require client certificates (default: true if TLS enabled) |

### API TLS (Optional)

| Variable | Description |
|----------|-------------|
| `FLOWPLANE_API_TLS_ENABLED` | Enable HTTPS for API |
| `FLOWPLANE_API_TLS_CERT_PATH` | API server certificate |
| `FLOWPLANE_API_TLS_KEY_PATH` | API server private key |
| `FLOWPLANE_API_TLS_CHAIN_PATH` | Certificate chain (optional) |

### Secrets (Vault Integration)

| Variable | Description |
|----------|-------------|
| `FLOWPLANE_SECRET_BOOTSTRAP_TOKEN` | Initial setup token |
| `VAULT_ADDR` | Vault server address |
| `VAULT_TOKEN` | Vault authentication token |
| `VAULT_NAMESPACE` | Vault namespace |
| `FLOWPLANE_VAULT_PKI_MOUNT_PATH` | Vault PKI mount for mTLS |
| `FLOWPLANE_VAULT_PKI_ROLE_NAME` | Vault PKI role |
| `FLOWPLANE_VAULT_PKI_TRUST_DOMAIN` | SPIFFE trust domain |

## Architecture

```mermaid
graph TD
    Client[DevOps / Developer] -->|REST / UI| API[API Server :8080]
    API -->|Persist| DB[(SQLite/PostgreSQL)]
    DB -.->|Query| XDS[xDS Server :18000]

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

## Docker

```bash
# Build and run with Docker Compose
docker-compose up -d

# Default ports:
# - API: 8080
# - xDS: 50051 (override via docker-compose.yml)
```

## API Overview

### Authentication
- `POST /api/v1/auth/login` - Login
- `POST /api/v1/auth/sessions` - Create session
- `GET /api/v1/tokens` - List tokens
- `POST /api/v1/tokens` - Create token
- `POST /api/v1/tokens/{id}/rotate` - Rotate token

### Resources
- `/api/v1/clusters` - Cluster management
- `/api/v1/listeners` - Listener management
- `/api/v1/route-configs` - Route configuration
- `/api/v1/filters` - HTTP filter management
- `/api/v1/teams/{team}/secrets` - Secret management

### Operations
- `GET /health` - Health check
- `POST /api/v1/openapi/import` - Import OpenAPI spec
- `GET /api/v1/learning-sessions` - API learning sessions
- `GET /api/v1/audit-logs` - Audit logs

Full API documentation available at `/swagger-ui/` when running.

## Documentation

- [Getting Started](docs/getting-started.md)
- [API Reference](docs/api.md)
- [Authentication](docs/authentication.md)
- [HTTP Filters](docs/filters.md)
- [Architecture](docs/architecture.md)
- [Operations](docs/operations.md)

## Acknowledgments

Flowplane's xDS implementation is built on [envoy-types](https://github.com/flemosr/envoy-types), a Rust crate providing pre-compiled protobuf types for the Envoy Proxy. This library enables type-safe gRPC communication with Envoy without requiring manual protobuf compilation.

## License

MIT
