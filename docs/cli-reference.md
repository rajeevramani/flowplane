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

---

## Import OpenAPI

Import API definitions from OpenAPI specification files. Creates clusters, routes, and listeners from the spec automatically.

### `import openapi`

```
flowplane import openapi [OPTIONS] <FILE>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<FILE>` | Path to the OpenAPI spec file (YAML or JSON) |

**Flags:**

| Flag | Description |
|------|-------------|
| `--name <NAME>` | Name for the imported service (derived from spec title if omitted) |
| `--port <PORT>` | Port for the listener (default: auto-assigned) |

The import process:
1. Reads the OpenAPI spec (YAML or JSON, detected by file extension)
2. Resolves the default dataplane for the team
3. Creates clusters from `servers` entries
4. Creates route configs from `paths`
5. Creates a listener to receive traffic

**Examples:**

```
$ flowplane import openapi petstore.yaml
Imported 'Petstore'
  Version:          1.0.0
  Import ID:        d507c462-43d9-4a76-8b6b-fd9c098c353c
  Routes created:   1
  Clusters created: 1
  Clusters reused:  0
  Listener:         petstore-listener
```

With a custom name and port:

```
$ flowplane import openapi petstore.yaml --name my-petstore --port 10005
Imported 'Petstore'
  Version:          1.0.0
  Import ID:        ebda20e2-2975-4c45-866c-7a19bbbddec7
  Routes created:   1
  Clusters created: 1
  Clusters reused:  0
  Listener:         my-petstore-listener
```

Reimporting the same spec creates a new import record (idempotent — reuses clusters when endpoints match).

**Error cases:**

```
$ flowplane import openapi /tmp/nonexistent.yaml
Error: Failed to read file '/tmp/nonexistent.yaml': No such file or directory (os error 2)
```

---

## Learning Sessions

Learning sessions observe live API traffic matching a route pattern, collect request/response samples, and automatically generate OpenAPI schemas when the target sample count is reached.

### Session Lifecycle

| Status | Description |
|--------|-------------|
| `active` | Collecting traffic samples |
| `completing` | Target reached, generating schema |
| `completed` | Schema generation finished |
| `cancelled` | Manually cancelled |
| `failed` | Encountered an error |

### `learn start`

Start a new learning session.

```
flowplane learn start [OPTIONS] --route-pattern <PATTERN> --target-sample-count <N>
```

**Required flags:**

| Flag | Description |
|------|-------------|
| `--route-pattern <PATTERN>` | Regex pattern to match routes (e.g., `'^/api/v1/.*'`) |
| `--target-sample-count <N>` | Number of traffic samples to collect (1-100000) |

**Optional flags:**

| Flag | Description |
|------|-------------|
| `--cluster-name <NAME>` | Filter traffic to a specific cluster |
| `--http-methods <METHODS>...` | HTTP methods to include (e.g., `GET POST PUT`) |
| `--max-duration-seconds <N>` | Maximum session duration in seconds |
| `--triggered-by <TAG>` | Tag identifying who/what started this session |
| `--deployment-version <VER>` | Deployment version being learned |
| `-o, --output <FORMAT>` | Output format: `json` (default), `yaml`, `table` |

**Examples:**

Basic session — learn all traffic matching `/get`:

```
$ flowplane learn start --route-pattern '^/get.*' --target-sample-count 50
{
  "id": "59972969-e418-491a-9061-aea3df9446e2",
  "team": "dev-default-team-id",
  "routePattern": "^/get.*",
  "clusterName": null,
  "httpMethods": null,
  "status": "active",
  "createdAt": "2026-04-03T12:10:26.333572+00:00",
  "startedAt": "2026-04-03T12:10:26.344478+00:00",
  "endsAt": null,
  "completedAt": null,
  "targetSampleCount": 50,
  "currentSampleCount": 0,
  "progressPercentage": 0.0,
  "triggeredBy": null,
  "deploymentVersion": null,
  "errorMessage": null
}
```

With cluster filter, HTTP methods, and trigger tag:

```
$ flowplane learn start \
    --route-pattern '^/api/v1/users/.*' \
    --cluster-name demo \
    --http-methods GET POST \
    --target-sample-count 100 \
    --triggered-by 'deploy-v2.3'
```

**Sample count guidance:**

| Use case | Recommended count |
|----------|------------------|
| Simple CRUD APIs | 10–50 |
| Complex APIs with varied responses | 100–500 |
| High-variance endpoints | 500+ |

### `learn list`

List learning sessions with optional filtering.

```
flowplane learn list [OPTIONS]
```

**Flags:**

| Flag | Description |
|------|-------------|
| `--status <STATUS>` | Filter by status: `pending`, `active`, `completing`, `completed`, `failed`, `cancelled` |
| `--limit <N>` | Maximum number of results |
| `--offset <N>` | Pagination offset |
| `-o, --output <FORMAT>` | Output format: `table` (default), `json`, `yaml` |

**Examples:**

```
$ flowplane learn list
ID                                     Status       Route Pattern                  Samples  Target   Progress
--------------------------------------------------------------------------------------------------------------
59972969-e418-491a-9061-aea3df9446e2   active       ^/get.*                        0        50       0.0%
```

Filter by status:

```
$ flowplane learn list --status active
```

JSON output:

```
$ flowplane learn list --output json
```

No sessions:

```
$ flowplane learn list
No learning sessions found
```

### `learn get`

Get detailed information about a learning session.

```
flowplane learn get [OPTIONS] <SESSION_ID>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<SESSION_ID>` | Session UUID to retrieve |

**Flags:**

| Flag | Description |
|------|-------------|
| `-o, --output <FORMAT>` | Output format: `json` (default), `yaml`, `table` |

**Examples:**

```
$ flowplane learn get 59972969-e418-491a-9061-aea3df9446e2
{
  "id": "59972969-e418-491a-9061-aea3df9446e2",
  "team": "dev-default-team-id",
  "routePattern": "^/get.*",
  "status": "active",
  "targetSampleCount": 50,
  "currentSampleCount": 0,
  "progressPercentage": 0.0,
  ...
}
```

Table format:

```
$ flowplane learn get 59972969-e418-491a-9061-aea3df9446e2 --output table
ID                                     Status       Route Pattern                  Samples  Target   Progress
--------------------------------------------------------------------------------------------------------------
59972969-e418-491a-9061-aea3df9446e2   active       ^/get.*                        0        50       0.0%
```

### `learn cancel`

Cancel an active or pending learning session. Completed, failed, and already-cancelled sessions cannot be cancelled.

```
flowplane learn cancel [OPTIONS] <SESSION_ID>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<SESSION_ID>` | Session UUID to cancel |

**Flags:**

| Flag | Description |
|------|-------------|
| `-y, --yes` | Skip confirmation prompt |

**Examples:**

```
$ flowplane learn cancel 59972969-e418-491a-9061-aea3df9446e2 -y
Learning session '59972969-e418-491a-9061-aea3df9446e2' cancelled successfully
```

---

## Secrets

Manage Envoy SDS (Secret Discovery Service) secrets — API keys, TLS certificates, and other credentials delivered to Envoy at runtime.

### `secret create`

Create a new secret for Envoy SDS.

```
flowplane secret create --name <NAME> --type <TYPE> --config '<JSON>' [OPTIONS]
```

**Flags:**

| Flag | Description |
|------|-------------|
| `--name <NAME>` | Secret name (must be unique within the team) |
| `--type <TYPE>` | Secret type (see table below) |
| `--config <JSON>` | Secret configuration as JSON string |
| `--description <TEXT>` | Optional description |
| `--expires-at <ISO8601>` | Optional expiration time (e.g., `2027-01-01T00:00:00Z`) |

**Secret types:**

| Type | Description |
|------|-------------|
| `generic_secret` | API keys, bearer tokens, or other opaque secrets |
| `tls_certificate` | TLS certificate + private key pair |
| `certificate_validation_context` | CA certificates for upstream validation |
| `session_ticket_keys` | TLS session ticket keys |

> **Note:** The `--config` JSON must include a `"type"` field matching the `--type` flag. The CLI does not auto-inject it.

**Examples:**

```
$ flowplane secret create --name my-api-key --type generic_secret \
    --config '{"type": "generic_secret", "secret": "dGVzdC1zZWNyZXQ="}' \
    --description 'API key for upstream service'
{
  "id": "ce457dcd-668a-4712-85fa-ccc364c47d60",
  "name": "my-api-key",
  "secretType": "generic_secret",
  "description": "API key for upstream service",
  "version": 1,
  "source": "native_api",
  "team": "default",
  "createdAt": "2026-04-03T12:13:53.667030Z",
  "updatedAt": "2026-04-03T12:13:53.667030Z",
  "expiresAt": null,
  ...
}
```

TLS certificate example:

```
flowplane secret create --name my-tls-cert --type tls_certificate \
    --config '{"type": "tls_certificate", "certificate_chain": "...", "private_key": "..."}'
```

### `secret list`

List secrets for the current team.

```
flowplane secret list [OPTIONS]
```

**Flags:**

| Flag | Description |
|------|-------------|
| `--type <TYPE>` | Filter by secret type |
| `--limit <N>` | Maximum number of results |
| `--offset <N>` | Offset for pagination |
| `-o, --output <FORMAT>` | Output format: `table` (default), `json`, `yaml` |

**Examples:**

```
$ flowplane secret list
ID                                     Name                      Type                           Source     Version  Expires
-----------------------------------------------------------------------------------------------------------------------------
ce457dcd-668a-4712-85fa-ccc364c47d60   my-api-key                generic_secret                 nativ...   1        never
```

JSON format:

```
$ flowplane secret list -o json
[
  {
    "id": "ce457dcd-668a-4712-85fa-ccc364c47d60",
    "name": "my-api-key",
    "secretType": "generic_secret",
    "description": "API key for upstream service",
    "version": 1,
    "source": "native_api",
    "team": "default",
    ...
  }
]
```

Filter by type:

```
$ flowplane secret list --type tls_certificate
No secrets found
```

### `secret get`

Get details of a specific secret by ID. Secret values are never returned in plaintext.

```
flowplane secret get <SECRET_ID> [OPTIONS]
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<SECRET_ID>` | Secret UUID to retrieve |

**Flags:**

| Flag | Description |
|------|-------------|
| `-o, --output <FORMAT>` | Output format: `json` (default), `yaml`, `table` |

**Examples:**

```
$ flowplane secret get ce457dcd-668a-4712-85fa-ccc364c47d60
{
  "id": "ce457dcd-668a-4712-85fa-ccc364c47d60",
  "name": "my-api-key",
  "secretType": "generic_secret",
  "description": "API key for upstream service",
  "version": 1,
  "source": "native_api",
  "team": "default",
  "createdAt": "2026-04-03T12:13:53.667030Z",
  "updatedAt": "2026-04-03T12:13:53.667030Z",
  "expiresAt": null,
  ...
}
```

### `secret delete`

Delete a secret by ID. Prompts for confirmation unless `--yes` is provided.

```
flowplane secret delete <SECRET_ID> [OPTIONS]
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<SECRET_ID>` | Secret UUID to delete |

**Flags:**

| Flag | Description |
|------|-------------|
| `-y, --yes` | Skip confirmation prompt |

**Examples:**

```
$ flowplane secret delete ce457dcd-668a-4712-85fa-ccc364c47d60 --yes
Secret 'ce457dcd-668a-4712-85fa-ccc364c47d60' deleted successfully
```

---

## Authentication

Manage login credentials and identity. Supports two modes:

- **Dev mode**: Uses a dev token generated by `flowplane init` — no OIDC needed
- **Prod mode**: Uses OIDC Authorization Code with PKCE (browser) or device code flow (headless)

### `auth login`

Log in to Flowplane. In dev mode, shows the existing dev token. In prod mode, opens a browser for OIDC PKCE authentication.

```
flowplane auth login [OPTIONS]
```

**Flags:**

| Flag | Description |
|------|-------------|
| `--device-code` | Use device code flow instead of browser-based PKCE |
| `--callback-url <URL>` | Override the callback URL for PKCE flow |
| `--issuer <URL>` | OIDC issuer URL (overrides config/env) |
| `--client-id <ID>` | OIDC client ID (overrides config/env) |

**Examples:**

Dev mode:

```
$ flowplane auth login
Control plane is running in dev mode.
No OIDC login needed — use the dev token from 'flowplane init'.

Current token: OA26XHab7Ge_kN0WBUG-
(stored in ~/.flowplane/credentials)
```

Prod mode (opens browser):

```
$ flowplane auth login
Opening browser for authentication...

If the browser doesn't open, visit this URL:
  https://zitadel.example.com/oauth/v2/authorize?response_type=code&...

Login successful! Credentials saved to ~/.flowplane/credentials
```

Headless environments:

```
flowplane auth login --device-code
```

### `auth token`

Print the current access token to stdout. Automatically refreshes expired OIDC tokens if a refresh token is available.

```
flowplane auth token
```

**Examples:**

```
$ flowplane auth token
OA26XHab7Ge_kN0WBUG-7Mv3plK2jYGcd7J-o7NPf24
```

Useful for piping into other tools:

```
curl -H "Authorization: Bearer $(flowplane auth token)" http://localhost:8080/api/v1/...
```

### `auth whoami`

Show the current authenticated user. In dev mode, displays the token type. In prod mode, decodes the JWT and shows email, name, issuer, and expiration.

```
flowplane auth whoami
```

**Examples:**

Dev mode (opaque token):

```
$ flowplane auth whoami
Token type: opaque (dev mode token)
Token:      OA26XHab7Ge_kN0WBUG-...
```

Prod mode (JWT):

```
$ flowplane auth whoami
Subject:  366579855535374341
Email:    admin@example.com
Name:     Admin User
Issuer:   http://localhost:8081
Expires:  2026-04-04 12:00:00 UTC (11h 30m remaining)
```

### `auth logout`

Clear stored credentials and log out.

```
flowplane auth logout
```

**Examples:**

```
$ flowplane auth logout
Logged out. Credentials removed.
```

Credentials are stored at `~/.flowplane/credentials`. Logout deletes this file.

---

## Configuration

Manage CLI configuration stored in `~/.flowplane/config.toml`.

### Token Resolution Order

The CLI resolves the authentication token from multiple sources in this priority order:

1. `--token` command line flag
2. `--token-file` command line flag
3. `~/.flowplane/credentials` file (OIDC JSON or plain-text)
4. `~/.flowplane/config.toml` `token` field (deprecated)
5. `FLOWPLANE_TOKEN` environment variable

### `config init`

Create a new configuration file with default values.

```
flowplane config init [OPTIONS]
```

**Flags:**

| Flag | Description |
|------|-------------|
| `-f, --force` | Overwrite existing configuration file |

**Examples:**

```
$ flowplane config init
✅ Configuration file created at: /Users/you/.flowplane/config.toml

$ flowplane config init
Error: Configuration file already exists at: /Users/you/.flowplane/config.toml
Use --force to overwrite
```

### `config show`

Display current configuration.

```
flowplane config show [OPTIONS]
```

**Flags:**

| Flag | Description |
|------|-------------|
| `-o, --output <FORMAT>` | Output format: `yaml` (default), `json`, `table` |

**Examples:**

```
$ flowplane config show
token: <your-token>
base_url: http://localhost:8080
timeout: 60
team: default
org: dev-org
oidc_issuer: http://localhost:8081
oidc_client_id: '366579823054749701'
```

Table format:

```
$ flowplane config show -o table

Key             Value
-----------------------------------------------------------------
token           eyJhbGciOi...
base_url        http://localhost:8080
timeout         60 seconds
team            default
org             dev-org

Config file: /Users/you/.flowplane/config.toml
```

### `config set`

Set a configuration value.

```
flowplane config set <KEY> <VALUE>
```

**Supported keys:**

| Key | Description |
|-----|-------------|
| `token` | API authentication token |
| `base_url` | Flowplane API base URL |
| `timeout` | Request timeout in seconds |
| `team` | Default team context |
| `org` | Default organization context |
| `oidc_issuer` | OIDC issuer URL |
| `oidc_client_id` | OIDC client ID |
| `callback_url` | Callback URL for OIDC PKCE login |

**Examples:**

```
$ flowplane config set team engineering
✅ Team set to: engineering
Configuration saved to: /Users/you/.flowplane/config.toml

$ flowplane config set base_url https://api.flowplane.io
✅ Base URL set to: https://api.flowplane.io
Configuration saved to: /Users/you/.flowplane/config.toml

$ flowplane config set timeout 120
✅ Timeout set to: 120 seconds
Configuration saved to: /Users/you/.flowplane/config.toml
```

### `config path`

Display the path to the configuration file.

```
flowplane config path
```

**Examples:**

```
$ flowplane config path
/Users/you/.flowplane/config.toml
```
