# Flowplane

![Version](https://img.shields.io/static/v1?label=version&message=0.1.0&color=blue) ![Status](https://img.shields.io/static/v1?label=status&message=early%20alpha&color=orange) ![License](https://img.shields.io/badge/license-MIT-green) ![Rust](https://img.shields.io/badge/rust-2021_edition-orange)

![Flowplane Demo](docs/images/flowplane-demo.gif)

## What is Flowplane?

Flowplane is a dynamic Envoy control plane that provides REST APIs for managing proxy configuration without writing raw protobuf. It translates high-level JSON resource definitions into Envoy's xDS protocol, enabling teams to configure clusters, routes, listeners, and filters through a standard API. Flowplane is designed for agent-operated workflows—AI agents can deploy, diagnose, and manage gateway configuration end-to-end via MCP tools.

Resources are organized into **organizations** containing **teams**, providing multi-tenant isolation where each team manages its own gateway configuration independently.

The platform addresses three challenges faced by teams operating Envoy proxies:

Configuration Complexity: Envoy requires protobuf-based xDS configuration that demands deep protocol knowledge. Flowplane exposes REST endpoints for clusters, routes, listeners, and filters, translating JSON payloads into the underlying xDS resources (LDS, RDS, CDS, EDS, SDS) that Envoy consumes via gRPC.

Undocumented APIs: Services in production often lack accurate schema documentation. Flowplane's learning sessions capture traffic samples through Envoy's Access Log Service and External Processor, then infer JSON schemas from observed request/response patterns—extracting type information without persisting actual
payload data.

Multi-tenant Isolation: Shared proxy infrastructure needs proper team boundaries. Flowplane scopes all resources to teams, enforces authorization through token-based access control with fine-grained scopes, and provides audit logging for security compliance.

The system provides three core capabilities: Configure (REST API and Web UI for managing xDS resources), Import (OpenAPI specifications materialized directly into routes and clusters), and Learn (traffic-based schema inference through learning sessions). These translate into Envoy configuration delivered via a Tonic-based gRPC xDS server supporting ADS, LDS, RDS, CDS, EDS, and SDS protocols.

Flowplane supports 15 HTTP filter types including JWT authentication, OAuth2, CORS, local and distributed rate limiting, header mutation, custom response handling, external authorization, RBAC, and health checks—all configurable through structured JSON rather than protobuf.

## Features

- **xDS Server** - gRPC-based configuration server for Envoy proxies (ADS, LDS, RDS, CDS, EDS, SDS)
- **REST API** - Management API for clusters, listeners, routes, filters, and secrets
- **Web UI** - SvelteKit dashboard for resource management and monitoring
- **Multi-tenant** - Org → team hierarchy with RBAC and resource isolation
- **HTTP Filters** - 15 filters including JWT Auth, OAuth2, Rate Limit, CORS, Header Mutation
- **API Learning** - Infer API schemas from traffic via ExtProc and Access Logs
- **Observability** - OpenTelemetry tracing, Prometheus metrics
- **Security** - OAuth2, JWT, mTLS with Vault PKI integration
- **Organization Multitenancy** - Orgs contain teams; org admins manage team membership and scopes
- **Invite-only Registration** - Org admins invite users via tokens; no open signup
- **Governance Admin** - Platform admins restricted to org/user management; cannot access tenant resources
- **Dataplane Abstraction** - Envoy instances modeled as named dataplanes with gateway_host

### MCP & AI Agents

Flowplane exposes 60 MCP tools and 7 prompt templates, enabling AI agents to deploy, diagnose, and manage API gateway configuration without touching the UI.

**Agent Capabilities:**
- Deploy APIs end-to-end with pre-flight validation
- Diagnose routing issues by tracing request paths
- Validate gateway configuration and detect misconfigurations
- Generate OpenAPI specs from live traffic via learning sessions

**Quick Demo:**

```bash
# Start Flowplane with httpbin and Envoy, then seed demo data
make up HTTPBIN=1 ENVOY=1 && make seed

# Run the dev agent (requires an OpenAI-compatible LLM)
FLOWPLANE_URL=http://localhost:8080 \
FLOWPLANE_TEAM=engineering \
FLOWPLANE_TOKEN=<token from seed output> \
LLM_BASE_URL=<your LLM endpoint> \
LLM_API_KEY=<your LLM API key> \
LLM_MODEL=<model name> \
python agents/dev_agent.py "Expose httpbin at localhost:8000 on path / at port 10001"
```

> `make seed` prints the API token and team name. LLM can be any OpenAI-compatible endpoint — e.g. `http://localhost:11434/v1` for Ollama with `qwen3-coder-next:cloud`, or `https://api.openai.com/v1` with `gpt-4o`.

Tested with GPT-4o and Qwen 3 (via Ollama). See [agents/README.md](agents/README.md) for details and [MCP Integration](docs/mcp.md) for protocol documentation.

### Import OpenApi
![Import OpenAppi](docs/images/import.gif)

### API Learning
![Learning API shape through requests](docs/images/learning-session.gif)

## Requirements

- Rust (edition 2021)
- Node.js 18+ (for UI)
- PostgreSQL 15+
- protoc (Protocol Buffers compiler)

## Quick Start

### Docker Compose (Recommended)

The fastest way to a working setup:

```bash
git clone https://github.com/rajeevramani/flowplane.git
cd flowplane
make up HTTPBIN=1 ENVOY=1    # Start Flowplane + Envoy + httpbin
make seed                     # Bootstrap admin, org, team, import httpbin API
```

Seed creates an admin user, organization (`acme-corp`), team (`engineering`), imports the [httpbin OpenAPI spec](.local/openapi/httpbin.yaml) into routes and clusters, spins up a dataplane with a listener on port 10016, and prints API tokens. Test it:

```bash
curl http://localhost:10016/get  # Traffic flowing through Envoy → httpbin 🎉
```

Open the [Web UI](http://localhost:8080) to see what was created, or explore the [Swagger UI](http://localhost:8080/swagger-ui/) for the full API.

For more `make` options:

```bash
make up                        # Backend + UI only
make up HTTPBIN=1              # Add httpbin test service
make up-tracing                # With Jaeger tracing
make up-mtls                   # With mTLS (Vault)
make down                      # Stop all services
make help                      # All options
```

### Binary

Download from [GitHub Releases](https://github.com/rajeevramani/flowplane/releases):

```bash
# Linux (x86_64)
curl -LO https://github.com/rajeevramani/flowplane/releases/latest/download/flowplane-x86_64-unknown-linux-gnu.tar.gz
tar xzf flowplane-x86_64-unknown-linux-gnu.tar.gz

# macOS (Apple Silicon)
curl -LO https://github.com/rajeevramani/flowplane/releases/latest/download/flowplane-aarch64-apple-darwin.tar.gz
tar xzf flowplane-aarch64-apple-darwin.tar.gz

# Run
./flowplane-*/flowplane
```

### Manual Setup (Optional)

> If you used `make seed` above, skip this section — everything below is already configured. These steps explain what seed does under the hood, or let you customize the setup.

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

# Create an organization (platform admin only)
# This auto-creates a default team named "my-org-default"
curl --request POST \
  --url http://localhost:8080/api/v1/admin/organizations \
  --header 'authorization: Bearer TOKEN_FROM_INITIALIZE' \
  --header 'content-type: application/json' \
  --data '{"name": "my-org", "displayName": "My Organization"}'

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

### Create Dataplane and Boot Envoy

A **Dataplane** represents an Envoy proxy instance. Create one first, then generate bootstrap config.

```bash
# Create a dataplane
curl --request POST \
  --url http://localhost:8080/api/v1/teams/my-org-default/dataplanes \
  --header 'authorization: Bearer API_TOKEN_FROM_PREVIOUS_STEP' \
  --header 'content-type: application/json' \
  --data '{
  "team": "my-org-default",
  "name": "default",
  "gatewayHost": "127.0.0.1",
  "description": "Default dataplane for local development"
}'

# Generate Envoy bootstrap config from the dataplane
curl --request GET \
  --url 'http://localhost:8080/api/v1/teams/my-org-default/dataplanes/default/envoy-config' \
  --header 'authorization: Bearer API_TOKEN_FROM_PREVIOUS_STEP' \
  > envoy.yaml

# Run Envoy
envoy -c envoy.yaml
```

#### Config from UI
![Envoy Config](docs/images/export-envoy-config.gif)

### Create the first API

```bash
# Create a cluster
curl --request POST \
  --url http://localhost:8080/api/v1/clusters \
  --header 'authorization: Bearer API_TOKEN_FROM_PREVIOUS_STEP' \
  --header 'content-type: application/json' \
  --data '{
  "team": "my-org-default",
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
  "team": "my-org-default",
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

# Create a listener (associated with the dataplane)
curl --request POST \
  --url http://localhost:8080/api/v1/listeners \
  --header 'authorization: Bearer API_TOKEN_FROM_PREVIOUS_STEP' \
  --header 'content-type: application/json' \
  --data '{
  "name": "httpbin-listener",
  "address": "0.0.0.0",
  "port": 10000,
  "team": "my-org-default",
  "dataplaneId": "DATAPLANE_ID_FROM_CREATE_RESPONSE",
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

> **Tip:** Instead of manually creating clusters, routes, and listeners, import an OpenAPI spec to generate everything automatically. See [OpenAPI Import](docs/getting-started.md#openapi-import) or use `make seed` which demonstrates this with the httpbin spec.

## Architecture

```mermaid
graph TD
    User[User / CI/CD] -->|REST / UI| API[API Server]

    subgraph "Flowplane Control Plane"
        direction TB

        API

        subgraph "Core xDS Engine"
            XDS[xDS Server]
            State[xDS State Cache]
            SecretReg[Secret Backend Registry]
            FilterReg[Filter Registry]
        end

        subgraph "Learning Engine"
            ALS[Access Log Service]
            ExtProc[External Processor]
            Worker[Access Log Processor]
            SchemaAgg[Schema Aggregator]
            SessionMgr[Learning Session Service]
        end

        API -->|Read/Write| DB[(Database)]

        %% State Sync Flow
        State -->|Poll/Refresh| DB
        XDS -->|Read| State

        %% Secret Flow
        SecretReg -->|Fetch| ExtSecrets
        State -.->|Resolve| SecretReg

        %% Learning Flow
        ALS -->|Stream| SessionMgr
        ExtProc -->|Stream| SessionMgr
        SessionMgr -->|Batch| Worker
        Worker -->|Infer| SchemaAgg
        SchemaAgg -->|Persist| DB
    end

    subgraph "Data Plane"
        Envoy[Envoy Proxy]
    end

    %% External Dependencies
    ExtSecrets[External Secrets\n Vault / AWS / GCP]
    PKI[Vault PKI]

    %% Flows
    Envoy -->|gRPC ADS/xDS| XDS
    Envoy -->|gRPC ALS| ALS
    Envoy -->|gRPC ExtProc| ExtProc

    %% Security
    XDS -.->|mTLS Auth| PKI
    Envoy -.->|mTLS Identity| PKI

    %% Traffic
    Envoy -->|Traffic| Upstream[Upstream Services]
```

## Documentation

### Getting Started
- [Getting Started](docs/getting-started.md)
- [Configuration](docs/configuration.md)
- [CLI](docs/cli.md)

### Features
- [HTTP Filters](docs/filters.md)
- [Custom WASM Filters](docs/wasm-filters.md)
- [TLS](docs/tls.md)
- [Secrets Management (SDS)](docs/secrets-sds.md)
- [Learning Gateway](docs/learning-manual/README.md)
- [MCP Integration](docs/mcp.md)
- [AI Agents](agents/README.md)

### Deployment
- [Kubernetes](docs/deployment/kubernetes.md)
- [Multi-Region](docs/deployment/multi-region.md)
- [Multi-Dataplane](docs/deployment/multi-dataplane.md)

### Architecture
- [Architecture Overview](docs/architecture.md)
- [Architecture Decision Records](docs/adr/README.md)

### Contributing
- [Contributing](CONTRIBUTING.md)

## Acknowledgments

Flowplane's xDS implementation is built on [envoy-types](https://github.com/flemosr/envoy-types), a Rust crate providing pre-compiled protobuf types for the Envoy Proxy. This library enables type-safe gRPC communication with Envoy without requiring manual protobuf compilation.

## License

MIT
