# CLI Usage Guide

Flowplane provides a comprehensive command-line interface (`flowplane`) for managing Envoy control plane resources, authentication tokens, and API definitions. This guide covers installation, configuration, and practical usage.

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/your-org/flowplane.git
cd flowplane

# Build the CLI binary (creates flowplane-cli executable)
cargo build --release --bin flowplane-cli

# Install to system path (optional)
# Note: Binary file is named flowplane-cli, but the command you use is 'flowplane'
cargo install --path . --bin flowplane-cli
```

### Verify Installation

```bash
flowplane --version
flowplane --help
```

## Configuration

### Initial Setup

The CLI requires authentication to interact with the Flowplane API. You can provide credentials using:

1. **Configuration file** (recommended for persistent settings)
2. **Environment variables** (recommended for CI/CD)
3. **Command-line flags** (recommended for ad-hoc commands)

### Configuration File Setup

Initialize the configuration file:

```bash
flowplane config init
```

This creates `~/.flowplane/config.toml`. Set your credentials:

```bash
# Set authentication token
flowplane config set token fp_pat_your_token_here

# Set API base URL (default: http://127.0.0.1:8080)
flowplane config set base_url https://api.flowplane.io

# Set request timeout in seconds (default: 30)
flowplane config set timeout 60
```

View current configuration:

```bash
# Show as YAML (default)
flowplane config show

# Show as JSON
flowplane config show --output json

# Show as table
flowplane config show --output table

# Show config file location
flowplane config path
```

### Environment Variables

```bash
export FLOWPLANE_TOKEN="fp_pat_your_token_here"
export FLOWPLANE_BASE_URL="http://127.0.0.1:8080"
export FLOWPLANE_TIMEOUT="30"
```

### Command-Line Flags

Override configuration for specific commands:

```bash
flowplane cluster list \
  --token fp_pat_your_token_here \
  --base-url https://api.example.com \
  --timeout 60
```

### Token File Authentication

Store token in a file for enhanced security:

```bash
echo "fp_pat_your_token_here" > ~/.flowplane/token
chmod 600 ~/.flowplane/token

flowplane cluster list --token-file ~/.flowplane/token
```

## Core Commands

The CLI is organized into seven main command groups:

| Command | Description |
|---------|-------------|
| `database` | Database migration and schema management |
| `auth` | Personal access token (PAT) administration |
| `config` | CLI configuration management |
| `api` | API definition and OpenAPI import management |
| `cluster` | Cluster (upstream service) management |
| `listener` | Listener (network socket) management |
| `route` | Route configuration management |

## Database Management

Manage database schema migrations and validation.

### Run Migrations

```bash
# Apply pending migrations
flowplane database migrate

# Dry run to preview migrations
flowplane database migrate --dry-run

# Override database URL
flowplane database migrate --database-url sqlite://./flowplane.db
```

### Check Migration Status

```bash
# Verify schema is up to date
flowplane database status

# List all applied migrations
flowplane database list

# Validate database schema integrity
flowplane database validate
```

## Authentication Token Management

Manage personal access tokens (PATs) for API authentication.

### Create Tokens

```bash
# Create token with specific scopes
flowplane auth create-token \
  --name "prod-deployment" \
  --scope "clusters:*" \
  --scope "routes:*" \
  --scope "listeners:*" \
  --expires-in 90d

# Create token with absolute expiration
flowplane auth create-token \
  --name "temporary-access" \
  --scope "clusters:read" \
  --expires-at "2025-12-31T23:59:59Z" \
  --description "Read-only access for auditing"

# Create token with creator metadata
flowplane auth create-token \
  --name "ci-pipeline" \
  --scope "api-definitions:*" \
  --created-by "platform-team"
```

**Available Scopes:**
- `clusters:*` / `clusters:read` / `clusters:write`
- `routes:*` / `routes:read` / `routes:write`
- `listeners:*` / `listeners:read` / `listeners:write`
- `api-definitions:*` / `api-definitions:read` / `api-definitions:write`
- `tokens:*` / `tokens:read` / `tokens:write`

### List Tokens

```bash
# List all tokens (default: table format)
flowplane auth list-tokens

# Limit results
flowplane auth list-tokens --limit 10 --offset 20

# List with JSON output
flowplane auth list-tokens --output json
```

### Revoke Tokens

```bash
# Revoke a token by ID
flowplane auth revoke-token pat_abc123xyz
```

### Rotate Tokens

```bash
# Generate new secret for existing token
flowplane auth rotate-token pat_abc123xyz
```

## Cluster Management

Manage upstream service cluster configurations.

### Create Clusters

```bash
# Create from JSON file
cat > backend-cluster.json <<EOF
{
  "name": "backend-api",
  "serviceName": "backend",
  "type": "STRICT_DNS",
  "endpoints": [
    {"host": "backend1.internal", "port": 8080},
    {"host": "backend2.internal", "port": 8080}
  ],
  "connectTimeoutSeconds": 5,
  "useTls": true,
  "tlsServerName": "backend.internal",
  "healthCheck": {
    "path": "/health",
    "intervalSeconds": 10,
    "timeoutSeconds": 2,
    "unhealthyThreshold": 3,
    "healthyThreshold": 2
  },
  "circuitBreakers": {
    "maxConnections": 1000,
    "maxRequests": 1000,
    "maxRetries": 3
  }
}
EOF

flowplane cluster create --file backend-cluster.json

# Create with YAML output
flowplane cluster create --file backend-cluster.json --output yaml
```

### List Clusters

```bash
# List all clusters (table format)
flowplane cluster list

# Filter by service name
flowplane cluster list --service backend

# Paginate results
flowplane cluster list --limit 10 --offset 20

# JSON output
flowplane cluster list --output json
```

### Get Cluster Details

```bash
# Get cluster by name
flowplane cluster get backend-api

# Get with YAML output
flowplane cluster get backend-api --output yaml

# Get with table output
flowplane cluster get backend-api --output table
```

### Update Clusters

```bash
# Update from JSON file
cat > updated-cluster.json <<EOF
{
  "connectTimeoutSeconds": 10,
  "healthCheck": {
    "path": "/healthz",
    "intervalSeconds": 15
  }
}
EOF

flowplane cluster update backend-api --file updated-cluster.json
```

### Delete Clusters

```bash
# Delete with confirmation prompt
flowplane cluster delete backend-api

# Delete without confirmation
flowplane cluster delete backend-api --yes
```

## Route Management

Manage HTTP routing configurations.

### Create Routes

```bash
cat > api-routes.json <<EOF
{
  "name": "api-routes",
  "virtualHosts": [
    {
      "name": "api-host",
      "domains": ["api.example.com", "*.api.example.com"],
      "routes": [
        {
          "name": "users-api",
          "match": {
            "path": {"type": "prefix", "value": "/api/users"}
          },
          "action": {
            "type": "forward",
            "cluster": "backend-api",
            "timeout": "30s"
          }
        },
        {
          "name": "products-api",
          "match": {
            "path": {"type": "prefix", "value": "/api/products"}
          },
          "action": {
            "type": "forward",
            "cluster": "products-cluster"
          }
        }
      ]
    }
  ]
}
EOF

flowplane route create --file api-routes.json
```

### List Routes

```bash
# List all routes
flowplane route list

# Filter by cluster
flowplane route list --cluster backend-api

# JSON output
flowplane route list --output json
```

### Get Route Details

```bash
flowplane route get api-routes
flowplane route get api-routes --output yaml
```

### Update Routes

```bash
flowplane route update api-routes --file updated-routes.json
```

### Delete Routes

```bash
flowplane route delete api-routes
flowplane route delete api-routes --yes
```

## Listener Management

Manage network listener configurations.

### Create Listeners

```bash
cat > http-listener.json <<EOF
{
  "name": "http-listener",
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
              "name": "envoy.filters.http.router",
              "filter": {"type": "router"}
            }
          ]
        }
      ]
    }
  ]
}
EOF

flowplane listener create --file http-listener.json
```

### List Listeners

```bash
# List all listeners
flowplane listener list

# Filter by protocol
flowplane listener list --protocol HTTP

# JSON output
flowplane listener list --output json
```

### Get Listener Details

```bash
flowplane listener get http-listener
flowplane listener get http-listener --output yaml
```

### Update Listeners

```bash
flowplane listener update http-listener --file updated-listener.json
```

### Delete Listeners

```bash
flowplane listener delete http-listener
flowplane listener delete http-listener --yes
```

## API Definition Management

Manage high-level API definitions and OpenAPI imports.

### Create API Definitions

```bash
cat > api-definition.json <<EOF
{
  "team": "platform",
  "domain": "users-api",
  "spec": {
    "openapi": "3.0.0",
    "info": {"title": "Users API", "version": "1.0.0"}
  }
}
EOF

flowplane api create --file api-definition.json
```

### Import from OpenAPI

```bash
# Import OpenAPI YAML specification
flowplane api import-openapi \
  --file openapi.yaml \
  --team platform

# Import OpenAPI JSON specification
flowplane api import-openapi \
  --file openapi.json \
  --team platform \
  --output json
```

**OpenAPI Import Features:**
- Automatically generates clusters from `servers[].url`
- Creates routes for all `paths` entries
- Supports `x-flowplane-filters` extension for filter configuration
- Supports `x-flowplane-custom-response` extension for error responses

### Validate x-flowplane-filters

Validate filter syntax before importing:

```bash
# Validate filters in OpenAPI spec
flowplane api validate-filters --file openapi-with-filters.yaml

# JSON output
flowplane api validate-filters --file openapi.yaml --output json
```

### List API Definitions

```bash
# List all API definitions
flowplane api list

# Filter by team
flowplane api list --team platform

# Filter by team and domain
flowplane api list --team platform --domain users

# Paginate results
flowplane api list --limit 10 --offset 20
```

### Get API Definition

```bash
flowplane api get api_def_abc123xyz
flowplane api get api_def_abc123xyz --output yaml
```

### Get Bootstrap Configuration

Generate Envoy bootstrap configuration for an API definition:

```bash
# Get bootstrap config in YAML (default)
flowplane api bootstrap api_def_abc123xyz

# Get in JSON format
flowplane api bootstrap api_def_abc123xyz --format json

# Scope to team listeners only
flowplane api bootstrap api_def_abc123xyz --scope team

# Use allowlist scope with specific listeners
flowplane api bootstrap api_def_abc123xyz \
  --scope allowlist \
  --allowlist http-listener,https-listener

# Include default listeners in team scope
flowplane api bootstrap api_def_abc123xyz \
  --scope team \
  --include-default
```

### Show Deployed Filters

View HTTP filter configurations for an API definition:

```bash
# Show filters as YAML (default)
flowplane api show-filters api_def_abc123xyz

# Show as JSON
flowplane api show-filters api_def_abc123xyz --output json

# Show as table
flowplane api show-filters api_def_abc123xyz --output table
```

## Output Formats

All CLI commands support multiple output formats:

| Format | Description | Best For |
|--------|-------------|----------|
| `json` | Machine-readable JSON | Scripting, automation |
| `yaml` | Human-readable YAML | Configuration inspection |
| `table` | Formatted table (default for list commands) | Terminal viewing |

Specify format with `--output` or `-o`:

```bash
flowplane cluster list --output json
flowplane route get my-route --output yaml
flowplane listener list --output table
```

## Common Workflows

### Initial System Setup

```bash
# 1. Initialize database
flowplane database migrate

# 2. Initialize CLI configuration
flowplane config init

# 3. Create bootstrap admin token using database
flowplane auth create-token \
  --name "admin-bootstrap" \
  --scope "tokens:*" \
  --scope "clusters:*" \
  --scope "routes:*" \
  --scope "listeners:*" \
  --scope "api-definitions:*"

# 4. Store token in config
flowplane config set token fp_pat_generated_token_here
```

### OpenAPI Import Workflow

```bash
# 1. Validate filters syntax first
flowplane api validate-filters --file openapi.yaml

# 2. Import OpenAPI specification
flowplane api import-openapi \
  --file openapi.yaml \
  --team my-team

# Output includes:
# {
#   "id": "api_def_abc123",
#   "bootstrapUri": "/api/v1/api-definitions/api_def_abc123/bootstrap?scope=team",
#   "routes": ["route1", "route2"]
# }

# 3. Get bootstrap configuration for Envoy
flowplane api bootstrap api_def_abc123 > envoy-bootstrap.yaml

# 4. Verify generated filters
flowplane api show-filters api_def_abc123
```

### Multi-Environment Deployment

```bash
# Development environment
flowplane cluster create \
  --file cluster-dev.json \
  --base-url http://dev.flowplane.internal \
  --token $DEV_TOKEN

# Staging environment
flowplane cluster create \
  --file cluster-staging.json \
  --base-url http://staging.flowplane.internal \
  --token $STAGING_TOKEN

# Production environment
flowplane cluster create \
  --file cluster-prod.json \
  --base-url https://api.flowplane.io \
  --token $PROD_TOKEN
```

### Batch Resource Creation

```bash
#!/bin/bash
set -e

# Create multiple clusters
for cluster_file in clusters/*.json; do
  echo "Creating cluster from $cluster_file"
  flowplane cluster create --file "$cluster_file"
done

# Create routes
for route_file in routes/*.json; do
  echo "Creating route from $route_file"
  flowplane route create --file "$route_file"
done

# Create listeners
for listener_file in listeners/*.json; do
  echo "Creating listener from $listener_file"
  flowplane listener create --file "$listener_file"
done
```

## Scripting & Automation

### Shell Script Example

```bash
#!/bin/bash
set -euo pipefail

# Configuration
FLOWPLANE_TOKEN="${FLOWPLANE_TOKEN:?Environment variable FLOWPLANE_TOKEN is required}"
FLOWPLANE_BASE_URL="${FLOWPLANE_BASE_URL:-http://127.0.0.1:8080}"
TEAM="${TEAM:-platform}"

# Function to wait for cluster health
wait_for_cluster() {
  local cluster_name=$1
  local max_attempts=30
  local attempt=0

  echo "Waiting for cluster '$cluster_name' to be healthy..."

  while [ $attempt -lt $max_attempts ]; do
    if flowplane cluster get "$cluster_name" --output json >/dev/null 2>&1; then
      echo "✅ Cluster '$cluster_name' is ready"
      return 0
    fi
    attempt=$((attempt + 1))
    sleep 2
  done

  echo "❌ Cluster '$cluster_name' failed health check"
  return 1
}

# Deploy backend service
echo "Creating backend cluster..."
flowplane cluster create \
  --file backend-cluster.json \
  --output json | jq -r '.name'

wait_for_cluster "backend-api"

# Import OpenAPI specification
echo "Importing OpenAPI specification..."
API_DEF_ID=$(flowplane api import-openapi \
  --file openapi.yaml \
  --team "$TEAM" \
  --output json | jq -r '.id')

echo "✅ API definition created: $API_DEF_ID"

# Generate bootstrap configuration
echo "Generating Envoy bootstrap configuration..."
flowplane api bootstrap "$API_DEF_ID" \
  --scope team \
  --format yaml > envoy-bootstrap.yaml

echo "✅ Bootstrap configuration saved to envoy-bootstrap.yaml"
```

### Python Client Example

```python
#!/usr/bin/env python3
import json
import os
import subprocess
import sys
from typing import Dict, List, Optional

class FlowplaneCLI:
    """Wrapper for flowplane commands"""

    def __init__(self, base_url: Optional[str] = None, token: Optional[str] = None):
        self.base_url = base_url or os.getenv('FLOWPLANE_BASE_URL', 'http://127.0.0.1:8080')
        self.token = token or os.getenv('FLOWPLANE_TOKEN')

        if not self.token:
            raise ValueError("FLOWPLANE_TOKEN environment variable or token parameter required")

    def _run_command(self, args: List[str]) -> Dict:
        """Execute CLI command and return JSON output"""
        cmd = ['flowplane'] + args + [
            '--base-url', self.base_url,
            '--token', self.token,
            '--output', 'json'
        ]

        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
        return json.loads(result.stdout)

    def create_cluster(self, config_file: str) -> Dict:
        """Create cluster from JSON file"""
        return self._run_command(['cluster', 'create', '--file', config_file])

    def list_clusters(self, service: Optional[str] = None) -> List[Dict]:
        """List all clusters with optional service filter"""
        args = ['cluster', 'list']
        if service:
            args.extend(['--service', service])
        return self._run_command(args)

    def import_openapi(self, spec_file: str, team: str) -> Dict:
        """Import OpenAPI specification"""
        return self._run_command([
            'api', 'import-openapi',
            '--file', spec_file,
            '--team', team
        ])

    def get_bootstrap(self, api_def_id: str, scope: str = 'all') -> str:
        """Get bootstrap configuration as YAML"""
        cmd = [
            'flowplane', 'api', 'bootstrap', api_def_id,
            '--base-url', self.base_url,
            '--token', self.token,
            '--scope', scope,
            '--format', 'yaml'
        ]

        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
        return result.stdout

def main():
    # Initialize client
    client = FlowplaneCLI()

    # Create cluster
    print("Creating backend cluster...")
    cluster = client.create_cluster('backend-cluster.json')
    print(f"✅ Created cluster: {cluster['name']}")

    # List clusters
    print("\nListing all clusters...")
    clusters = client.list_clusters()
    for c in clusters:
        print(f"  - {c['name']} (service: {c['serviceName']})")

    # Import OpenAPI
    print("\nImporting OpenAPI specification...")
    api_def = client.import_openapi('openapi.yaml', 'platform')
    print(f"✅ API definition created: {api_def['id']}")

    # Get bootstrap config
    print("\nGenerating bootstrap configuration...")
    bootstrap = client.get_bootstrap(api_def['id'], scope='team')

    with open('envoy-bootstrap.yaml', 'w') as f:
        f.write(bootstrap)
    print("✅ Bootstrap saved to envoy-bootstrap.yaml")

if __name__ == '__main__':
    main()
```

## CI/CD Integration

### GitHub Actions Example

```yaml
name: Deploy to Flowplane

on:
  push:
    branches: [main]

jobs:
  deploy:
    runs-on: ubuntu-latest

    steps:
      - uses: actions/checkout@v3

      - name: Install Flowplane CLI
        run: |
          cargo install --git https://github.com/your-org/flowplane.git flowplane

      - name: Deploy Resources
        env:
          FLOWPLANE_TOKEN: ${{ secrets.FLOWPLANE_TOKEN }}
          FLOWPLANE_BASE_URL: https://api.flowplane.io
        run: |
          # Validate OpenAPI spec
          flowplane api validate-filters --file openapi.yaml

          # Import specification
          API_DEF_ID=$(flowplane api import-openapi \
            --file openapi.yaml \
            --team production \
            --output json | jq -r '.id')

          echo "Deployed API definition: $API_DEF_ID"

          # Generate bootstrap config
          flowplane api bootstrap "$API_DEF_ID" \
            --scope team \
            --format yaml > envoy-bootstrap.yaml

          # Upload to artifact storage or deploy to Envoy instances
```

### GitLab CI Example

```yaml
stages:
  - validate
  - deploy

variables:
  FLOWPLANE_BASE_URL: https://api.flowplane.io

validate_openapi:
  stage: validate
  script:
    - flowplane api validate-filters --file openapi.yaml

deploy_production:
  stage: deploy
  only:
    - main
  script:
    - |
      API_DEF_ID=$(flowplane api import-openapi \
        --file openapi.yaml \
        --team production \
        --output json | jq -r '.id')

      flowplane api bootstrap "$API_DEF_ID" \
        --scope team \
        --format yaml > envoy-bootstrap.yaml

      # Deploy to production Envoy instances
      kubectl create configmap envoy-bootstrap \
        --from-file=envoy-bootstrap.yaml \
        --dry-run=client -o yaml | kubectl apply -f -
  environment:
    name: production
```

## Troubleshooting

### Authentication Errors

**Problem**: `401 Unauthorized` errors

**Solutions**:
```bash
# 1. Verify token is set
flowplane config show

# 2. Test token with simple command
flowplane cluster list

# 3. Check token expiration
flowplane auth list-tokens | grep your-token-name

# 4. Rotate expired token
flowplane auth rotate-token pat_abc123xyz
```

### Connection Errors

**Problem**: `Connection refused` or timeout errors

**Solutions**:
```bash
# 1. Verify base URL is correct
flowplane config show | grep base_url

# 2. Test API server is running
curl http://127.0.0.1:8080/health

# 3. Check network connectivity
ping api.flowplane.io

# 4. Increase timeout for slow connections
flowplane config set timeout 120
```

### Invalid JSON Errors

**Problem**: `Failed to parse JSON from file`

**Solutions**:
```bash
# 1. Validate JSON syntax
cat cluster.json | jq .

# 2. Check file encoding (must be UTF-8)
file cluster.json

# 3. Use verbose mode for detailed errors
flowplane cluster create --file cluster.json --verbose

# 4. Validate against schema
# (Check examples/ directory for valid schemas)
```

### Database Migration Failures

**Problem**: `Migration failed` or schema errors

**Solutions**:
```bash
# 1. Check current migration status
flowplane database status

# 2. List applied migrations
flowplane database list

# 3. Validate database integrity
flowplane database validate

# 4. Override database URL if needed
flowplane database migrate \
  --database-url postgresql://user:pass@localhost/flowplane
```

### Permission Errors

**Problem**: `403 Forbidden` errors

**Solutions**:
```bash
# 1. Check token scopes
flowplane auth list-tokens --output json | jq '.[] | {name, scopes}'

# 2. Create token with required scopes
flowplane auth create-token \
  --name "full-access" \
  --scope "clusters:*" \
  --scope "routes:*" \
  --scope "listeners:*" \
  --scope "api-definitions:*"

# 3. Use token with appropriate permissions
flowplane cluster list --token fp_pat_your_admin_token
```

## Debug Information

### Enable Verbose Logging

```bash
# CLI verbose mode
flowplane cluster list --verbose

# Rust debug logging
RUST_LOG=debug flowplane cluster list

# Trace-level logging
RUST_LOG=trace flowplane cluster create --file cluster.json
```

### Collect Debug Information

```bash
#!/bin/bash
# Collect diagnostic information for support

echo "=== Flowplane CLI Version ==="
flowplane --version

echo -e "\n=== Configuration ==="
flowplane config show --output json

echo -e "\n=== Database Status ==="
flowplane database status

echo -e "\n=== Cluster List ==="
flowplane cluster list --output json

echo -e "\n=== Route List ==="
flowplane route list --output json

echo -e "\n=== Listener List ==="
flowplane listener list --output json

echo -e "\n=== Environment ==="
env | grep FLOWPLANE
```

## Additional Resources

- **Getting Started**: [getting-started.md](getting-started.md) - Initial setup tutorial
- **API Reference**: [api.md](api.md) - REST API documentation
- **Configuration**: [config-model.md](config-model.md) - Configuration schema reference
- **Filters**: [filters.md](filters.md) - HTTP filter configuration
- **Architecture**: [architecture.md](architecture.md) - System design overview
- **Operations**: [operations.md](operations.md) - Production deployment guide
- **Tutorials**: [tutorials.md](tutorials.md) - Step-by-step tutorials
- **Interactive Docs**: http://127.0.0.1:8080/swagger-ui - Live API exploration
