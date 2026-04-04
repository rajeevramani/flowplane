# CLI Reference

Complete reference for the `flowplane` CLI. For a guided walkthrough, see [Getting Started](getting-started.md).

## Global Flags

Every subcommand accepts these flags:

| Flag | Purpose |
|---|---|
| `--token <TOKEN>` | Bearer token for API auth |
| `--token-file <PATH>` | Read token from file |
| `--base-url <URL>` | API base URL (default: `http://localhost:8080`) |
| `--timeout <SECS>` | Request timeout in seconds |
| `--team <NAME>` | Team context for resource commands |
| `--database-url <URL>` | Database URL override |
| `-v, --verbose` | Verbose logging (`serve` only) |

Most `create`, `list`, and `get` subcommands accept `-o, --output <FMT>` (`json`, `yaml`, `table`). Default is `json` for create/get, `table` for list.

---

## Stack Management

### flowplane init

Bootstrap a local dev environment with Docker/Podman.

```
flowplane init [--with-envoy] [--with-httpbin]
```

| Flag | Effect |
|---|---|
| `--with-envoy` | Start an Envoy proxy (ports 10000-10020) |
| `--with-httpbin` | Start httpbin test backend (port 8000) |

Generates a dev token, starts PostgreSQL + control plane, waits for health, saves credentials to `~/.flowplane/credentials`. Auto-seeds org `dev-org`, team `default`, user `dev@flowplane.local`, and dataplane `dev-dataplane`.

```bash
flowplane init --with-envoy --with-httpbin
```

### flowplane down

Stop the dev stack.

```
flowplane down [--volumes]
```

| Flag | Effect |
|---|---|
| `--volumes` | Remove persistent volumes (deletes database) |

```bash
flowplane down --volumes
```

### flowplane serve

Start the control plane directly (outside Docker).

```
flowplane serve [--dev]
```

| Flag | Effect |
|---|---|
| `--dev` | Dev mode: opaque token auth, auto-seed, no Zitadel |

```bash
flowplane serve --dev
```

### flowplane status

Show system overview or look up a specific listener.

```
flowplane status [<NAME>]
```

```bash
flowplane status
flowplane status demo-listener
```

### flowplane doctor

Run diagnostic health checks.

```
flowplane doctor
```

### flowplane logs

View dev stack container logs.

```
flowplane logs [-f|--follow]
```

```bash
flowplane logs -f
```

### flowplane list

List all exposed services.

```
flowplane list
```

```bash
flowplane list
# Name                           Port     Protocol
# -------------------------------------------------------
# demo                           10001    HTTP
```

---

## Authentication

### flowplane auth login

Authenticate with the control plane. In dev mode this is a no-op. In prod mode, opens a browser-based PKCE flow.

```
flowplane auth login [--device-code] [--callback-url <URL>] [--issuer <URL>] [--client-id <ID>]
```

| Flag | Effect |
|---|---|
| `--device-code` | Use device code flow instead of browser PKCE |
| `--callback-url <URL>` | Override the PKCE callback URL |
| `--issuer <URL>` | OIDC issuer URL (overrides config) |
| `--client-id <ID>` | OIDC client ID (overrides config) |

```bash
flowplane auth login
flowplane auth login --device-code
```

### flowplane auth token

Print the current access token to stdout.

```
flowplane auth token
```

### flowplane auth whoami

Show the authenticated identity, org, and team. In dev mode, shows the token type and truncated value (no real identity exists). In prod mode, shows user email, org, and team from JWT claims.

```
flowplane auth whoami
```

### flowplane auth logout

Clear stored credentials.

```
flowplane auth logout
```

---

## Quick Actions

### flowplane expose

Expose an upstream service through the gateway in one command. Creates a cluster, route config, and listener automatically.

```
flowplane expose <UPSTREAM> [--name <NAME>] [--path <PATH>]... [--port <PORT>]
```

| Flag | Effect |
|---|---|
| `--name <NAME>` | Service name. If omitted, derived from upstream (e.g., `http://httpbin:80` -> `httpbin-80`) |
| `--path <PATH>` | Path prefix to route (repeatable). Default: `/` |
| `--port <PORT>` | Listener port. Must be 10001-10020. Auto-assigned if omitted |

**Naming convention:** cluster = `<name>`, route config = `<name>-routes`, listener = `<name>-listener`.

**Behavior:**
- Idempotent: same name + same upstream returns 200 with existing port
- Different upstream + same name: 409 Conflict
- Port collision: 409
- Port out of range: 400

```bash
flowplane expose http://httpbin:80 --name demo
flowplane expose http://httpbin:80 --name demo --path /get --path /post --port 10001
```

> ⚠️ Use Docker service hostnames (e.g., `httpbin`), not `localhost`. Inside the Docker network, `localhost` refers to the control plane container.

### flowplane unexpose

Remove an exposed service. Deletes the cluster, route config, and listener by naming convention.

```
flowplane unexpose <NAME>
```

Returns 404 if none of the three resources existed.

```bash
flowplane unexpose demo
```

---

## Resources

All resource types follow a consistent CRUD pattern:

```
flowplane <resource> create -f <FILE> [-o json|yaml|table]
flowplane <resource> list [--limit N] [--offset N] [-o json|yaml|table]
flowplane <resource> get <NAME> [-o json|yaml|table]
flowplane <resource> update <NAME> -f <FILE> [-o json|yaml|table]
flowplane <resource> delete <NAME> [--yes]
```

### Clusters

Manage upstream service clusters (backend endpoints + load balancing).

**List filter:** `--service <NAME>` — filter by service name.

#### Create payload

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

#### Circuit breakers

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

#### Outlier detection

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

#### Examples

```bash
flowplane cluster create -f cluster.json
flowplane cluster list --service payments -o table
flowplane cluster get my-backend -o json
flowplane cluster update my-backend -f updated-cluster.json
flowplane cluster delete my-backend --yes
```

### Listeners

Manage Envoy listeners (entry points for traffic).

**List filter:** `--protocol <PROTO>` — filter by protocol.

> For most use cases, `flowplane expose` is simpler than manual listener creation.

#### Create payload

```json
{
  "name": "my-listener",
  "address": "0.0.0.0",
  "port": 10001,
  "protocol": "HTTP",
  "dataplaneId": "<from dataplane list>",
  "routeConfigName": "my-routes"
}
```

| Field | Required | Description |
|---|---|---|
| `name` | Yes | Unique listener name |
| `address` | No | Bind address (default: `0.0.0.0`) |
| `port` | Yes | Port in 10000-10020 range |
| `protocol` | No | `HTTP` (default), `HTTPS`, `TCP` |
| `dataplaneId` | Yes | UUID of the target dataplane |
| `routeConfigName` | No | Route config to bind to this listener |

> ⚠️ Port range: Envoy exposes ports 10000-10020. Listener ports outside this range work in the DB but Envoy won't serve traffic on them.

#### Examples

```bash
flowplane listener create -f listener.json
flowplane listener list --protocol HTTP -o table
flowplane listener get my-listener -o json
flowplane listener update my-listener -f updated-listener.json
flowplane listener delete my-listener --yes
```

### Route Configs

Manage routing rules (path matching -> cluster forwarding).

**List filter:** `--cluster <NAME>` — filter by cluster name.

#### Create payload

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

#### Route with retry policy

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

#### Weighted routing (blue/green)

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

#### Examples

```bash
flowplane route create -f routes.json
flowplane route list --cluster my-backend -o table
flowplane route get my-routes -o json
flowplane route update my-routes -f updated-routes.json
flowplane route delete my-routes --yes
```

> ⚠️ `update` performs a **full replacement** of the `virtualHosts` array. Fetch existing config first, then include all routes plus changes.

### Teams

Manage teams (prod mode). Requires `--org` flag.

#### Create payload

```json
{
  "name": "engineering",
  "display_name": "Engineering Team"
}
```

#### Examples

```bash
flowplane team create --org acme-corp -f team.json
flowplane team list --org acme-corp -o table
flowplane team list --admin                     # Platform admin: all teams across orgs
flowplane team get --org acme-corp engineering
flowplane team update --org acme-corp engineering -f updated-team.json
flowplane team delete --org acme-corp engineering --yes
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

Available filter types: `local_rate_limit`, `rate_limit`, `jwt_auth`, `oauth2`, `ext_authz`, `rbac`, `cors`, `header_mutation`, `custom_response`, `compressor`, `mcp`.

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
flowplane filter create -f ratelimit.json
```

### flowplane filter list

```
flowplane filter list [--limit N] [--offset N] [-o json|yaml|table]
```

```bash
flowplane filter list -o table
```

### flowplane filter get

```
flowplane filter get <NAME> [-o json|yaml|table]
```

```bash
flowplane filter get my-rate-limit -o json
```

### flowplane filter delete

```
flowplane filter delete <NAME> [--yes]
```

```bash
flowplane filter delete my-rate-limit --yes
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

```bash
flowplane filter detach my-rate-limit --listener demo-listener
```

---

## API Management

### flowplane import openapi

Import routes from an OpenAPI spec file. Creates cluster, route config, and listener.

```
flowplane import openapi <FILE> [--name <NAME>] [--port <PORT>]
```

| Flag | Effect |
|---|---|
| `--name <NAME>` | Service name. If omitted, derived from spec `info.title` |
| `--port <PORT>` | Listener port (default: 10000) |

**Requirements:** Spec must have a `servers` block and valid operations with `responses`.

**Behavior:**
- Idempotent on spec name: re-importing deletes previous and recreates
- Port collision: 409 Conflict

```bash
flowplane import openapi ./petstore.yaml --name petstore --port 10002
```

### flowplane learn start

Start a learning session to record API traffic and infer schemas.

```
flowplane learn start --route-pattern <REGEX> --target-sample-count <N> \
  [--cluster-name <NAME>] [--http-methods <M>...] \
  [--max-duration-seconds <N>] [--triggered-by <WHO>] \
  [--deployment-version <VER>] [-o json|yaml|table]
```

| Flag | Required | Description |
|---|---|---|
| `--route-pattern` | Yes | Regex to match request paths |
| `--target-sample-count` | Yes | Number of samples to collect |
| `--cluster-name` | No | Filter to a specific cluster |
| `--http-methods` | No | Filter by HTTP methods (space-separated) |
| `--max-duration-seconds` | No | Max session duration |
| `--triggered-by` | No | Label for who/what triggered the session |
| `--deployment-version` | No | Version label |

Sessions auto-activate and begin collecting immediately. They can collect more samples than the target.

```bash
flowplane learn start --route-pattern '^/api/.*' --target-sample-count 50
flowplane learn start --route-pattern '^/get.*' --target-sample-count 100 \
  --cluster-name my-cluster --http-methods GET POST
```

### flowplane learn list

```
flowplane learn list [--status <STATUS>] [--limit N] [--offset N] [-o json|yaml|table]
```

Status values: `pending`, `active`, `completing`, `completed`, `failed`, `cancelled`.

```bash
flowplane learn list -o table
flowplane learn list --status active
```

### flowplane learn get

```
flowplane learn get <SESSION_ID> [-o json|yaml|table]
```

```bash
flowplane learn get <session-id> -o json
```

### flowplane learn cancel

```
flowplane learn cancel <SESSION_ID> [--yes]
```

> ⚠️ In non-interactive mode (piped stdin), cancel without `--yes` silently does nothing. Always use `--yes` in scripts.

```bash
flowplane learn cancel <session-id> --yes
```

---

## Secrets

> ⚠️ Secret commands are available via the REST API (`/api/v1/teams/{team}/secrets`) and MCP tools (`cp_create_secret`, `cp_list_secrets`, `cp_get_secret`, `cp_delete_secret`). See the [Getting Started](getting-started.md) guide for the REST and MCP interfaces. CLI subcommands for secrets are in development.

---

## Configuration

### flowplane config show

Display current configuration from `~/.flowplane/config.toml`.

```
flowplane config show [-o json|yaml|table]
```

Default output is `yaml`.

```bash
flowplane config show
flowplane config show -o json
```

### flowplane config set

Set a configuration value.

```
flowplane config set <KEY> <VALUE>
```

| Key | Description |
|---|---|
| `token` | API authentication token |
| `base_url` | API base URL |
| `timeout` | Request timeout in seconds |
| `team` | Default team context |
| `org` | Default organization context |
| `oidc_issuer` | OIDC issuer URL (prod mode) |
| `oidc_client_id` | OIDC client ID (prod mode) |
| `callback_url` | PKCE callback URL (prod mode) |

```bash
flowplane config set team engineering
flowplane config set base_url http://flowplane.internal:8080
```

### flowplane config init

Create a default configuration file at `~/.flowplane/config.toml`.

```
flowplane config init [--force]
```

| Flag | Effect |
|---|---|
| `--force` | Overwrite existing config file |

```bash
flowplane config init
```

### flowplane config path

Print the config file path.

```
flowplane config path
```

---

## Database

Admin commands for managing the PostgreSQL schema. These connect directly to the database, not through the API.

### flowplane database migrate

Run pending migrations.

```
flowplane database migrate [--dry-run]
```

```bash
flowplane database migrate --dry-run
```

### flowplane database status

Show migration status.

```
flowplane database status
```

### flowplane database list

List all applied migrations.

```
flowplane database list
```

### flowplane database validate

Validate database schema integrity.

```
flowplane database validate
```

---

## Gotchas

> ⚠️ **Upstream URLs in `expose`:** Use Docker service hostnames (e.g., `httpbin`), not `localhost`. The proxy runs inside Docker.

> ⚠️ **Port range:** Envoy serves traffic on ports 10000-10020. The `expose` auto-assign pool is 10001-10020.

> ⚠️ **JSON spec format:** All `-f FILE` flags expect JSON matching the REST API body. See payload examples above.

> ⚠️ **Output formats:** `-o table` for humans, `-o json` for scripting.

> ⚠️ **Team context:** Most resource commands need `--team`. Set a default: `flowplane config set team <NAME>`. In dev mode, `flowplane init` sets this to `default` automatically.

> ⚠️ **Filter config nesting:** Filters use `"config": {"type": "<filterType>", "config": {...}}` — not a flat structure. The inner `type` must match the `filterType`.

> ⚠️ **Route config update is full replacement:** `flowplane route update` replaces the entire `virtualHosts` array. Always fetch with `flowplane route get` first.
