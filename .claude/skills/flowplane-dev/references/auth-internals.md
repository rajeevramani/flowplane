# Auth Internals Reference

## Auth Mode Selection

`FLOWPLANE_AUTH_MODE` env var parsed in `src/config/mod.rs`:
- `"dev"` → `AuthMode::Dev`
- `"prod"` or unset → `AuthMode::Prod` (default)

The mode determines which middleware is installed in `src/api/routes.rs`.

## Dev Mode Auth Flow

**Middleware:** `dev_authenticate()` in `src/auth/middleware.rs` (line 248)

**State:** `DevAuthState` — holds rate limiter and deterministic user ID (`dev-user-id`)

**Flow:**
1. Pass through OPTIONS (CORS preflight)
2. Rate limit by client IP
3. Extract `Bearer <token>` from `Authorization` header
4. Read `FLOWPLANE_DEV_TOKEN` from env (read on every request — rotation requires restart)
5. Compare token — reject if mismatch
6. Build synthetic `AuthContext`:
   - `token_id`: `dev-token-id`
   - `user_id`: `dev-user-id`
   - `email`: `dev@flowplane.local`
   - `scopes`: `["org:dev-org:admin"]`
   - `org_id`: `dev-org-id`
   - `org_name`: `dev-org`
7. Insert `AuthContext` into request extensions

**Token generation:** `src/auth/dev_token.rs`
- `resolve_or_generate_dev_token()` — checks `~/.flowplane/credentials`, generates 43-char base64 token if none exists
- Sets `FLOWPLANE_DEV_TOKEN` env var for the compose file to pick up

**Credential storage:** Plain text token in `~/.flowplane/credentials`

## Prod Mode Auth Flow

**Middleware:** `authenticate()` in `src/auth/middleware.rs` (line 52)

**State:** `ZitadelAuthState` — holds Zitadel config, JWKS cache, permission cache, DB pool, rate limiter

**Flow:**
1. Pass through OPTIONS (CORS preflight)
2. Rate limit by client IP
3. Extract `Bearer <token>` from `Authorization` header
4. Validate JWT via `validate_jwt_extract_sub()` in `src/auth/zitadel.rs`
   - Validates against Zitadel JWKS endpoint
   - Extracts `sub`, `email`, `name` from claims
5. Check permission cache (keyed by `sub`)
6. On cache miss:
   a. JIT upsert user from JWT claims (`upsert_from_jwt()`)
   b. If email missing from token, fetch from Zitadel userinfo endpoint
   c. Load permissions via `load_permissions()` — single call for all user types
   d. Parse `AgentContext` for machine users
   e. Cache the permission snapshot
7. Build `AuthContext` from cached/loaded snapshot
8. Run `require_resource_access()` — derives resource + action from path + method
9. Insert `AuthContext` into request extensions

**Credential storage:** JSON with `access_token`, `refresh_token`, `token_type`, `expires_at` in `~/.flowplane/credentials`

## Scope Format

Scopes follow the pattern: `team:{team_name}:{resource}:{action}`

| Scope Pattern | Meaning |
|---|---|
| `admin:all` | Platform admin (governance only, cannot see inside orgs) |
| `org:{org}:admin` | Org admin (all resources in org) |
| `team:{team}:admin` | Team admin |
| `team:{team}:{resource}:{action}` | Specific resource access |
| `team:{team}:cp:read` | Read control plane tools |
| `team:{team}:cp:write` | Write control plane tools |
| `team:{team}:api:read` | Read gateway API tools |
| `team:{team}:api:execute` | Execute gateway API tools |

**Actions:** `read`, `create`, `update`, `delete`, `execute`

## check_resource_access()

**Location:** `src/auth/authorization.rs`

**Single enforcement point** for all API paradigms: REST handlers, MCP control plane tools, and MCP gateway tools all call this function.

**Flow:**
1. `resource_from_path(path)` — extracts resource type from URL path
2. `action_from_request(method)` — maps HTTP method to action:
   - GET → `read`
   - POST → `create`
   - PUT/PATCH → `update`
   - DELETE → `delete`
3. `require_resource_access(auth_context, resource, action)` — checks if user has required scope

**Key invariants:**
- Platform admin (`admin:all`) is governance only — cannot access team resources
- Platform admin cannot see inside orgs (hard boundary)
- All DB queries enforce team isolation (WHERE team = $1)

## Team Resolution

**Location:** `src/auth/team.rs`

**Dev mode:** Hardcoded to `default` team in `dev-org`

**Prod mode:** Resolved from:
1. `?team=<name>` query parameter (explicit)
2. Token scopes matching `team:{name}:*` (implicit)
3. DB-backed team memberships via `user_team_memberships` table

## Permission Cache

**Location:** `src/auth/cache.rs`

`CachedPermissions` — LRU cache keyed by Zitadel `sub` claim
- Stores: user_id, email, org_scopes, grants, org_id/name/role, agent_context
- Cache miss triggers full DB load via `load_permissions()`
- Used only in prod mode (dev mode has static identity)

## Key Files

| File | Purpose |
|---|---|
| `src/auth/middleware.rs` | `authenticate()` (prod) and `dev_authenticate()` (dev) middleware |
| `src/auth/authorization.rs` | `check_resource_access()`, `require_resource_access()`, scope checking |
| `src/auth/models.rs` | `AuthContext`, `AgentContext`, `AuthError` types |
| `src/auth/zitadel.rs` | JWT validation (RS256, issuer/audience), JWKS cache |
| `src/auth/dev_token.rs` | Dev token generation and credential file I/O |
| `src/auth/team.rs` | Team resolution (dev: hardcoded, prod: DB-backed) |
| `src/auth/cache.rs` | Permission cache for prod mode |
| `src/auth/permissions.rs` | `load_permissions()` — single DB call for all user types |
| `src/auth/scope_registry.rs` | Scope definitions and validation |
