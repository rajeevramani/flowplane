> Audience: api-consumers, operators · Status: stable

# REST API Reference

Conventions and the endpoint catalogue for the Flowplane control-plane REST API served under `/api/v1`.

## Conventions

These conventions apply uniformly across the secured API surface. They are enforced in code (`crates/fp-api/src`) and reflected in the generated OpenAPI document.

### Base path

All application endpoints are served under `/api/v1`. Operational endpoints (`/healthz`, `/readyz`, `/metrics`, `/api-docs/openapi.json`) are served at the root.

### Authentication (Bearer)

The API uses HTTP Bearer authentication. The OpenAPI document declares a single security scheme, `bearerAuth` (`http`/`bearer`), applied globally. Send credentials as:

```
Authorization: Bearer <token>
```

Two principal kinds are accepted:

- **User tokens** — OIDC-issued access tokens, validated against the configured issuer/audience (`FLOWPLANE_OIDC_ISSUER` / `FLOWPLANE_OIDC_AUDIENCE`). On first sight a user is JIT-provisioned, then loaded into the request's principal context.
- **Agent tokens** — long-lived tokens with the `fpat_` prefix. A token beginning with `fpat_` is routed to agent authentication and resolved to an agent principal (its org is fixed by the token; no org selector applies).

A missing or unparseable bearer token yields `401`. Authentication failures are audited best-effort.

Two endpoints sit outside the secured surface and do **not** use the global Bearer scheme:

- `POST /api/v1/bootstrap/initialize` is guarded by the one-shot, operator-supplied bootstrap token (`Authorization: Bearer <token>`). See [How-to: bootstrap the first platform admin](../how-to/bootstrap-platform.md).
- `GET /api/v1/bootstrap/status`, `/healthz`, `/readyz`, `/metrics`, and `/api-docs/openapi.json` are public.

### Active-org selector (`X-Flowplane-Org`)

For tenant-scoped operations a user principal must resolve to exactly one active org. The active org is chosen by the D-014 policy:

- **Selector present** — the `X-Flowplane-Org` header (an org **name** or **UUID**) must resolve to an org the caller is actually a member of; otherwise the request fails closed and the caller is told to pick a valid org.
- **Selector absent, exactly one non-platform membership** — that org is used implicitly.
- **Selector absent, zero or two-or-more candidates** — no active org is set. With two-or-more candidates the response indicates a selector is required.

The platform org is never an inferable or selectable tenant context. Agent principals ignore this header (their org is fixed). Use `GET /api/v1/auth/whoami` to inspect the resolved org, whether a selector is required, and the selectable memberships.

### Optimistic concurrency (`If-Match`)

Mutations on revisioned resources require the current resource revision, sent as a plain integer in the `If-Match` header. This applies to `PATCH` and `DELETE`, and to revision-guarded `POST` mutations such as `POST /api/v1/teams/{team}/secrets/{name}/rotate`:

```
If-Match: <revision>
```

The revision is returned as the `revision` field on each resource view (and increments on update). A missing or unparseable `If-Match` yields a validation error (`validation_failed` → `400`); a stale revision yields a conflict (`revision_mismatch` → `409`). Read the resource, then echo its `revision`.

### Pagination envelope

Most paged collection endpoints (clusters, listeners, route-configs, api-definitions, learning/discovery sessions, dataplanes, secrets, ai providers/routes/budgets, etc.) accept two `ListQuery` query parameters:

| Parameter | Default | Notes |
|-----------|---------|-------|
| `limit`   | `50`    | Clamped to `1..=500`. |
| `offset`  | `0`     | Floored at `0`. |

Not every collection uses this. Some endpoints return a plain JSON array or a custom query/response shape instead — for example `GET /api/v1/teams`, `GET /api/v1/orgs`, `GET /api/v1/agents`, `GET /api/v1/teams/{team}/proxy-certificates`, `GET /api/v1/teams/{team}/xds/nacks`, and `GET /api/v1/teams/{team}/ai/usage`. The generated OpenAPI document is authoritative for each endpoint's exact request/response shape.

Endpoints that use `ListQuery` return the uniform `Page<T>` envelope:

```json
{
  "items": [ ... ],
  "total": 0,
  "limit": 50,
  "offset": 0
}
```

### Request correlation (`x-request-id`, `traceparent`)

- Every request is assigned a request id, honoring a syntactically valid inbound `x-request-id` header (otherwise generated). The id is echoed in the `x-request-id` response header and included in the error envelope as `request_id`.
- A W3C `traceparent` header on the request is honored, joining Flowplane spans to the caller's distributed trace.

### Errors

Errors always use the envelope `{code, message, hint?, details?, request_id}`. Status codes and error codes are documented in [./errors.md](./errors.md) — not restated here.

## Endpoint catalogue

Grouped by resource. Per-field request/response schemas are in the generated OpenAPI document (see [Source of truth](#openapi-source-of-truth)). Team-scoped paths take `{team}` as a team **name** (within the active org) or **UUID**.

### Auth

| Method | Path |
|--------|------|
| GET | `/api/v1/auth/whoami` |

### Bootstrap (public)

| Method | Path |
|--------|------|
| GET  | `/api/v1/bootstrap/status` |
| POST | `/api/v1/bootstrap/initialize` |

### Organizations

| Method | Path |
|--------|------|
| GET    | `/api/v1/orgs` |
| POST   | `/api/v1/orgs` |
| GET    | `/api/v1/orgs/{org}` |
| DELETE | `/api/v1/orgs/{org}` |
| GET    | `/api/v1/orgs/{org}/members` |
| POST   | `/api/v1/orgs/{org}/members` |
| DELETE | `/api/v1/orgs/{org}/members/{user_id}` |

### Teams (members & grants)

| Method | Path |
|--------|------|
| GET    | `/api/v1/teams` |
| POST   | `/api/v1/teams` |
| DELETE | `/api/v1/teams/{team}` |
| GET    | `/api/v1/teams/{team}/members` |
| POST   | `/api/v1/teams/{team}/members` |
| DELETE | `/api/v1/teams/{team}/members/{user_id}` |
| GET    | `/api/v1/teams/{team}/grants` |
| POST   | `/api/v1/teams/{team}/grants` |
| DELETE | `/api/v1/teams/{team}/grants/{grant_id}` |

### Agents

| Method | Path |
|--------|------|
| GET  | `/api/v1/agents` |
| POST | `/api/v1/agents` |
| GET  | `/api/v1/agents/{agent_id}` |
| POST | `/api/v1/agents/{agent_id}/rotate-token` |
| POST | `/api/v1/agents/{agent_id}/disable` |

### Clusters

| Method | Path |
|--------|------|
| GET    | `/api/v1/teams/{team}/clusters` |
| POST   | `/api/v1/teams/{team}/clusters` |
| GET    | `/api/v1/teams/{team}/clusters/{name}` |
| PATCH  | `/api/v1/teams/{team}/clusters/{name}` |
| DELETE | `/api/v1/teams/{team}/clusters/{name}` |

### Listeners

| Method | Path |
|--------|------|
| GET    | `/api/v1/teams/{team}/listeners` |
| POST   | `/api/v1/teams/{team}/listeners` |
| GET    | `/api/v1/teams/{team}/listeners/{name}` |
| PATCH  | `/api/v1/teams/{team}/listeners/{name}` |
| DELETE | `/api/v1/teams/{team}/listeners/{name}` |

### Route configs

| Method | Path |
|--------|------|
| GET    | `/api/v1/teams/{team}/route-configs` |
| POST   | `/api/v1/teams/{team}/route-configs` |
| GET    | `/api/v1/teams/{team}/route-configs/{name}` |
| PATCH  | `/api/v1/teams/{team}/route-configs/{name}` |
| DELETE | `/api/v1/teams/{team}/route-configs/{name}` |

### API definitions (+ specs)

| Method | Path |
|--------|------|
| GET    | `/api/v1/teams/{team}/api-definitions` |
| POST   | `/api/v1/teams/{team}/api-definitions` |
| GET    | `/api/v1/teams/{team}/api-definitions/{name}` |
| DELETE | `/api/v1/teams/{team}/api-definitions/{name}` |
| GET    | `/api/v1/teams/{team}/api-definitions/{name}/status` |
| POST   | `/api/v1/teams/{team}/api-definitions/{name}/specs/{version}/reject` |
| POST   | `/api/v1/teams/{team}/api-definitions/{name}/specs/{version}/publish` |

### MCP (tools & status)

| Method | Path |
|--------|------|
| GET   | `/api/v1/teams/{team}/mcp/status` |
| GET   | `/api/v1/teams/{team}/mcp/connections` |
| PATCH | `/api/v1/teams/{team}/mcp/tools/{name}` |
| POST  | `/api/v1/mcp` |

> `POST /api/v1/mcp` is the MCP JSON-RPC endpoint. It is mounted on the secured router but registered outside the `routes!` set, so it does not appear in the generated OpenAPI document.

### Learning sessions

| Method | Path |
|--------|------|
| GET    | `/api/v1/teams/{team}/learning-sessions` |
| POST   | `/api/v1/teams/{team}/learning-sessions` |
| GET    | `/api/v1/teams/{team}/learning-sessions/{session}` |
| DELETE | `/api/v1/teams/{team}/learning-sessions/{session}` |
| POST   | `/api/v1/teams/{team}/learning-sessions/{session}/stop` |
| POST   | `/api/v1/teams/{team}/learning-sessions/{session}/spec-version` |

### Discovery sessions

| Method | Path |
|--------|------|
| GET  | `/api/v1/teams/{team}/learning-discovery-sessions` |
| POST | `/api/v1/teams/{team}/learning-discovery-sessions` |
| GET  | `/api/v1/teams/{team}/learning-discovery-sessions/{session}` |
| POST | `/api/v1/teams/{team}/learning-discovery-sessions/{session}/stop` |
| POST | `/api/v1/teams/{team}/learning-discovery-sessions/{session}/spec-versions` |

### Expose

| Method | Path |
|--------|------|
| POST   | `/api/v1/teams/{team}/expose` |
| DELETE | `/api/v1/teams/{team}/expose/{name}` |

### Route generation plans

| Method | Path |
|--------|------|
| POST | `/api/v1/teams/{team}/route-generation-plans` |
| POST | `/api/v1/teams/{team}/route-generation-plans/{plan_id}/apply` |

### AI (providers, routes, budgets, usage)

| Method | Path |
|--------|------|
| GET    | `/api/v1/teams/{team}/ai/providers` |
| POST   | `/api/v1/teams/{team}/ai/providers` |
| GET    | `/api/v1/teams/{team}/ai/providers/{name}` |
| PATCH  | `/api/v1/teams/{team}/ai/providers/{name}` |
| DELETE | `/api/v1/teams/{team}/ai/providers/{name}` |
| GET    | `/api/v1/teams/{team}/ai/routes` |
| POST   | `/api/v1/teams/{team}/ai/routes` |
| GET    | `/api/v1/teams/{team}/ai/routes/{name}` |
| PATCH  | `/api/v1/teams/{team}/ai/routes/{name}` |
| DELETE | `/api/v1/teams/{team}/ai/routes/{name}` |
| GET    | `/api/v1/teams/{team}/ai/budgets` |
| POST   | `/api/v1/teams/{team}/ai/budgets` |
| GET    | `/api/v1/teams/{team}/ai/budgets/{name}` |
| PATCH  | `/api/v1/teams/{team}/ai/budgets/{name}` |
| DELETE | `/api/v1/teams/{team}/ai/budgets/{name}` |
| GET    | `/api/v1/teams/{team}/ai/usage` |

### Dataplanes (+ proxy certificates, telemetry, envoy config)

| Method | Path |
|--------|------|
| GET    | `/api/v1/teams/{team}/dataplanes` |
| POST   | `/api/v1/teams/{team}/dataplanes` |
| GET    | `/api/v1/teams/{team}/dataplanes/{name}` |
| POST   | `/api/v1/teams/{team}/dataplanes/{name}/telemetry` |
| GET    | `/api/v1/teams/{team}/dataplanes/{name}/envoy-config` |
| GET    | `/api/v1/teams/{team}/proxy-certificates` |
| POST   | `/api/v1/teams/{team}/proxy-certificates` |
| POST   | `/api/v1/teams/{team}/proxy-certificates/issue` |
| POST   | `/api/v1/teams/{team}/proxy-certificates/{serial_number}/revoke` |

### Stats

| Method | Path |
|--------|------|
| GET | `/api/v1/teams/{team}/stats/overview` |

### Secrets

| Method | Path |
|--------|------|
| GET  | `/api/v1/teams/{team}/secrets` |
| POST | `/api/v1/teams/{team}/secrets` |
| GET  | `/api/v1/teams/{team}/secrets/{name}` |
| POST | `/api/v1/teams/{team}/secrets/{name}/rotate` |

### xDS status & ops

| Method | Path |
|--------|------|
| GET | `/api/v1/teams/{team}/xds/nacks` |
| GET | `/api/v1/teams/{team}/xds/status` |
| GET | `/api/v1/teams/{team}/ops/trace` |

### Operational (root, public)

| Method | Path |
|--------|------|
| GET | `/healthz` |
| GET | `/readyz` |
| GET | `/metrics` |
| GET | `/api-docs/openapi.json` |

## OpenAPI: source of truth

The **generated OpenAPI document is the source of truth** for per-field request and response schemas. The router and the document are built from the same `routes!` registration, so they cannot drift. Per-field detail is intentionally **not** hand-copied into this reference (it would drift).

Obtain the document:

- `GET /api-docs/openapi.json` — served by a running control plane.
- `flowplane openapi` — prints the exact document this binary serves.

## Source of truth

- `crates/fp-api/src/routes.rs` — router assembly (`build_router`, `secured_api`), Bearer security scheme, middleware chain (request-id → auth → write-throttle), health/openapi/bootstrap routes, `whoami`.
- `crates/fp-api/src/resources.rs` — `ListQuery`, `Page<T>`, `revision_from` (`If-Match`), and the gateway-resource (`clusters`/`listeners`/`route-configs`) endpoint macro.
- `crates/fp-api/src/auth.rs` — Bearer/`fpat_` authentication and the `x-flowplane-org` active-org selector (D-014 policy).
- `crates/fp-api/src/middleware.rs` — `x-request-id` and W3C `traceparent` handling.
- `crates/fp-api/src/*_api.rs` — per-resource handlers and their `#[utoipa::path]` declarations.
- `crates/flowplane/src/main.rs` — the `flowplane openapi` command (calls `fp_api::routes::openapi_document`).

Regenerate the document with: `flowplane openapi`.
