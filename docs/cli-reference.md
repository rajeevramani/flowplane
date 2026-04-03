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
