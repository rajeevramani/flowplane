# CLI Usage Guide

Flowplane provides both a dedicated CLI tool and curl-based workflows for managing the control plane. This guide covers both approaches for common operations.

## Authentication

All CLI operations require authentication. Export your token as an environment variable:

```bash
# Export token for CLI/curl usage
export FLOWPLANE_TOKEN="fp_pat_..."

# Or for direct CLI
export FLOWPLANE_BASE_URL="http://127.0.0.1:8080"
export FLOWPLANE_TOKEN="fp_pat_..."
```

## Getting Started

### First-Time Setup

On first startup, Flowplane displays a bootstrap admin token:

```bash
# Start control plane
DATABASE_URL=sqlite://./data/flowplane.db \
FLOWPLANE_API_BIND_ADDRESS=0.0.0.0 \
cargo run --bin flowplane

# Extract token from output (look for ASCII banner)
# Token: fp_pat_a1b2c3d4-e5f6-7890-abcd-ef1234567890.xJ9k...

# Export for use
export FLOWPLANE_TOKEN="fp_pat_..."
```

See [authentication.md](authentication.md) for complete token management workflows.

## Resource Management

### Clusters

**List Clusters:**

```bash
# Using curl
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/clusters | jq

# With pagination
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/clusters?limit=10&offset=0" | jq
```

**Create Cluster:**

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "backend-api",
    "serviceName": "api",
    "endpoints": [
      {"host": "api.example.com", "port": 443}
    ],
    "connectTimeoutSeconds": 5,
    "useTls": true,
    "tlsServerName": "api.example.com",
    "healthCheck": {
      "path": "/healthz",
      "intervalSeconds": 30,
      "timeoutSeconds": 5,
      "unhealthyThreshold": 3,
      "healthyThreshold": 2
    }
  }' | jq
```

**Get Cluster:**

```bash
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/clusters/backend-api | jq
```

**Update Cluster:**

```bash
curl -sS -X PUT http://127.0.0.1:8080/api/v1/clusters/backend-api \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "backend-api",
    "serviceName": "api",
    "endpoints": [
      {"host": "api1.example.com", "port": 443},
      {"host": "api2.example.com", "port": 443}
    ],
    "connectTimeoutSeconds": 5,
    "useTls": true,
    "tlsServerName": "api.example.com"
  }' | jq
```

**Delete Cluster:**

```bash
curl -sS -X DELETE \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/clusters/backend-api
```

### Routes

**List Routes:**

```bash
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/routes | jq
```

**Create Route:**

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/routes \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "api-routes",
    "virtualHosts": [{
      "name": "api-host",
      "domains": ["api.example.com"],
      "routes": [{
        "name": "users-route",
        "match": {
          "path": {"type": "prefix", "value": "/users"},
          "methods": ["GET", "POST"]
        },
        "action": {
          "type": "forward",
          "cluster": "backend-api",
          "timeoutSeconds": 30
        }
      }]
    }]
  }' | jq
```

**Update Route:**

```bash
curl -sS -X PUT http://127.0.0.1:8080/api/v1/routes/api-routes \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d @route-config.json | jq
```

**Delete Route:**

```bash
curl -sS -X DELETE \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/routes/api-routes
```

### Listeners

**List Listeners:**

```bash
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/listeners | jq
```

**Create Listener:**

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "api-listener",
    "address": "0.0.0.0",
    "port": 8080,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "api-routes",
        "httpFilters": [
          {
            "name": "envoy.filters.http.cors",
            "filter": {
              "type": "cors",
              "allow_origin_string_match": [{"exact": "https://app.example.com"}],
              "allow_methods": ["GET", "POST", "PUT", "DELETE"]
            }
          },
          {
            "name": "envoy.filters.http.router",
            "filter": {"type": "router"}
          }
        ]
      }]
    }]
  }' | jq
```

**Delete Listener:**

```bash
curl -sS -X DELETE \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/listeners/api-listener
```

## Token Management

### Create Token

```bash
curl -sS -X POST http://127.0.0.1:8080/api/v1/tokens \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "ci-pipeline",
    "description": "Token for CI/CD automation",
    "scopes": ["clusters:write", "routes:write", "listeners:read"],
    "expiresAt": "2026-12-31T23:59:59Z"
  }' | jq

# Save the returned token immediately!
# Response: {"id": "...", "token": "fp_pat_..."}
```

### List Tokens

```bash
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/tokens?limit=20&offset=0" | jq
```

### Get Token Details

```bash
TOKEN_ID="a1b2c3d4-e5f6-7890-abcd-ef1234567890"

curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/tokens/$TOKEN_ID" | jq
```

### Update Token

```bash
TOKEN_ID="a1b2c3d4-e5f6-7890-abcd-ef1234567890"

curl -sS -X PATCH "http://127.0.0.1:8080/api/v1/tokens/$TOKEN_ID" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "ci-pipeline-updated",
    "scopes": ["clusters:write", "routes:write", "listeners:write"],
    "status": "active"
  }' | jq
```

### Rotate Token

```bash
TOKEN_ID="a1b2c3d4-e5f6-7890-abcd-ef1234567890"

curl -sS -X POST "http://127.0.0.1:8080/api/v1/tokens/$TOKEN_ID/rotate" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" | jq

# Response contains new token - update immediately!
# Previous token stops working immediately
```

### Revoke Token

```bash
TOKEN_ID="a1b2c3d4-e5f6-7890-abcd-ef1234567890"

curl -sS -X DELETE "http://127.0.0.1:8080/api/v1/tokens/$TOKEN_ID" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN"
```

## OpenAPI Import (Platform API)

### Import OpenAPI Spec

**Basic Import (Shared Listener):**

```bash
# Import to default-gateway-listener (port 10000)
curl -sS -X POST "http://127.0.0.1:8080/api/v1/api-definitions/from-openapi?team=myteam" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  --data-binary @openapi.json | jq
```

**Dedicated Listener:**

```bash
# Create dedicated listener for team
curl -sS -X POST "http://127.0.0.1:8080/api/v1/api-definitions/from-openapi?team=myteam&listener=myteam-gateway&port=8080" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  --data-binary @openapi.json | jq
```

**YAML Format:**

```bash
curl -sS -X POST "http://127.0.0.1:8080/api/v1/api-definitions/from-openapi?team=myteam" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/yaml" \
  --data-binary @openapi.yaml | jq
```

### List API Definitions

```bash
curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/api-definitions | jq
```

### Get API Definition

```bash
API_DEF_ID="myteam-api-v1"

curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/api-definitions/$API_DEF_ID" | jq
```

### Get Envoy Bootstrap for API

```bash
API_DEF_ID="myteam-api-v1"

curl -sS \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  "http://127.0.0.1:8080/api/v1/api-definitions/$API_DEF_ID/bootstrap" | jq
```

## Health & Monitoring

### Health Check

```bash
# Control plane health (no auth required)
curl -sS http://127.0.0.1:8080/healthz

# Expected: 200 OK
```

### Prometheus Metrics

```bash
# Metrics endpoint (no auth required if enabled)
curl -sS http://127.0.0.1:8080/metrics

# Filter specific metrics
curl -sS http://127.0.0.1:8080/metrics | grep http_requests_total
```

### OpenAPI Specification

```bash
# Get OpenAPI JSON spec
curl -sS http://127.0.0.1:8080/api-docs/openapi.json | jq

# Access Swagger UI (browser)
open http://127.0.0.1:8080/swagger-ui
```

## Scripting & Automation

### Bash Scripts

**Complete Gateway Setup Script:**

```bash
#!/bin/bash
set -e

FLOWPLANE_TOKEN="${FLOWPLANE_TOKEN:?Environment variable FLOWPLANE_TOKEN is required}"
FLOWPLANE_URL="${FLOWPLANE_URL:-http://127.0.0.1:8080}"

echo "Creating cluster..."
curl -sS -X POST "$FLOWPLANE_URL/api/v1/clusters" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "backend",
    "serviceName": "backend",
    "endpoints": [{"host": "backend.internal", "port": 8080}],
    "connectTimeoutSeconds": 5
  }' | jq -r '.name'

echo "Creating routes..."
curl -sS -X POST "$FLOWPLANE_URL/api/v1/routes" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "backend-routes",
    "virtualHosts": [{
      "name": "backend-host",
      "domains": ["*"],
      "routes": [{
        "name": "all-traffic",
        "match": {"path": {"type": "prefix", "value": "/"}},
        "action": {"type": "forward", "cluster": "backend"}
      }]
    }]
  }' | jq -r '.name'

echo "Creating listener..."
curl -sS -X POST "$FLOWPLANE_URL/api/v1/listeners" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "backend-listener",
    "address": "0.0.0.0",
    "port": 10000,
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "backend-routes",
        "httpFilters": [
          {"name": "envoy.filters.http.router", "filter": {"type": "router"}}
        ]
      }]
    }]
  }' | jq -r '.name'

echo "Gateway setup complete!"
```

**Backup All Resources:**

```bash
#!/bin/bash
BACKUP_DIR="./flowplane-backup-$(date +%Y%m%d_%H%M%S)"
mkdir -p "$BACKUP_DIR"

echo "Backing up resources to $BACKUP_DIR..."

curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/clusters > "$BACKUP_DIR/clusters.json"

curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/routes > "$BACKUP_DIR/routes.json"

curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/listeners > "$BACKUP_DIR/listeners.json"

curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/tokens > "$BACKUP_DIR/tokens.json"

echo "Backup complete: $BACKUP_DIR"
```

**Restore Resources:**

```bash
#!/bin/bash
BACKUP_DIR="${1:?Usage: $0 <backup-dir>}"

echo "Restoring from $BACKUP_DIR..."

# Restore clusters
jq -c '.[]' "$BACKUP_DIR/clusters.json" | while read -r cluster; do
  curl -sS -X POST http://127.0.0.1:8080/api/v1/clusters \
    -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
    -H "Content-Type: application/json" \
    -d "$cluster" || echo "Failed to restore cluster"
done

# Restore routes
jq -c '.[]' "$BACKUP_DIR/routes.json" | while read -r route; do
  curl -sS -X POST http://127.0.0.1:8080/api/v1/routes \
    -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
    -H "Content-Type: application/json" \
    -d "$route" || echo "Failed to restore route"
done

# Restore listeners
jq -c '.[]' "$BACKUP_DIR/listeners.json" | while read -r listener; do
  curl -sS -X POST http://127.0.0.1:8080/api/v1/listeners \
    -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
    -H "Content-Type: application/json" \
    -d "$listener" || echo "Failed to restore listener"
done

echo "Restore complete!"
```

### Python Script Example

```python
#!/usr/bin/env python3
import os
import requests
import json

class FlowplaneClient:
    def __init__(self, base_url=None, token=None):
        self.base_url = base_url or os.getenv('FLOWPLANE_BASE_URL', 'http://127.0.0.1:8080')
        self.token = token or os.getenv('FLOWPLANE_TOKEN')
        if not self.token:
            raise ValueError("FLOWPLANE_TOKEN environment variable is required")

        self.headers = {
            'Authorization': f'Bearer {self.token}',
            'Content-Type': 'application/json'
        }

    def list_clusters(self):
        response = requests.get(f'{self.base_url}/api/v1/clusters', headers=self.headers)
        response.raise_for_status()
        return response.json()

    def create_cluster(self, config):
        response = requests.post(
            f'{self.base_url}/api/v1/clusters',
            headers=self.headers,
            json=config
        )
        response.raise_for_status()
        return response.json()

    def delete_cluster(self, name):
        response = requests.delete(
            f'{self.base_url}/api/v1/clusters/{name}',
            headers=self.headers
        )
        response.raise_for_status()

if __name__ == '__main__':
    client = FlowplaneClient()

    # Create cluster
    cluster = client.create_cluster({
        'name': 'python-example',
        'serviceName': 'example',
        'endpoints': [{'host': 'example.com', 'port': 443}],
        'connectTimeoutSeconds': 5,
        'useTls': True
    })
    print(f"Created cluster: {cluster['name']}")

    # List clusters
    clusters = client.list_clusters()
    print(f"Total clusters: {len(clusters)}")
```

## Common Workflows

### Initial Setup Workflow

```bash
# 1. Start control plane and capture bootstrap token
docker-compose up -d
BOOTSTRAP_TOKEN=$(docker-compose logs control-plane 2>&1 | grep -oP 'token: \Kfp_pat_[^\s]+')

# 2. Create service token for automation
SERVICE_TOKEN=$(curl -sS -X POST http://127.0.0.1:8080/api/v1/tokens \
  -H "Authorization: Bearer $BOOTSTRAP_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "automation",
    "scopes": ["clusters:write", "routes:write", "listeners:write"]
  }' | jq -r '.token')

# 3. Store service token
echo "FLOWPLANE_TOKEN=$SERVICE_TOKEN" >> ~/.flowplane.env

# 4. Revoke bootstrap token
BOOTSTRAP_ID=$(echo $BOOTSTRAP_TOKEN | cut -d. -f1 | sed 's/fp_pat_//')
curl -sS -X DELETE "http://127.0.0.1:8080/api/v1/tokens/$BOOTSTRAP_ID" \
  -H "Authorization: Bearer $SERVICE_TOKEN"
```

### OpenAPI Import Workflow

```bash
# 1. Prepare OpenAPI spec
cat > my-api.yaml <<EOF
openapi: 3.0.0
info:
  title: My API
  version: 1.0.0
servers:
  - url: https://api.example.com
paths:
  /users:
    get:
      summary: List users
      responses:
        '200':
          description: Success
EOF

# 2. Import to Flowplane
curl -sS -X POST "http://127.0.0.1:8080/api/v1/api-definitions/from-openapi?team=myteam" \
  -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  -H "Content-Type: application/yaml" \
  --data-binary @my-api.yaml | jq

# 3. Verify resources created
curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/clusters | jq '.[] | .name'

curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/api/v1/listeners | jq '.[] | .name'
```

### CI/CD Integration

```bash
# .gitlab-ci.yml or GitHub Actions
deploy_gateway:
  script:
    - |
      curl -sS -X POST "$FLOWPLANE_URL/api/v1/api-definitions/from-openapi?team=$CI_PROJECT_NAME" \
        -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
        -H "Content-Type: application/json" \
        --data-binary @openapi.json
    - |
      # Verify deployment
      curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
        "$FLOWPLANE_URL/api/v1/clusters" | jq '.[] | select(.name | contains("'"$CI_PROJECT_NAME"'"))'
```

## Troubleshooting CLI Issues

### Authentication Errors

```bash
# Test token validity
curl -sS -H "Authorization: Bearer $FLOWPLANE_TOKEN" \
  http://127.0.0.1:8080/healthz

# If 401 Unauthorized:
# - Token may be expired
# - Token may be revoked
# - Token format incorrect (should be fp_pat_...)

# Check token in database (requires admin access)
# psql -U flowplane -d flowplane -c "SELECT * FROM tokens WHERE token_id = 'your-token-id';"
```

### Connection Errors

```bash
# Test connectivity
curl -v http://127.0.0.1:8080/healthz

# If connection refused:
# - Control plane may not be running
# - Wrong port (check FLOWPLANE_API_PORT)
# - Firewall blocking connection

# Check control plane logs
docker logs flowplane-cp
```

### Invalid JSON Errors

```bash
# Validate JSON before sending
echo '{"name": "test"}' | jq empty

# Use jq to format from file
jq . cluster-config.json > /dev/null && echo "Valid JSON"

# Pretty-print response errors
curl -sS ... | jq .
```

## Additional Resources

- [API Reference](api.md) - Complete endpoint documentation
- [Authentication](authentication.md) - Token management and scopes
- [Token Management CLI](token-management.md) - Dedicated CLI tool
- [Getting Started](getting-started.md) - Step-by-step tutorial
- [Operations Guide](operations.md) - Production deployment
- [Swagger UI](http://127.0.0.1:8080/swagger-ui) - Interactive API explorer
