---
name: flowplane-cli
description: Flowplane CLI command reference — exact syntax, flags, and examples for every flowplane command. Use when you need the precise flags and options for a flowplane CLI command, writing CLI examples, scripting flowplane, or answering "how do I do X with the CLI". Do NOT use for MCP tools (use flowplane-api) or troubleshooting (use flowplane-ops).
license: Apache-2.0
metadata:
  author: rajeevramani
  version: "0.1.0"
---

# Flowplane CLI Reference

Binary: `flowplane-cli` (typically aliased to `flowplane`). Built with `cargo build --bin flowplane-cli`.

## Global Options

Most subcommands accept these flags:

| Flag | Applies To | Purpose |
|---|---|---|
| `--token <TOKEN>` | All | Bearer token for API auth |
| `--token-file <PATH>` | All | Read token from file |
| `--base-url <URL>` | All | API base URL (default: `http://localhost:8080`) |
| `--timeout <SECS>` | All | Request timeout |
| `--team <NAME>` | All | Team context for resource commands |
| `--database-url <URL>` | All | Database URL override |
| `-v, --verbose` | All (global) | Enable verbose logging |
| `-V, --version` | Top-level | Print version |
| `-o, --output <FMT>` | `create`, `list`, `get`, `config show` | Output format: `json`, `yaml`, `table`. Defaults: `json` for create/get, `table` for list, `yaml` for config show |

## Stack Management

### `flowplane init`
Bootstrap a local dev environment (Docker/Podman).
```bash
flowplane init [--with-envoy] [--with-httpbin]
```
Generates dev token, starts PostgreSQL + control plane, waits for health, saves credentials to `~/.flowplane/credentials`.

### `flowplane down`
Stop the dev environment.
```bash
flowplane down [--volumes]
```
`--volumes` removes persistent data (database).

### `flowplane status`
Show system overview or lookup a specific listener.
```bash
flowplane status [<listener-name>]
```

### `flowplane doctor`
Run diagnostic health checks.
```bash
flowplane doctor
```

### `flowplane logs`
View local dev stack logs.
```bash
flowplane logs [-f|--follow]
```

### `flowplane list`
List currently exposed services.
```bash
flowplane list
```

## Auth

### `flowplane auth login`
Log in to Flowplane. In prod mode, opens PKCE browser flow. In dev mode, no-op.
```bash
flowplane auth login [--device-code] [--callback-url <URL>] [--issuer <URL>] [--client-id <ID>]
```
- `--device-code` — Use device code flow instead of browser-based PKCE
- `--callback-url` — Override the PKCE callback URL
- `--issuer` — OIDC issuer URL (overrides config/env)
- `--client-id` — OIDC client ID (overrides config/env)

### `flowplane auth token`
Print the current access token.
```bash
flowplane auth token
```

### `flowplane auth whoami`
Show the current authenticated user, org, and team.
```bash
flowplane auth whoami
```

### `flowplane auth logout`
Clear stored credentials.
```bash
flowplane auth logout
```

## Quick Actions

### `flowplane expose`
Expose a backend service through the gateway in one command. Creates cluster + route config + listener.
```bash
flowplane expose <UPSTREAM> [--name <NAME>] [--path <PATH>]... [--port <PORT>]
```
- `UPSTREAM` — URL with port (e.g., `http://httpbin:80`). Port is required. Use the Docker service name, not localhost. Scheme and path are stripped — only host:port is used for the cluster endpoint.
- `--name` — Service name. If omitted, auto-derived from upstream: strip scheme, take host:port, replace non-alphanumeric with `-`, truncate to 48 chars (e.g., `http://httpbin:80` → `httpbin-80`).
- `--path` — Path prefix to route, repeatable (default: `/`). Creates one route per path in the virtual host.
- `--port` — Listener port. Must be in 10001-10020 range. If omitted, auto-assigns first free port in that range (scans all listeners cross-team).

**Naming convention**: cluster = `<name>`, route config = `<name>-routes`, listener = `<name>-listener`. Virtual host = `<name>-routes-vhost` with domains `["*"]`.

**Behavior:**
- **Idempotent**: Re-exposing with same name + same upstream returns 200 with existing port (no duplicate resources created).
- **Different upstream, same name**: Returns 409 Conflict.
- **Port collision**: Returns 409 if the port is already in use.
- **Port exhaustion**: Returns 409 "Port pool exhausted" when all 20 ports are taken.
- **Port out of range**: Returns 400 for ports outside 10001-10020.
- Requires a dataplane for the team (auto-created by `flowplane init`).

```bash
flowplane expose http://httpbin:80 --name httpbin --path /get --port 10001
```

### `flowplane unexpose`
Remove an exposed service (tears down cluster + route config + listener by naming convention).
```bash
flowplane unexpose <NAME>
```
Deletes `<NAME>` (cluster), `<NAME>-routes` (route config), `<NAME>-listener` (listener). Silently skips any individual resource that doesn't exist. Returns 404 if none of the three resources existed (`"<NAME> is not currently exposed"`).

## Resource CRUD

All resource commands follow the same pattern: `create -f <JSON>`, `list`, `get <NAME>`, `update <NAME> -f <JSON>`, `delete <NAME> [--yes]`.

### Clusters
```bash
flowplane cluster create -f cluster.json
flowplane cluster list [--service <SVC>] [--limit N] [--offset N]
flowplane cluster get <NAME>
flowplane cluster update <NAME> -f cluster.json
flowplane cluster delete <NAME> [--yes]
```

### Listeners
```bash
flowplane listener create -f listener.json
flowplane listener list [--protocol <PROTO>] [--limit N] [--offset N]
flowplane listener get <NAME>
flowplane listener update <NAME> -f listener.json
flowplane listener delete <NAME> [--yes]
```

### Routes (Route Configs)
```bash
flowplane route create -f route-config.json
flowplane route list [--cluster <NAME>] [--limit N] [--offset N]
flowplane route get <NAME>
flowplane route update <NAME> -f route-config.json
flowplane route delete <NAME> [--yes]
```

### Teams
All team commands require `--org <ORG>`.
```bash
flowplane team create --org <ORG> -f team.json
flowplane team list --org <ORG>
flowplane team list --admin                     # Platform admin: all teams across orgs
flowplane team get --org <ORG> <TEAM>
flowplane team update --org <ORG> <TEAM> -f team.json
flowplane team delete --org <ORG> <TEAM> [--yes]
```

## Filters

### `flowplane filter create`
Create a filter from a JSON spec file.
```bash
flowplane filter create -f filter.json
```

### `flowplane filter list`
List all filters.
```bash
flowplane filter list
```

### `flowplane filter get`
Get filter details including attachment info.
```bash
flowplane filter get <NAME>
```

### `flowplane filter delete`
Delete a filter.
```bash
flowplane filter delete <NAME>
```

### `flowplane filter attach`
Attach a filter to a listener.
```bash
flowplane filter attach <FILTER_NAME> --listener <LISTENER_NAME> [--order <N>]
```
- `--order` — Execution order (lower = earlier in chain). Must be unique per listener.

### `flowplane filter detach`
Detach a filter from a listener.
```bash
flowplane filter detach <FILTER_NAME> --listener <LISTENER_NAME>
```

## API Management

### `flowplane import openapi`
Import routes from an OpenAPI specification file. Creates a cluster, route config, and listener from the spec.
```bash
flowplane import openapi <FILE> [--name <NAME>] [--port <PORT>]
```
- `FILE` — Path to OpenAPI spec (YAML or JSON). Must include `servers` block and valid operation objects (with `responses`).
- `--name` — Service name. Sets the listener name to `<NAME>-listener`. If omitted, derived from spec `info.title` (sanitized: lowercased, non-alphanumeric chars replaced with `-`, max 48 chars).
- `--port` — Listener port (default: 10000). Must not conflict with an existing listener port.

**Behavior:**
- Creates 1 cluster (named `<sanitized-title>-<server-host>`, e.g., `test-api-localhost`), 1 route config, and 1 listener per import.
- Multiple OpenAPI paths become routes within a single route config — `routes_created` always reports `1` (one route config).
- **Idempotent on spec name**: Re-importing a spec with the same `info.title` for the same team deletes the previous import (cascade) and creates a fresh one.
- **Port collision**: Returns 409 Conflict with a helpful message suggesting an alternative port or `listener_mode=existing`.
- **Name collision**: Returns 409 if a listener with the derived name already exists.
- Requires at least one dataplane for the team (created automatically by `flowplane init`).

### `flowplane learn start`
Start a learning session to record API traffic. Sessions auto-activate by default and begin collecting samples immediately.
```bash
flowplane learn start --route-pattern <REGEX> --target-sample-count <N> \
  [--name <NAME>] [--auto-aggregate] \
  [--cluster-name <NAME>] [--http-methods GET POST ...] \
  [--max-duration-seconds <N>] [--triggered-by <WHO>] \
  [--deployment-version <VERSION>] [-o json|yaml|table]
```
- `--name` — Human-readable session name. If omitted, auto-generated from route_pattern.
- `--auto-aggregate` — Enable snapshot mode: periodic aggregation while continuing to collect samples.

Default output is JSON. Returns session ID, name, status, and timestamps.

Example:
```bash
flowplane learn start --route-pattern '^/.*' --target-sample-count 5 --name mockbank-v1
# Sessions can exceed target (e.g., 200% progress) — all samples in the
# collection window are captured once target is reached.
```

### `flowplane learn stop`
Stop an active learning session. Triggers final aggregation, then completes.
```bash
flowplane learn stop <NAME_OR_ID>         # Accepts session name or UUID
```

### `flowplane learn list`
List learning sessions. Supports `-o json|yaml|table` (default: table).
```bash
flowplane learn list [-o table]        # Table: ID, Status, Pattern, Samples, Target, Progress
flowplane learn list -o json           # Full JSON with all fields
flowplane learn list --status active   # Filter by status
flowplane learn list --limit 10        # Pagination
```

### `flowplane learn get`
Get learning session details. Accepts name or UUID. Default output is json; use `-o table` for summary.
```bash
flowplane learn get <NAME_OR_ID>               # Table format
flowplane learn get <NAME_OR_ID> -o json       # Full JSON including timestamps, team, error_message
```

### `flowplane learn cancel`
Cancel an active learning session. Accepts name or UUID. Requires confirmation in interactive mode.
```bash
flowplane learn cancel <NAME_OR_ID>            # Prompts y/N (errors if stdin is not a terminal)
flowplane learn cancel <NAME_OR_ID> --yes      # Skip confirmation
flowplane learn cancel <NAME_OR_ID> -y         # Short form
```
In non-interactive mode (piped stdin), cancel without `--yes` returns an error directing you to use `--yes`. Always use `--yes` in scripts.

### `flowplane learn export`
Convenience shortcut for `flowplane schema export --all`. Exports all schemas as OpenAPI 3.1.
```bash
flowplane learn export                                          # All schemas to stdout (YAML)
flowplane learn export --session mockbank-v1 -o api.yaml        # From specific session
flowplane learn export --min-confidence 0.7 --title "My API"    # High-confidence only
```
Flags: `--session`, `--min-confidence`, `--title`, `--version`, `--description`, `-o` (output file path).

## Schema Management

### `flowplane schema list`
List discovered API schemas.
```bash
flowplane schema list                              # Table view
flowplane schema list --min-confidence 0.7         # Filter by confidence
flowplane schema list --session mockbank-v1        # Filter by session name
flowplane schema list --path /api/users            # Filter by path
flowplane schema list --method GET                 # Filter by HTTP method
flowplane schema list --latest-only                # Only latest snapshot per endpoint
flowplane schema list --limit 20 --offset 0        # Pagination
flowplane schema list -o json                      # JSON output
```

### `flowplane schema get`
Show schema detail by ID.
```bash
flowplane schema get <ID>                          # JSON output (default)
flowplane schema get <ID> -o table                 # Human-readable summary
flowplane schema get <ID> -o yaml                  # YAML output
```

### `flowplane schema export`
Export schemas as OpenAPI 3.1. Auto-detects format from file extension (`.yaml`/`.json`). Stdout defaults to YAML.
```bash
flowplane schema export --all                      # All schemas to stdout (YAML)
flowplane schema export --all -o api.yaml          # Write to file
flowplane schema export --id 1,2,3 -o api.json     # Specific schemas, JSON format
flowplane schema export --all --min-confidence 0.7 # High-confidence only
flowplane schema export --session mockbank-v1 --all -o api.yaml  # From specific session
flowplane schema export --all --title "My API" --version "1.0.0" --description "Generated from traffic"
```
Flags: `--id` (comma-separated IDs), `--all`, `--min-confidence`, `--session`, `--title`, `--version`, `--description`, `-o` (output file path).

## Secrets

Commands: `flowplane secret create`, `list`, `get`, `delete`. See the **`flowplane-secrets` skill** for full syntax, examples, secret types, and filter integration.

```bash
flowplane secret create --name oauth-secret --type generic_secret \
  --config '{"type":"generic_secret","secret":"dGVzdC1zZWNyZXQ="}'
flowplane secret list
flowplane secret get <SECRET_ID> -o json
flowplane secret delete <SECRET_ID> --yes
```

## Config

### `flowplane config show`
Show current CLI configuration.
```bash
flowplane config show
```

### `flowplane config set`
Set a configuration value.
```bash
flowplane config set <KEY> <VALUE>
```
Keys: `token`, `base_url`, `timeout`, `team`, `org`, `oidc_issuer`, `oidc_client_id`, `callback_url`.

### `flowplane config init`
Initialize config file with defaults.
```bash
flowplane config init [--force]
```
`--force` overwrites existing configuration file.

### `flowplane config path`
Show config file location.
```bash
flowplane config path
```

## Database (Admin)

### `flowplane database migrate`
Run pending migrations.
```bash
flowplane database migrate [--dry-run]
```

### `flowplane database list`
List all applied migrations.
```bash
flowplane database list
```

### `flowplane database status`
Show migration status.
```bash
flowplane database status
```

### `flowplane database validate`
Validate database schema.
```bash
flowplane database validate
```

## Gotchas

- **Upstream URLs in `expose`**: Use the Docker service hostname (e.g., `httpbin`), not `localhost`. Inside the Docker network, `localhost` refers to the control plane container, not the host.
- **Port range**: Envoy exposes ports 10000-10020. Listener ports outside this range work in the DB but Envoy won't serve traffic on them.
- **JSON spec format**: All `create` commands use `-f <FILE>` with JSON body. The JSON structure matches the REST API request body. See the `flowplane-api` skill for payload examples.
- **Output format**: Use `-o table` for human-readable output, `-o json` for scripting.
- **Team context**: Most commands require `--team` unless configured via `flowplane config set team <NAME>` or auto-set by `flowplane init`.

## References

- [references/commands.md](references/commands.md) — Detailed per-command reference with JSON payload examples
