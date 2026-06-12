# 01 — REST API Contract

Extracted from Flowplane v1 (v0.2.10). Sources: `src/api/routes.rs` (route table, middleware),
`src/api/error.rs`, `src/api/handlers/pagination.rs`, `src/api/docs.rs` (utoipa OpenAPI), and the
**generated OpenAPI document** committed alongside this file as
[`01-api-contract.v1-openapi.json`](01-api-contract.v1-openapi.json) — that JSON is the exact
machine contract for request/response schemas; this document is the human-level contract plus
facts the OpenAPI doc does not carry (middleware, limits, drift).

## 1. Conventions

### Base URL, versioning, serving

- All API routes live under `/api/v1/`. The same Axum server (port 8080) serves the REST API, the
  MCP endpoint, Swagger UI (`/swagger-ui`, OpenAPI at `/api-docs/openapi.json` — deliberately
  unauthenticated; static build-time document, accepted risk H9), and the SvelteKit SPA as a
  static fallback (`FLOWPLANE_UI_DIR`, default `./ui/build`). Unknown `/api/*` paths return JSON
  404 `{"error":"not_found","message":"API endpoint not found"}` rather than the SPA.

### Authentication & middleware stack (secured router, outermost-first)

1. **Auth layer**: Zitadel JWT bearer auth (`authenticate` middleware). Dev mode uses the same
   code path against an in-process mock OIDC server. If Zitadel is not configured at all, the
   server starts **degraded**: every secured endpoint returns
   503 `{"error":"Auth not configured — run setup-zitadel"}`.
2. **Per-tenant write throttle**: POST/PUT/PATCH/DELETE are rate-limited per tenant, keyed
   `org:{org_id}` (fallback `token:{token_id}`; requests with no AuthContext fail closed under a
   shared `unauthenticated` bucket). Over-limit → 429 with `Retry-After` header. Reads are not
   throttled.
3. **Dynamic scope layer** (`ensure_dynamic_scopes`) then OTel HTTP tracing.

Edge layers applied to *everything* (API + static fallback):

- **Body limit**: default 1 MiB (`FLOWPLANE_API_MAX_BODY_SIZE`, min 1024). The custom-WASM-filter
  upload route overrides with `MAX_WASM_BINARY_SIZE * 2` (base64 inflation headroom). Excess →
  413 `payload_too_large`.
- **Security headers** on every response: `X-Content-Type-Options: nosniff`,
  `X-Frame-Options: DENY`, `Referrer-Policy: strict-origin-when-cross-origin`, HSTS
  `max-age=31536000; includeSubDomains`, and CSP — for HTML:
  `frame-ancestors 'none'; object-src 'none'; base-uri 'self'` (script-src left to the SPA's
  meta CSP); for non-HTML: `default-src 'none'; script-src 'self'; frame-ancestors 'none';
  object-src 'none'; base-uri 'self'`.
- **CORS**: origins from `FLOWPLANE_UI_ORIGIN` (comma-separated; default
  `http://localhost:3000,http://localhost:6274`). `allow_credentials` is **only** enabled when
  origins are explicitly configured (finding H10 — defaults fail closed). Allowed headers include
  `x-csrf-token`, `mcp-protocol-version`, `mcp-session-id`, `last-event-id`; exposed:
  `x-csrf-token`, `mcp-session-id`.
- Startup safety check: `FLOWPLANE_COOKIE_SECURE=false` + an `https://` base URL panics at boot
  (refuses dangerous misconfiguration).

### Path-parameter naming (v1's documented convention)

| Param | Meaning |
|---|---|
| `{team}` | team name **or** UUID (dual lookup via `resolve_team_name`) |
| `{name}` | human-readable unique name (clusters, route configs, listeners, dataplanes) |
| `{id}` | UUID primary key (filters, secrets, certs, learning sessions, schemas) |
| `{<res>_name}` / `{<res>_id}` | parent resource in nested paths |
| `{org_name}`, `{agent_name}` | organization / agent name |

RPC-style action sub-paths (`/revoke`, `/export`, `/activate`, `/stop`, `/rotate`, `/invite`,
`/download`, `/enable`, `/disable`, `/refresh`) are used intentionally for non-CRUD transitions.

Path values containing NUL/control characters are rejected as 404 before DB lookup
(`validate_path_name`).

### Scoping tiers

- `/api/v1/teams/{team}/…` — team-scoped resource CRUD (most endpoints).
- `/api/v1/orgs/{org_name}/…` — org-scoped user-facing endpoints (teams, agents, grants).
- `/api/v1/admin/…` — platform-admin endpoints (org/team management, governance aggregates).
- A handful of cross-cutting authenticated endpoints sit at top level (`/api/v1/route-views`,
  `/api/v1/dataplanes`, `/api/v1/audit-logs`, `/api/v1/filter-types`, …).

### Error contract

All errors are JSON `{"error": "<kind>", "message": "<human text>"}`:

| Kind | Status | Notes |
|---|---|---|
| `bad_request` | 400 | validation, malformed JSON/query, FK violations ("Referenced resource does not exist: …") |
| `unauthorized` | 401 | missing/malformed/unknown/inactive bearer token |
| `forbidden` | 403 | authenticated, insufficient permission; cross-org violations |
| `not_found` | 404 | |
| `conflict` | 409 | duplicate names, unique/check constraint violations |
| `payload_too_large` | 413 | body over limit |
| `unprocessable_entity` | 422 | semantically invalid (e.g. self-invite) |
| `too_many_requests` | 429 | + `Retry-After: <seconds>` header |
| `internal_error` | 500 | message redacted to "An internal error occurred"; detail only in logs |
| `bad_gateway` | 502 | upstream dependency (Zitadel) failed after partial local commit |
| `service_unavailable` | 503 | degraded mode / overload |

Malformed JSON bodies produce JSON 400s (custom extractor), not Axum's text/plain default.

**v2 note:** the v1 error object has no machine-actionable `code` per failure (kind is coarse), no
`what-to-do` field, and no correlation/request id. The v2 contract upgrades this (see spec/10);
behavior above is the v1 baseline.

### Pagination

List endpoints use `?limit=` (default 50, clamped to [1, max]) and `?offset=` (default 0,
clamped ≥0), returning:

```json
{ "items": [...], "total": <i64>, "limit": <i64>, "offset": <i64> }
```

Field names are camelCase. **Not uniformly adopted** — see §4 drift notes.

### OpenAPI tags

Clusters, Listeners, Routes, Filters, Custom WASM Filters, Secrets, API Discovery,
Administration, System. Security scheme: HTTP bearer (`bearerAuth`), applied globally.

## 2. Endpoint inventory

Complete route table from `build_router_with_registry` (`src/api/routes.rs:526-975`). "Auth"
column: `public` = no auth; everything else is behind the bearer-auth layer (authorization
specifics per endpoint are in spec/05; admin endpoints additionally require platform-admin).
Exact request/response schemas: see the committed OpenAPI JSON.

### Public (unauthenticated)

| Method | Path | Behavior |
|---|---|---|
| GET | `/health` | health check |
| GET | `/api/v1/auth/mode` | reports dev vs prod auth mode |
| GET | `/api/v1/bootstrap/status` | first-run bootstrap state |
| POST | `/api/v1/bootstrap/initialize` | first-run initialization |
| GET | `/api/v1/scopes` | public scope list (token-creation UI) |
| GET | `/.well-known/openid-configuration` | OAuth metadata proxying to Zitadel (only if configured) |
| GET | `/api/v1/auth/config` | OIDC config for SPA runtime (only if `FLOWPLANE_OIDC_SPA_CLIENT_ID` set) |
| GET | `/api/v1/auth/login`, `/api/v1/auth/callback`, `/api/v1/auth/logout` | BFF OIDC flow → encrypted HttpOnly session cookie (only if OIDC + secret-encryption key configured) |
| GET | `/swagger-ui`, `/api-docs/openapi.json` | API docs (accepted-risk public) |

### Gateway resources (team-scoped)

| Method | Path | Behavior |
|---|---|---|
| GET/POST | `/api/v1/teams/{team}/clusters` | list / create cluster |
| GET/PUT/DELETE | `/api/v1/teams/{team}/clusters/{name}` | get / update / delete cluster |
| GET/POST | `/api/v1/teams/{team}/route-configs` | list / create route config |
| GET/PUT/DELETE | `/api/v1/teams/{team}/route-configs/{name}` | get / update / delete route config |
| GET/POST | `/api/v1/teams/{team}/listeners` | list / create listener |
| GET/PUT/DELETE | `/api/v1/teams/{team}/listeners/{name}` | get / update / delete listener |
| POST | `/api/v1/teams/{team}/expose` | one-shot expose: cluster + route config + listener |
| DELETE | `/api/v1/teams/{team}/expose/{name}` | unexpose (tear down the trio) |
| GET | `/api/v1/route-views` | flattened route view across teams the caller can see (UI) |
| GET | `/api/v1/route-views/stats` | route stats for the view |
| GET | `/api/v1/reports/route-flows` | route-flow report |

### Filters

| Method | Path | Behavior |
|---|---|---|
| GET/POST | `/api/v1/teams/{team}/filters` | list / create filter |
| GET/PATCH/DELETE | `/api/v1/teams/{team}/filters/{id}` | get / update / delete filter |
| GET | `/api/v1/teams/{team}/filters/{filter_id}/status` | aggregate install/config status |
| GET/POST | `/api/v1/teams/{team}/filters/{filter_id}/installations` | list / install on listener |
| DELETE | `/api/v1/teams/{team}/filters/{filter_id}/installations/{listener_id}` | uninstall |
| GET/POST | `/api/v1/teams/{team}/filters/{filter_id}/configurations` | list / add scoped config |
| DELETE | `/api/v1/teams/{team}/filters/{filter_id}/configurations/{scope_type}/{scope_id}` | remove scoped config |
| GET | `/api/v1/filter-types` | list registered filter types (dynamic framework) |
| GET | `/api/v1/filter-types/{filter_type}` | get one filter type schema |
| GET/POST | `/api/v1/teams/{team}/route-configs/{rc}/filters` | list / attach at route-config scope |
| DELETE | `/api/v1/teams/{team}/route-configs/{rc}/filters/{filter_id}` | detach |
| GET | `/api/v1/teams/{team}/route-configs/{rc}/virtual-hosts` | list vhosts |
| GET/POST | `…/virtual-hosts/{vhost}/filters` | list / attach at vhost scope |
| DELETE | `…/virtual-hosts/{vhost}/filters/{filter_id}` | detach |
| GET | `…/virtual-hosts/{vhost}/routes` | list route rules |
| GET/POST | `…/virtual-hosts/{vhost}/routes/{route}/filters` | list / attach at route-rule scope |
| DELETE | `…/virtual-hosts/{vhost}/routes/{route}/filters/{filter_id}` | detach |
| GET/POST | `/api/v1/teams/{team}/listeners/{listener}/filters` | list / attach at listener scope |
| DELETE | `/api/v1/teams/{team}/listeners/{listener}/filters/{filter_id}` | detach |
| GET/POST | `/api/v1/teams/{team}/custom-filters` | list / upload custom WASM filter (larger body limit) |
| GET/PATCH/DELETE | `/api/v1/teams/{team}/custom-filters/{id}` | get / update / delete |
| GET | `/api/v1/teams/{team}/custom-filters/{id}/download` | download WASM binary |

### Rate limiting (global RLS)

| Method | Path | Behavior |
|---|---|---|
| GET/POST | `/api/v1/teams/{team}/rate-limit-domains` | list / create RLS domain |
| GET/PUT/DELETE | `/api/v1/teams/{team}/rate-limit-domains/{id}` | get / update / delete |
| GET/POST | `/api/v1/teams/{team}/rate-limit-policies` | list / create policy |
| GET/PUT/DELETE | `/api/v1/teams/{team}/rate-limit-policies/{id}` | get / update / delete |
| GET/PUT | `/api/v1/admin/rate-limit-overrides/{org_name}/{team_name}` | per-team override (admin) |
| POST | `/api/v1/admin/rate-limit/reap-tombstones` | reap deleted-limit tombstones (admin) |
| POST | `/api/v1/admin/rls/force-repush` | force CP→RLS config repush (admin) |

### Secrets & certificates

| Method | Path | Behavior |
|---|---|---|
| GET/POST | `/api/v1/teams/{team}/secrets` | list / create secret (value stored encrypted) |
| POST | `/api/v1/teams/{team}/secrets/reference` | create reference to external backend (e.g. Vault) |
| GET/PATCH/DELETE | `/api/v1/teams/{team}/secrets/{secret_id}` | get (redacted) / update / delete |
| POST | `/api/v1/teams/{team}/secrets/{secret_id}/rotate` | rotate |
| GET/POST | `/api/v1/teams/{team}/proxy-certificates` | list / generate dataplane client cert (rate-limited to protect Vault PKI) |
| GET | `/api/v1/teams/{team}/proxy-certificates/{id}` | get |
| POST | `/api/v1/teams/{team}/proxy-certificates/{id}/revoke` | revoke |
| GET | `/api/v1/mtls/status` | xDS mTLS posture |

### Dataplanes

| Method | Path | Behavior |
|---|---|---|
| GET | `/api/v1/dataplanes` | list across visible teams |
| GET/POST | `/api/v1/teams/{team}/dataplanes` | list / register dataplane |
| GET/PATCH/DELETE | `/api/v1/teams/{team}/dataplanes/{name}` | get / update / delete |
| GET | `/api/v1/teams/{team}/dataplanes/{name}/envoy-config` | generate Envoy bootstrap |

### Learning & schemas (API discovery)

| Method | Path | Behavior |
|---|---|---|
| GET/POST | `/api/v1/teams/{team}/learning-sessions` | list / create session |
| GET/DELETE | `/api/v1/teams/{team}/learning-sessions/{id}` | get / delete |
| POST | `/api/v1/teams/{team}/learning-sessions/{id}/activate` | start capture |
| POST | `/api/v1/teams/{team}/learning-sessions/{id}/stop` | stop capture |
| GET | `/api/v1/teams/{team}/aggregated-schemas` | list learned schemas |
| GET | `/api/v1/teams/{team}/aggregated-schemas/{id}` | get |
| GET | `/api/v1/teams/{team}/aggregated-schemas/{id}/compare` | diff against another version |
| GET | `/api/v1/teams/{team}/aggregated-schemas/{id}/export` | export OpenAPI 3.1 |
| POST | `/api/v1/teams/{team}/aggregated-schemas/export` | export multiple |
| POST | `/api/v1/teams/{team}/openapi/import` | import an OpenAPI spec |
| GET | `/api/v1/teams/{team}/openapi/imports` | list imports |
| GET/DELETE | `/api/v1/openapi/imports/{id}` | get / delete import (NOT team-prefixed — see drift) |

### MCP management (REST side; protocol itself in spec/02)

| Method | Path | Behavior |
|---|---|---|
| POST/GET/DELETE | `/api/v1/mcp` | MCP Streamable HTTP endpoint (2025-11-25) |
| GET | `/api/v1/mcp/connections` | list active MCP connections |
| GET | `/api/v1/teams/{team}/mcp/tools` | list team's gateway tools |
| GET/PATCH | `/api/v1/teams/{team}/mcp/tools/{name}` | get / update tool (enable, rename, description) |
| GET | `/api/v1/teams/{team}/routes/{route_id}/mcp/status` | MCP enablement for a route |
| POST | `/api/v1/teams/{team}/routes/{route_id}/mcp/enable` / `…/disable` | toggle |
| POST | `/api/v1/teams/{team}/routes/{route_id}/mcp/refresh` | re-derive tool schema |
| POST | `/api/v1/teams/{team}/mcp/bulk-enable` / `…/bulk-disable` | bulk toggle |
| GET | `/api/v1/teams/{team}/mcp/routes/{route_id}/learned-schema` | check learned schema availability |
| POST | `/api/v1/teams/{team}/mcp/routes/{route_id}/apply-learned` | apply learned schema to tool |

### Ops diagnostics (team-scoped)

| Method | Path | Behavior |
|---|---|---|
| GET | `/api/v1/teams/{team}/ops/trace` | request tracing |
| GET | `/api/v1/teams/{team}/ops/topology` | resource topology |
| GET | `/api/v1/teams/{team}/ops/validate` | config preflight validation |
| GET | `/api/v1/teams/{team}/ops/xds/status` | xDS delivery status |
| GET | `/api/v1/teams/{team}/ops/xds/nacks` | NACK history |
| GET | `/api/v1/teams/{team}/ops/audit` | team audit query |
| GET | `/api/v1/teams/{team}/ops/audit/volume` | audit volume |
| GET | `/api/v1/teams/{team}/ops/learning/{id}/health` | learning-session pipeline health |

### Stats

| Method | Path | Behavior |
|---|---|---|
| GET | `/api/v1/stats/enabled` | whether stats dashboard app is enabled |
| GET | `/api/v1/teams/{team}/stats/overview` | team traffic overview (10s cache) |
| GET | `/api/v1/teams/{team}/stats/clusters` | per-cluster stats |
| GET | `/api/v1/teams/{team}/stats/clusters/{cluster}` | one cluster |

### Identity, orgs, teams, grants

| Method | Path | Behavior |
|---|---|---|
| GET | `/api/v1/auth/session` | current session: identity + DB-sourced permissions |
| GET | `/api/v1/teams` | teams visible to caller |
| GET | `/api/v1/orgs/current` | caller's org |
| GET/POST | `/api/v1/orgs/{org_name}/teams` | list / create team in org |
| PATCH/DELETE | `/api/v1/orgs/{org_name}/teams/{team_name}` | update / delete team |
| GET/POST | `/api/v1/orgs/{org_name}/teams/{team_name}/members` | list / add member |
| DELETE | `/api/v1/orgs/{org_name}/teams/{team_name}/members/{user_id}` | remove member |
| GET/POST | `/api/v1/orgs/{org_name}/agents` | list / create org agent (machine identity) |
| DELETE | `/api/v1/orgs/{org_name}/agents/{agent_name}` | delete agent |
| GET/POST | `/api/v1/orgs/{org_name}/principals/{principal_id}/grants` | list / create scope grant |
| DELETE | `/api/v1/orgs/{org_name}/principals/{principal_id}/grants/{grant_id}` | revoke grant |
| POST | `/api/v1/oauth/register` | OAuth DCR (org admin) |

### Platform admin

| Method | Path | Behavior |
|---|---|---|
| GET/POST | `/api/v1/admin/teams`; GET/PATCH/DELETE `…/{id}` | team management |
| GET/POST | `/api/v1/admin/organizations`; GET/PATCH/DELETE `…/{id}` | org management |
| GET/POST | `/api/v1/admin/organizations/{id}/members`; PUT/DELETE `…/{user_id}` | org membership |
| POST | `/api/v1/admin/organizations/{id}/invite` | invite via Zitadel admin API |
| GET | `/api/v1/admin/resources/summary` | cross-org resource summary |
| GET | `/api/v1/admin/audit/volume`, `/admin/xds/health`, `/admin/filters/by-org`, `/admin/orgs/activity`, `/admin/provisioning/recent`, `/admin/mcp/connections/summary` | governance dashboard aggregates (B1 set) |
| GET | `/api/v1/admin/audit/feed` | redacted cross-org audit feed |
| GET | `/api/v1/audit-logs` | audit log query (admin) |
| GET | `/api/v1/org/audit/logs`, `/api/v1/org/audit/volume` | org-admin audit lens |
| GET | `/api/v1/admin/scopes` | all scopes incl. hidden (`admin:all`) |
| POST | `/api/v1/admin/filter-schemas/reload` | reload filter schema registry |
| GET | `/api/v1/admin/apps`; GET/PUT `…/{app_id}` | instance app toggles (e.g. stats dashboard) |

## 3. Exact schemas — the OpenAPI artifact

The committed [`01-api-contract.v1-openapi.json`](01-api-contract.v1-openapi.json) is generated
directly from v1's `ApiDoc::openapi()` (utoipa) at v0.2.10. It is the byte-exact contract v1
serves at `/api-docs/openapi.json` and the baseline for the v2 contract diff required at the end
of the REST slices.

## 4. Drift between route table and OpenAPI doc (v1 bugs to not repeat)

_Filled from the generated artifact — see `§4 analysis` below._

## 5. Gaps and smells (feeds spec/08)

- Error objects lack machine-actionable codes, remediation hints, and request ids — an agent
  cannot reliably branch on `message` strings.
- Pagination wrapper exists but adoption is inconsistent across list endpoints (verify per
  endpoint in the OpenAPI artifact).
- Mixed identifier style: some resources addressed by `{name}`, others `{id}`; `{team}` accepts
  name **or** UUID (dual lookup) — convenient but ambiguous for caching/idempotency.
- `/api/v1/openapi/imports/{id}` GET/DELETE escapes the `/teams/{team}` prefix while its
  list/create live under it — inconsistent scoping shape.
- PUT vs PATCH is inconsistent (clusters/route-configs/listeners use PUT; filters, dataplanes,
  secrets, custom filters use PATCH; admin org-member role uses PUT).
- No idempotency mechanism on mutating endpoints (no `Idempotency-Key`, no create-or-get).
- RPC action paths are pragmatic but unevenly named (`/activate` vs `/enable` vs `/rotate`).
- The `expose` convenience endpoint creates three resources non-atomically from the client's view
  (single call, but partial-failure semantics undocumented).
- One env-var-driven body limit + per-route WASM override is sane, but limits are not surfaced in
  the OpenAPI doc.
