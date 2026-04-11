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
| `-v, --verbose` | Verbose logging |

Most `create`, `list`, and `get` subcommands accept `-o, --output <FMT>` (`json`, `yaml`, `table`). Default is `json` for create/get, `table` for list.

---

## Stack, Auth & Config  →  [full reference](cli-stack.md)

```bash
flowplane init [--with-envoy] [--with-httpbin]    # Bootstrap dev environment
flowplane down [--volumes]                         # Stop stack (--volumes deletes data)
flowplane status [<NAME>]                          # System overview or listener lookup
flowplane doctor                                   # Diagnostic health checks
flowplane logs [-f]                                # Container logs
flowplane list                                     # List exposed services
```

```bash
flowplane auth login [--device-code]               # OIDC login (no-op in dev mode)
flowplane auth token                               # Print bearer token
flowplane auth whoami                              # Show current identity
flowplane auth logout                              # Clear credentials
```

```bash
flowplane config show|set|init|path                # Config management
flowplane database migrate|status|list|validate    # DB admin
```

---

## Resources  →  [full reference](cli-resources.md)

```bash
flowplane cluster create|list|get|update|delete    # Upstream endpoints
flowplane listener create|list|get|update|delete   # Envoy listeners
flowplane route create|list|get|update|delete      # Route configs
flowplane filter create|list|get|update|delete     # HTTP filters
flowplane filter attach|detach|types|type|scaffold # Filter management
flowplane team create|list|get|update|delete       # Teams (prod mode)
flowplane dataplane list|get|create|update|delete|config  # Dataplanes
flowplane vhost list|get                           # Virtual hosts
flowplane secret create|list|get|rotate|delete     # Secrets (SDS)
flowplane apply -f <FILE_OR_DIR>                   # Declarative create-or-update
```

---

## API Management  →  [full reference](cli-api.md)

```bash
flowplane expose <UPSTREAM> [--name N] [--path P]... [--port P]   # One-command expose
flowplane unexpose <NAME>                                          # Remove exposed service
```

```bash
flowplane learn start|stop|list|get|cancel|activate|health|export
flowplane schema list|get|compare|export
flowplane import openapi|list|get|delete
```

---

## Ops & Diagnostics  →  [full reference](cli-ops.md)

```bash
flowplane trace <PATH> [--port P]                  # Trace request routing
flowplane topology                                 # Gateway layout
flowplane validate                                 # Config validation
flowplane xds status|nacks                         # xDS delivery status
flowplane audit list                               # Audit trail
flowplane route-views list|stats                   # Route views
flowplane stats overview|clusters|cluster           # Statistics
```

---

## Admin & Platform  →  [full reference](cli-admin.md)

```bash
flowplane admin scopes|resources|reload-filter-schemas   # Platform admin
flowplane org list|get|create|delete|members              # Organizations
flowplane agent list|create|delete                        # Machine identities
flowplane mcp tools|enable|disable                        # MCP tool management
flowplane wasm list|get|create|update|delete|download     # WASM filters
flowplane mtls status                                     # mTLS status
flowplane cert list|get|create|revoke                     # Proxy certificates
```

---

## Gotchas

> **Upstream URLs in `expose`:** Use Docker service hostnames (e.g., `httpbin`), not `localhost`. The proxy runs inside Docker.

> **Port range:** Envoy serves traffic on ports 10000-10020. The `expose` auto-assign pool is 10001-10020.

> **File format:** `-f FILE` accepts JSON (`.json`) and YAML (`.yaml`/`.yml`). Format is detected by extension.

> **Output formats:** `-o table` for humans, `-o json` for scripting.

> **Team context:** Most resource commands need `--team`. Set a default: `flowplane config set team <NAME>`. In dev mode, `flowplane init` sets this to `default` automatically.

> **Filter config nesting:** Filters use `"config": {"type": "<filterType>", "config": {...}}` — not a flat structure. The inner `type` must match the `filterType`.

> **Route config update is full replacement:** `flowplane route update` replaces the entire `virtualHosts` array. Always fetch with `flowplane route get` first.
