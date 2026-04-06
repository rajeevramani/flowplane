# CLI Commands Detailed Reference

Derived from `src/cli/*.rs` clap definitions and `flowplane --help` output.

## JSON Payload Examples

### Cluster Create (`-f cluster.json`)
```json
{
  "name": "my-backend",
  "serviceName": "my-backend-service",
  "endpoints": [
    { "host": "backend-svc", "port": 8000 }
  ],
  "connectTimeoutSeconds": 5,
  "useTls": false,
  "lbPolicy": "ROUND_ROBIN"
}
```
- `endpoints` — list of `{host, port}` objects. Use Docker service names for host.
- `lbPolicy` — `ROUND_ROBIN`, `LEAST_REQUEST`, `RANDOM`, `RING_HASH`, `MAGLEV` (camelCase)
- `serviceName` — logical service name
- `connectTimeoutSeconds` — upstream connection timeout
- `useTls` — enable TLS to upstream (auto-enabled for port 443)

#### Cluster with Circuit Breakers
```json
{
  "name": "my-backend",
  "serviceName": "my-backend-service",
  "endpoints": [{ "host": "backend-svc", "port": 8000 }],
  "lbPolicy": "ROUND_ROBIN",
  "circuitBreakers": {
    "default": {
      "maxConnections": 10,
      "maxPendingRequests": 5,
      "maxRequests": 20,
      "maxRetries": 2
    }
  }
}
```

#### Cluster with Outlier Detection
Passive health checking — ejects unhealthy hosts based on observed errors. This is a cluster-level setting (not a filter).
```json
{
  "name": "my-backend",
  "serviceName": "my-backend-service",
  "endpoints": [{ "host": "backend-svc", "port": 8000 }],
  "lbPolicy": "ROUND_ROBIN",
  "outlierDetection": {
    "consecutive5xx": 5,
    "intervalSeconds": 10,
    "baseEjectionTimeSeconds": 30,
    "maxEjectionPercent": 50,
    "minHosts": 3
  }
}
```
- `consecutive5xx` — number of consecutive 5xx before ejecting a host (range: 1–1000)
- `intervalSeconds` — time between outlier analysis sweeps (range: 1–300)
- `baseEjectionTimeSeconds` — base ejection duration, increases with consecutive ejections (range: 1–3600)
- `maxEjectionPercent` — max percentage of hosts that can be ejected simultaneously (range: 1–100)
- `minHosts` — minimum number of hosts required before ejection kicks in (range: 1–100)
All fields are optional. Values outside the valid ranges return 400. Maps directly to Envoy's outlier detection.
```

### Listener Create (`-f listener.json`)
```json
{
  "name": "my-listener",
  "address": "0.0.0.0",
  "port": 10001,
  "protocol": "HTTP",
  "dataplaneId": "<from dataplane list>",
  "filterChains": [
    {
      "name": "default",
      "filters": [
        {
          "name": "envoy.filters.network.http_connection_manager",
          "type": "httpConnectionManager",
          "routeConfigName": "my-routes",
          "httpFilters": [
            { "filter": { "type": "router" } }
          ]
        }
      ]
    }
  ]
}
```
- `port` — must be in 10000-10020 range for Envoy
- `dataplaneId` — UUID of the target dataplane (get from API or `cp_list_dataplanes`)
- `routeConfigName` — binds a route config to this listener's filter chain
- `filterChains` — the router filter is required but Flowplane auto-appends it if omitted in MCP tools

> For most use cases, `flowplane expose` is simpler — it creates cluster + route config + listener in one command.

### Route Config Create (`-f route-config.json`)
```json
{
  "name": "my-routes",
  "virtualHosts": [{
    "name": "default-vhost",
    "domains": ["*"],
    "routes": [{
      "name": "catch-all",
      "match": { "path": { "type": "prefix", "value": "/" } },
      "action": { "type": "forward", "cluster": "my-backend" }
    }]
  }]
}
```
- `virtualHosts[].domains` — matched against `Host` header. `["*"]` matches everything.
- `routes[].match.path.type` — `exact`, `prefix`, `regex`, or `template`
- `routes[].action.type` — `forward` (single cluster), `weighted` (traffic split), `redirect`
- Route order matters — first match wins (more-specific paths before broader prefixes)

#### Route Action with Retry Policy
```json
{
  "action": {
    "type": "forward",
    "cluster": "my-backend",
    "timeoutSeconds": 30,
    "retryPolicy": {
      "maxRetries": 3,
      "retryOn": ["5xx", "reset", "connect-failure"],
      "perTryTimeoutSeconds": 10,
      "backoff": {
        "baseIntervalMs": 100,
        "maxIntervalMs": 1000
      }
    }
  }
}
```

### Filter Create (`-f filter.json`)
```json
{
  "name": "my-rate-limit",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "api_rl",
      "token_bucket": {
        "max_tokens": 100,
        "tokens_per_fill": 100,
        "fill_interval_ms": 1000
      }
    }
  }
}
```
- `filterType` — one of 11 types (10 implemented): `oauth2`, `jwt_auth`, `ext_authz`, `rbac`, `local_rate_limit`, `rate_limit` (not implemented), `cors`, `header_mutation`, `custom_response`, `compressor`, `mcp`
- `config` — nested `{type, config}` structure. Use `cp_get_filter_type` MCP tool or `cp_list_filter_types` to get schemas.

### Team Create (`-f team.json`)
```json
{
  "name": "engineering",
  "display_name": "Engineering Team"
}
```

## Common Workflows

### Expose httpbin (quick)
```bash
flowplane expose http://httpbin:80 --name httpbin --port 10001
```
Output:
```
Exposed 'httpbin' -> http://httpbin:80
  Port:   10001
  Paths:  /

  curl http://localhost:10001/
```
Resources created: cluster `httpbin`, route config `httpbin-routes`, listener `httpbin-listener` on port 10001.

**Without --name** (auto-derives from upstream):
```bash
flowplane expose http://httpbin:80
```
Output:
```
Exposed 'httpbin-80' -> http://httpbin:80
  Port:   10001
  Paths:  /
```
Name derived: `http://httpbin:80` → `httpbin-80`.

**With paths**:
```bash
flowplane expose http://httpbin:80 --name demo --path /api --path /health
```
Output:
```
Exposed 'demo' -> http://httpbin:80
  Port:   10001
  Paths:  /api, /health
```

**Idempotent re-expose** (same name + same upstream = 200, not 201):
```bash
flowplane expose http://httpbin:80 --name demo
# Returns existing port, no new resources created
```

**Different upstream, same name** (409):
```bash
flowplane expose http://other:8080 --name demo
# Error: "Service 'demo' already exists with a different upstream"
```

**Port collision** (409):
```bash
flowplane expose http://httpbin:80 --name demo2 --port 10001  # already in use
# Error: "Port 10001 is already in use"
```

**Port out of range** (400):
```bash
flowplane expose http://httpbin:80 --name demo --port 9999
# Error: "Port 9999 is outside the valid range 10001-10020"
```

**Unexpose**:
```bash
flowplane unexpose httpbin
# Output: Removed exposed service 'httpbin'
# Deletes: cluster 'httpbin', route config 'httpbin-routes', listener 'httpbin-listener'
```

### Expose httpbin (manual)
```bash
# 1. Cluster
cat > /tmp/c.json <<'EOF'
{"name":"httpbin","endpoints":[{"host":"httpbin","port":80}],"lbPolicy":"ROUND_ROBIN"}
EOF
flowplane cluster create -f /tmp/c.json

# 2. Route config
cat > /tmp/r.json <<'EOF'
{"name":"httpbin-rc","virtualHosts":[{"name":"vhost","domains":["*"],"routes":[{"name":"all","match":{"path":{"type":"prefix","value":"/"}},"action":{"type":"forward","cluster":"httpbin"}}]}]}
EOF
flowplane route create -f /tmp/r.json

# 3. Listener
cat > /tmp/l.json <<'EOF'
{"name":"httpbin-listener","address":"0.0.0.0","port":10001,"route_config_name":"httpbin-rc"}
EOF
flowplane listener create -f /tmp/l.json

# 4. Test
curl http://localhost:10001/get
```

### Add rate limiting
```bash
cat > /tmp/rl.json <<'EOF'
{"name":"rl","filterType":"local_rate_limit","config":{"type":"local_rate_limit","config":{"stat_prefix":"rl","token_bucket":{"max_tokens":5,"tokens_per_fill":5,"fill_interval_ms":60000}}}}
EOF
flowplane filter create -f /tmp/rl.json
flowplane filter attach rl --listener httpbin-listener --order 1
```

### Import OpenAPI spec

**Requirements**: The OpenAPI spec must have (1) a `servers` block, (2) valid operations with `responses`. Minimal working spec:
```yaml
openapi: "3.0.0"
info:
  title: My API
  version: "1.0.0"
servers:
  - url: http://localhost:3000
paths:
  /users:
    get:
      summary: List users
      responses:
        "200":
          description: OK
```

**Basic import** (listener name and cluster derived from spec title):
```bash
flowplane import openapi ./my-api.yaml --name my-api --port 10002
```
Output:
```
Imported 'My API'
  Version:          1.0.0
  Import ID:        179b5ca4-d047-41f7-8e6f-e4301f615e0e
  Routes created:   1
  Clusters created: 1
  Clusters reused:  0
  Listener:         my-api-listener
```
Resources created:
- Cluster: `my-api-localhost` (from sanitized spec title + server host)
- Route config: `my-api-routes`
- Listener: `my-api-listener` on port 10002

**Without --name** (derives from spec title):
```bash
flowplane import openapi ./my-api.yaml --port 10003
```
Listener name derived from `info.title`: `my-api-listener`.

**Re-import same spec** (idempotent — replaces previous import):
```bash
flowplane import openapi ./my-api.yaml --name my-api --port 10002
# Deletes previous import with same spec name, creates fresh resources
# Gets a new Import ID but same resource naming
```

**Port collision** (409 error):
```bash
flowplane import openapi ./other-api.yaml --name other --port 10002
# Error: Import failed with status 409 Conflict:
# "A listener already exists at 0.0.0.0:10002 (name: 'my-api-listener').
#  Use a different port with new_listener_port, or use
#  listener_mode=existing with existing_listener_name=my-api-listener"
```

**Spec without servers block** (400 error):
```bash
# Error: "Failed to build gateway plan: OpenAPI document has no servers defined"
```

**Spec with bare operations (no responses)** (400 error):
```bash
# Error: "Invalid OpenAPI YAML document: paths: data did not match any variant..."
```

### Learn API from traffic
```bash
# Start a learning session (auto-activates, begins collecting immediately)
flowplane learn start --route-pattern '^/api/.*' --target-sample-count 50

# Auto-aggregate mode: periodic snapshots while continuing to collect
flowplane learn start --route-pattern '^/api/.*' --target-sample-count 50 --auto-aggregate

# Optional filters: cluster, HTTP methods, duration limit
flowplane learn start --route-pattern '^/get.*' --target-sample-count 100 \
  --cluster-name my-cluster --http-methods GET POST --max-duration-seconds 3600

# Monitor sessions
flowplane learn list                              # Table view
flowplane learn list -o json                      # JSON with all fields
flowplane learn get <session-id>                  # Single session details
flowplane learn get <session-id> -o json          # Full JSON output

# Stop session (triggers final aggregation, then completes)
flowplane learn stop <session-id>

# Cancel (discards without aggregating — use --yes in scripts)
flowplane learn cancel <session-id> --yes
```

Full workflow:
```bash
flowplane expose http://httpbin:80 --name httpbin        # 1. Expose service
flowplane learn start --route-pattern '^/.*' --target-sample-count 5  # 2. Start learning
for i in $(seq 1 10); do curl -s http://localhost:10001/get > /dev/null; done  # 3. Traffic
flowplane learn get <session-id> -o json                 # 4. Check completion
# Schemas available via MCP: cp_list_aggregated_schemas, cp_export_schema_openapi
```

### Manage secrets
```bash
# Create a secret (for use with oauth2, jwt_auth, ext_authz filters)
flowplane secret create --name oauth-secret --type generic_secret \
  --config '{"type":"generic_secret","secret":"dGVzdC1zZWNyZXQ="}'

# List and inspect
flowplane secret list
flowplane secret get <secret-id> -o json

# Delete (--yes required in scripts)
flowplane secret delete <secret-id> --yes
```

### Switch teams
```bash
flowplane config set team engineering
flowplane cluster list  # now scoped to engineering team
```

## Source Files

| Command Module | Source File |
|---|---|
| Top-level commands | `src/cli/mod.rs` |
| auth (login, token, whoami, logout) | `src/cli/auth.rs` |
| cluster CRUD | `src/cli/clusters.rs` |
| listener CRUD | `src/cli/listeners.rs` |
| route CRUD | `src/cli/routes.rs` |
| filter CRUD + attach/detach | `src/cli/filter.rs` |
| expose/unexpose | `src/cli/expose.rs` |
| import openapi | `src/cli/import.rs` |
| learn (sessions) | `src/cli/learn.rs` |
| secret CRUD | `src/cli/secrets.rs` |
| config management | `src/cli/config_cmd.rs` |
| team CRUD | `src/cli/teams.rs` |
| init/down (compose) | `src/cli/compose.rs` |
| status/doctor | `src/cli/status.rs` |
| list (exposed services) | `src/cli/list.rs` |
| logs | `src/cli/logs.rs` |
| HTTP client | `src/cli/client.rs` |
| Output formatting | `src/cli/output.rs` |
| Credential handling | `src/cli/credentials.rs` |
