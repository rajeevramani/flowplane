# CLI Reference

Complete command reference for the `flowplane` CLI.

## Global Flags

These flags can be used with any subcommand:

| Flag | Description |
|------|-------------|
| `--base-url <URL>` | Base URL for the Flowplane API (default: `http://localhost:8080`) |
| `--token <TOKEN>` | Personal access token for API authentication |
| `--token-file <PATH>` | Path to file containing personal access token |
| `--timeout <SECONDS>` | Request timeout in seconds |
| `--team <TEAM>` | Team context for resource commands (default: `default`) |
| `-v, --verbose` | Enable verbose logging |
| `-h, --help` | Print help |
| `-V, --version` | Print version |

```
$ flowplane --version
flowplane 0.2.0
```

---

## Stack Management

### `init`

Bootstrap a local dev environment (PostgreSQL + control plane via Docker/Podman).

```
flowplane init [OPTIONS]
```

**Flags:**

| Flag | Description |
|------|-------------|
| `--with-envoy` | Also start an Envoy sidecar proxy |
| `--with-httpbin` | Also start an httpbin test backend (available at localhost:8000) |

**Examples:**

```
$ flowplane init
# Control plane + PostgreSQL only

$ flowplane init --with-envoy
# Add an Envoy proxy

$ flowplane init --with-httpbin
# Add a test backend

$ flowplane init --with-envoy --with-httpbin
# Full dev stack (recommended)
```

A full init produces:

```
Flowplane is running!

  API:     http://localhost:8080
  xDS:     localhost:18000
  Envoy:   localhost:10000 (admin: localhost:9901)
  httpbin: http://localhost:8000
```

### `down`

Stop the local dev environment started by `flowplane init`.

```
flowplane down [OPTIONS]
```

**Flags:**

| Flag | Description |
|------|-------------|
| `--volumes` | Also remove persistent volumes (deletes database data) |

**Examples:**

```
$ flowplane down
Stopping services...
Flowplane services stopped.

$ flowplane down --volumes
Stopping services...
Flowplane services stopped.
Volumes removed — database data has been deleted.
```

### `serve`

Start the Flowplane control plane server directly (without Docker). Use this when you want to run the server outside of a container.

```
flowplane serve [OPTIONS]
```

**Flags:**

| Flag | Description |
|------|-------------|
| `--dev` | Run in dev mode (synthetic identity, no Zitadel) |

Most developers should use `flowplane init` instead. `serve` is for production deployments or custom setups where you manage PostgreSQL and Envoy separately.

### `status`

Show system status or look up a specific listener.

```
flowplane status [NAME]
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `[NAME]` | Listener name to look up (omit for system overview) |

**Examples:**

System overview (no arguments):

```
$ flowplane status
Flowplane Status (team: default)
----------------------------------------
Listeners:  1
Clusters:   1
Filters:    0
```

Look up a specific listener:

```
$ flowplane status demo-listener
Listener: demo-listener
----------------------------------------
Team:     default
Address:  0.0.0.0
Port:     10001
Protocol: HTTP
```

### `doctor`

Run diagnostic health checks against the control plane and Envoy.

```
flowplane doctor
```

**Example:**

```
$ flowplane doctor
Flowplane Doctor
----------------------------------------
[ok]    Control plane health: ok
[ok]    Envoy proxy: ready
```

Possible check results:

| Result | Meaning |
|--------|---------|
| `[ok]` | Component is healthy |
| `[warn]` | Component responded but reported degraded health |
| `[fail]` | Component is unreachable |
| `[skip]` | Check skipped (e.g., Envoy check skipped for remote servers) |

### `list`

List exposed services. Strips the `-listener` suffix from listener names for a cleaner display.

```
flowplane list
```

**Examples:**

With exposed services:

```
$ flowplane list

Name                           Port     Protocol
-------------------------------------------------------
demo                           10001    HTTP
```

With no exposed services:

```
$ flowplane list
No exposed services found
```

### `logs`

View local dev stack logs. Only works for stacks started with `flowplane init`.

```
flowplane logs [OPTIONS]
```

**Flags:**

| Flag | Description |
|------|-------------|
| `-f, --follow` | Follow log output (stream continuously) |

**Examples:**

```
$ flowplane logs
# Show current logs and exit

$ flowplane logs -f
# Stream logs continuously (Ctrl+C to stop)
```

### `database`

Database management commands for migrations and schema validation.

```
flowplane database <COMMAND>
```

**Subcommands:**

| Command | Description |
|---------|-------------|
| `migrate` | Run pending migrations |
| `status` | Show migration status |
| `list` | List all applied migrations |
| `validate` | Validate database schema |

These commands require a `--database-url` flag or `DATABASE_URL` environment variable pointing to your PostgreSQL instance. They are primarily used for production deployments — `flowplane init` handles migrations automatically.

---

## Expose / Unexpose

High-level commands that create or remove an entire service exposure (cluster + route config + listener) in a single step.

### `expose`

Expose a local or Docker-network service through the Envoy gateway.

```
flowplane expose [OPTIONS] <UPSTREAM>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<UPSTREAM>` | Upstream URL to expose (e.g., `http://localhost:3000`) |

**Flags:**

| Flag | Description |
|------|-------------|
| `--name <NAME>` | Name for the exposed service (auto-generated from the URL if omitted) |
| `--path <PATH>` | Path prefix to route (can be specified multiple times) |
| `--port <PORT>` | Listener port override (auto-assigned from 10001-10020 if omitted) |

Behind the scenes, `expose` creates three resources atomically: a cluster pointing at the upstream, a route config with the path prefix(es), and a listener on the assigned port.

**Examples:**

```
$ flowplane expose http://httpbin:80 --name demo
Exposed 'demo' -> http://httpbin:80
  Port:   10001
  Paths:  /

  curl http://localhost:10001/
```

With a custom path and port:

```
$ flowplane expose http://httpbin:80 --name demo-api --path /api --port 10002
Exposed 'demo-api' -> http://httpbin:80
  Port:   10002
  Paths:  /api

  curl http://localhost:10002/
```

Auto-generated names strip the scheme and replace non-alphanumeric characters with hyphens (e.g., `http://localhost:3000` becomes `localhost-3000`).

**Validation errors:**

```
$ flowplane expose http://httpbin:80 --name bad-port --port 9999
Error: Port 9999 is outside the valid range 10001-10020
```

### `unexpose`

Remove an exposed service. Tears down the listener, route config, and cluster created by `expose`.

```
flowplane unexpose [OPTIONS] <NAME>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<NAME>` | Name of the exposed service to remove |

**Examples:**

```
$ flowplane unexpose demo
Removed exposed service 'demo'
```

Attempting to unexpose a service that doesn't exist:

```
$ flowplane unexpose nonexistent
nonexistent is not currently exposed
```

---

## Resource CRUD

Low-level commands for managing individual gateway resources. Use these when you need fine-grained control beyond what `expose` provides.

All resource commands share these patterns:

- **Create/Update** take a JSON file via `-f, --file <FILE>`
- **Output format** via `-o, --output <FORMAT>` (json, yaml, or table)
- **Delete** prompts for confirmation unless `-y, --yes` is passed
- **List** supports `--limit` and `--offset` for pagination
- JSON payloads use **camelCase** field names

### `cluster`

Manage upstream service clusters (endpoint groups).

```
flowplane cluster <create|list|get|update|delete> [OPTIONS]
```

#### `cluster create`

```
flowplane cluster create --file <FILE> [-o <FORMAT>]
```

Minimal cluster JSON:

```json
{
  "name": "my-backend",
  "serviceName": "my-backend",
  "endpoints": [
    { "host": "httpbin", "port": 80 }
  ]
}
```

```
$ flowplane cluster create -f cluster.json
{
  "name": "my-backend",
  "team": "default",
  "serviceName": "my-backend",
  "version": null,
  "importId": null,
  "config": {
    "endpoints": [
      {
        "host": "httpbin",
        "port": 80
      }
    ]
  }
}
```

Optional fields for advanced configuration:

| Field | Type | Description |
|-------|------|-------------|
| `connectTimeoutSeconds` | number | Connection timeout (default: 5) |
| `useTls` | boolean | Enable TLS to upstream (default: false) |
| `tlsServerName` | string | SNI server name for TLS |
| `dnsLookupFamily` | string | `AUTO`, `V4_ONLY`, `V6_ONLY`, `V4_PREFERRED`, `ALL` |
| `lbPolicy` | string | `ROUND_ROBIN`, `LEAST_REQUEST`, `RANDOM`, `RING_HASH`, `MAGLEV` |
| `healthChecks` | array | Active health check definitions |
| `circuitBreakers` | object | Circuit breaker thresholds |
| `outlierDetection` | object | Passive outlier detection |
| `protocolType` | string | `HTTP2` or `GRPC` for HTTP/2 upstreams |

#### `cluster list`

```
$ flowplane cluster list
Name                                Team                 Service
-----------------------------------------------------------------------------------------------
my-backend                          default              my-backend
```

Filter by service name: `flowplane cluster list --service my-backend`

#### `cluster get`

```
$ flowplane cluster get my-backend
```

Returns the full cluster JSON including resolved config.

#### `cluster update`

```
flowplane cluster update <NAME> --file <FILE> [-o <FORMAT>]
```

The update JSON must include the `name` field:

```json
{
  "name": "my-backend",
  "serviceName": "my-backend-v2",
  "endpoints": [
    { "host": "httpbin", "port": 80 },
    { "host": "httpbin", "port": 8080 }
  ]
}
```

```
$ flowplane cluster update my-backend -f updated-cluster.json
{
  "name": "my-backend",
  "team": "default",
  "serviceName": "my-backend-v2",
  ...
}
```

#### `cluster delete`

```
$ flowplane cluster delete my-backend -y
Cluster 'my-backend' deleted successfully
```

### `route`

Manage route configurations that define URL path matching and upstream routing.

```
flowplane route <create|list|get|update|delete> [OPTIONS]
```

#### `route create`

```
flowplane route create --file <FILE> [-o <FORMAT>]
```

Route config JSON:

```json
{
  "name": "my-routes",
  "virtualHosts": [
    {
      "name": "default",
      "domains": ["*"],
      "routes": [
        {
          "name": "catch-all",
          "match": { "path": { "type": "prefix", "value": "/" } },
          "action": { "type": "forward", "cluster": "my-backend" }
        }
      ]
    }
  ]
}
```

```
$ flowplane route create -f route.json
{
  "name": "my-routes",
  "team": "default",
  "pathPrefix": "/",
  "clusterTargets": "my-backend",
  "routeOrder": null,
  "config": { ... }
}
```

**Path match types:**

| Type | Example | Description |
|------|---------|-------------|
| `prefix` | `{ "type": "prefix", "value": "/api" }` | Match URL prefix |
| `exact` | `{ "type": "exact", "value": "/health" }` | Match exact path |
| `regex` | `{ "type": "regex", "value": "^/v[0-9]+/.*" }` | Match regex pattern |
| `template` | `{ "type": "template", "template": "/api/v1/users/{user_id}" }` | Match URI template |

**Route action fields:**

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | `forward` (required) |
| `cluster` | string | Target cluster name |
| `timeoutSeconds` | number | Route-level timeout |
| `prefixRewrite` | string | Rewrite matched prefix |
| `retryPolicy` | object | Retry configuration |

Routes can also match on headers and query parameters — see the schema example in `flowplane route create --help`.

#### `route list`

```
$ flowplane route list
Name                           Team            Path Prefix               Cluster Targets
----------------------------------------------------------------------------------------------------
my-routes                      default         /                         my-backend
```

Filter by cluster: `flowplane route list --cluster my-backend`

#### `route get`

```
$ flowplane route get my-routes
```

Returns the full route config JSON including virtual hosts, routes, and per-filter config.

#### `route update`

```
flowplane route update <NAME> --file <FILE> [-o <FORMAT>]
```

Same JSON format as create.

#### `route delete`

```
$ flowplane route delete my-routes -y
Route config 'my-routes' deleted successfully
```

### `listener`

Manage Envoy listeners that accept incoming connections.

```
flowplane listener <create|list|get|update|delete> [OPTIONS]
```

#### `listener create`

```
flowplane listener create --file <FILE> [-o <FORMAT>]
```

Listener JSON:

```json
{
  "name": "my-listener",
  "address": "0.0.0.0",
  "port": 10003,
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

The `dataplaneId` field is required. In dev mode, the default dataplane ID is `dev-dataplane-id`.

Filter types in the `filterChains` array:

| Type | Description |
|------|-------------|
| `httpConnectionManager` | HTTP/1.1 and HTTP/2 connections (most common) |
| `tcpProxy` | Raw TCP proxying to a cluster |

`httpConnectionManager` fields:

| Field | Description |
|-------|-------------|
| `routeConfigName` | Name of a route config to reference |
| `inlineRouteConfig` | Inline route config (alternative to `routeConfigName`) |
| `accessLog` | Access log configuration |
| `tracing` | Distributed tracing configuration |
| `httpFilters` | HTTP filter chain entries |

```
$ flowplane listener create -f listener.json
{
  "name": "my-listener",
  "team": "default",
  "address": "0.0.0.0",
  "port": 10003,
  "protocol": "HTTP",
  "version": 1,
  ...
}
```

#### `listener list`

```
$ flowplane listener list
Name                                Team               Address            Port     Protocol
-----------------------------------------------------------------------------------------------
my-listener                         default            0.0.0.0            10003    HTTP
```

Filter by protocol: `flowplane listener list --protocol HTTP`

#### `listener get`

```
$ flowplane listener get my-listener
```

Returns the full listener JSON including filter chains and HCM configuration.

#### `listener update`

```
flowplane listener update <NAME> --file <FILE> [-o <FORMAT>]
```

Update JSON (no `name` or `dataplaneId` required):

```json
{
  "address": "0.0.0.0",
  "port": 10003,
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

#### `listener delete`

```
$ flowplane listener delete my-listener -y
Listener 'my-listener' deleted successfully
```

### `filter`

Manage HTTP filters (rate limiting, auth, CORS, etc.) and attach/detach them from listeners.

```
flowplane filter <create|list|get|delete|attach|detach> [OPTIONS]
```

#### `filter create`

```
flowplane filter create --file <FILE> [-o <FORMAT>]
```

Filter JSON uses a nested config structure — the outer `config` contains `type` and an inner `config` with the filter-specific settings:

```json
{
  "name": "my-rate-limit",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "http_local_rate_limiter",
      "token_bucket": {
        "max_tokens": 5,
        "tokens_per_fill": 5,
        "fill_interval_ms": 60000
      },
      "filter_enabled": {
        "numerator": 100,
        "denominator": "hundred"
      },
      "filter_enforced": {
        "numerator": 100,
        "denominator": "hundred"
      }
    }
  }
}
```

```
$ flowplane filter create -f filter.json
{
  "id": "1c031903-...",
  "name": "my-rate-limit",
  "filterType": "local_rate_limit",
  "version": 1,
  "source": "native_api",
  "team": "default",
  ...
  "allowedAttachmentPoints": ["route", "listener"]
}
```

See [filters.md](filters.md) for working examples of every filter type.

#### `filter list`

```
$ flowplane filter list
Name                           Type                 Team            Version    Attached
------------------------------------------------------------------------------------------
my-rate-limit                  local_rate_limit     default         1          0
```

#### `filter get`

```
$ flowplane filter get my-rate-limit
```

Returns the full filter JSON including config, attachment count, and allowed attachment points.

#### `filter attach`

Attach a filter to a listener. The filter starts processing traffic immediately.

```
flowplane filter attach <FILTER_NAME> --listener <LISTENER> [--order <N>]
```

| Flag | Description |
|------|-------------|
| `--listener <LISTENER>` | Listener name to attach to (required) |
| `--order <N>` | Execution order — lower numbers execute first |

```
$ flowplane filter attach my-rate-limit --listener demo-listener
Filter 'my-rate-limit' attached to listener 'demo-listener'
```

#### `filter detach`

Remove a filter from a listener.

```
flowplane filter detach <FILTER_NAME> --listener <LISTENER>
```

```
$ flowplane filter detach my-rate-limit --listener demo-listener
Filter 'my-rate-limit' detached from listener 'demo-listener'
```

#### `filter delete`

```
$ flowplane filter delete my-rate-limit -y
Filter 'my-rate-limit' deleted successfully
```

A filter must be detached from all listeners before it can be deleted.
