# CLI: Stack, Auth & Configuration

Setup, authentication, and configuration commands.

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

Admin commands for managing the PostgreSQL schema. These connect directly to the database (requires `FLOWPLANE_DATABASE_URL` env var), not through the API.

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
