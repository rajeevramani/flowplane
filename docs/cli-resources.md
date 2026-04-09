# CLI: Resource Management

CRUD operations for gateway resources: clusters, listeners, routes, filters, teams, dataplanes, virtual hosts, secrets, scaffolding, and declarative apply.

All resource types follow a consistent CRUD pattern:

```
flowplane <resource> create -f <FILE> [-o json|yaml|table]
flowplane <resource> list [--limit N] [--offset N] [-o json|yaml|table]
flowplane <resource> get <NAME> [-o json|yaml|table]
flowplane <resource> update <NAME> -f <FILE> [-o json|yaml|table]
flowplane <resource> delete <NAME> [--yes]
```

**File format:** `-f` accepts both YAML (`.yaml`/`.yml`) and JSON (`.json`) files. Format is detected by file extension.

---

## Clusters

Manage upstream service clusters (backend endpoints + load balancing).

**List filter:** `--service <NAME>` — filter by service name.

### Create payload

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

| Field | Required | Description |
|---|---|---|
| `name` | Yes | Unique cluster name |
| `serviceName` | Yes | Logical service name |
| `endpoints` | Yes | Array of `{host, port}` objects |
| `lbPolicy` | No | `ROUND_ROBIN` (default), `LEAST_REQUEST`, `RANDOM`, `RING_HASH`, `MAGLEV` |
| `connectTimeoutSeconds` | No | Upstream TCP connection timeout |
| `useTls` | No | Enable TLS to upstream (auto-enabled for port 443) |
| `circuitBreakers` | No | See below |
| `outlierDetection` | No | See below |
| `healthChecks` | No | Active health checking |

### Circuit breakers

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

### Outlier detection

Passive health checking — ejects unhealthy hosts based on observed 5xx errors.

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

| Field | Range | Default | Description |
|---|---|---|---|
| `consecutive5xx` | 1-1000 | 5 | Consecutive 5xx before ejection |
| `intervalSeconds` | 1-300 | 10 | Time between analysis sweeps |
| `baseEjectionTimeSeconds` | 1-3600 | 30 | Minimum ejection duration |
| `maxEjectionPercent` | 1-100 | 10 | Max % of hosts ejected at once |
| `minHosts` | 1-100 | 1 | Minimum hosts before allowing ejections |

### Examples

```bash
flowplane cluster create -f cluster.yaml
flowplane cluster list --service payments -o table
flowplane cluster get my-backend -o json
flowplane cluster update my-backend -f updated-cluster.json
flowplane cluster delete my-backend --yes
```

---

## Listeners

Manage Envoy listeners (entry points for traffic).

**List filter:** `--protocol <PROTO>` — filter by protocol.

> For most use cases, `flowplane expose` is simpler than manual listener creation.

### Create payload

```json
{
  "name": "my-listener",
  "address": "0.0.0.0",
  "port": 10001,
  "protocol": "HTTP",
  "dataplaneId": "dev-dataplane-id",
  "filterChains": [
    {
      "name": "default",
      "filters": [
        {
          "name": "envoy.filters.network.http_connection_manager",
          "type": "httpConnectionManager",
          "routeConfigName": "my-routes"
        }
      ]
    }
  ]
}
```

| Field | Required | Description |
|---|---|---|
| `name` | Yes | Unique listener name |
| `address` | No | Bind address (default: `0.0.0.0`) |
| `port` | Yes | Port in 10000-10020 range |
| `protocol` | No | `HTTP` (default), `HTTPS`, `TCP` |
| `dataplaneId` | Yes | ID of the target dataplane (use `dev-dataplane-id` in dev mode) |
| `filterChains` | Yes | Array of filter chains. For HTTP, use a single chain with an `httpConnectionManager` filter |

> Port range: Envoy exposes ports 10000-10020. Listener ports outside this range work in the DB but Envoy won't serve traffic on them.

### Examples

```bash
flowplane listener create -f listener.yaml
flowplane listener list --protocol HTTP -o table
flowplane listener get my-listener -o json
flowplane listener update my-listener -f updated-listener.json
flowplane listener delete my-listener --yes
```

---

## Route Configs

Manage routing rules (path matching -> cluster forwarding).

**List filter:** `--cluster <NAME>` — filter by cluster name.

### Create payload

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

| Field | Description |
|---|---|
| `virtualHosts[].domains` | Matched against `Host` header. `["*"]` matches all |
| `routes[].match.path.type` | `exact`, `prefix`, `regex`, or `template` |
| `routes[].action.type` | `forward` (single cluster), `weighted` (traffic split), `redirect` |

Route order matters — first match wins. Place more-specific paths before broader prefixes.

### Route with retry policy

```json
{
  "name": "resilient-routes",
  "virtualHosts": [{
    "name": "default-vhost",
    "domains": ["*"],
    "routes": [{
      "name": "api-route",
      "match": { "path": { "type": "prefix", "value": "/api" } },
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
    }]
  }]
}
```

### Weighted routing (blue/green)

```json
{
  "name": "blue-green-routes",
  "virtualHosts": [{
    "name": "app",
    "domains": ["*"],
    "routes": [{
      "name": "weighted-split",
      "match": { "path": { "type": "prefix", "value": "/" } },
      "action": {
        "type": "weighted",
        "totalWeight": 100,
        "clusters": [
          { "name": "app-v1", "weight": 90 },
          { "name": "app-v2", "weight": 10 }
        ]
      }
    }]
  }]
}
```

### Examples

```bash
flowplane route create -f routes.yaml
flowplane route list --cluster my-backend -o table
flowplane route get my-routes -o json
flowplane route update my-routes -f updated-routes.json
flowplane route delete my-routes --yes
```

> `update` performs a **full replacement** of the `virtualHosts` array. Fetch existing config first, then include all routes plus changes.

---

## Teams

Manage teams (prod mode). Requires `--org` flag.

### Create payload

```json
{
  "name": "engineering",
  "display_name": "Engineering Team"
}
```

### Examples

```bash
flowplane team create --org acme-corp -f team.json
flowplane team list --org acme-corp -o table
flowplane team list --admin                     # Platform admin: all teams across orgs
flowplane team get --org acme-corp engineering
flowplane team update --org acme-corp engineering -f updated-team.json
flowplane team delete --org acme-corp engineering --yes
```

---

## Dataplanes

Manage Envoy dataplane registrations. Dataplanes represent an Envoy instance connected via xDS.

Get and config support `-o json|yaml` (no table). List supports all three formats.

```bash
flowplane dataplane list -o table
flowplane dataplane get dev-dataplane -o json
flowplane dataplane config dev-dataplane                # Envoy bootstrap config (xDS, admin, node)
flowplane dataplane create -f dataplane.yaml
flowplane dataplane update dev-dataplane -f updated.json
flowplane dataplane delete dev-dataplane --yes
```

---

## Virtual Hosts

Virtual hosts are scoped to a route config. Use these to inspect the virtual hosts and routes within a route config without fetching the entire resource.

Get supports `-o json|yaml` (no table).

```bash
flowplane vhost list --route-config my-routes -o table
flowplane vhost get my-routes my-vhost -o json
```

---

## Filters

Manage HTTP filters and attach them to listeners. For filter concepts and all filter types, see [Filters](filters.md).

### flowplane filter create

Create a filter from a JSON spec file.

```
flowplane filter create -f <FILE> [-o json|yaml|table]
```

The JSON structure uses nested `config.type` + `config.config`:

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

Available filter types: `local_rate_limit`, `jwt_auth`, `oauth2`, `ext_authz`, `rbac`, `cors`, `header_mutation`, `custom_response`, `compressor`, `mcp`.

<details>
<summary>CORS filter payload</summary>

```json
{
  "name": "my-cors",
  "filterType": "cors",
  "config": {
    "type": "cors",
    "config": {
      "policy": {
        "allow_origin": [
          { "type": "exact", "value": "https://example.com" }
        ],
        "allow_methods": ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
        "allow_headers": ["authorization", "content-type"],
        "max_age": 86400,
        "allow_credentials": true
      }
    }
  }
}
```

</details>

<details>
<summary>Header mutation filter payload</summary>

```json
{
  "name": "my-headers",
  "filterType": "header_mutation",
  "config": {
    "type": "header_mutation",
    "config": {
      "request_headers_to_add": [
        { "key": "X-Request-Id", "value": "test-123", "append": false }
      ],
      "response_headers_to_add": [
        { "key": "X-Content-Type-Options", "value": "nosniff", "append": false }
      ],
      "response_headers_to_remove": ["X-Powered-By"]
    }
  }
}
```

</details>

<details>
<summary>JWT auth filter payload</summary>

```json
{
  "name": "my-jwt-auth",
  "filterType": "jwt_auth",
  "config": {
    "type": "jwt_auth",
    "config": {
      "providers": {
        "my-provider": {
          "issuer": "https://auth.example.com",
          "audiences": ["my-api"],
          "jwks": {
            "type": "remote",
            "http_uri": {
              "uri": "https://auth.example.com/.well-known/jwks.json",
              "cluster": "auth-jwks-cluster",
              "timeout_ms": 5000
            },
            "cache_duration_seconds": 300
          },
          "fromHeaders": [{ "name": "Authorization", "value_prefix": "Bearer " }],
          "forward": true
        }
      }
    }
  }
}
```

</details>

```bash
flowplane filter create -f ratelimit.yaml
```

### flowplane filter list

```
flowplane filter list [--limit N] [--offset N] [-o json|yaml|table]
```

### flowplane filter get

```
flowplane filter get <NAME> [-o json|yaml|table]
```

### flowplane filter update

Update an existing filter. Bumps the filter version.

```
flowplane filter update <NAME> -f <FILE> [-o json|yaml|table]
```

### flowplane filter delete

```
flowplane filter delete <NAME> [--yes]
```

### flowplane filter attach

Attach a filter to a listener. Lower order numbers execute first.

```
flowplane filter attach <FILTER> --listener <LISTENER> [--order N]
```

Each order value must be unique per listener. Auth filters should have lower order numbers than rate limiters.

```bash
flowplane filter attach my-rate-limit --listener demo-listener --order 1
```

### flowplane filter detach

```
flowplane filter detach <FILTER> --listener <LISTENER>
```

### flowplane filter types

List all available filter types.

```
flowplane filter types [-o json|yaml|table]
```

### flowplane filter type

Get detailed information about a specific filter type, including its config schema and attachment points.

```
flowplane filter type <TYPE_NAME> [-o json|yaml]
```

Use this to discover required fields before creating a filter of that type.

### flowplane filter scaffold

Generate a starter filter manifest for a specific type.

```
flowplane filter scaffold <TYPE> [-o json|yaml]
```

Default output is YAML.

```bash
flowplane filter scaffold local_rate_limit
flowplane filter scaffold jwt_auth -o json
```

---

## Secrets

### flowplane secret create

```
flowplane secret create --name <NAME> --type <TYPE> --config <JSON> [-o json|yaml|table]
```

| Flag | Required | Description |
|---|---|---|
| `--name` | Yes | Unique secret name |
| `--type` | Yes | Secret type: `generic_secret`, `tls_certificate`, `validation_context` |
| `--config` | Yes | JSON config string (structure depends on type) |

```bash
flowplane secret create --name my-secret --type generic_secret \
  --config '{"type":"generic_secret","secret":"dGVzdC1zZWNyZXQ="}'
```

### flowplane secret list

```
flowplane secret list [-o json|yaml|table]
```

### flowplane secret get

```
flowplane secret get <SECRET_ID> [-o json|yaml|table]
```

### flowplane secret rotate

Rotate a secret by ID, bumping its version.

```
flowplane secret rotate <SECRET_ID> --config <JSON> [-o json|yaml|table]
```

`--config` is required and uses the same JSON shape as `secret create --config`.

```bash
flowplane secret rotate abc-123 --config '{"type":"generic_secret","secret":"bmV3LXNlY3JldA=="}'
```

### flowplane secret delete

```
flowplane secret delete <SECRET_ID> [--yes]
```

---

## Resource Scaffolding

Generate comprehensive starter manifests for any resource type. Scaffold output includes ALL fields with `[REQUIRED]`/`[OPTIONAL]` annotations. Optional complex sections (health checks, TLS, tracing, circuit breakers, etc.) are commented out with example values.

```bash
flowplane cluster scaffold                              # Cluster YAML (all fields)
flowplane cluster scaffold -o json                      # JSON scaffold
flowplane listener scaffold                             # Listener (HCM, TLS, tracing, TCP proxy)
flowplane route scaffold                                # Route config (forward, weighted, redirect)
flowplane filter scaffold <TYPE>                        # Filter scaffold by type
flowplane filter scaffold <TYPE> -o json                # Filter scaffold as JSON
```

Available filter types: `header_mutation`, `cors`, `custom_response`, `rbac`, `mcp`, `local_rate_limit`, `compressor`, `ext_authz`, `jwt_auth`, `oauth2`.

**Scaffold details:**
- All field names use camelCase (matching API serialization)
- Includes `kind` field for `apply -f` compatibility (`create -f` strips it automatically)
- Filter scaffolds include `filterType` at top level (required by API)
- YAML comments out optional complex sections; JSON includes all fields with defaults

**Example workflow:**

```bash
# Generate, edit, then create or apply
flowplane cluster scaffold > my-cluster.yaml
# Edit my-cluster.yaml — replace <placeholders> with real values
flowplane cluster create -f my-cluster.yaml     # Direct create (YAML or JSON)
# OR
flowplane apply -f my-cluster.yaml              # Declarative create-or-update
```

---

## Bulk Operations

### flowplane apply

Declarative create-or-update for resources from YAML/JSON manifests.

```
flowplane apply -f <FILE_OR_DIRECTORY>
```

**Manifest format:**

```yaml
kind: Cluster
name: my-cluster
serviceName: my-service
endpoints:
  - host: "127.0.0.1"
    port: 8080
lbPolicy: ROUND_ROBIN
```

| Behavior | Description |
|---|---|
| Create-or-update | Creates if new, updates if exists |
| Directory mode | Processes all `.yaml` and `.json` files |
| Output | Reports `created` or `updated` per resource |
| Errors | Reports per-file errors without stopping other files |

Supported kinds: `Cluster`, `Listener`, `Route` (alias: `RouteConfig`, `route-config`), `Filter`, `Secret`, `Dataplane`.

```bash
flowplane apply -f my-cluster.yaml
flowplane apply -f manifests/                           # apply all files in directory
```
