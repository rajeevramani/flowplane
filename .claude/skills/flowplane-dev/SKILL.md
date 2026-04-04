---
name: flowplane-dev
description: Flowplane development knowledge — architecture, domain model, auth modes, boot lifecycle, module map, filter system, and dev workflow. Use when writing Flowplane code, adding features, understanding architecture, writing docs, working with dev/prod mode, boot, auth, filters, or the domain model. Do NOT use for end-user operations (use flowplane-ops) or API configuration (use flowplane-api).
license: Apache-2.0
metadata:
  author: rajeevramani
  version: "0.1.0"
---

# Flowplane Development

Flowplane is an MCP-first Envoy control plane. It translates high-level JSON REST APIs into Envoy's xDS protocol (ADS with LDS, RDS, CDS, EDS, SDS). Teams expose backend services as MCP tools for Claude Code and other MCP clients. Three interaction surfaces: CLI (`flowplane` binary), MCP tools (`/api/v1/mcp`), and REST API (`/api/v1/`).

## Quick Path — Zero to Working Gateway

```bash
make build                                      # Build images (first time only)
make up ENVOY=1 HTTPBIN=1                       # Start stack (prod mode)
make seed                                        # Seed demo data
flowplane auth login                             # Authenticate CLI
flowplane expose http://httpbin:80 --name demo   # Expose httpbin through Envoy
curl http://localhost:10001/get                  # Test via gateway
```

The `expose` command (`src/cli/expose.rs`) wraps cluster + route config + listener creation into one call. For manual control over each resource, see the `flowplane-api` skill. Swagger UI is at `http://localhost:8080/swagger-ui/`.

## 1. Auth Modes — Read This First

Every operation — boot, CLI, seeding, team model — depends on the auth mode. Detect it before doing anything.

| | Dev Mode | Prod Mode (default) |
|---|---|---|
| **Env var** | `FLOWPLANE_AUTH_MODE=dev` | `FLOWPLANE_AUTH_MODE=prod` (or unset) |
| **Purpose** | Local development, zero config | Production, multi-tenant |
| **Auth** | Opaque 43-char base64 token | Zitadel OIDC + JWT |
| **Identity** | `dev@flowplane.local`, scope `org:dev-org:admin` | From JWT claims, JIT-provisioned |
| **CLI login** | No-op (token from `flowplane init`) | PKCE browser flow or device code |
| **Credentials** | Plain text in `~/.flowplane/credentials` | JSON in `~/.flowplane/credentials` |
| **Team model** | Single implicit `default` team | Multi-tenant, DB-backed grants |
| **Seeding** | Auto on startup (org, team, user, dataplane) | Manual: `make setup-zitadel` + `make seed` |
| **Docker** | `docker-compose-dev.yml` (no Zitadel) | `docker-compose.yml` (includes Zitadel) |
| **Boot** | `flowplane init [--with-envoy] [--with-httpbin]` | `make up [ENVOY=1] [HTTPBIN=1]` |

**Detect current mode:**
- `GET /api/v1/auth/mode` — returns `{"auth_mode":"dev"}` or `{"auth_mode":"prod",...}`
- Credentials file format: plain text = dev, JSON with `access_token` = prod
- Check `FLOWPLANE_AUTH_MODE` in `.env` or docker-compose environment

## 2. Boot Lifecycle

### Dev Mode — One Command

**Prerequisite:** Docker images must exist. Run `make build` first if this is a fresh checkout.

```bash
flowplane init --with-envoy --with-httpbin
```
This: detects Docker/Podman (probes standard Docker, OrbStack, Rancher Desktop, rootless Podman sockets), generates a dev token, generates or reuses a secret encryption key (`~/.flowplane/encryption.key`), writes `docker-compose-dev.yml` to `~/.flowplane/`, starts containers (PostgreSQL + control plane + optional Envoy/httpbin), waits for health, writes token to `~/.flowplane/credentials`, sets `base_url`/`team`/`org` in config. Secrets are enabled by default — the encryption key persists across restarts so encrypted secrets survive `flowplane down` + `flowplane init` cycles.

Services after boot: API at `localhost:8080`, xDS at `localhost:18000`, Envoy at `localhost:10000` (if `--with-envoy`), httpbin at `localhost:8000` (if `--with-httpbin`).

### Prod Mode — Three Steps

**Prerequisite:** Docker images must exist. Run `make build` first if this is a fresh checkout.

```bash
make up ENVOY=1 HTTPBIN=1    # Start stack (auto-runs setup-zitadel on first run)
make seed                     # Seed demo data (org, users, teams)
make seed-info                # Show credentials
```

First `make up` detects missing `.env.zitadel` and runs `scripts/setup-zitadel.sh` automatically. Subsequent runs skip setup.

Services: API at `localhost:8080`, Zitadel at `localhost:8081`, xDS at `localhost:50051`, Envoy at `localhost:10000` (if `ENVOY=1`), httpbin at `localhost:8000` (if `HTTPBIN=1`).

**Default credentials (prod):** `demo@acme-corp.com` / `Flowplane1!` (primary login, org: acme-corp, team: engineering). Platform admin: `admin@flowplane.local` / `Flowplane1!` (governance only). Run `make seed-info` for all credentials.

### Optional Services
| Flag | Service | Purpose |
|---|---|---|
| `ENVOY=1` | Envoy proxy | Data plane, ports 10000-10020 |
| `HTTPBIN=1` | httpbin | Demo backend on port 8000 |
| `MOCKBACKEND=1` | MockBank API | Demo API on port 3001 |

### Boot Gotchas

**Network conflict switching modes:** `flowplane init` auto-removes stale networks, but switching from prod→dev or vice versa can still cause a Docker network conflict (`flowplane-network` label mismatch). Fix: `docker network rm flowplane-network`, then re-run.

**xDS port differs between modes:** Dev uses port **18000**, prod uses **50051**. Configure Envoy bootstrap or xDS clients accordingly.

**Images must be built first:** Run `make build` before first boot. Without it you get "image not found".

See [references/boot-modes.md](references/boot-modes.md) for full step-by-step recipes.

## 3. Domain Model

Core entity chain — this is the request flow through the gateway:

```
Listener (address:port + filter chain)
  → RouteConfig (routing rules, bound to listener)
    → VirtualHost (domain grouping, matched by Host header)
      → Route (path match + action)
        → Cluster (backend service + LB policy + resilience: circuit breakers, outlier detection, health checks)
          → Endpoints (actual server addresses)
```

**Filters** are standalone resources that attach to Listeners (global, via filter chain) or Routes/VirtualHosts (per-route, via `typedPerFilterConfig`). 15 filter types covering auth, rate limiting, CORS, header manipulation, compression, and more.

**Dataplanes** are registered Envoy instances that receive xDS config pushes via ADS (Aggregated Discovery Service).

**xDS mapping:**
| Flowplane Entity | Envoy xDS Resource |
|---|---|
| Cluster + Endpoints | CDS (Cluster Discovery) + EDS (Endpoint Discovery) |
| Listener | LDS (Listener Discovery) |
| RouteConfig | RDS (Route Discovery) |

See [references/domain-model.md](references/domain-model.md) for field details and relationships.

## 4. Three Interaction Surfaces

Same operations, different interfaces. All share the same service layer (`src/services/`).

| Surface | Entry Point | When to Use |
|---|---|---|
| **CLI** | `flowplane <command>` | Local dev, scripts, CI/CD |
| **MCP Tools** | `POST /api/v1/mcp` | AI agents (Claude Code, etc.) |
| **REST API** | `GET/POST/PUT/DELETE /api/v1/...` | Programmatic access, UI |

All three converge on a unified Internal API layer (`src/internal_api/`) which handles team validation, permission checks, and delegates to services. The CLI calls the REST API over HTTP; MCP tools call the internal API directly.

## 5. Module Map

| Directory | Owns | Key Files |
|---|---|---|
| `src/api/` | REST handlers, middleware, routes | `routes.rs`, `handlers/*.rs` |
| `src/auth/` | Auth middleware, authorization, dev token | `middleware.rs`, `authorization.rs`, `dev_token.rs`, `team.rs` |
| `src/cli/` | CLI entry point, all subcommands | `mod.rs` (clap definitions), `compose.rs`, `filter.rs`, `expose.rs` |
| `src/config/` | Config management, env vars | `mod.rs` (AuthMode), `settings.rs` |
| `src/domain/` | Domain types (Cluster, Listener, Route, Filter, etc.) | One file per entity type |
| `src/mcp/` | MCP server, tool registry, tools | `handler.rs`, `tool_registry.rs`, `tools/*.rs` |
| `src/services/` | Business logic layer | Shared by REST, MCP, and CLI |
| `src/storage/` | SQLx repositories, migrations | `migrations/*.sql`, `repos/*.rs` |
| `src/xds/` | Envoy control plane, xDS protocol | `server.rs`, `filters/http/*.rs` |
| `src/validation/` | Input validation | Shared validators |
| `src/observability/` | Tracing, metrics | OpenTelemetry setup |

**MCP tool categories:**
- **control_plane** (`cp_*`) — Manage gateway config: create/update/delete clusters, listeners, routes, filters. Scopes: `team:{name}:cp:read`, `team:{name}:cp:write`.
- **gateway_api** (`gw_*`) — Proxy to upstream services through the gateway. Scopes: `team:{name}:api:read`, `team:{name}:api:execute`.

**Single auth enforcement:** All authorization goes through `check_resource_access()` in `src/auth/authorization.rs` — REST, MCP CP, and MCP Gateway all converge here.

See [references/module-map.md](references/module-map.md) for the full breakdown.

## 6. Filter System

11 filter types in the `FilterType` enum: `header_mutation`, `jwt_auth`, `cors`, `compressor`, `local_rate_limit`, `rate_limit` (not implemented), `ext_authz`, `rbac`, `oauth2`, `custom_response`, `mcp`. Additional xDS modules exist for `wasm`, `credential_injector`, `ext_proc`, `health_check`, `rate_limit_quota` but these are NOT registered as FilterTypes and cannot be created via API. Grouped by category:

| Category | Filters |
|---|---|
| **Auth** | `oauth2`, `jwt_auth`, `ext_authz`, `rbac` |
| **Rate Limiting** | `local_rate_limit`, `rate_limit` (not implemented) |
| **Manipulation** | `header_mutation`, `custom_response`, `cors` |
| **Processing** | `compressor` |
| **Protocol** | `mcp` |

### Create → Attach → Verify Workflow
1. `cp_list_filter_types` — discover available types and schemas
2. `cp_create_filter` — create with type-specific configuration
3. `cp_attach_filter` — attach to a listener (global) or route config (scoped)
4. `cp_get_filter` — verify attachment in `listenerInstallations` or `routeConfigInstallations`

**Listener-level** filters apply to all requests on that listener. **Route-level** overrides via `typedPerFilterConfig` customize or disable filters per-route/per-virtual-host. Not all filter types support per-route overrides — check the `Per-Route` column in the filter table.

**Source:** Filter type implementations in `src/xds/filters/http/` (one file per type). Filter domain types in `src/domain/filter.rs`.

See [references/filter-types.md](references/filter-types.md) for detailed config examples.

## 7. Dev Workflow

**Branch convention:**
```
feature/<description> → release/vX.Y.Z → main
bugfix/<description>  → release/vX.Y.Z → main
```
Always branch from the current release branch, never from main.

**Task tracking:** Use `bd` CLI (Beads) — `bd create`, `bd ready`, `bd close`.

**Quality gate — run before every commit:**
```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings && cargo test
```

**Critical rules** (see CLAUDE.md for the full list):
- Never `unwrap()`/`expect()` in production code
- Never `any` in TypeScript
- Always write tests (unit + integration + E2E)
- Always enforce team isolation in DB queries
- Never commit without passing fmt/clippy/test

**Testing layers:**
1. **Unit** — alongside the code, same agent
2. **Integration** — spec-driven (not implementation-driven), real DB, different agent
3. **E2E** — against running Docker stack, `tests/e2e/` with `TestHarness`

## 8. Key Source Paths Quick Reference

```
src/cli/mod.rs              — CLI clap definitions (all subcommands)
src/cli/compose.rs          — flowplane init/down
src/auth/middleware.rs       — Dev + prod auth middleware
src/auth/authorization.rs   — check_resource_access()
src/startup.rs              — Dev mode auto-seeding
src/config/mod.rs           — AuthMode enum, env var parsing
src/mcp/tool_registry.rs    — MCP tool registration + authorization
src/xds/filters/http/       — Filter type implementations
src/domain/filter_schema.rs — Filter schema definitions (YAML-loaded)
src/services/filter_validation.rs — Filter config validation
filter-schemas/built-in/    — YAML schema files per filter type
Makefile                    — Boot targets, test targets
docker-compose.yml          — Prod mode stack
docker-compose-dev.yml      — Dev mode stack
scripts/setup-zitadel.sh    — First-time Zitadel config
scripts/seed-demo.sh        — Demo data seeding
```

## References

- [references/boot-modes.md](references/boot-modes.md) — Step-by-step boot recipes for dev and prod modes
- [references/auth-internals.md](references/auth-internals.md) — How auth works in the code
- [references/domain-model.md](references/domain-model.md) — Detailed entity relationships and xDS mapping
- [references/module-map.md](references/module-map.md) — Every src/ directory with key files
- [references/filter-types.md](references/filter-types.md) — All filter types with config examples
- [references/frontend.md](references/frontend.md) — Frontend overview for backend devs (SvelteKit, API client, auth flow)
