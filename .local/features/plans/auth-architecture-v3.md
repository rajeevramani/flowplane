# Auth Architecture v3: Zitadel as IDP, Flowplane as Tenancy Layer

**Decided**: 2026-03-02
**Status**: Approved — supersedes all prior auth design docs
**Context**: Conversation between Rajeev and Narad, 2026-03-02 09:34–10:13 AEDT

---

## 1. The One Rule

**Zitadel is identity. Flowplane is everything else.**

| Concern | Owner | Details |
|---------|-------|---------|
| Who is this person? | Zitadel | Authentication, passwords, MFA, SSO federation |
| What org/team do they belong to? | Flowplane DB | `org_memberships`, `user_team_memberships` |
| What can they do? | Flowplane DB | Scopes, roles, resource permissions |
| Is this request allowed? | Flowplane | `check_resource_access()` — single enforcement point |

The JWT is a **proof of identity** (the `sub` claim). It carries **no permission information**. Flowplane never reads Zitadel role keys for authorization decisions.

**Acceptance test for this rule:** If `parse_role_claims()` is deleted entirely, all authorization MUST still work. Every permission comes from the DB.

---

## 2. Auth Hierarchy

Three levels. Hard boundaries. No leaking.

```
Platform Admin
  └─ Governance only: create/delete orgs, onboard org admins
  └─ CANNOT see inside any org (no access to clusters, routes, listeners, etc.)

Org Admin
  └─ Manages their org: create teams, invite users, assign roles
  └─ Implicit access to all team resources within their org
  └─ CANNOT see other orgs

Team Member
  └─ Scoped access to specific resources in specific teams
  └─ CANNOT see other teams or manage the org
```

---

## 3. Authentication Flow

```
User ──► Zitadel (OIDC/PKCE or client_credentials)
     ◄── JWT with `sub` claim

User ──► Flowplane API (Bearer JWT)
         │
         ├─ Validate JWT signature (JWKS from Zitadel)
         ├─ Extract `sub` from JWT
         ├─ JIT user provisioning (see §4)
         ├─ Load permissions from DB:
         │    SELECT org_memberships WHERE user_id = ?
         │    SELECT user_team_memberships WHERE user_id = ?
         ├─ Build AuthContext from DB results
         └─ check_resource_access() enforces
```

**What the middleware does NOT do anymore:**
- Does NOT call `parse_role_claims()` to extract scopes from JWT — **`parse_role_claims()` is removed**
- Does NOT read `urn:zitadel:iam:org:project:*:roles` for authorization
- Does NOT derive `org_name` from Zitadel role claim values

**What does NOT change:** `check_resource_access()` is unchanged. It reads scopes from `AuthContext.scopes` (a `HashSet<String>`) and applies the same matching logic. The only difference is that scopes are populated from DB queries instead of JWT parsing. The scope format is identical: `org:acme-corp:admin`, `team:engineering:*:*`, `team:engineering:clusters:read`, `admin:all`.

---

## 4. JIT User Provisioning

On every authenticated request:

1. Extract `sub` from validated JWT
2. Check **in-memory permission cache** (keyed on `sub`, short TTL ~60s)
3. **Cache hit**: use cached `AuthContext`
4. **Cache miss**:
   a. Look up `users` table by `zitadel_sub` column
   b. **If not found**: create user row from JWT claims (`sub`, `email`, `name`)
   c. **If found**: use existing user record
   d. Load `org_memberships` and `user_team_memberships` for this user
   e. Build `AuthContext` with DB-sourced permissions
   f. Store in cache with TTL

**Cache invalidation:** When membership changes (invite, role update, removal), evict the affected user's `sub` from the cache. For distributed deployments, use a cache version counter in the DB or pub/sub.

**Performance note:** Phase 2 did zero DB queries per auth request (scopes came from JWT). v3 adds DB queries, but with caching, the amortized cost is one DB round-trip per user per TTL window. For high-throughput MCP paths, the cache is essential.

### Users Table Changes

```sql
ALTER TABLE users ADD COLUMN zitadel_sub TEXT UNIQUE;
-- Index for fast lookup on every request
CREATE INDEX idx_users_zitadel_sub ON users(zitadel_sub);
```

The `zitadel_sub` is the stable identifier that bridges Zitadel identity to Flowplane permissions.

---

## 5. Bootstrap Sequence (First Startup)

### Prerequisites
- PostgreSQL running
- Zitadel will be needed (for superadmin seeding) but does NOT need to be ready at startup
- Environment variable: `FLOWPLANE_SUPERADMIN_EMAIL=rajeev@itfrombit.com.au`

### Sequence

```
1. Migrations run → empty DB (schema created)

2. ensure_platform_resources()  [runs on EVERY startup, idempotent]
   ├─ Does org "platform" exist?
   │   └─ No → INSERT platform org + platform-admin team
   │   └─ Yes → skip
   └─ Does a platform owner exist?
       └─ No → continue to step 3
       └─ Yes → skip to step 4

3. Server starts accepting requests
   └─ Zitadel may not be ready yet — that's fine, server can start

4. Seed superadmin (background task, retries until Zitadel is ready)
   ├─ Read FLOWPLANE_SUPERADMIN_EMAIL
   ├─ Poll Zitadel readiness (GET /debug/ready) with backoff
   ├─ Call Zitadel Admin API: create or find user by email
   ├─ Get back Zitadel `sub`
   ├─ Create local `users` row (zitadel_sub = sub, email = env var)
   ├─ Create `org_memberships` row: user → platform org → role: owner
   └─ Zitadel sends welcome/password-set email (if new user)
```

**Startup ordering:** Steps 1-2 require only PostgreSQL. Step 3 starts the server immediately. Step 4 runs in the background — if Zitadel is still starting (common in Docker Compose), it retries with exponential backoff. The server is functional before Zitadel is ready (existing users with cached auth can still authenticate via JWKS cached from a prior run).

**No Zitadel role keys are created or assigned.** The env var is the only seed.

---

## 6. Org Onboarding Flow

### Step 1: Platform admin creates org

```
POST /api/v1/admin/organizations
{ "name": "acme-corp", "displayName": "Acme Corp" }

Flowplane:
  ├─ INSERT org "acme-corp"
  ├─ INSERT default team "acme-corp-default"
  └─ No Zitadel interaction
```

### Step 2: Platform admin invites org admin

```
POST /api/v1/admin/organizations/acme-corp/invite
{ "email": "alice@acme.com", "role": "admin" }

Flowplane:
  ├─ Call Zitadel Admin API: create or find user "alice@acme.com"
  ├─ Get back Zitadel `sub`
  ├─ Create local `users` row (zitadel_sub, email, name)
  ├─ Create `org_memberships` row: alice → acme-corp → admin
  └─ Return invite link (if new user) or confirmation (if existing)
```

### Step 3: Org admin invites team members

```
POST /api/v1/orgs/acme-corp/members/invite
{ "email": "bob@acme.com", "role": "member", "teams": ["engineering"] }

Flowplane (authorized by alice's org_memberships → acme-corp → admin):
  ├─ Call Zitadel Admin API: create or find user "bob@acme.com"
  ├─ Get back Zitadel `sub`
  ├─ Create local `users` row
  ├─ Create `org_memberships` row: bob → acme-corp → member
  ├─ Create `user_team_memberships` row: bob → engineering (with scopes)
  └─ Return confirmation
```

### Step 4: Users log in

```
Alice logs in via Zitadel → JWT with sub: alice-123
Flowplane middleware:
  ├─ Validate JWT
  ├─ Lookup users WHERE zitadel_sub = 'alice-123' → found
  ├─ Load org_memberships → acme-corp: admin
  ├─ Build AuthContext with org admin permissions
  └─ Alice can manage acme-corp, see all teams, access all resources
```

---

## 7. OAuth 2.0 for MCP

### Roles

| System | OAuth 2.0 Role |
|--------|---------------|
| Zitadel | Authorization Server (issues tokens) |
| Flowplane | Resource Server (validates tokens, enforces permissions) |

### Human MCP Client Flow

1. MCP client discovers auth via `/.well-known/oauth-authorization-server` → Zitadel
2. Authorization code + PKCE → user authenticates at Zitadel
3. JWT returned with `sub` claim
4. MCP client calls Flowplane MCP endpoint with Bearer token
5. Flowplane validates JWT, resolves permissions from DB, authorizes tool calls

### Machine/Agent MCP Client Flow

Agents follow the same JWT-is-identity principle as human users. The JWT carries only the `sub` claim. All permissions live in `agent_grants` in the DB.

**Step 1 — Team member creates agent (zero access):**
```
POST /api/v1/orgs/acme-corp/agents
{ "name": "payments-bot", "context": "gateway-tool", "team": "payments-team" }
```
- Requires `agents:write` scope on the team (not org-admin-only)
- Flowplane creates Zitadel machine user → gets `sub` + client credentials
- Creates `users` row with `agent_context`, `zitadel_sub`
- Returns `client_id` + `client_secret` + `token_endpoint` (shown once)
- **Agent has ZERO permissions at this point**

**Step 2 — Org admin creates grants:**
```
POST /api/v1/orgs/acme-corp/agents/payments-bot/grants
{ "grantType": "gateway-tool", "routeId": "...", "allowedMethods": ["GET"] }
```
- Requires org admin
- Validates `route.exposure == 'external'` before creating grant
- For `route` grants: triggers xDS snapshot update → Envoy picks up RBAC rules via ADS

**Step 3 — Agent authenticates and calls:**
```
POST /oauth/v2/token (client_credentials) → JWT with sub only
Agent → Flowplane MCP endpoint (Bearer JWT)
Flowplane → JIT lookup by sub → load agent_grants from DB → enforce
```

**Two-key model:** Developer (routes:write) marks route as `external` → org admin creates grant. Neither alone grants agent access.

### Three Agent Contexts

| Context | MCP Tools Visible | Enforcement | Typical Use |
|---------|------------------|-------------|-------------|
| `cp-tool` | `cp_*` tools matching grants | DB grant check in MCP handler | CI/CD, config automation |
| `gateway-tool` | `api_*` tools for granted routes only | DB grant check in MCP handler | Internal AI agents calling upstream APIs |
| `api-consumer` | None (no MCP) | Envoy JWT + RBAC filters (xDS) | External consumers calling APIs directly |

### Route Exposure

Routes default to `internal`. Only `external` routes can be granted to agents:
- Developer with `routes:write` marks route `external`
- `tools/list` never returns tools for internal routes
- xDS generation skips internal routes
- Changing `external → internal` blocked if active grants exist (409)

---

## 8. Customer SSO

Zitadel acts as **federation broker**:

```
Customer IDP (Okta/Azure AD/Google Workspace)
    │
    └─► Zitadel (federates authentication)
            │
            └─► JWT with `sub` claim
                    │
                    └─► Flowplane (resolves permissions from DB)
```

**Why this works cleanly with v3:** Because permissions live in Flowplane's DB (not Zitadel role keys), adding a new customer IDP requires zero role mapping configuration. Zitadel handles the federation protocol. Flowplane just sees a `sub` and does a DB lookup.

**The IDP is a deployment decision, not a code change.**

---

## 9. What Changes from Phase 2

| Component | Phase 2 (current) | v3 (target) |
|-----------|-------------------|-------------|
| `parse_role_claims()` | Maps Zitadel role keys → Flowplane scopes | **Removed** — not used for authorization |
| `AuthContext.scopes` | Populated from JWT role claims | Populated from DB queries |
| `AuthContext.org_name` | Zitadel org UUID (wrong) | Flowplane org name from `org_memberships` |
| `AuthContext.user_id` | `None` for JWT users | Always set (JIT provisioned) |
| `org_memberships` table | Exists but unused for JWT users | **Primary source** of org permissions |
| `user_team_memberships` | Exists but unused for JWT users | **Primary source** of team permissions |
| `users` table | Exists but not populated by JWT flow | Populated by JIT provisioning |
| Zitadel role keys | Encode permissions (e.g., `acme-corp:org-admin`) | **Not used for authorization** — Zitadel just needs to know the user exists |
| Org/team creation | Must also create Zitadel roles | **No Zitadel interaction** |
| DCR proxy | Assigns Zitadel role grants | Creates Flowplane DB permissions |
| `scope_registry.rs` | Queries `scopes` table at startup | **Repurposed**: scope definitions become code-only constants (valid resource/action pairs). No longer queries DB. `scopes` table can be dropped. |
| Setup script | Creates Zitadel roles, grants them to users | Creates Zitadel users only, calls Flowplane invite API for permissions |

---

## 10. Zitadel Admin API Usage

Flowplane needs a Zitadel service account for user CRUD only:

| Operation | When | Zitadel API |
|-----------|------|-------------|
| Create human user | Invite endpoint called | `POST /management/v1/users/human` |
| Find user by email | Invite endpoint called | `POST /management/v1/users/_search` |
| Create machine user | Agent provisioning | `POST /management/v1/users/machine` |
| Generate client secret | Agent provisioning | `PUT /management/v1/users/{id}/secret` |
| Delete user | Remove from platform | `DELETE /management/v1/users/{id}` |

**NOT needed anymore:**
- `add_user_grant()` — no role grants to assign
- Role key creation/deletion on org/team lifecycle
- Role key management of any kind

### Service Account Configuration

```bash
FLOWPLANE_ZITADEL_SERVICE_TOKEN=<PAT or client credentials>
# Needs: IAM_USER_MANAGER permission in Zitadel
# Does NOT need: project role management permissions
```

---

## 11. Deployment Considerations

- **Zitadel Cloud pricing**: Flowplane creates Zitadel users via Admin API for every invited user. On Zitadel Cloud, user counts affect pricing. For self-hosted Zitadel (current setup), this is a non-issue. If Flowplane ever uses Zitadel Cloud, factor user counts into cost modeling.
- **Zitadel availability**: Flowplane can start and serve cached users without Zitadel. But new logins, invites, and agent provisioning require Zitadel. Plan for Zitadel HA in production.
- **IDP portability**: Because Flowplane owns all permissions, switching from Zitadel to another OIDC provider (Keycloak, Auth0, etc.) requires only changing JWT validation config and the user CRUD client. No permission migration needed.

---

## 12. Security Invariants

These must hold true in the implemented system. Test each one.

1. **Platform admin cannot see into orgs.** A request from a platform owner to `GET /api/v1/teams/{team}/clusters` MUST return 403/404.

2. **Org admin cannot see other orgs.** A request from acme-corp org admin to access beta-inc resources MUST return 403/404.

3. **Team member cannot see other teams.** A user with access to `engineering` team requesting `payments-api` resources MUST return 403/404.

4. **JWT is authentication only.** If `parse_role_claims()` is removed entirely, all authorization MUST still work (permissions come from DB).

5. **Flowplane DB is single source of truth for permissions.** No authorization decision may depend on Zitadel role claims, org IDs, or any JWT field other than `sub`.

6. **Zitadel is single source of truth for identity.** User credentials (password, MFA) are never stored in Flowplane.

7. **`check_resource_access()` is the single enforcement point.** No handler may implement its own authorization bypass.

8. **JIT provisioning is idempotent.** Creating a user that already exists (by `zitadel_sub`) is a no-op.

9. **Bootstrap is idempotent.** Running the startup sequence on an already-bootstrapped DB changes nothing.

10. **Invite is idempotent.** Inviting a user who already has the specified membership is a no-op (returns success, not error).

11. **Agent JWT carries no permissions.** Agent JWT contains only `sub`. No scopes, no role claims. All agent permissions live in `agent_grants`. If `agent_grants` is empty, the agent can do nothing.

12. **Internal routes are never visible to agents.** Routes with `exposure = 'internal'` must not appear in `tools/list` responses and must not generate Envoy RBAC rules, regardless of any grants that may exist.

13. **Agents cannot exceed their route grants.** A gateway-tool agent's `tools/list` response must contain only tools for routes it holds explicit grants on. An api-consumer agent must be denied by Envoy for any route+method combination not covered by an active grant.

14. **Exposure rollback is safe.** Changing a route's exposure from `external → internal` when active grants exist MUST be rejected (409). Grants must be revoked before exposure can be changed.

15. **Agent CRUD requires team scope, not org admin.** Creating/deleting agents requires `agents:write` scoped to the team. Grant creation always requires org admin. These must be enforced independently — no handler may conflate the two.

---

## 13. Implementation Phases

### Phase A: Foundation (~8 days)
**Goal: JIT provisioning + DB-based permission resolution working end-to-end**
**Acceptance test: delete `parse_role_claims()` — all authorization still works**

1. Add `zitadel_sub` column to `users` table (migration)
2. Implement JIT user provisioning in auth middleware
3. Implement in-memory permission cache (keyed on `sub`, ~60s TTL, evict on membership changes)
4. Rewrite `AuthContext` construction to query `org_memberships` + `user_team_memberships` instead of parsing JWT claims
5. Verify `check_resource_access()` works unchanged with DB-sourced permissions (scope format is identical)
6. Auto-create platform org + platform-admin team at startup
7. Implement superadmin seeding from `FLOWPLANE_SUPERADMIN_EMAIL` (background task with Zitadel retry)
8. Tests: all three hierarchy levels, cross-org isolation, JIT provisioning, cache invalidation

Note: this phase touches middleware.rs, zitadel.rs, models.rs, authorization.rs, main.rs, a new migration, and the full test suite (2,128 tests depend on AuthContext shape).

### Phase B: Org Lifecycle (~3 days)
**Goal: Platform admin can create orgs and invite org admins**

1. `POST /api/v1/admin/organizations` — create org (DB only)
2. `POST /api/v1/admin/organizations/{org}/invite` — invite org admin (Zitadel user creation + DB membership)
3. Verify org admin can log in and manage their org
4. Tests: org creation, invite flow, org admin permissions

### ~~Phase C: Team Lifecycle + Member Management~~
**Collapsed** — all endpoints already existed with correct patterns. B.6 fixed auth checks.

### Phase C (was D): Machine Users + Team UI (~5 days)
**Goal: Org admins can provision machine users for MCP via API, manage teams via frontend UI**

1. Add `user_type` column to `users` table, extend ZitadelAdminClient with `search_user_by_username()`
2. `POST /api/v1/orgs/{org}/agents` — provision machine user (CP tool scopes only)
   - Uses Zitadel `AddMachineUser` — searches by `userName` via `userNameQuery`
   - Returns `client_id` + `client_secret` + `token_endpoint` (shown once)
   - Creates DB user row + team memberships
3. `GET/DELETE /api/v1/orgs/{org}/agents[/{name}]` — agent lifecycle
4. Rework DCR proxy to create DB permissions instead of Zitadel role grants
5. Update `seed-demo.sh` to use agent API (remove all psql_exec)
6. Tests: agent provisioning, client_credentials flow, MCP authorization
7. Org admin team management UI (SvelteKit frontend at `/organizations/[orgName]/teams/`)

### Phase D: Agent Credentials UI + Vestigial Code Cleanup (~3 days)
**Goal: Build org admin agent management frontend, remove dead admin users and PAT code**

1. Agent credentials UI — SvelteKit pages at `/organizations/[orgName]/agents/` for create (with one-time credential display), list, and delete
2. Remove vestigial admin users pages — `/admin/users/` routes, dead API client methods, dead types, navigation links
3. Remove vestigial PAT code — `/tokens` route, frontend API/types, backend models/validation, CLI token methods

### Phase E (was D): Unified Agent Permission Model (~6 days)
**Goal: Replace coarse JWT-scope-based agent permissions with fine-grained DB grants across all three agent contexts**

**Full design:** `.local/features/plans/auth-v3-tasks/phase-e/ARCHITECTURE-v2.md`

**Core architectural shift:** JWT carries only `sub` for agents (same as humans). All agent permissions live in `agent_grants` table, checked at runtime via the permission cache. No re-provisioning required to change permissions.

**Key deliverables:**

- **E.1 — Foundation:** `agent_context` column on `users`; `exposure` column on `routes`; `agent_grants` table (unified: cp-tool grants use `resource_type+action`, gateway/route grants use `route_id+allowed_methods`); `agents:read/agents:write` scopes; agent CRUD changed from org-admin-only to team-scoped
- **E.2 — Gateway tool routing fix:** One-line fix in `src/mcp/handler.rs` — prefix check before `is_none()` so `api_*` tools reach `execute_gateway_tool()` correctly
- **E.3 — CP tool grants enforcement:** `check_resource_access()` queries `agent_grants` for machine users; `tools/list` filtered by grants for cp-tool agents
- **E.4 — Gateway tool + route grants:** Route exposure UI; `tools/list` filtered to granted routes for gateway-tool agents; grant API endpoints; JWT `claim_to_headers` for `sub → x-flowplane-sub`; xDS RBAC generation from route grants using `PrincipalRule::Header` on forwarded sub header
- **E.5 — Tests + cleanup:** Full agent lifecycle integration tests; remove dead scope validation code

**Prerequisite learning (validated in Phase C smoke test):** Agent `client_credentials` token
requests MUST include the Zitadel project audience scope
`urn:zitadel:iam:org:project:id:{projectId}:aud` — without it, the JWT `aud` claim is the
client_id only, which fails Flowplane's audience validation. Any agent provisioning or
documentation must surface this scope requirement (see `scripts/test-agent-mcp.sh`).

### Phase F (was E): Cleanup (~2 days)
**Goal: Remove remaining dead code from Phase 2**

1. Drop remaining unused tables if FK dependencies resolved
2. Update setup script to not create Zitadel roles
3. Repurpose `scope_registry.rs` to code-only constants, drop `scopes` table

**Total: ~25 days**

---

## 14. Questions Resolved

| Question | Answer |
|----------|--------|
| Where does identity reside? | Users → Zitadel. Orgs, teams → Flowplane DB. |
| Where do permissions reside? | Flowplane DB. Not in Zitadel role keys. |
| Where does authorization happen? | Flowplane. `check_resource_access()`. Always. |
| How does the first superadmin get created? | `FLOWPLANE_SUPERADMIN_EMAIL` env var → Zitadel user creation → DB membership. |
| How does org admin onboarding work? | Platform admin invites via API → Zitadel user + DB membership. |
| How does MCP OAuth work? | Zitadel = Authorization Server, Flowplane = Resource Server. Permissions from DB. |
| How does customer SSO work? | Zitadel federates with customer IDP. Flowplane sees `sub`, looks up DB. |
| Can Flowplane work with a different IDP? | Yes. Replace Zitadel with any OIDC provider. Only JWT validation config changes. |

---

## 15. What This Document Supersedes

- `auth-gap-analysis.md` — gaps identified there are resolved differently under v3
- `auth-mental-model-current.md` — describes Phase 2 state which v3 replaces
- `auth-mental-model-target.md` — was designed around "permissions in JWT" which v3 rejects
- Phase 2 gap matrix (18 gaps) — many gaps disappear entirely under v3 (e.g., #1, #2, #3, #5, #6, #12, #13, #17, #18)
