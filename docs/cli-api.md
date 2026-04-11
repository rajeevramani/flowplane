# CLI: API Management

Quick actions, OpenAPI imports, API learning sessions, and schema management.

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
flowplane expose http://httpbin:80 --name demo --path /get --path /post
```

> Use Docker service hostnames (e.g., `httpbin`), not `localhost`. Inside the Docker network, `localhost` refers to the control plane container.

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

## OpenAPI Import

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

### flowplane import list

List all OpenAPI imports.

```
flowplane import list [-o json|yaml|table]
```

### flowplane import get

Get details of a specific import.

```
flowplane import get <IMPORT_ID> [-o json|yaml]
```

### flowplane import delete

Delete an import and its associated resources (cascade).

```
flowplane import delete <IMPORT_ID> [--yes]
```

---

## Learning Sessions

Start a learning session to record API traffic and infer schemas.

### flowplane learn start

```
flowplane learn start --route-pattern <REGEX> --target-sample-count <N> \
  [--name <NAME>] [--auto-aggregate] \
  [--cluster-name <NAME>] [--http-methods <M>...] \
  [--max-duration-seconds <N>] [--triggered-by <WHO>] \
  [--deployment-version <VER>] [-o json|yaml|table]
```

| Flag | Required | Description |
|---|---|---|
| `--route-pattern` | Yes | Regex to match request paths |
| `--target-sample-count` | Yes | Number of samples to collect |
| `--name` | No | Human-readable session name. Auto-generated from route pattern if omitted |
| `--auto-aggregate` | No | Enable snapshot mode: aggregate every N samples and keep collecting |
| `--cluster-name` | No | Filter to a specific cluster |
| `--http-methods` | No | Filter by HTTP methods (space-separated) |
| `--max-duration-seconds` | No | Max session duration |
| `--triggered-by` | No | Label for who/what triggered the session |
| `--deployment-version` | No | Version label |

Sessions auto-activate and begin collecting immediately. They can collect more samples than the target.

```bash
flowplane learn start --name my-api --route-pattern '^/api/.*' --target-sample-count 50
flowplane learn start --name continuous --route-pattern '^/api/.*' --target-sample-count 100 --auto-aggregate
```

### flowplane learn stop

Stop an active session and trigger final schema aggregation.

```
flowplane learn stop <NAME_OR_ID> [-o json|yaml|table]
```

All learn subcommands accept a session **name** or **UUID**.

### flowplane learn list

```
flowplane learn list [--status <STATUS>] [--limit N] [--offset N] [-o json|yaml|table]
```

Status values: `pending`, `active`, `completing`, `completed`, `failed`, `cancelled`.

### flowplane learn get

```
flowplane learn get <NAME_OR_ID> [-o json|yaml|table]
```

### flowplane learn cancel

```
flowplane learn cancel <NAME_OR_ID> [--yes]
```

> In non-interactive mode (piped stdin), cancel without `--yes` silently does nothing. Always use `--yes` in scripts.

### flowplane learn activate

Activate a pending learning session. Only applies to sessions created via MCP with `autoStart: false`. CLI-created sessions auto-activate.

```
flowplane learn activate <NAME_OR_ID> [-o json|yaml|table]
```

### flowplane learn health

Run diagnostic checks on a learning session: status, regex matching, sample progress.

```
flowplane learn health <NAME_OR_ID> [-o json|yaml|table]
```

JSON output includes `diagnosis` (e.g., `healthy`, `stalled`) and a `fix` suggestion when issues are detected.

```bash
flowplane learn health my-api -o json
```

### flowplane learn export

Export schemas from learning sessions as OpenAPI 3.1. Convenience shortcut for `flowplane schema export --all`.

```
flowplane learn export [--session <NAME_OR_ID>] [--min-confidence <FLOAT>] \
  [--title <TITLE>] [--version <VER>] [--description <DESC>] [-o <FILE>]
```

Output goes to stdout as YAML by default. Use `-o file.yaml` or `-o file.json` to write to a file (format auto-detected from extension).

```bash
flowplane learn export                                    # all schemas, YAML to stdout
flowplane learn export --session my-api -o api.yaml       # from specific session
flowplane learn export --min-confidence 0.7 -o api.json   # high-confidence only
```

---

## Schemas

List, inspect, compare, and export API schemas discovered by learning sessions.

### flowplane schema list

```
flowplane schema list [--min-confidence <FLOAT>] [--path <PATTERN>] [--method <METHOD>] \
  [--session <NAME_OR_ID>] [--latest-only] [--limit N] [--offset N] [-o json|yaml|table]
```

```bash
flowplane schema list
flowplane schema list --min-confidence 0.7
flowplane schema list --session my-api
```

### flowplane schema get

```
flowplane schema get <ID> [-o json|yaml|table]
```

Table output shows field names, types, formats, and required markers.

### flowplane schema compare

Compare two aggregated schema versions to see differences.

```
flowplane schema compare <ID> --with <OTHER_ID> [-o json|diff]
```

`--with` is required. Output formats: `json` (default), `diff`.

```bash
flowplane schema compare 1 --with 2
flowplane schema compare 1 --with 2 -o diff
```

### flowplane schema export

Export schemas as an OpenAPI 3.1 specification. Shared schemas are deduplicated into `components/schemas` with `$ref` references.

```
flowplane schema export (--all | --id <ID,ID,...>) \
  [--session <NAME_OR_ID>] [--min-confidence <FLOAT>] \
  [--title <TITLE>] [--version <VER>] [--description <DESC>] [-o <FILE>]
```

| Flag | Description |
|---|---|
| `--all` | Export all latest schemas |
| `--id 1,2,3` | Export specific schema IDs (comma-separated) |
| `--session` | Filter to schemas from a specific learning session |
| `--min-confidence` | Only export schemas above this confidence threshold |
| `--title` | API title in the spec (default: "Learned API") |
| `--version` | API version (default: "1.0.0") |
| `-o <FILE>` | Write to file. Format auto-detected: `.yaml`/`.yml` = YAML, `.json` = JSON. Stdout defaults to YAML |

```bash
flowplane schema export --all                               # YAML to stdout
flowplane schema export --all -o api.yaml                   # write to file
flowplane schema export --id 1,2,3 -o subset.json           # specific schemas
flowplane schema export --session my-api --all -o api.yaml  # from specific session
flowplane schema export --all --min-confidence 0.7          # high-confidence only
```
