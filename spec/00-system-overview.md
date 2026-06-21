# 00 — System Overview

Extracted from Flowplane v1 (v0.2.10, commit 3a510a4). This file is the orientation map; the numbered specs that follow are the authoritative behavioral contract per subsystem.

## What Flowplane is

Flowplane is an API gateway **control plane** for an Envoy data plane, built for two audiences at once: human operators and AI agents. Its pitch: publish your APIs once and get governance (auth, rate limits, audit), a managed Envoy proxy, **schema learning** that infers OpenAPI from live traffic, and an **MCP server** so LLMs can both manage the gateway and call the proxied APIs as tools.

The control plane is out-of-band of request traffic. It stores gateway configuration in PostgreSQL, builds Envoy protobuf from it, and pushes it over xDS. Envoy serves all end-user traffic and never persists configuration — on restart it reconnects and rebuilds its world from a fresh xDS push. **Every Envoy snapshot is a deterministic projection of database state**; there is no out-of-band Envoy configuration.

## Process topology

Two shipped binaries plus an optional sidecar service:

| Binary | Role |
|---|---|
| `flowplane-cp` | Control plane + CLI in one binary. Serves REST API + UI + MCP on :8080, xDS gRPC (ADS) on :18000, plus ALS (access log service), ExtProc, and a diagnostics gRPC service. |
| `flowplane-agent` | Sidecar next to each Envoy. Polls Envoy `/config_dump` over loopback, extracts listener/cluster/route warming failures, streams `DiagnosticsReport`s to the CP over a bidi gRPC stream. MVP scope is warming-failure reporting only. |
| `flowplane-rls` (crate) | Standalone global rate-limit service implementing Envoy's RateLimitService gRPC API, with its own admin endpoint, counter store, and per-team limit domains. |

Supporting infrastructure: PostgreSQL (source of truth), Zitadel (OIDC identity, prod mode), Envoy (data plane), optional Vault (secrets backend), OTel collector (observability).

`flowplane init` boots the whole local stack in containers; `flowplane expose <url>` provisions cluster + route config + listener in one command — the end-to-end "hello world" is two commands.

## The entity chain

A request through the gateway traverses, in order:

```
Listener (address:port + filter chain)
  → RouteConfig (bound to listener)
    → VirtualHost (Host-header match)
      → Route (path match + action)
        → Cluster (backend + LB + resilience)
          → Endpoints
```

Each entity is independently addressable, versioned, and **team-scoped**. Filters (JWT auth, OAuth2, CORS, local + global rate limit, header mutation, ext authz, RBAC, compression, custom response, MCP traffic validation) attach at listener scope (global) or route/virtual-host scope (per-route overrides). A **Dataplane** is a registered Envoy instance tied to a team; it only receives resources visible to that team.

## Management surfaces

Three surfaces converge on one service layer (v1's stated design; the critique spec assesses how true this holds):

| Surface | Entrypoint | Primary user |
|---|---|---|
| CLI | `flowplane <noun> <verb>` (REST client) | humans, CI |
| MCP | `POST /api/v1/mcp` (streamable HTTP) | AI agents |
| REST + Web UI | `/api/v1/*`, SvelteKit app on :8080 | programmatic clients, browsers |

Call path: surface handler → `internal_api/` (surface-agnostic team validation + permission checks) → `services/` (the only layer that mutates state) → `storage/` (SQLx repositories) and `xds/state.rs` (in-memory snapshot cache per dataplane).

MCP tools split by plane and prefix:

- `cp_*` — control-plane CRUD on gateway resources (fixed set, same on every install).
- `api_*` — dynamic per-team tools that proxy calls through Envoy to upstream services; derived from registered upstream definitions / learned specs.
- `ops_*`, `devops_*`, `dev_*` — diagnostics: request tracing, deployment status, preflight checks, audit queries, xDS delivery status.

## Identity and tenancy (overview)

- Hierarchy: **organization → team → user**, with Zitadel-backed OIDC + RBAC in prod and a synthetic dev token + seeded dev org/team/user/dataplane in dev mode (`AuthMode::Dev`). First-run requires `FLOWPLANE_OIDC_PROJECT_ID`.
- One authorization gate (`src/auth/authorization.rs`) governs REST, MCP control plane, and MCP gateway calls.
- Dataplane identity is separate from operator identity: each Envoy/agent authenticates the xDS channel with **mandatory mTLS**, and its client cert carries a SPIFFE URI SAN (`spiffe://<trust-domain>/org/<org>/team/<team>/proxy/<name>`) that is bound to a team via a `proxy_certificates` DB row — TLS validates the cert, the row scopes the tenant.

## Data flow 1 — configuration change → Envoy update

1. CLI / MCP / REST handler validates input, calls `internal_api` → `services`.
2. Service writes PostgreSQL inside a transaction.
3. Service marks affected xDS resource types dirty and triggers a rebuild.
4. `xds/state.rs` rebuilds the protobuf snapshot for affected dataplanes.
5. The ADS gRPC server pushes resources (CDS/EDS/RDS/LDS/SDS multiplexed on one stream).
6. Envoy ACKs or NACKs; NACKs and warming failures surface in the audit log, `flowplane xds status` / `xds nacks`, and via agent diagnostics reports.

## Data flow 2 — the learning loop (config-first)

1. Operator creates a **learning session** targeting an existing route (a named recording window).
2. Live traffic flows through Envoy; the ALS/ExtProc gRPC services stream request/response data back to the CP.
3. A Tokio worker pool (`access_log_processor`) consumes the entries: schema inference with confidence scoring, batch DB writes, retry/backpressure.
4. Inferred schemas aggregate per endpoint (enum detection, path normalization, domain-model deduplication) and export as **OpenAPI 3.1**.
5. From learned/imported specs, per-team `api_*` MCP tools are generated so agents can call the upstream APIs through the gateway.

**Traffic-first (v2 addition):** v1 requires routes to exist before traffic can be learned from. The extent of this gap and the designed v2 behavior (capture unmatched traffic → generate spec → propose routes/clusters with approval + `--dry-run`) is specified in `06-learning.md`.

## Data flow 3 — MCP client → gateway API

An MCP client (Claude Code, Cursor) connects to the CP's MCP endpoint, authenticates as an operator, and either: manages the gateway (`cp_*`/`ops_*`, control-plane path), or invokes `api_*` tools whose execution proxies HTTP calls through Envoy to upstream services — including upstream MCP servers, which an MCP HTTP filter on the data path inspects and validates (JSON-RPC 2.0 and SSE).

## Where state lives

| Layer | Stores |
|---|---|
| PostgreSQL (`migrations/`) | All resources, orgs/teams/users/grants, learning sessions, inferred schemas, audit log, proxy certs |
| In-memory xDS cache | Last-published snapshot per dataplane (rebuilt on CP restart) |
| Secrets subsystem (`src/secrets/`, backends incl. env + Vault, encrypted at rest, audited, cached) | TLS certs, OAuth2 client secrets, JWKS material — delivered to Envoy via SDS |
| Local CLI config `~/.flowplane/` | Credentials, dev mTLS material, generated compose file |

## v1 module map (reference for the per-subsystem specs)

| Module | Responsibility | Spec file |
|---|---|---|
| `src/api/` | REST handlers, middleware, OpenAPI docs | 01 |
| `src/mcp/` | MCP server, tool registry, 16 tool modules, sessions/transport | 02 |
| `src/domain/`, `src/storage/`, `migrations/` | Pure domain types, repositories, schema | 03 |
| `src/xds/` | ADS server, resource builders, filter codecs, ALS/ExtProc/diagnostics, mTLS | 04 |
| `src/auth/`, `src/internal_api/auth.rs` | JWT validation, dev token, authorization gate | 05 |
| `src/schema/`, `src/services/{access_log_processor,learning_session_service,schema_aggregator,path_normalizer,schema_diff}`, `src/openapi/` | Learning pipeline | 06 |
| `src/cli/`, `ui/` | CLI implementation; SvelteKit UI (workflow inventory only) | 07 |
| `src/secrets/`, `src/services/secret_encryption.rs` | Secrets backends, encryption, SDS delivery | 04 + 08a |
| `src/config/`, `src/errors/`, `src/observability/`, `src/validation/` | Cross-cutting | 08, 10 |
| `crates/flowplane-rls` | Global rate-limit service | 01/04 (usage), 09 (vs token-aware AI limits) |
| `crates/flowplane-agent` | Dataplane diagnostics sidecar | 04 |

## Non-goals settled in v1 that carry into v2

- Envoy is the data plane; no multi-proxy abstraction.
- The CP stays out of the end-user request path (operator identity stops at the CP; data-path identity is enforced by filter chains the CP composes at config time).

## What v2 changes at the system level (forward pointers)

- No web UI; CLI + REST + MCP are the only surfaces (`07`, `12`).
- The learning loop becomes the central domain concept with explicit lifecycle states and bidirectional operation (`06`, `10`).
- Native AI gateway capability: LLM provider routing, credentials via secrets, token-aware usage/budgets, failover (`09`, `10`).
