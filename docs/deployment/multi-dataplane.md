# Multi-Dataplane Deployment Guide

This guide covers deploying multiple Envoy dataplanes managed by a single Flowplane control plane.

## Overview

A **dataplane** represents an Envoy proxy instance (or cluster of instances) that receives configuration from Flowplane. Multi-dataplane deployments enable:

- **Environment isolation**: Separate dev/staging/production dataplanes
- **Team separation**: Different teams manage their own gateways
- **Regional deployment**: Dataplanes in different geographic regions
- **Specialized routing**: Different dataplanes for different traffic types

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    Flowplane Control Plane                      │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
│  │   REST API   │  │  xDS Server  │  │  MCP Server          │  │
│  │   Port 8080  │  │  Port 50051  │  │  (uses gateway_host) │  │
│  └──────────────┘  └──────┬───────┘  └──────────────────────┘  │
└──────────────────────────┼──────────────────────────────────────┘
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
        ▼                  ▼                  ▼
┌───────────────┐  ┌───────────────┐  ┌───────────────┐
│  Dataplane 1  │  │  Dataplane 2  │  │  Dataplane 3  │
│  (Production) │  │  (Staging)    │  │  (Development)│
│               │  │               │  │               │
│  gateway_host:│  │  gateway_host:│  │  gateway_host:│
│  10.0.1.100   │  │  10.0.2.100   │  │  localhost    │
└───────────────┘  └───────────────┘  └───────────────┘
```

## Dataplane Model

### Database Schema

Dataplanes are stored in the `dataplanes` table:

```sql
CREATE TABLE dataplanes (
    id TEXT PRIMARY KEY,
    team TEXT NOT NULL,           -- Team ownership
    name TEXT NOT NULL,           -- Human-readable name
    gateway_host TEXT,            -- IP/hostname for MCP tools
    description TEXT,
    created_at TIMESTAMP,
    updated_at TIMESTAMP,
    UNIQUE(team, name)
);
```

### Key Fields

| Field | Description |
|-------|-------------|
| `team` | Team that owns this dataplane (multi-tenancy) |
| `name` | Unique name within team (e.g., "production", "staging") |
| `gateway_host` | IP address or hostname where Envoy is reachable |
| `description` | Human-readable description |

### The `gateway_host` Field

The `gateway_host` is critical for MCP tool execution:

- **MCP tools** need to know where to send HTTP requests
- When an AI assistant calls an API tool, Flowplane sends the request to `gateway_host`
- Without `gateway_host`, MCP gateway tools will fail with a configuration error

## Creating Dataplanes

### Via REST API

```bash
# Create production dataplane
curl -X POST http://localhost:8080/api/v1/teams/platform/dataplanes \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "platform",
    "name": "production",
    "gateway_host": "envoy-prod.example.com",
    "description": "Production API gateway"
  }'

# Create staging dataplane
curl -X POST http://localhost:8080/api/v1/teams/platform/dataplanes \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "platform",
    "name": "staging",
    "gateway_host": "envoy-staging.example.com",
    "description": "Staging API gateway"
  }'
```

### Via UI

1. Navigate to **Dataplanes** in the sidebar
2. Click **Create Dataplane**
3. Fill in:
   - Name: `production`
   - Gateway Host: `envoy-prod.example.com`
   - Description: `Production API gateway`
4. Click **Create**

## Assigning Listeners to Dataplanes

Listeners must be assigned to a dataplane for:
- xDS configuration scoping
- MCP tool execution routing

### Via REST API

```bash
# Create listener assigned to production dataplane
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "team": "platform",
    "name": "api-listener",
    "address": "0.0.0.0",
    "port": 10000,
    "dataplane_id": "dp_abc123"
  }'

# Update existing listener to assign dataplane
curl -X PUT http://localhost:8080/api/v1/listeners/listener_xyz \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "dataplane_id": "dp_abc123"
  }'
```

### Via UI

1. Navigate to **Listeners**
2. Click on a listener to edit
3. Select **Dataplane** from dropdown
4. Click **Save**

## Envoy Bootstrap Configuration

Each dataplane needs an Envoy bootstrap configuration pointing to Flowplane's xDS server.

### Basic Bootstrap

```yaml
# envoy-bootstrap.yaml
node:
  cluster: production-dataplane
  id: envoy-prod-001

admin:
  address:
    socket_address:
      address: 127.0.0.1
      port_value: 9901

dynamic_resources:
  ads_config:
    api_type: GRPC
    transport_api_version: V3
    grpc_services:
    - envoy_grpc:
        cluster_name: xds_cluster

  lds_config:
    ads: {}
    resource_api_version: V3

  cds_config:
    ads: {}
    resource_api_version: V3

static_resources:
  clusters:
  - name: xds_cluster
    connect_timeout: 5s
    type: STRICT_DNS
    http2_protocol_options: {}
    lb_policy: ROUND_ROBIN
    load_assignment:
      cluster_name: xds_cluster
      endpoints:
      - lb_endpoints:
        - endpoint:
            address:
              socket_address:
                address: flowplane.example.com
                port_value: 50051
```

### Generate Bootstrap via API

Flowplane can generate bootstrap configurations:

```bash
curl http://localhost:8080/api/v1/teams/platform/dataplanes/production/bootstrap \
  -H "Authorization: Bearer $TOKEN" \
  > envoy-bootstrap.yaml
```

## Docker Compose Example

### Multi-Dataplane Setup

```yaml
# docker-compose.yml
version: '3.8'

services:
  # Flowplane Control Plane
  flowplane:
    image: ghcr.io/flowplane/flowplane:latest
    ports:
      - "8080:8080"
      - "50051:50051"
    environment:
      FLOWPLANE_DATABASE_URL: sqlite:///app/data/flowplane.db
    volumes:
      - flowplane-data:/app/data

  # Production Dataplane
  envoy-production:
    image: envoyproxy/envoy:v1.31-latest
    command: -c /etc/envoy/bootstrap.yaml
    ports:
      - "10000:10000"  # Listener
      - "9901:9901"    # Admin
    volumes:
      - ./envoy-production.yaml:/etc/envoy/bootstrap.yaml:ro
    depends_on:
      - flowplane

  # Staging Dataplane
  envoy-staging:
    image: envoyproxy/envoy:v1.31-latest
    command: -c /etc/envoy/bootstrap.yaml
    ports:
      - "10100:10000"  # Different port
      - "9902:9901"
    volumes:
      - ./envoy-staging.yaml:/etc/envoy/bootstrap.yaml:ro
    depends_on:
      - flowplane

  # Development Dataplane
  envoy-development:
    image: envoyproxy/envoy:v1.31-latest
    command: -c /etc/envoy/bootstrap.yaml
    ports:
      - "10200:10000"
      - "9903:9901"
    volumes:
      - ./envoy-development.yaml:/etc/envoy/bootstrap.yaml:ro
    depends_on:
      - flowplane

volumes:
  flowplane-data:
```

### Bootstrap Files

Each Envoy needs its own bootstrap with unique `node.id`:

**envoy-production.yaml:**
```yaml
node:
  cluster: production
  id: envoy-production-001
# ... rest of bootstrap
```

**envoy-staging.yaml:**
```yaml
node:
  cluster: staging
  id: envoy-staging-001
# ... rest of bootstrap
```

## MCP Tool Execution Flow

When an AI assistant calls an MCP gateway tool:

```
1. MCP Request: tools/call { name: "api_getUsers" }
       │
       ▼
2. Flowplane looks up tool → finds listener → finds dataplane
       │
       ▼
3. Resolve gateway_host from dataplane
       │
       ▼
4. Build HTTP request: http://{gateway_host}:{listener_port}/users
       │
       ▼
5. Send request to Envoy
       │
       ▼
6. Envoy routes to upstream cluster
       │
       ▼
7. Return response to AI assistant
```

### Configuration Error

If a listener has no dataplane assigned:

```json
{
  "error": "configuration_error",
  "message": "Listener 'api-listener' has no dataplane configured. Assign a dataplane with gateway_host to enable MCP tool execution."
}
```

## Use Cases

### Environment Isolation

```
Dataplanes:
├── production (gateway_host: prod.internal)
│   └── Listeners: api-prod-listener
├── staging (gateway_host: staging.internal)
│   └── Listeners: api-staging-listener
└── development (gateway_host: localhost)
    └── Listeners: api-dev-listener
```

### Team Separation

```
Teams:
├── platform-team
│   └── Dataplanes: platform-gateway
│       └── Listeners: platform-api
├── mobile-team
│   └── Dataplanes: mobile-gateway
│       └── Listeners: mobile-api
└── web-team
    └── Dataplanes: web-gateway
        └── Listeners: web-api
```

### Traffic Type Separation

```
Dataplanes:
├── external-gateway (gateway_host: external.example.com)
│   └── Listeners: public-api (port 443)
├── internal-gateway (gateway_host: internal.local)
│   └── Listeners: internal-api (port 8080)
└── admin-gateway (gateway_host: admin.local)
    └── Listeners: admin-api (port 9000)
```

## Best Practices

### Naming Conventions

- Use descriptive names: `production`, `staging`, `us-east-prod`
- Include environment in name: `api-gateway-prod`
- Include region if applicable: `eu-west-gateway`

### Gateway Host Configuration

- Use IP addresses for reliability: `10.0.1.100`
- Use DNS names for flexibility: `envoy.example.com`
- Ensure the host is reachable from Flowplane

### Listener Assignment

- Always assign listeners to dataplanes for MCP tools
- Group related listeners on the same dataplane
- Use different ports for different services on same dataplane

### Monitoring

- Monitor each dataplane independently
- Track connection status per dataplane
- Alert on disconnected dataplanes

## Troubleshooting

### Dataplane Not Receiving Configuration

1. Check Envoy logs for xDS connection errors
2. Verify xDS server address in bootstrap
3. Check network connectivity to Flowplane
4. Verify listener is assigned to correct dataplane

### MCP Tools Failing

1. Verify `gateway_host` is set on dataplane
2. Check `gateway_host` is reachable from Flowplane
3. Verify listener is assigned to dataplane
4. Check Envoy admin endpoint for listener status

### Configuration Changes Not Propagating

1. Check Flowplane logs for xDS push errors
2. Verify Envoy is connected (check xDS connection count)
3. Check for validation errors in configuration

## Next Steps

- Review [Kubernetes deployment](kubernetes.md)
- Configure [multi-region deployment](multi-region.md)
- Set up [TLS configuration](../tls.md)
