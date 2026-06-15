# 05 — Authentication and Authorization

Extracted from Flowplane v1 (v0.2.10). Sources cited inline as v1 paths. This spec is
self-contained: it defines the behavior the v2 rewrite must implement without reference to v1
source.

Two separate questions, two separate layers:

- **Authentication** — "who are you?" — is OIDC. Zitadel in prod mode, an embedded mock OIDC
  server in dev mode. JWTs are identity-only; they carry **no** authorization data.
- **Authorization** — "what can you do?" — is one in-process function,
  `check_resource_access(ctx, resource, action, team)`, that every surface (REST, MCP control
  plane, MCP gateway) converges on. Permissions live in the database, never in the token.

Anchor doc in v1: `docs/explanation/auth-model.md`. Gate: `src/auth/authorization.rs`.

---

## 1. Identity model

### 1.1 Entities

| Entity | Key fields | Notes |
|---|---|---|
| **Organization** (`organizations`) | `id`, `name` (immutable, lowercase-hyphen `NAME_REGEX`), `display_name`, `description`, `owner_user_id?`, `settings` (JSON), `status` (`active`/`suspended`/`archived`) | Governance layer above teams. A special org named **`platform`** holds platform operators. (`src/auth/organization.rs`) |
| **Team** (`teams`) | `id` (UUID), `name` (immutable, unique **per org**), `display_name`, `description?`, `owner_user_id?`, `org_id` (required FK), `settings`, `status` (`active`/`suspended`/`archived`), `envoy_admin_port?` | Tenancy unit. All tenant resources carry a team FK. (`src/auth/team.rs`) |
| **User** (`users`) | `id` (UUID), `email` (NOT NULL UNIQUE), `name`, `status` (`active`/`inactive`/`suspended`), `is_admin` (legacy bool, not consulted by the gate), `zitadel_sub` (UNIQUE — OIDC `sub` claim), `user_type` (`"human"` \| `"machine"`), `agent_context` (NULL for humans; `"cp-tool"` \| `"gateway-tool"` \| `"api-consumer"` for machines), `password_hash` (legacy, always `''`) | `zitadel_sub` is the bridge from OIDC identity to local permissions. (`src/auth/user.rs`) |
| **Org membership** (`organization_memberships`) | `(user_id, org_id)` unique, `role` ∈ {`owner`, `admin`, `member`, `viewer`} | Source of org-level scopes. |
| **Team membership** (`user_team_memberships`) | `(user_id, team)` unique (team column stores team UUID) | Records which teams a user belongs to. Membership alone grants **nothing** — grants do. Grant creation requires membership (§3.4). |
| **Grant** (`grants`) | `id`, `principal_id` (user id, human or machine), `org_id`, `team_id`, `grant_type` ∈ {`resource`, `gateway-tool`, `route`}, `resource_type?`, `action?` (resource grants), `route_id?`, `allowed_methods?` (gateway/route grants), `created_by`, `created_at`, `expires_at?` | The team-level permission row. Unique index prevents duplicates (conflict → 409). (`src/auth/models.rs`) |

### 1.2 Org roles → scopes

Org-level access is expressed as **scope strings** computed at request time from
`organization_memberships` (`src/auth/permissions.rs::map_org_role_to_scopes`):

| DB role | Org | Scopes produced |
|---|---|---|
| `owner` | `platform` | `admin:all` **and** `org:platform:admin` |
| `owner` or `admin` | any other org | `org:{org_name}:admin` |
| `member` | any | `org:{org_name}:member` |
| `viewer` | any | `org:{org_name}:viewer` |
| anything else | — | nothing (warn-logged, skipped) |

`admin:all` exists **only** via platform-org ownership. There is no other path to it.

### 1.3 User types

- **human** — interactive operator (UI/CLI). `agent_context = NULL`.
- **machine** — Zitadel machine user (service account / AI agent) provisioned by an org admin.
  `agent_context` partitions what the machine may touch:
  - `cp-tool`: control-plane MCP tools; uses the same `resource` grants as humans.
  - `gateway-tool`: MCP gateway tools (proxy calls to upstream APIs); `gateway-tool` grants only.
  - `api-consumer`: direct data-plane access enforced via Envoy RBAC; `route` grants only.

Grant-type ↔ principal compatibility is validated at grant creation (§3.4).

### 1.4 Provisioning flows

- **JIT (every authenticated request):** after JWT validation, the middleware upserts the user by
  `zitadel_sub` (`src/storage/repositories/user.rs::upsert_from_jwt`):
  `INSERT ... ON CONFLICT (zitadel_sub) DO UPDATE`. If the token has no `email` claim, a
  placeholder `zitadel-{sub}@jit.local` is used; on conflict a real (non-placeholder) email is
  never overwritten by a placeholder. Name is updated only when non-empty. JIT creates the user
  row only — no memberships, no grants; a fresh JIT user can do nothing until invited/granted.
- **Invite (org member):** `POST /api/v1/admin/organizations/{id}/members/invite`
  (platform admin **or** that org's admin). Searches Zitadel by email; creates a Zitadel human
  user if absent (optionally with `initial_password`, set with `changeRequired:false` via Zitadel
  v2 password API); upserts local user; inserts `organization_memberships` row with the requested
  role; fans out `user_team_memberships` across all teams in the org; cross-org conflict (user
  already belongs to another org) → 403. (`src/api/handlers/organizations.rs`)
- **Agent provisioning:** `POST /api/v1/orgs/{org_name}/agents` — caller needs
  `agents:create` grant on the first listed team (checked via `check_resource_access`). Creates a
  Zitadel machine user named `{org}--{agent-name}`, generates client credentials
  (`client_id`/`client_secret` returned **once**, at creation only), creates local user
  (`user_type='machine'`, `agent_context='cp-tool'` — hardcoded in v1, TODO to make
  configurable), org membership, and team memberships with **empty** scopes — access is then
  managed via the grants table. Idempotent: re-creating the same name returns 200 without
  credentials. Audited as governance `user.create`.
- **DCR proxy:** `POST /api/v1/oauth/register` (RFC 7591) — authenticated, org-admin-only
  (`require_org_admin_only`, platform admin excluded). Same `provision_machine_user()` helper;
  requested `scope` string is parsed into per-team groups (`team:{name}:{res}:{act}` entries; all
  others silently dropped); teams must exist in the caller's org. (`src/api/handlers/oauth.rs`)
- **Superadmin seeding (boot, prod):** if `FLOWPLANE_SUPERADMIN_EMAIL` is set, a background task
  polls Zitadel readiness (1s→60s exponential backoff), finds/creates that human user (password
  from `FLOWPLANE_SUPERADMIN_INITIAL_PASSWORD` if set), upserts the local row, and creates an
  `owner` membership in the `platform` org → `admin:all`. Idempotent.
  (`src/api/handlers/bootstrap.rs::seed_superadmin`)
- **Bootstrap endpoint:** `POST /api/v1/bootstrap/initialize` creates the `platform` org +
  `platform-admin` team and the first tenant org/team. Not on the JWT path (runs before any admin
  exists); gated by a one-time static token: caller presents
  `Authorization: Bearer <FLOWPLANE_BOOTSTRAP_TOKEN>`. Env unset/empty → 403 (endpoint
  self-disabled); missing bearer or mismatch → 401. Comparison is constant-time over SHA-256
  digests of both values (length non-leaking). Returns 409 once a non-platform org exists.
- **Dev seeding (boot, dev mode):** `src/startup.rs::seed_dev_resources` — idempotent inserts:
  org `dev-org` (id `dev-org-id`), team `default` (id `dev-default-team-id`), user `dev-user-id`
  (email `dev@flowplane.local`, `zitadel_sub = "dev-sub"`, human, `is_admin=false`), org
  membership role **`admin`** (→ scope `org:dev-org:admin`; note: *not* `owner`, and there is no
  platform org in dev, so no `admin:all` exists in dev), team membership, default dataplane
  `dev-dataplane` (is_default), a registered proxy certificate row binding the dev SPIFFE URI to
  the team, and the stats-dashboard app enabled. Canonical constants `DEV_USER_SUB = "dev-sub"`,
  `DEV_USER_EMAIL = "dev@flowplane.local"` live in `src/auth/dev_token.rs`.

---

## 2. Authentication

### 2.1 Mode selection

`AuthMode` ∈ {`Dev`, `Prod`}, parsed from `FLOWPLANE_AUTH_MODE` (`"dev"` / `"prod"`); **defaults
to Prod**; any other value is a hard config error (`src/config/mod.rs`). Dev mode requires the
`dev-oidc` cargo feature; a non-dev build refuses to start with `FLOWPLANE_AUTH_MODE=dev`.

Critical property: **dev and prod share one auth code path.** Dev differs only in who signs the
JWT (embedded mock vs Zitadel). There is no `if dev { skip_auth() }` branch anywhere; the same
`validate_jwt_extract_sub → JIT upsert → load_permissions → check_resource_access` chain runs in
both modes.

`GET /api/v1/auth/mode` (unauthenticated) returns `{auth_mode, oidc_issuer?, oidc_client_id?}`
for CLI/frontend discovery; issuer/client-id only in prod
(`FLOWPLANE_OIDC_CLI_CLIENT_ID`, falling back to `FLOWPLANE_OIDC_SPA_CLIENT_ID`).

### 2.2 Prod: Zitadel JWT validation (`src/auth/zitadel.rs`)

Config (`ZitadelConfig::from_env`, opt-in on `FLOWPLANE_OIDC_ISSUER`):

| Env var | Meaning | Default |
|---|---|---|
| `FLOWPLANE_OIDC_ISSUER` | must equal JWT `iss` | required |
| `FLOWPLANE_OIDC_PROJECT_ID` | Zitadel project id | required when issuer set (hard error otherwise) |
| `FLOWPLANE_OIDC_AUDIENCE` | expected `aud` | defaults to project_id |
| `FLOWPLANE_OIDC_JWKS_URL` | JWKS endpoint | `{issuer}/oauth/v2/keys` |
| `FLOWPLANE_OIDC_USERINFO_URL` | userinfo fallback | `{jwks base}/oidc/v1/userinfo` |

JWT validation (`validate_jwt_extract_sub`):
1. Decode header; require `kid` (missing → 401).
2. Look up `kid` in the cached JWKS; on miss, `refresh_if_due(kid)` then retry; still missing →
   401 `no JWKS key for kid=...`.
3. Only RSA JWKs accepted; build the decoding key from `(n, e)`.
4. Validation pins **RS256** (no algorithm confusion), validates `iss`, `aud`, `exp`, and
   **explicitly enables `nbf`** (the jsonwebtoken crate default leaves `validate_nbf=false` — a
   v2 implementation must not regress this), with explicit 60 s clock-skew leeway.
5. Extract **only** `sub` (required, string — fail closed otherwise), `email?`, `name?`. Role
   claims (`urn:zitadel:iam:org:project:id:{pid}:roles`), scopes, and everything else are
   ignored at request time — permissions come from the DB.

**Userinfo fallback:** Zitadel access tokens commonly omit `email`. On JIT-provision cache miss
with empty email, the middleware calls the userinfo endpoint with the bearer token to fetch
`email`/`name`; any failure degrades gracefully (placeholder email used).

**Host-header handling:** Zitadel resolves instances by hostname. When JWKS/userinfo/token URLs
point at a container-internal address, requests carry a `Host:` header equal to the public
issuer's `host[:port]`.

**JWKS cache/rotation** (`JwksCache`):
- TTL cache, 1 hour (`JWKS_CACHE_TTL`).
- Unknown-`kid` triggers `refresh_if_due`: single-flight via a mutex-guarded last-attempt
  timestamp; per-issuer **30 s cooldown** (`REFRESH_COOLDOWN`) bounds attacker-driven refetch
  floods (N random kids → at most one burst per window). Inside the cooldown the current cache
  (or empty set) is returned without network I/O.
- Burst with expected kid: initial fetch + retries after backoffs **200ms, 500ms, 1000ms** (4
  fetches max) to cover Zitadel key-publish propagation lag.
- Each HTTP fetch hard-capped at **2 s** (`JWKS_FETCH_TIMEOUT`).
- Timestamp bumped both before (failed bursts still consume cooldown — no hammering a down IdP)
  and after the burst (slow bursts don't degrade single-flight).

**Revocation latency (accepted risk):** validation is stateless — no `jti` blocklist, no
introspection. A revoked Zitadel session's access token remains valid until `exp` (≤ ~1 h);
grant/permission changes propagate within the 60 s permission-cache TTL (plus explicit eviction,
§2.6). Documented v1 decision; v2 may keep or tighten.

### 2.3 Request authentication middleware (`src/auth/middleware.rs::authenticate`)

Applied to all `/api/v1/*` routes except the deliberately public set (§2.7). Order of
operations:

1. **OPTIONS passthrough** (CORS preflight).
2. **Per-IP rate limit** before any JWT work. Client IP resolved through trusted-proxy config:
   `FLOWPLANE_TRUSTED_PROXY_HEADER` (default `X-Forwarded-For`) and
   `FLOWPLANE_TRUSTED_PROXY_DEPTH` (default 1). Depth 0 → socket peer only, header ignored.
   Depth N → take the entry N positions from the right of the header; fewer than N hops →
   fall back to peer (truncated/spoofed chain). Prevents forged-XFF rate-limit-key rotation
   and audit-IP spoofing. 429 with `retry_after` on limit.
3. **Credential extraction:** `Authorization: Bearer <jwt>` always wins (CLI/machine). If
   absent and the BFF is enabled, decrypt the `fp_session` cookie; absent/tampered cookie →
   401 "authentication required". BFF disabled + no bearer → 401 (cookies never read).
4. **CSRF (cookie-authenticated, state-changing only):** POST/PUT/PATCH/DELETE authenticated via
   cookie require the double-submit check — non-HttpOnly `fp_csrf` cookie value must equal the
   `x-csrf-token` header, non-empty; else 403. Bearer clients are exempt (token is not ambient).
5. **Transparent refresh (cookie path):** if the session's access token is expired or within
   60 s of expiry (`REFRESH_SKEW_SECS`), exchange the stored refresh token at the provider; on
   success re-seal the cookie and attach `Set-Cookie` to the response; no refresh token or
   provider rejection → 401 (SPA bounces to login). Must surface 401, never 500, on a dead
   provider.
6. **JWT validation** (§2.2) on whichever token was resolved.
7. **Permission resolution:** check the in-memory **permission cache keyed by `sub`** (§2.6);
   on miss: JIT user upsert (with userinfo email fallback) → `load_permissions(pool, user_id)` →
   cache insert. `load_permissions` returns:
   - `org_scopes` from `organization_memberships` (§1.2);
   - `grants` from the `grants` table joined to team names, **filtered by
     `expires_at IS NULL OR expires_at > NOW()`**;
   - org memberships from `organization_memberships` are retained as a set. For tenant-scoped
     operations, `org_id`/`org_name`/`org_role` are derived from the validated request org
     context (`X-Flowplane-Org`, CLI `--org`, or active CLI config). With no selector, the
     server may infer an org only when the user has exactly one active non-platform org
     membership; the platform org never becomes the request org context.
   For machine users, `agent_context` is parsed from the user row.
8. **Build `AuthContext`** and insert into request extensions:
   `token_id = "zitadel:{sub}"`, `token_name = "zitadel/{sub}"`, `user_id`, `user_email`,
   `user_name?`, `org_scopes` (private set, queried via `has_scope`), `grants`,
   `agent_context?`, all active org memberships, validated request `org_id?`/`org_name?`, plus
   `client_ip`/`user_agent` for audit. A user with multiple tenant memberships and no selector
   must never receive an implicit default org for tenant-scoped authorization; `/auth/whoami`
   exposes the membership set so CLI context setup can offer/select an org without platform
   governance access.

### 2.4 Dev mode: embedded mock OIDC (`src/dev/oidc_server.rs`, feature `dev-oidc`)

- On boot with `AuthMode::Dev`, the CP starts an in-process mock OIDC server on an ephemeral
  loopback port and points the standard Zitadel middleware at it
  (`ZitadelConfig::from_mock` — issuer/jwks/userinfo all derived from the live bound URL; BFF is
  disabled in dev, bearer-only).
- Mock endpoints: OIDC discovery, JWKS, `/authorize` (PKCE S256; skipped if no challenge sent),
  `/token` (authorization_code, refresh_token, device_code grants), `/device/authorize`,
  `/userinfo`. Signs RS256 JWTs with an ephemeral in-memory 2048-bit RSA key (fresh per process);
  default token expiry 1 h, refresh 24 h; claims: `iss` (mock base URL), `sub = "dev-sub"`,
  `aud` (`test-project-id` by default), `exp`, `iat`, `email`/`name`. Configurable failure modes
  exist for tests (invalid_grant, expired tokens, slow, 500, device-pending).
- If `FLOWPLANE_CREDENTIALS_PATH` is set, the CP mints a dev JWT and writes it as JSON
  `{access_token, refresh_token: null, expires_at: null, issuer}` to that path (atomic
  write-then-rename, dir 0700, file 0600 — `src/auth/dev_token.rs`). `flowplane init` relies on
  this; the CLI then reads `~/.flowplane/credentials`.
- The minted token's `sub` matches the seeded dev user, so the normal JIT/permission path
  resolves to `org:dev-org:admin` — dev user is an org admin of the one dev org, **not** a
  platform admin.

### 2.5 Session/BFF auth for the UI (`src/api/handlers/auth_bff.rs`)

Prod-only backend-for-frontend so the SPA never holds raw tokens. Enabled when
`FLOWPLANE_OIDC_ISSUER` + `FLOWPLANE_OIDC_SPA_CLIENT_ID` are set **and** the secret-encryption
key is available (otherwise BFF is disabled, not insecure). Endpoint URLs default to Zitadel's
layout under the issuer (`/oauth/v2/authorize`, `/oauth/v2/token`, `/oidc/v1/end_session`,
`/oauth/v2/revoke`), each individually overridable via env. `FLOWPLANE_APP_URL` (default
`http://localhost:8080`) determines `redirect_uri = {app}/api/v1/auth/callback` and gates the
cookie `Secure` attribute (`https://` app URL ⇒ Secure).

Three unauthenticated endpoints:
- `GET /api/v1/auth/login` — generates PKCE verifier (48 random bytes) + state (24 bytes),
  seals `{state, verifier}` into the encrypted `fp_oidc_tx` cookie, 302 to the provider's
  authorize URL (`response_type=code`, scopes `openid profile email offline_access`,
  S256 challenge).
- `GET /api/v1/auth/callback` — provider error → redirect to `{app}/login?error=provider_error`.
  Otherwise: open tx cookie, verify `state` equality, exchange code (+verifier) for tokens,
  seal `{access_token, id_token?, refresh_token?, access_expires_at}` into the **encrypted,
  HttpOnly, SameSite=Lax, Path=/** `fp_session` cookie; also issue a fresh random `fp_csrf`
  cookie (non-HttpOnly, same flags) for double-submit; remove tx cookie; redirect to the app.
  All failures redirect to `/login?error=auth_failed` — never a raw 500 leaking provider detail.
- `GET /api/v1/auth/logout` — read session cookie; **revoke refresh token first** (RFC 7009 —
  Zitadel cascades to its access tokens), then access token; revocations awaited but failures
  logged-and-swallowed (user is still signed out locally); remove `fp_session` + `fp_csrf`;
  302 to provider end_session with `post_logout_redirect_uri = {app}/login`.

Cookie crypto: AES-GCM via the platform `SecretEncryption` (master key
`FLOWPLANE_SECRET_ENCRYPTION_KEY`); value layout `base64url(nonce[12] || ciphertext||tag)`;
distinct AEAD associated-data labels per cookie purpose (`flowplane:bff:oidc-tx:v1` vs
`flowplane:bff:session:v1`) so a tx cookie can never be replayed as a session cookie. Any
tamper/AAD mismatch ⇒ "no session".

`GET /api/v1/auth/session` (authenticated) returns the DB-sourced session shape for the SPA:
`{user_id, email, name (OIDC name or email-prefix fallback), is_admin/is_platform_admin
(= has admin:all), org_scopes, grants[{teamName,resourceType,action}], teams (from grants),
org_id?, org_name?, org_role?}`. The frontend never parses JWT claims.

### 2.6 Permission cache (`src/auth/cache.rs`)

In-memory TTL map keyed by `sub`. TTL from `FLOWPLANE_PERMISSION_CACHE_TTL_SECS`, default
**60 s**. Snapshot holds user_id/email/name/org_scopes/grants/org context/agent_context.
Explicit eviction (`evict(sub)` / `evict_by_user_id`) is called by every mutation that changes a
principal's permissions: grant create/delete, membership add/remove/role-change, agent
provisioning/reconciliation, DCR. Worst-case staleness for a change without eviction = TTL.

### 2.7 CLI auth (`src/cli/auth.rs`)

`flowplane auth login | token | whoami | logout`.

- Mode discovery via `GET /api/v1/auth/mode`. Dev mode → no login; use the credentials file
  written at `flowplane init`.
- Prod login resolves issuer/client-id from `--flags` → CP-advertised values → env
  (`FLOWPLANE_OIDC_ISSUER` / `FLOWPLANE_OIDC_CLIENT_ID`). The cached `config.toml` values are
  deliberately **not** consulted (stale Zitadel app ids after a rebuild cause
  `Errors.App.NotFound`).
- **PKCE flow (default):** OIDC discovery → spawn loopback HTTP listener on an ephemeral
  `127.0.0.1` port → open browser to authorize URL (S256 challenge, random state, scopes
  `openid profile email offline_access`, **`prompt=login`** so re-login after logout cannot
  silently reuse the SSO session) → callback validated for state (CSRF) and error params →
  code exchange. 5-minute timeout with a hint to use `--device-code`.
- **Device-code flow (`--device-code`):** requires the provider to advertise
  `device_authorization_endpoint`; prints `verification_uri` + `user_code`; polls token endpoint
  every 5 s handling `authorization_pending` / `slow_down` / `expired_token` / `access_denied`.
- Credentials stored at `~/.flowplane/credentials` as JSON
  `{access_token, refresh_token?, expires_at?, issuer?}`, file 0600 / dir 0700.
- **Auto-refresh:** before use, if expired (60 s buffer), refresh-token grant against the stored
  issuer's token endpoint; persists rotated tokens.
- **Logout:** best-effort RFC 7009 revocation (refresh token preferred) when the provider
  advertises `revocation_endpoint`, then delete the local file; prints an honest warning that
  already-issued access tokens stay valid until `exp` (stateless CP validation) and the browser
  IdP session persists.

### 2.8 MCP client auth

MCP is served over streamable HTTP at `/api/v1/mcp` behind the **same** `authenticate`
middleware — bearer JWT (human via CLI token, or machine via Zitadel `client_credentials`).
There is no separate MCP credential type. Additional MCP-layer protections
(`src/mcp/security.rs`): Origin-header allowlist as CSRF defense-in-depth for browser clients
(default allowlist `http://localhost`, `http://127.0.0.1`, `http://[::1]` — any port; override
via comma-separated `FLOWPLANE_MCP_ALLOWED_ORIGINS`; matching ignores port, requires exact
scheme+host); session ids are `mcp-{uuidv4}` and format-validated; connection ids are
`conn-{team}-{uuid}`; `check_team_ownership(resource_team, request_team)` requires exact,
case-sensitive equality.

---

## 3. Authorization

### 3.1 The single gate: `check_resource_access(context, resource, action, team) -> bool`

(`src/auth/authorization.rs`). Inputs: the request `AuthContext`; a resource string
(e.g. `"routes"`, `"clusters"`, `"organizations"`); an action (`"read"|"create"|"update"|
"delete"|"execute"`); an optional team **name** (not UUID — callers resolve UUIDs to names
first via `resolve_team_id_to_name`).

Exact decision logic (order matters):

```
fn check_resource_access(ctx, resource, action, team) -> bool:

  # 0. Agent structural guard — evaluated FIRST, short-circuits everything below
  if ctx.agent_context is Some(agent):
      match agent:
        CpTool      -> if team is Some(t): return has_grant(resource, action, t)   # GrantType::Resource only
                       else:               return has_any_grant(resource, action)
        GatewayTool -> return false        # never reaches CP resources
        ApiConsumer -> return false        # never reaches CP resources

  # 1. Platform-admin governance bypass (governance resources ONLY)
  if ctx.has_scope("admin:all") and is_governance_resource(resource):
      return true

  if team is Some(team_name):
      # 2a. Exact grant row
      if ctx.has_grant(resource, action, team_name):       # grant_type==Resource,
          return true                                       # matching (resource, action, team_name)
      # 2b. Org-admin implicit team access — only when the scope's org
      #     matches the user's DB-loaded org_name (defense in depth against
      #     a corrupted/foreign scope; warn-logged denial otherwise)
      for (org, role) in parse org_scopes matching "org:{name}:{admin|member|viewer}":
          if role == "admin" and ctx.org_name == Some(org):
              return true
      return false

  else:  # team is None
      # 3a. ANY-team grant — lets team-scoped users hit list endpoints,
      #     which then filter rows to their teams
      if ctx.has_any_grant(resource, action):
          return true
      # 3b. Governance resources, org-scoped callers:
      #     reads: ANY org scope (admin/member/viewer) suffices
      #     writes: NEVER reachable via org scopes (admin:all only, via step 1)
      if is_governance_resource(resource):
          return action == "read" and org_scopes is non-empty
      # 3c. Tenant resources without team: org ADMIN only, with the same
      #     scope-org == ctx.org_name cross-check; member/viewer blocked
      for (org, role) in org_scopes:
          if role == "admin" and ctx.org_name == Some(org):
              return true
      return false
```

`require_resource_access(...)` = same check, `Err(Forbidden)` → HTTP 403 on failure.

**Governance resource list** (`is_governance_resource`) — exact closed set:

```
admin · admin-orgs · admin-users · admin-audit · admin-summary
admin-teams · admin-scopes · admin-apps · admin-filter-schemas
organizations · users · teams · stats
```

Everything else (clusters, routes, listeners, filters, secrets, dataplanes, learning-sessions,
proxy-certificates, agents, …) is a **tenant** resource.

### 3.2 Three hard invariants (test-asserted in v1; v2 must preserve)

1. **`admin:all` is governance, not resource bypass.** A platform admin manages orgs, users,
   teams, audit, and platform stats. They cannot read/list/create/delete any tenant resource —
   tenant resources are invisible to a pure `admin:all` context. Likewise
   `InternalAuthContext.is_admin` never bypasses `can_access_team`/`can_create_for_team`/
   `verify_team_access`. Tenant-internal reads use `has_org_membership_only` /
   `require_org_admin_only` variants that exclude the `admin:all` arm (member CRUD, team
   management, DCR, grants).
2. **Cross-org access requires explicit grant.** Org context (`org_id`/`org_name`) is loaded
   from the DB, never from the URL. The org-admin implicit-team arm cross-checks the scope's
   org against `ctx.org_name`. `verify_org_boundary(ctx, team_org_id)` returns **404** (not
   403, anti-enumeration) when user-org ≠ team-org or when the user has no org and the team
   does; platform admins do NOT bypass it.
3. **Team isolation is enforced in DB queries.** Every tenant resource read/write filters by
   team FK; `verify_team_access` (REST: `src/api/handlers/team_access.rs`; internal:
   `src/internal_api/auth.rs`) returns 404 for cross-team hits (existence-hiding) and emits a
   cross-team-access metric. There is no API path returning a tenant resource without a team
   check.

### 3.3 Role summary

| Role / context | Can |
|---|---|
| Platform admin (`admin:all`, platform-org owner) | All governance reads/writes (orgs, users, teams, audit, summary, admin-apps, admin-filter-schemas, scopes listing); create/delete orgs; invite the first org admin; query all audit logs. Cannot touch tenant resources, org member CRUD, org team CRUD, grants, or DCR. |
| Org admin (`org:{o}:admin` = DB role `owner`/`admin`) | Everything in every team of their own org (implicit team access, expanded to all org teams for listing via `get_effective_team_scopes_with_org`); manage org members/roles, org teams, agents (with `agents:*` grant for create), grants, DCR; governance reads; update own org. Not platform governance writes; nothing in other orgs. |
| Org member (`org:{o}:member`) | Governance **reads** (org/user/team listings, stats); tenant access only through explicit grant rows. |
| Org viewer (`org:{o}:viewer`) | Same gate behavior as member at this layer (governance reads; needs grants for tenant resources); intended read-only. NOTE: the scope-format regex (`scope_registry.rs`) accepts only `admin|member` for org scopes while the runtime parser accepts `viewer` — see Gaps. |
| Team-scoped user (grants only) | Exactly the `(resource, action, team)` triples in their grant rows. |
| cp-tool agent | Grant-based like a human, `Resource` grants only; no governance bypass arm, no org-admin arm. |
| gateway-tool agent | MCP gateway tools only (per `gateway-tool` grants + route exposure); structurally denied all CP resources. |
| api-consumer agent | Data-plane only via Envoy RBAC compiled from `route` grants; structurally denied all CP resources. |

### 3.4 Grant model and validation

Valid resource/action pairs are a **code constant** (`scope_registry.rs::VALID_GRANTS` — the
`scopes` DB table was dropped):

```
clusters/routes/listeners/filters/secrets/dataplanes/custom-wasm-filters/agents:
    read create update delete
ai-providers/ai-routes/ai-budgets:
    read create update delete
ai-usage:
    read
learning-sessions: read create execute delete
aggregated-schemas: read execute
proxy-certificates: read create delete
reports/audit/stats: read
```

S10 AI gateway resources are tenant resources. `AiProvider`, `AiRoute`, and `AiBudget` CRUD
uses the `ai-providers`, `ai-routes`, and `ai-budgets` grants above. `AiUsage` is read-only:
usage rows are append-only system events, not caller-created resources, and `ai-usage:read`
must filter by the caller's authorized team before aggregating token counts, model names,
provider names, or budget status. Creating/updating an `AiProvider` validates that the
referenced `Secret` belongs to the same team and that the caller can read/reference that secret;
deleting an AI resource follows the same dependency-blocking pattern as gateway resources.

Scope-string grammar (for DCR scope parsing and validation):
`{resource}:{action}` · `team:{name}:{resource}:{action}` · `team:{name}:*:*` ·
`org:{name}:{admin|member}` · `admin:all`. Team/org name segments use the canonical name regex
(lowercase-letter-leading, single-hyphen-separated; `a--b`, `-foo`, `foo-` rejected).

Grant CRUD: `POST/GET/DELETE /api/v1/orgs/{org}/principals/{principal_id}/grants` —
`require_org_admin_only`. Creation validates: grant_type ∈ {resource, gateway-tool, route};
principal must hold a membership row in this org (404 otherwise); type compatibility (human →
resource only; machine → grant type must match its `agent_context`); resource grants need a
valid `(resource_type, action)` pair; gateway-tool/route grants need a `route_id` whose route
has `exposure == "external"`; the team must exist in the org and the principal must be a member
of that team. On create/delete: permission-cache eviction (by `zitadel_sub` when known) and, for
`route` grants, an xDS listener refresh so the Envoy RBAC filter updates. Audited as governance
`grant.create` / `grant.delete`. Grants may carry `expires_at`; expired rows are excluded at
load time (§2.3 step 7) — expiry is enforcement-by-omission, no reaper required.

### 3.5 Where the gate is invoked, per surface

- **REST, layer 1 — dynamic scope middleware** (`ensure_dynamic_scopes`, on the secured router):
  derives `resource = resource_from_path(path)`, `action = action_from_request(method, path)`,
  and calls `require_resource_access(ctx, resource, action, None)` (team=None — team checked at
  handler level). Path → resource mapping rules (`resource_from_path`):
  - non-`/api/v1/*` paths and these prefixes **skip the middleware entirely** (handler enforces):
    `/api/v1/teams` (bare list), `/api/v1/auth/*`, `/api/v1/orgs/*`,
    `/api/v1/admin/organizations/*`, `/api/v1/audit-logs`, `/api/v1/org/*` (in-org audit),
    `/api/v1/mcp` (JSON-RPC method-level auth).
  - renames: `route-configs|route-views → routes`, `filter-types → filters`,
    `openapi/* → openapi-import`; team-scoped sub-resources
    `/api/v1/teams/{team}/{res}` → `{res}` with `custom-filters → custom-wasm-filters`,
    `rate-limit-domains|rate-limit-policies → rate-limit`,
    `/teams/{team}/bootstrap → generate-envoy-config`, `/teams/{team}/ops/{sub}` →
    {trace→routes, topology|validate|xds→clusters, audit→audit, learning→learning-sessions,
    default→clusters}.
  - action mapping: GET/HEAD/OPTIONS→read, POST→create, PUT/PATCH→update, DELETE→delete; paths
    ending `/export` or `/compare`, or containing a whole segment `search` or `query`, are
    semantically **read** regardless of method (no substring false-positives:
    `search-configs` POST stays create).
- **REST, layer 2 — handlers**: re-check with the concrete team
  (`require_resource_access_resolved` resolves UUID→name first), then row-level
  `verify_team_access` / `verify_org_boundary` on fetched resources. Org/member/team/grant
  management handlers use the `require_admin` / `require_org_admin[_only]` /
  `has_org_membership[_only]` helpers as in §3.3's table.
- **MCP control plane**: every `tools/call` goes through `check_tool_authorization(tool, team)` —
  each tool in the static registry declares `(resource, action)`
  (`src/mcp/tool_registry.rs::get_tool_authorization`), and the handler calls
  `check_resource_access(ctx, resource, action, team_opt)` (empty team string → None,
  governance tools). Tool/resource listing filters teams by `check_resource_access(ctx, "api",
  "read", team)`; gateway tool execution requires `api:execute` on at least one team and
  searches only authorized teams. `resources/read` paths re-check per team.
- **Internal API** (`src/internal_api/auth.rs::InternalAuthContext`): surface-agnostic adapter.
  `from_rest` / `from_rest_with_org` (org-admin team expansion) for REST;
  `from_mcp(team, org_id, org_name)` for MCP — **an empty team string yields a powerless
  context, never admin** (a v1 vuln fix: `is_admin` is never inferred from emptiness;
  governance callers must use the explicit `InternalAuthContext::admin()`, which still cannot
  touch team resources). `requested_team` (from URL path) additionally pins
  `verify_team_access` to the exact team, preventing cross-team reads via unrelated paths.
  `can_create_for_team(None) == false` — no create path may write a NULL-team (global)
  resource.

### 3.6 Org-scoped helper semantics (handler-level checks)

| Helper | admin:all passes? | Logic |
|---|---|---|
| `has_org_admin(ctx, org)` | yes | `admin:all` OR `org:{org}:admin` — for governance ops like inviting the first org admin |
| `has_org_admin_only(ctx, org)` | **no** | `org:{org}:admin` only — member CRUD, team CRUD, grants, DCR |
| `has_org_membership(ctx, org)` | yes | admin/member/viewer scope OR `admin:all` |
| `has_org_membership_only(ctx, org)` | **no** | admin/member/viewer scope — tenant-internal reads platform admin must not see |
| `verify_same_org(team_org, user_org, is_admin)` | yes (explicit flag) | org equality required; team-without-org (global) or both-None allowed |

---

## 4. Token / credential lifecycle

| Credential | Issued by | Lifetime | Refresh | Revocation |
|---|---|---|---|---|
| Access JWT (human, prod) | Zitadel (PKCE / device code) | provider-set, ≤ ~1 h | refresh-token grant (CLI auto-refresh with 60 s buffer; BFF server-side silent refresh with 60 s skew) | RFC 7009 revoke on CLI/BFF logout; **stateless CP keeps accepting until `exp`** (documented accepted risk) |
| Access JWT (machine) | Zitadel `client_credentials` | provider-set | re-request | client-secret rotation via Zitadel; permission changes via grants + cache evict |
| Dev JWT | embedded mock OIDC | 1 h default | mock supports refresh grant; the seeded credentials file has no refresh token | none (ephemeral signing key dies with the process) |
| BFF session cookie | CP (encrypts provider tokens) | bounded by refresh-token life | transparent middleware refresh | logout revokes underlying tokens + clears cookie; cookie is AEAD-sealed, tamper ⇒ no session |
| `client_secret` (agents/DCR) | Zitadel via Management API | until rotated | n/a | delete agent (`DELETE /orgs/{o}/agents/{id}`, requires `agents:delete`) |
| Bootstrap token | operator env `FLOWPLANE_BOOTSTRAP_TOKEN` | until env cleared | n/a | unset env (endpoint 403s) |
| Zitadel admin PAT | Zitadel (`FLOWPLANE_OIDC_ADMIN_PAT`) | Zitadel-managed | n/a | rotate in Zitadel; absence disables DCR/invite/agent provisioning (503) |
| Grants (authz rows) | org admin | optional `expires_at` | n/a | delete row; effective within cache TTL / on evict |

There are **no Flowplane-native personal access tokens or API keys** in v1: every credential is
an OIDC token (or the one bootstrap env token). The legacy `password_hash` column is dead
(always `''`).

`GET /api/v1/scopes` lists UI-visible scope definitions (any authenticated user);
`GET /api/v1/admin/scopes` (admin:all) additionally includes `admin:all`. Both are generated
from the code constant — there is no scopes table.

---

## 5. Dataplane identity (cross-ref spec/04)

Operator identity (JWT) and dataplane identity (mTLS client cert) are disjoint; neither
credential works on the other surface. xDS mTLS is unconditional — no plaintext gRPC.

Authz-relevant part (`src/xds/services/mtls.rs`):
- The Envoy client cert's URI SAN must be SPIFFE:
  `spiffe://{trust_domain}/org/{org}/team/{team}/proxy/{proxy_id}` (org-scoped) or legacy
  `spiffe://{trust_domain}/team/{team}/proxy/{proxy_id}`.
- `ClientIdentity {org?, team, proxy_id, spiffe_uri, serial_number}` is parsed from the
  **certificate**, never from node metadata (node-metadata team is logged, never authorized).
- The SPIFFE URI must match a registered, non-revoked `proxy_certificates` row binding it to a
  team; that team scopes every xDS read — the connection only ever receives that team's
  resources.
- Because the URI SAN *content* is trusted as identity, the CA must refuse client-chosen URI
  SANs. For the Vault PKI backend the CP enforces this with a **boot probe**: it refuses to
  start if the PKI role's `allowed_uri_sans` is empty or contains `*`
  (`src/secrets/vault.rs::read_pki_role_uri_sans`). Other cert backends (cert-manager, GCP CAS,
  AWS PCA, Roles Anywhere) rely on operator-side issuance policy — operator checklist in
  v1 `docs/explanation/auth-model.md` §"Cert-backend invariant".
- Dev mode seeds one proxy-certificate row for the generated dev dataplane cert
  (`DEV_DATAPLANE_SPIFFE_URI`), 30-day expiry, upserted on each boot.

---

## 6. Audit

Audit writes are **best-effort** (failures are error-logged and swallowed, never failing the
request) into the `audit_log` table (`src/internal_api/audit.rs`, `src/storage/.../audit_log.rs`).

Auth-relevant audited events:
- **Governance actions** (`record_governance_action`): resource_type ∈ {`organization`, `user`,
  `org_membership`, `grant`, `team`}, action a bare verb (`create`, `update`, `delete`,
  `invite`, `add`, `remove`); carries acting `user_id` and affected `org_id`; deliberately
  carries **no resource id/name** (redaction posture of the platform-admin feed). Emitted by:
  org create/update/delete, member add/invite/role-change/remove, team create/update/delete,
  team-member add/remove, agent create/delete, grant create/delete.
- **Provisioning creates** (`record_provisioning_create`): clusters/listeners/route-configs
  create, dataplane register — with org/team/user context (feeds governance rollups).
- **Secrets access** via the audited secrets client (`src/secrets/audited.rs`).
- `AuthContext.to_audit_context()` supplies `(user_id, client_ip, user_agent)` — client IP from
  the trusted-proxy resolution, so forged XFF cannot spoof audit IPs.

Query surfaces: `GET /api/v1/audit-logs` — requires `admin:all`; org-scoped admins (admin:all +
org_id) are forcibly filtered to their own org; pure platform admins can query any org.
In-org views `GET /api/v1/org/audit-logs|audit-volume` require `audit:read` access and 403 when
the context carries no org_id. Team-scoped `/teams/{team}/ops/audit` maps to the `audit:read`
grant.

**Not audited** (log-only via `tracing`): authentication failures, JWT validation errors,
rate-limit hits, cross-team access attempts (metric only), CSRF failures, BFF login/logout.
v2 should consider promoting login failures and authz denials to audit rows (see Gaps).

---

## 7. Gaps and smells (feeds spec/08, 08a)

1. **No login/authn-failure audit trail.** Failed JWT validation, rate-limit trips, CSRF
   failures, and cookie-session refresh failures are tracing-logs only — no `audit_log` rows, no
   alerting hook. Authorization denials likewise (cross-team attempts are a metric only).
2. **Stateless revocation window.** Revoked/rotated Zitadel sessions remain valid until `exp`
   (≤ ~1 h) — no `jti` blocklist or introspection. Explicitly accepted in v1 and surfaced
   honestly in CLI logout, but it is a real window; v2 should decide deliberately.
3. **`viewer` role inconsistency.** `map_org_role_to_scopes` and `parse_org_from_scope` accept
   `viewer`, and `has_org_membership*` honor it, but `scope_registry.rs`'s
   `SCOPE_FORMAT_REGEX`/`is_valid_org_scope` accept only `admin|member` — `org:{name}:viewer`
   fails scope validation while existing at runtime. At the gate, viewer behaves identically to
   member (governance reads); nothing actually enforces read-only-ness for viewers holding
   grants.
4. **Dual authz vocabularies.** Org-level = scope strings, team-level = grant rows, plus the
   legacy `team:{name}:{res}:{act}` *string* grammar still validated by `scope_registry` and
   used in DCR scope parsing — three representations of permission that must be kept coherent
   by hand. v2 should collapse to one.
5. **`users.is_admin` is dead weight.** Column exists, is seeded `false` everywhere, and is
   never consulted by the gate — confusing; drop or use.
6. **Middleware-skip list is hand-maintained.** `resource_from_path` returning `None` (orgs,
   admin/organizations, audit-logs, org/*, mcp, auth, bare teams list) shifts the entire burden
   to handler-level checks; a new handler under a skipped prefix that forgets its check ships
   with **no** authorization. Same risk for the semantic-read rules (`/export`, `/compare`,
   `search`, `query` segments): any future state-changing endpoint under such a path is
   auto-downgraded to `read`. v2 should make authz declarations per-route and fail-closed.
7. **`verify_same_org` allows team-without-org ("global team") and unscoped-user/unscoped-team
   combinations**, and `verify_team_access` treats NULL-team resources as globally readable.
   Combined with the nullable `team` columns on clusters/route_configs/listeners (creates are
   blocked from writing NULL by `can_create_for_team`, but legacy rows may exist), a NULL-team
   row is visible to everyone. Make team non-nullable in v2.
8. **Bootstrap token is a static bearer secret in an env var** — constant-time-compared and
   self-disabling, but long-lived if the operator never unsets it; any holder can create orgs
   pre-JIT. Prefer one-shot/expiring bootstrap in v2.
9. **Dev-mode prod-leak surfaces.** (a) `auth_mode_handler` reads `FLOWPLANE_AUTH_MODE` directly
   from env at request time rather than the parsed `AuthMode` — drift risk between what the
   server runs and what it advertises; (b) dev mock OIDC binds an unauthenticated token-minting
   endpoint on loopback — fine locally, but nothing prevents `FLOWPLANE_AUTH_MODE=dev` in a
   container exposed to a network (guarded only by the cargo feature); (c) dev seeds and
   credentials use fixed IDs (`dev-sub`, `dev-user-id`, `dev-org-id`) — harmless if dev mode
   never runs in prod, catastrophic if it does. v2: refuse dev mode when a prod-ish env marker
   is present.
10. **Hardcoded/printed credentials paths.** `initial_admin_password` /
    `initial_password` / `FLOWPLANE_SUPERADMIN_INITIAL_PASSWORD` flow plaintext passwords
    through API bodies/env into Zitadel (deliberate no-SMTP convenience; marked never-logged but
    review for accidental logging); `setup-zitadel.sh` writes the Zitadel admin PAT into
    `.env.zitadel` (`ZITADEL_ADMIN_PAT`, `FLOWPLANE_OIDC_ADMIN_PAT`) on disk; agent/DCR
    `client_secret` is returned once in an HTTP response (acceptable, but ensure no
    request/response logging captures it).
11. **`create_org_agent` authz asymmetry.** The `agents:create` check runs against only the
    *first* team in the request; the agent is then made a member of *all* listed teams. A
    grantee with `agents:create` on one team can attach an agent to other org teams (grants
    still gate what it can do there, but team membership is a prerequisite for grants —
    membership escalation). Also `agent_context` is hardcoded to `cp-tool` (open TODO E.3).
12. **`list_org_agents`/`delete_org_agent` org check is name-based** (`context.org_name ==
    org_name` plus `agents:read|delete` any-team) rather than the id-based
    `verify_org_boundary`; consistent renames/odd states could diverge. Minor, but v2 should
    standardize on org-id comparison.
13. **Permission cache has no size bound** — keyed by attacker-suppliable-ish `sub` (only after
    a *valid* signature, so bounded by IdP user count in practice) but unbounded growth and no
    LRU; also `evict_by_user_id` is O(n).
14. **MCP origin allowlist defaults are permissive for any-port localhost** and origin matching
    ignores ports entirely — fine as defense-in-depth (bearer auth is primary), but document
    that origin checks are not a security boundary.
15. **JWKS fetch lacks TLS pinning/issuer-cert validation knobs** and `JWKS_FETCH_TIMEOUT=2s`
    with empty-cache cooldown returns an empty key set → all logins 401 for the cooldown window
    after a failed first fetch at boot (cold-start brownout on slow IdP).
