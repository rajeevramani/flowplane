# CLI Reference

Complete reference for the `flowplane` CLI. For a guided walkthrough, see [Getting Started](getting-started.md).

## Global Flags

| Flag | Purpose |
|---|---|
| `--token <TOKEN>` | Bearer token for API auth |
| `--token-file <PATH>` | Read token from file |
| `--base-url <URL>` | API base URL (default: `http://localhost:8080`) |
| `--timeout <SECS>` | Request timeout in seconds |
| `--team <NAME>` | Team context for resource commands |
| `-v, --verbose` | Verbose logging (`serve` only) |

Most `create`, `list`, and `get` subcommands also accept `-o, --output <FMT>` for output format (`json`, `yaml`, `table`).

## Stack Management

### flowplane init

Bootstrap a local dev environment with Docker Compose.

```
flowplane init [--with-envoy] [--with-httpbin]
```

```sh
flowplane init --with-envoy --with-httpbin
```

### flowplane down

Stop the dev environment and optionally remove volumes.

```
flowplane down [--volumes]
```

```sh
flowplane down --volumes
```

### flowplane serve

Start the control plane directly (outside Docker).

```
flowplane serve [--dev] [-v]
```

```sh
flowplane serve --dev -v
```

### flowplane status

Show system overview, or look up a specific listener by name.

```
flowplane status [NAME]
```

```sh
flowplane status my-listener
```

### flowplane doctor

Run diagnostic health checks against the running stack.

```
flowplane doctor
```

### flowplane logs

View dev stack logs, optionally following in real time.

```
flowplane logs [-f]
```

```sh
flowplane logs -f
```

### flowplane list

List all exposed services.

```
flowplane list
```

## Authentication

### flowplane auth login

Authenticate with the control plane. Supports browser-based OIDC and device-code flows.

```
flowplane auth login [--device-code] [--callback-url URL] [--issuer URL]
```

```sh
flowplane auth login --device-code
```

### flowplane auth token

Print the current access token to stdout.

```
flowplane auth token
```

### flowplane auth whoami

Display the authenticated identity and roles.

```
flowplane auth whoami
```

### flowplane auth logout

Clear stored credentials.

```
flowplane auth logout
```

## Resources

All resource types (cluster, listener, route, team) follow a consistent CRUD pattern:

```
flowplane <resource> create -f <FILE> [-o json|yaml|table]
flowplane <resource> list [--limit N] [--offset N] [-o json|yaml|table]
flowplane <resource> get <NAME> [-o json|yaml|table]
flowplane <resource> update -f <FILE> <NAME>
flowplane <resource> delete <NAME> [--yes]
```

### Cluster

Manage upstream service clusters.

**List filter:** `--service <SVC>` — filter by service name.

**Create payload:**

```json
{
  "name": "my-backend",
  "endpoints": [{ "host": "backend-svc", "port": 8000 }],
  "lb_policy": "ROUND_ROBIN"
}
```

```sh
flowplane cluster create -f cluster.json --team engineering
```

```sh
flowplane cluster list --service payments --team engineering -o table
```

```sh
flowplane cluster get my-backend --team engineering -o json
```

```sh
flowplane cluster delete my-backend --team engineering --yes
```

### Listener

Manage Envoy listeners.

**List filter:** `--protocol <PROTO>` — filter by protocol.

> For most use cases, `flowplane expose` is easier than manual listener creation (see [Quick Actions](#flowplane-expose)).

```sh
flowplane listener list --protocol HTTP --team engineering -o table
```

> Port range for Envoy listeners is 10000–10020.

### Route

Manage route configurations and virtual hosts.

**List filter:** `--cluster <CLUSTER>` — filter by cluster name.

**Create payload:**

```json
{
  "name": "my-routes",
  "virtualHosts": [
    {
      "name": "default-vhost",
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

```sh
flowplane route create -f routes.json --team engineering
```

```sh
flowplane route list --cluster my-backend --team engineering -o table
```

### Team

Manage teams.

**Create payload:**

```json
{
  "name": "engineering",
  "display_name": "Engineering Team"
}
```

```sh
flowplane team create --org dev-org -f team.json
```

```sh
flowplane team list --org dev-org -o table
```

## Filters

Manage HTTP filters and attach them to listeners. For filter concepts and advanced usage, see [Filters](filters.md).

### flowplane filter create

Create a filter from a JSON spec file.

```
flowplane filter create -f <FILE>
```

**Rate-limit filter payload:**

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

```sh
flowplane filter create -f ratelimit.json --team engineering
```

### flowplane filter list

List all filters.

```
flowplane filter list [--team <NAME>] [-o json|yaml|table]
```

```sh
flowplane filter list --team engineering -o table
```

### flowplane filter get

Get a filter by name.

```
flowplane filter get <NAME> [--team <NAME>] [-o json|yaml|table]
```

```sh
flowplane filter get my-rate-limit --team engineering -o json
```

### flowplane filter delete

Delete a filter by name.

```
flowplane filter delete <NAME> [--yes] [--team <NAME>]
```

```sh
flowplane filter delete my-rate-limit --team engineering --yes
```

### flowplane filter attach

Attach a filter to a listener with optional ordering.

```
flowplane filter attach <FILTER> --listener <LISTENER> [--order N] [--team <NAME>]
```

```sh
flowplane filter attach my-rate-limit --listener my-listener --order 1 --team engineering
```

### flowplane filter detach

Detach a filter from a listener.

```
flowplane filter detach <FILTER> --listener <LISTENER> [--team <NAME>]
```

```sh
flowplane filter detach my-rate-limit --listener my-listener --team engineering
```

## Quick Actions

### flowplane expose

Expose an upstream service through the gateway in one command. Creates cluster, route, and listener automatically.

```
flowplane expose <UPSTREAM> [--name NAME] [--path PATH]... [--port PORT] [--team <NAME>]
```

```sh
flowplane expose backend-svc:8000 --name my-api --path /api --port 10001 --team engineering
```

> Use Docker service hostnames for `<UPSTREAM>`, not `localhost`. The proxy runs inside Docker and cannot reach host-local addresses.

### flowplane unexpose

Remove a previously exposed service and its generated resources.

```
flowplane unexpose <NAME> [--team <NAME>]
```

```sh
flowplane unexpose my-api --team engineering
```

## API Management

### flowplane import openapi

Import an OpenAPI spec and generate routes automatically.

```
flowplane import openapi <FILE> [--name NAME] [--port PORT] [--team <NAME>]
```

```sh
flowplane import openapi petstore.json --name petstore --port 10002 --team engineering
```

### flowplane learn start

Start a learning session that captures traffic patterns to build route configurations.

```
flowplane learn start --route-pattern <REGEX> --target-sample-count <N> [--cluster-name NAME] [--http-methods M...] [--max-duration-seconds N] [--team <NAME>]
```

```sh
flowplane learn start \
  --route-pattern "/api/v1/.*" \
  --target-sample-count 100 \
  --cluster-name my-backend \
  --http-methods GET POST \
  --team engineering
```

### flowplane learn list

List all learning sessions.

```
flowplane learn list [--team <NAME>] [-o json|yaml|table]
```

```sh
flowplane learn list --team engineering -o table
```

### flowplane learn get

Get details for a learning session by ID.

```
flowplane learn get <ID> [--team <NAME>] [-o json|yaml|table]
```

```sh
flowplane learn get abc123 --team engineering -o json
```

### flowplane learn cancel

Cancel an active learning session.

```
flowplane learn cancel <ID> [--team <NAME>]
```

```sh
flowplane learn cancel abc123 --team engineering
```

## Configuration

### flowplane config show

Display current configuration.

```
flowplane config show
```

### flowplane config set

Set a configuration key. Valid keys: `base_url`, `team`, `org`.

```
flowplane config set <KEY> <VALUE>
```

```sh
flowplane config set team engineering
flowplane config set base_url http://flowplane.internal:8080
```

### flowplane config init

Create a default configuration file.

```
flowplane config init
```

### flowplane config path

Print the path to the configuration file.

```
flowplane config path
```

## Database

### flowplane database migrate

Run pending database migrations. Use `--dry-run` to preview without applying.

```
flowplane database migrate [--dry-run]
```

```sh
flowplane database migrate --dry-run
```

### flowplane database status

Show current migration status.

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

## Gotchas

> **Upstream URLs in `expose`:** Use the Docker service hostname (e.g., `backend-svc`), not `localhost`. The proxy runs inside Docker and cannot reach host-local addresses.

> **Envoy port range:** Listeners must use ports 10000–10020.

> **JSON spec format:** The `-f FILE` flag expects JSON matching the REST API request body. See the payload examples under each resource type.

> **Output formats:** Use `-o table` for human-readable output, `-o json` for scripting and piping.

> **Team context:** Most resource commands require `--team`. Set a default with `flowplane config set team <NAME>` to avoid repeating it.
