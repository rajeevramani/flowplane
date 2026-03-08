# Auth v3 Phase E — Unified Agent Permission Model

## Context

Phases A (Foundation), B (Org Lifecycle), C (Machine Users + Team UI), and D (Agent Credentials UI + Vestigial Code Cleanup) are complete. The system supports:
- JIT provisioning and DB-based permission resolution with caching
- Human user invite flow (org admin → Zitadel user creation → DB membership)
- Machine user (agent) provisioning with CP tool scopes (`POST/GET/DELETE /api/v1/orgs/{org}/agents`)
- Agent credentials UI with one-time credential display
- DCR proxy reworked to use DB permissions instead of Zitadel role grants
- `check_resource_access()` enforcing team-scoped permissions for all API paradigms

Phase E is a **redesign** of how agent permissions work. The original plan (scopes baked into JWT, separate `route_access_grants` table) was replaced during design review in favour of a unified, DB-only grants model. See `ARCHITECTURE-v2.md` for the full design and rationale.

**Phase E goal:** Five deliverables:
1. **Foundation** — `agent_context` on users, `exposure` on routes, unified `agent_grants` table, agent CRUD scoping, `agents:read/write` scopes
2. **Fix gateway tool execution routing** — one-line bug fix in `handler.rs` that blocks all `api_*` tool calls
3. **CP tool grants enforcement** — `check_resource_access()` queries `agent_grants` for machine users; `tools/list` filtered per grants
4. **Gateway tool + route grants** — route exposure toggle, grant CRUD API, `tools/list` filtering, JWT sub forwarding, xDS RBAC generation
5. **Tests + dead code removal** — full lifecycle integration tests, remove obsolete scope-based agent code

---

## The Core Architectural Shift

**Before (Phases A–D):** Agents had scopes baked into their JWT at provisioning time (`clusters:read`, `api:execute`, etc.). Changing permissions required re-provisioning.

**After (Phase E):** Agent JWT carries only `sub` (same as humans). All agent permissions live in `agent_grants` table, checked at runtime via the permission cache. Permissions can be granted/revoked without touching the agent.

This mirrors what auth-v3 did for human users — JWT is identity only, DB is permissions.

---

## Three Agent Contexts

| # | Context | MCP Tools Visible | Enforcement | Typical Use |
|---|---------|------------------|-------------|-------------|
| 1 | `cp-tool` | `cp_*` tools matching grants | `check_resource_access()` → `agent_grants` | CI/CD, config automation |
| 2 | `gateway-tool` | `api_*` tools for granted routes only | `check_resource_access()` → `agent_grants` | Internal AI agents calling upstream APIs via MCP |
| 3 | `api-consumer` | None (no MCP) | Envoy JWT + RBAC filters (xDS from grants) | External consumers calling APIs directly |

**All three contexts use `agent_grants` in the DB.** No JWT scopes for agents.

### Internal vs External Tools

- **Internal (`cp_*`):** Manage Flowplane config. Only `cp-tool` agents. Team-scoped grants.
- **External (`api_*`):** Proxy to upstream services. `gateway-tool` via MCP, `api-consumer` via Envoy. Route+verb grants.

A `gateway-tool` agent never sees `cp_*` tools. A `cp-tool` agent never sees `api_*` tools. Enforced at `tools/list`.

---

## Route Exposure

Routes default to `internal`. Only `external` routes can be granted to agents or appear in gateway tool listings.

**Two-key model:**
1. Developer (with `routes:write`) marks route `external` — "this API is safe to expose"
2. Org admin creates a grant — "this agent can access it"

Neither alone grants access. Changing `external → internal` is blocked with 409 if active grants exist.

---

## Agent CRUD Scoping

Creating an agent is now a team-scoped operation, not org-admin-only:
- `agents:write` → create/delete agents in your team
- `agents:read` → list agents in your team
- Grant creation is still org-admin-only

Zero-access on creation. The agent has no permissions until an org admin creates grants.

---

## Key Technical Discoveries

### 1. Gateway Tool Routing Bug (E.2)

**File:** `src/mcp/handler.rs:424-432`

`get_tool_authorization("api_getUser")` returns `Some(&GATEWAY_AUTH)` (matches `api_*` prefix in `tool_registry.rs`), so `is_none()` is `false`. The tool falls through to the static CP tool match → `_ =>` → `ToolNotFound`. Fix: add a `starts_with("api_")` prefix check before the `is_none()` dispatch.

### 2. RBAC `PrincipalRule` Has No Metadata Variant

**File:** `src/xds/filters/http/rbac.rs`

`PrincipalRule` has no `Metadata` variant — there's no code path to match JWT payload claims as principals. The original E.4 design (metadata-based `sub` matching) would not work.

**Fix:** Use `claim_to_headers` in the JWT provider config to forward `sub` as `x-flowplane-sub` header, then use `PrincipalRule::Header` to match it. Both are fully implemented. No new proto types needed.

### 3. `PermissionRule::Metadata` Is a Stub

The `PermissionRule::Metadata` variant returns `Rule::Any(true)` — not implemented. Not needed for E.4: path matching uses `PermissionRule::UrlPath`, method matching uses `PermissionRule::Header { name: ":method" }`. Both work.

### 4. `RbacPerRouteConfig::Override` Exists and Works

The per-route RBAC override mechanism is fully implemented. E.4 uses it to generate per-route policies from `agent_grants`.

---

## What Needs to Be Built

### E.1: Foundation — Agent Context + Grants Schema
- Migrations: `agent_context` on `users`, `exposure` on `routes`, `agent_grants` table (unified: cp-tool grants use `resource_type+action`, gateway/route grants use `route_id+allowed_methods`)
- `AgentContext` enum, `AgentGrant`/`CpGrant`/`RouteGrant` types
- `load_agent_grants()` in permissions module
- Extend `CachedPermissions` with agent grant fields
- `agents:read` / `agents:write` in scope registry
- Change agent CRUD from `require_org_admin_only()` to `check_resource_access("agents", ...)`
- Update `provision_machine_user_db()` to accept and store `agent_context`
- **No `host_filter`** — dropped, superseded by per-route grants

### E.2: Fix Gateway Tool Execution Routing
- Add `starts_with("api_")` prefix check before `is_none()` dispatch in `handle_tools_call()`
- No changes to `execute_gateway_tool()` — already correct once reached

### E.3: CP Tool Grants Enforcement
- Add machine-user branch to `check_resource_access()` — checks `agent_grants` for cp-tool agents, returns `false` for gateway-tool/api-consumer agents on CP resources
- Update `handle_tools_list()` — filter CP tools by agent's cp_grants via `TOOL_AUTHORIZATIONS` map
- Grant CRUD API: `POST/GET/DELETE /api/v1/orgs/{org}/agents/{name}/grants` (org-admin only, cp-tool grants in this task)
- Cache invalidation: `permission_cache.evict(agent_zitadel_sub)` on grant create/delete

### E.4: Gateway Tool + Route Grants
- Route exposure toggle in update handler with 409 check (block if active grants exist)
- Extend grant CRUD API for `gateway-tool` and `route` grant types
- Validate `route.exposure == 'external'` on grant creation
- `handle_tools_list()` filtering for gateway-tool agents (only granted route_ids → `api_*` tools)
- Gateway tool execution: check `agent_grants` before `execute_gateway_tool()`
- JWT filter config: add `claim_to_headers` for `sub` → `x-flowplane-sub` header
- xDS RBAC generation from route grants: `build_rbac_config_for_listener()` using `PrincipalRule::Header` on `x-flowplane-sub`
- xDS trigger: grant create/delete → regenerate snapshot → Envoy picks up via ADS

### E.5: Tests + Dead Code Removal
- Full agent lifecycle integration tests (create → grant → use → revoke)
- Dead code removal: scope-based agent validation, `update_org_agent_scopes` endpoint, superseded task files
- `cargo fmt && cargo clippy --all-targets --all-features -- -D warnings && cargo test` passes

---

## Required Reading

1. **Architecture**: `phase-e/ARCHITECTURE-v2.md` — full design, schema, enforcement pseudocode
2. **Auth architecture**: `.local/features/plans/auth-architecture-v3.md` — sections 7 (OAuth for MCP), 12 (Security Invariants), 13 (Phase E)
3. **Task breakdown**: `.local/features/plans/auth-v3-tasks/README.md` — phase overview
4. **Individual tasks** (implement in dependency order):
   - `phase-e/task-e1-migration-agent-context.md` — foundation (blocks E.3, E.4, E.5)
   - `phase-e/task-e2-gateway-tool-routing-fix.md` — routing fix (no dependencies, do first)
   - `phase-e/task-e3-cp-tool-grants.md` — CP tool grants enforcement (depends on E.1)
   - `phase-e/task-e4-gateway-route-grants.md` — gateway/route grants + xDS (depends on E.1, E.2)
   - `phase-e/task-e5-tests.md` — tests + cleanup (depends on all)

---

## Branch

Work on: `feature/auth-v3-db-permissions` (same branch as Phases A–D).

---

## How to Work

- **E.1 and E.2 are independent** — do both first, in parallel or sequence (~0.5d each)
- **E.3 depends on E.1 only** — needs agent_grants table and CachedPermissions extensions
- **E.4 depends on E.1 and E.2** — needs agent_grants + working gateway tool dispatch
- **E.5 depends on all prior tasks**
- Commit after each task: `auth-v3: E.1 — agent context + grants schema`
- Run `cargo fmt && cargo clippy --all-targets --all-features -- -D warnings && cargo test` after each backend task
- Update task status in README.md as you complete each one

---

## Acceptance Tests (Phase E gate)

### E.1 — Foundation
1. All three migrations apply cleanly on existing DB
2. Existing machine users get `agent_context = NULL` (treated as `cp-tool` in application code)
3. Existing routes get `exposure = 'internal'` (safe default, invisible to agents)
4. `AgentContext` enum serializes correctly: `"cp-tool"`, `"gateway-tool"`, `"api-consumer"`
5. `provision_machine_user_db()` stores `agent_context` on new machine users
6. Team member with `agents:write` can create an agent (no longer requires org admin)
7. Created agent has zero permissions — `agent_grants` table empty for new agent

### E.2 — Gateway Tool Routing Fix
8. `api_*` tools appear in `tools/list` (was already working)
9. `api_*` tools can be called via `tools/call` and reach `execute_gateway_tool()` (was broken)
10. CP tools (`cp_*`) continue to dispatch through the static CP match — no regression

### E.3 — CP Tool Grants Enforcement
11. `check_resource_access()` returns `true` for cp-tool agent with matching grant
12. `check_resource_access()` returns `false` for cp-tool agent without matching grant
13. `check_resource_access()` returns `false` for gateway-tool/api-consumer agents on any CP resource
14. Human users unaffected — existing scope-based path unchanged
15. `handle_tools_list()` returns only tools matching cp-tool agent's grants
16. `handle_tools_list()` returns zero CP tools for gateway-tool and api-consumer agents
17. Grant CRUD API works (org-admin only): create returns 201, list returns grants, delete returns 204
18. Duplicate cp-tool grant returns 409
19. Non-org-admin grant request returns 403
20. Permission cache evicted on grant create/delete — next request uses fresh grants

### E.4 — Gateway Tool + Route Grants
21. Developer with `routes:write` can mark route `external`
22. `external → internal` change blocked with 409 if active grants exist
23. Grant creation on `internal` route returns 400
24. Gateway-tool agent `tools/list` returns only `api_*` tools for granted routes
25. Gateway-tool agent cannot call tool for non-granted route (403)
26. API consumer agent with route grant: Envoy allows `sub` + matching path + method
27. API consumer agent without grant: Envoy returns 403
28. API consumer agent with grant for GET but calling POST: Envoy returns 403
29. Revoking a route grant triggers xDS update — RBAC rule removed within ADS poll interval
30. `cargo test` passes — all new tests green

### E.5 — Tests + Cleanup
31. Full agent lifecycle integration test passes (create → expose route → grant → call → revoke)
32. Cross-context isolation: cp-tool agent cannot see/call `api_*` tools
33. Cross-context isolation: gateway-tool agent cannot call `cp_*` tools
34. Dead code removed — no scope-based agent validation code remains

---

## Key Constraints

- **JWT carries only `sub` for agents.** No scopes, no permission claims. If `agent_grants` is empty, the agent can do nothing.
- **`check_resource_access()` is modified for machine users.** A new branch checks `agent_context` first. If `Some(AgentContext)`, query `agent_grants` instead of scopes. Human user path is unchanged.
- **`tools/list` must be filtered per grants.** Agents see only tools they can actually call. Internal routes are never visible.
- **All three enforcement layers must check exposure.** Grant creation, `tools/list`, and xDS generation all validate `route.exposure == 'external'`.
- **Grant creation is org-admin-only.** Agent CRUD is team-scoped (`agents:write`). These are independent.
- **JWT sub forwarding via `claim_to_headers`.** RBAC principal matching uses `PrincipalRule::Header` on `x-flowplane-sub`, not metadata matching (no metadata variant exists in `PrincipalRule`).
- **No `host_filter` column.** Dropped — per-route grants are more precise and make it redundant.
- **No `route_access_grants` table.** Replaced by `agent_grants` with `grant_type = 'route'`.
- **API consumer auth is data plane only.** Envoy enforces via JWT + RBAC filters. Flowplane does NOT participate in per-request authorization for API consumers (no ext_authz callback).
- **No backward compatibility concerns.** Flowplane has zero customers.

## Do NOT

- Do not add `host_filter` column — the design dropped it
- Do not create `route_access_grants` table — replaced by unified `agent_grants`
- Do not bake permissions into agent JWT as scopes — JWT is identity only (`sub` claim)
- Do not use `PrincipalRule::Metadata` — the implementation is a stub (`Any(true)`)
- Do not add Flowplane ext_authz callback for API consumer auth — JWT + RBAC in Envoy only
- Do not store `client_secret` in Flowplane DB — show-once only
- Do not allow platform admin to provision agents (existing invariant)
- Do not create Zitadel role keys or grants — v3 doesn't use them
- Do not break existing CP tool agent provisioning — `agent_context = NULL` treated as `cp-tool`
- Do not reference `task-e3-context-aware-provisioning.md` or `task-e4-route-level-permissions.md` — superseded, pending deletion
