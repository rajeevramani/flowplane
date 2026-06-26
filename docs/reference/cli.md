> Audience: cli-users · Status: stable

# Flowplane CLI Reference

Exhaustive reference for the `flowplane` binary: global options, every top-level command, and its subcommands, arguments, and flags.

## Global options

These flags are accepted on every command (`global = true`). Place them before or after the subcommand.

| Flag | Short | Env var | Default | Meaning |
|------|-------|---------|---------|---------|
| `--context <NAME>` | | | (current context) | Select a named context from the config file. Errors if the named context does not exist. |
| `--server <URL>` | | `FLOWPLANE_SERVER` | `http://127.0.0.1:8080` | Control-plane base URL. |
| `--team <NAME>` | | `FLOWPLANE_TEAM` | | Team scope. |
| `--org <NAME>` | | `FLOWPLANE_ORG` | | Organization scope. |
| `--token <TOKEN>` | | `FLOWPLANE_TOKEN` | | Bearer token. Highest-priority token source; falls back to the selected context, the config-file token, then `~/.flowplane/credentials`. Redacted in `config show`. |
| `--output <FMT>` | `-o` | | `table` on a TTY, `json` when stdout is piped | Output format: `table`, `json`, `yaml`, `wide`. An explicit `-o` always wins; otherwise the default is `table` on an interactive terminal and `json` when stdout is not a TTY (so `… \| jq` works without `-o json`). |
| `--json` | | | `false` | Exactly equivalent to `--output json`. |
| `--no-color` | | | `false` | Disable colored output. Color is also disabled by the `NO_COLOR` environment variable, when stdout is not a TTY, or when output is redirected with `--out`. |
| `--quiet` | | | `false` | Suppress non-essential output. |
| `--verbose` | | | `false` | Verbose output. |
| `--dry-run` | | | `false` | Do not perform mutating actions. |
| `--yes` | `-y` | | `false` | Assume "yes" for confirmation prompts. |
| `--revision <N>` | | | | Optimistic-concurrency precondition for `update`/`delete`, sent as `If-Match`. Omit it and the CLI does read-modify-write: it reads the resource's current revision and sends that, so a concurrent edit is detected. A stale revision fails with a `409` whose error envelope names both the `attempted_revision` and the server's current one. |
| `--fields <a,b,c>` | | | | Project reader output to only these comma-separated keys, applied inside the envelope `data` (per item for lists). `schemaVersion`/`kind` always survive; an absent key is omitted (no `null`). |
| `--timeout <SECS>` | | `FLOWPLANE_TIMEOUT` | `30` | HTTP timeout in seconds (u64). |
| `--out <PATH>` | | | | Write command output to a file. |

### Other environment variables

These are read directly (not as flags) by config resolution:

| Env var | Meaning |
|---------|---------|
| `FLOWPLANE_CONFIG` | Path to the config TOML file. Default: `$HOME/.flowplane/config.toml`. |
| `FLOWPLANE_TOKEN` | Bearer token (env tier; below `--token`, above context/file). |
| `FLOWPLANE_ORG` | Organization (env tier of the precedence rule). |
| `FLOWPLANE_TEAM` | Team (env tier of the precedence rule). |
| `FLOWPLANE_TIMEOUT` | HTTP timeout in seconds (env tier; below `--timeout`, above the `30` default). |
| `FLOWPLANE_OIDC_ISSUER` | OIDC issuer URL fallback. |
| `FLOWPLANE_OIDC_CLIENT_ID` | OIDC client ID fallback. |
| `FLOWPLANE_OIDC_SCOPE` | OIDC scope fallback. |
| `FLOWPLANE_OIDC_CALLBACK_URL` | OIDC callback URL fallback. |

Configuration precedence is uniform for every value — `flag > env > context > file > default`.
Token resolution follows the same rule: `--token` → `FLOWPLANE_TOKEN` → selected context token →
config-file token → `~/.flowplane/credentials` file. The config directory and credential files are
written with `0700`/`0600` permissions on Unix, and the token is redacted in `config show`.

## JSON output envelope

Under `-o json` (and `-o yaml`) every command's success payload is wrapped in a typed
envelope so machine consumers can rely on a stable shape:

```json
{
  "schemaVersion": 1,
  "kind": "cluster",
  "data": { "name": "alpha", "revision": 3 }
}
```

- `schemaVersion` — integer version of the **output contract** (not the product version).
  It starts at `1` and is bumped only on a breaking change to this envelope shape.
- `kind` — the resource kind. Singular for a single resource (`cluster`), suffixed with
  `List` for a collection (`clusterList`). `version` → `version`; a body-less mutation
  (e.g. `delete`) → `mutationResult`; `apply` → `applyResult`.
- `data` — the resource object, the array/`items` collection, or the command-specific
  payload.

This envelope wraps reader output, mutation results (including `version`, `delete`, and
`apply`), so a command that previously printed prose (e.g. `version`, `cluster delete`)
now emits the envelope under `-o json`. Human formats (`table`, `wide`) are unwrapped and
remain free-form.

## Errors & exit codes

Every failure — an HTTP error from the server, a transport/network failure, or an `apply`
partial-failure — is written as a structured error envelope to **stderr** (stdout stays
empty), and the process exits with a scriptable code. Under `-o json` the error envelope is
JSON; otherwise it is a compact `error (code): message` line. The error envelope is
separate from the success envelope above (it is **not** wrapped in `{schemaVersion,kind,data}`):

```json
{
  "code": "not_found",
  "message": "cluster \"alpha\" not found",
  "status": 404,
  "retryable": false,
  "request_id": "01J…",
  "hint": "…"
}
```

- `retryable` — `true` for transient failures (HTTP `429`, any `5xx`, and transport/timeout
  failures), `false` for terminal `4xx`.
- `hint` — on a `401` with no server-supplied hint the client synthesizes
  `run \`flowplane auth login\` to authenticate`; a `403` carries the server's hint naming
  the `(resource, action)`.
- `status` — the HTTP status (absent for transport failures, whose `code` is
  `connection_failed`/`timeout`/`transport_error`).

### Exit codes

| Code | Meaning | Trigger |
|------|---------|---------|
| `0` | Success | — |
| `1` | Generic / internal CLI error | Unclassified local failure |
| `2` | Usage error | Invalid flags/arguments (clap-native) |
| `3` | Authentication / authorization | HTTP `401`, `403` |
| `4` | Not found / conflict / precondition | HTTP `404`, `409`, `412` |
| `5` | Validation | HTTP `400`, `422` |
| `6` | Rate limited | HTTP `429` |
| `7` | Server / transport | HTTP `5xx`, connection refused, timeout |

## Top-level commands

`serve`, `db`, `openapi`, `auth`, `config`, `org`, `team`, `cluster`, `listener`, `route`, `api`, `mcp`, `ai`, `rate-limit`, `learn`, `secret`, `dataplane`, `expose`, `unexpose`, `stats`, `ops`, `apply`, `completion`, `version`, `schema`.

---

### `serve`
Run the control-plane server (REST + MCP; xDS from S5). No subcommands or args.

### `db`
Database operations.

| Subcommand | Purpose |
|------------|---------|
| `db migrate` | Apply pending migrations (forward-only) and exit. |

### `openapi`
Print the OpenAPI document this binary serves (the exact API contract). No subcommands or args.

### `auth`
Client auth helpers.

| Subcommand | Args / Flags |
|------------|--------------|
| `auth whoami` | — |
| `auth token` | — |
| `auth logout` | — |
| `auth login` | `--token <TOKEN>`, `--token-stdin`, `--device` (alias `--device-code`), `--pkce`, `--issuer <URL>`, `--client-id <ID>`, `--callback-url <URL>`, `--scope <SCOPE>` (default `openid email profile`) |

### `config`
Client configuration.

| Subcommand | Args / Flags |
|------------|--------------|
| `config path` | — |
| `config show` | — |
| `config set-context <NAME>` | `--server <URL>` (required), `--org <ORG>`, `--team <TEAM>`, `--token <TOKEN>`, `--token-stdin` |
| `config use-context <NAME>` | positional `name` |
| `config get-contexts` | — |

### `org`
Organization management.

| Subcommand | Args / Flags |
|------------|--------------|
| `org list` | — |
| `org get <ORG>` | positional `org` |
| `org create <NAME>` | positional `name`, `--display-name <NAME>` |
| `org delete <ORG>` | positional `org` |
| `org member list <ORG>` | positional `org` |
| `org member add <ORG>` | positional `org`, `--email <EMAIL>`, `--subject <SUBJECT>`, `--user-id <ID>`, `--role <ROLE>` (required) |
| `org member remove <ORG> <USER_ID>` | positionals `org`, `user_id` |

### `team`
Team management.

| Subcommand | Args / Flags |
|------------|--------------|
| `team list` | — |
| `team create <NAME>` | positional `name`, `--display-name <NAME>` |
| `team delete` | `--team <TEAM>` |
| `team member list` | `--team <TEAM>` |
| `team member add <EMAIL>` | `--team <TEAM>`, positional `email` |
| `team member remove <USER_ID>` | `--team <TEAM>`, positional `user_id` |
| `team grant list` | `--team <TEAM>` |
| `team grant add <EMAIL>` | `--team <TEAM>`, positional `email`, `--resource <RES>` (required), `--action <ACT>` (required) |
| `team grant remove <GRANT_ID>` | `--team <TEAM>`, positional `grant_id` |

### `cluster`
Gateway clusters. Uses the shared resource subcommand set.

| Subcommand | Args / Flags |
|------------|--------------|
| `cluster list` | `--team <TEAM>` |
| `cluster get <NAME>` | `--team <TEAM>`, positional `name` |
| `cluster create` | `--team <TEAM>`, `--file <PATH>` / `-f` (required) |
| `cluster update <NAME>` | `--team <TEAM>`, positional `name`, `--file <PATH>` / `-f` (required) |
| `cluster delete <NAME>` | `--team <TEAM>`, positional `name` |

### `listener`
Gateway listeners. Same shared resource subcommand set as `cluster` (`list`, `get`, `create`, `update`, `delete`) with identical flags.

### `route`
Route configs.

| Subcommand | Args / Flags |
|------------|--------------|
| `route list` | `--team <TEAM>` |
| `route get <NAME>` | `--team <TEAM>`, positional `name` |
| `route create` | `--team <TEAM>`, `--file <PATH>` / `-f` (required) |
| `route update <NAME>` | `--team <TEAM>`, positional `name`, `--file <PATH>` / `-f` (required) |
| `route delete <NAME>` | `--team <TEAM>`, positional `name` |
| `route generate` | `--team <TEAM>`, `--from-spec <ID>` (required), `--listener-port <PORT>` (u16, required) |
| `route apply <PLAN_ID>` | `--team <TEAM>`, positional `plan_id` |

### `rate-limit`
Global rate-limit domains, policies, per-team overrides, and the CP→RLS repush trigger. `create`/`update`/`set` read the JSON body from `--file`; the file content is the REST body, sent verbatim. See [Enable global rate limiting](../how-to/global-rate-limit.md) and the [rate-limit REST reference](rest-api.md#rate-limiting).

| Subcommand | Args / Flags | `--file` body |
|------------|--------------|---------------|
| `rate-limit domain list` | `--team <TEAM>` | — |
| `rate-limit domain get <NAME>` | `--team`, positional `name` | — |
| `rate-limit domain create` | `--team`, `--file <PATH>` / `-f` (required) | `{"name":"checkout"}` |
| `rate-limit domain update <NAME>` | `--team`, positional `name`, `--file` / `-f` (required) | `{"name":"checkout"}` |
| `rate-limit domain delete <NAME>` | `--team`, positional `name` | — |
| `rate-limit policy list` | `--team`, `--domain <DOMAIN>` (required) | — |
| `rate-limit policy get <NAME>` | `--team`, `--domain` (required), positional `name` | — |
| `rate-limit policy create` | `--team`, `--domain` (required), `--file` / `-f` (required) | `{"name":"per-client","spec":{"descriptors":{"api_key":"acme"},"requests_per_unit":100,"unit":"minute"}}` |
| `rate-limit policy update <NAME>` | `--team`, `--domain` (required), positional `name`, `--file` / `-f` (required) | `{"spec":{"descriptors":{"api_key":"acme"},"requests_per_unit":200,"unit":"minute"}}` |
| `rate-limit policy delete <NAME>` | `--team`, `--domain` (required), positional `name` | — |
| `rate-limit override get` | `--team`, `--domain` (required), `--policy` (required) | — |
| `rate-limit override set` | `--team`, `--domain` (required), `--policy` (required), `--file` / `-f` (required) | `{"spec":{"requests_per_unit":500}}` |
| `rate-limit override update` | `--team`, `--domain` (required), `--policy` (required), `--file` / `-f` (required) | `{"spec":{"requests_per_unit":500}}` |
| `rate-limit override delete` | `--team`, `--domain` (required), `--policy` (required) | — |
| `rate-limit force-repush` | (none) | — |

The `update` and `delete` subcommands (domain, policy, and override) are concurrency-controlled: pass the current resource revision with the global `--revision <N>` flag (the `If-Match` value, read from a `GET`). Omitting it fails with `this operation requires the resource revision`; a stale value returns `409`. `create`/`set` do not take `--revision`.

`rate-limit force-repush` triggers an immediate CP→RLS reconcile and requires the `platform:execute` permission (held by the `admin:all` / platform-admin role); an org/team token gets `403` (`missing permission: platform:execute`). The 60 s reconcile loop is the backstop, so a repush is only a fast path — see [`FLOWPLANE_RLS_RECONCILE_SECS`](configuration.md). `unit` is one of `second`, `minute`, `hour`, `day`.

### `api`
API definitions, imported specs, and generated API tool rows.

| Subcommand | Args / Flags |
|------------|--------------|
| `api list` | `--team <TEAM>` |
| `api get <NAME>` | `--team <TEAM>`, positional `name` |
| `api status <NAME>` | `--team <TEAM>`, positional `name` |
| `api create <NAME>` | `--team <TEAM>`, positional `name`, `--display-name <NAME>`, `--description <TEXT>` (default empty), `--from-openapi <PATH>`, `--route-config-id <ID>`, `--listener-id <ID>`, `--virtual-host <HOST>`, `--route <ROUTE>` |
| `api delete <NAME>` | `--team <TEAM>`, positional `name` |
| `api spec reject <API> <VERSION>` | `--team <TEAM>`, positionals `api`, `version` (i64), `--reason <TEXT>` (default empty) |
| `api spec publish <API> <VERSION>` | `--team <TEAM>`, positionals `api`, `version` (i64), `--reason <TEXT>` (default empty) |

### `mcp`
MCP server and generated API tool operations.

| Subcommand | Args / Flags |
|------------|--------------|
| `mcp status` | `--team <TEAM>` |
| `mcp connections` | `--team <TEAM>` |
| `mcp enable` | `--api <API>` (alias `--tool`, required), `--team <TEAM>` |
| `mcp disable` | `--api <API>` (alias `--tool`, required), `--team <TEAM>` |

### `ai`
AI gateway resources. `providers`, `routes`, and `budgets` each use the shared resource subcommand set (`list`, `get`, `create`, `update`, `delete`).

| Subcommand | Args / Flags |
|------------|--------------|
| `ai providers <RESOURCE_CMD>` | shared resource subcommands (see `cluster`) |
| `ai routes <RESOURCE_CMD>` | shared resource subcommands (see `cluster`) |
| `ai budgets <RESOURCE_CMD>` | shared resource subcommands (see `cluster`) |
| `ai usage` | `--team <TEAM>`, `--provider-id <ID>`, `--route-config-id <ID>`, `--limit <N>` (i64, default 50), `--offset <N>` (i64, default 0) |

### `learn`
Learning capture sessions.

| Subcommand | Args / Flags |
|------------|--------------|
| `learn start <NAME>` | `--team <TEAM>`, positional `name`, `--api <API>`, `--api-definition-id <ID>`, `--route-config-id <ID>`, `--listener-id <ID>`, `--virtual-host <HOST>`, `--route <ROUTE>`, `--target-sample-count <N>` (i32, default 1000), `--max-duration-seconds <N>` (i32), `--max-bytes <N>` (i64, default 10485760), `--max-distinct-paths <N>` (i32, default 500) |
| `learn list` | `--team <TEAM>`, `--status <STATUS>`, `--limit <N>` (i64, default 50), `--offset <N>` (i64, default 0) |
| `learn get <SESSION>` | `--team <TEAM>`, positional `session` |
| `learn stop <SESSION>` | `--team <TEAM>`, positional `session` |
| `learn generate-spec <SESSION>` | `--team <TEAM>`, positional `session` |
| `learn cancel <SESSION>` | `--team <TEAM>`, positional `session` |
| `learn discover <DISCOVER_CMD>` | nested discovery subcommands (below) |

#### `learn discover`
Passive upstream discovery sessions.

| Subcommand | Args / Flags |
|------------|--------------|
| `learn discover start <NAME>` | `--team <TEAM>`, positional `name`, `--upstream <ADDR>` (required), `--listener-port <PORT>` (i32, required), `--upstream-tls`, `--target-sample-count <N>` (i32, default 1000), `--max-duration-seconds <N>` (i32), `--max-bytes <N>` (i64, default 10485760), `--max-distinct-paths <N>` (i32, default 500) |
| `learn discover list` | `--team <TEAM>`, `--status <STATUS>`, `--limit <N>` (i64, default 50), `--offset <N>` (i64, default 0) |
| `learn discover status <SESSION>` | `--team <TEAM>`, positional `session` |
| `learn discover stop <SESSION>` | `--team <TEAM>`, positional `session` |
| `learn discover generate-spec <SESSION>` | `--team <TEAM>`, positional `session` |

### `secret`
Write-only secrets.

| Subcommand | Args / Flags |
|------------|--------------|
| `secret list` | `--team <TEAM>` |
| `secret get <NAME>` | `--team <TEAM>`, positional `name` |
| `secret create` | `--team <TEAM>`, `--file <PATH>` / `-f` (required) |
| `secret rotate <NAME>` | `--team <TEAM>`, positional `name`, `--revision <N>` (i64, required), `--file <PATH>` / `-f` (required) |

### `dataplane`
Dataplane registration and certificates.

| Subcommand | Args / Flags |
|------------|--------------|
| `dataplane list` | `--team <TEAM>` |
| `dataplane get <NAME>` | `--team <TEAM>`, positional `name` |
| `dataplane create <NAME>` | `--team <TEAM>`, positional `name`, `--description <TEXT>` (default empty) |
| `dataplane telemetry <NAME>` | `--team <TEAM>`, positional `name`, `--file <PATH>` / `-f` (required) |
| `dataplane bootstrap <NAME>` | (alias `dataplane envoy-config`) `--team <TEAM>`, positional `name`, `--mode <MODE>` (`dev`\|`mtls`, default `dev`), `--xds-host <HOST>` (default `127.0.0.1`), `--xds-port <PORT>` (u16, default 18000), `--admin-port <PORT>` (u16, default 9901), `--cert-path <PATH>`, `--key-path <PATH>`, `--ca-path <PATH>` |
| `dataplane cert <CERT_CMD>` | nested certificate subcommands (below) |

#### `dataplane cert`
Dataplane certificate management.

| Subcommand | Args / Flags |
|------------|--------------|
| `dataplane cert list` | `--team <TEAM>` |
| `dataplane cert register` | `--team <TEAM>`, `--file <PATH>` / `-f` (required) |
| `dataplane cert issue <DATAPLANE>` | `--team <TEAM>`, positional `dataplane`, `--ttl-hours <N>` (i64, default 24) |
| `dataplane cert revoke <SERIAL>` | `--team <TEAM>`, positional `serial`, `--reason <TEXT>` (required) |

### `expose`
Expose an upstream through Envoy with cluster + route + listener resources. Flattened args (no subcommands):

`flowplane expose <UPSTREAM> --name <NAME> [--team <TEAM>] [--path <PATH>] [--port <PORT>] [--public-base-url <URL>]`

| Arg / Flag | Default | Meaning |
|------------|---------|---------|
| `<UPSTREAM>` (positional) | | Upstream address. |
| `--name <NAME>` | (required) | Resource name. |
| `--team <TEAM>` | | Team scope. |
| `--path <PATH>` | `/` | Route match path. |
| `--port <PORT>` | | Listener port (u16). |
| `--public-base-url <URL>` | | Public gateway base URL clients use to reach the listener. |

### `unexpose`
Remove resources created by `expose`. Flattened args (no subcommands):

`flowplane unexpose <NAME> [--team <TEAM>]`

| Arg / Flag | Meaning |
|------------|---------|
| `<NAME>` (positional) | Name of the previously exposed resource set. |
| `--team <TEAM>` | Team scope. |

### `stats`
Team stats.

| Subcommand | Args / Flags |
|------------|--------------|
| `stats overview` | `--team <TEAM>` |

### `ops`
Operations diagnostics.

| Subcommand | Args / Flags |
|------------|--------------|
| `ops xds status` | `--team <TEAM>` |
| `ops xds nacks` | `--team <TEAM>` |
| `ops trace` | `--team <TEAM>`, `--request-id <ID>`, `--trace-id <ID>`, `--path <PATH>`, `--limit <N>` (i64, default 50) |

### `apply`
Apply a declarative JSON resource manifest. Flattened args (no subcommands):

`flowplane apply --file <PATH> [--diff] [--prune]`

| Arg / Flag | Default | Meaning |
|------------|---------|---------|
| `--file <PATH>` / `-f` | (required) | Manifest file path. |
| `--diff` | `false` | Show the diff. |
| `--prune` | `false` | Apply is additive-only until server batch support; prune requests are not silently ignored. |

### `completion`
Generate a shell completion script.

`flowplane completion <SHELL>`

| Arg | Meaning |
|-----|---------|
| `<SHELL>` (positional) | Target shell, as supported by `clap_complete::Shell` (e.g. `bash`, `zsh`, `fish`, `powershell`, `elvish`). |

### `version`
Print the binary version (from `CARGO_PKG_VERSION`). No subcommands or args.

### `schema`
Print the machine-readable CLI catalog — the whole command tree (commands, subcommands,
flags, types, enums, required/defaults, help) as JSON. Makes no network call. This is the
**canonical CLI contract source and the MCP-derivation seam** (ADR FP-DEC-0003): generated
consumers (shell completion, the future MCP tool catalog, docs) derive from `flowplane schema`
rather than scraping `--help`. Under `-o json` the payload is the standard envelope with
`kind: "cliSchema"`; `data` carries its own integer `catalogVersion` (distinct from the
envelope `schemaVersion`) and the `command` tree. A generator drift test pins the catalog to
`Cli::command()`. No subcommands or args.

---

## Source of truth

- Command/subcommand/flag definitions: `crates/flowplane/src/cli/commands.rs`
- Top-level command dispatch and the `serve`, `db`, `openapi`, `completion`, `version` commands: `crates/flowplane/src/main.rs`
- Global options and env-var fallbacks: `crates/flowplane/src/cli/config.rs`

Shell completions are generated via the `completion` command (`flowplane completion <shell>`), which renders the script for the requested shell to stdout.
