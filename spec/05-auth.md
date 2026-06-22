# 05 — Authentication and Authorization

v2 as-built. Sources cited inline as v2 paths (`crates/fp-*/src/...`, `crates/fp-storage/migrations/...`). Authorization design rationale is Q-004 (provider-agnostic identity) and D-014 (multi-org active-org selection).

Two separate questions, two separate layers:

- **Authentication** — "who are you?" — is OIDC, against any compliant IdP in prod mode and an in-process issuer in dev mode. JWTs are identity-only; they carry **no** authorization data. Machine identities (agents) authenticate with locally-issued bearer tokens, not OIDC. (`crates/fp-core/src/oidc.rs`, `crates/fp-api/src/auth.rs`)
- **Authorization** — "what can you do?" — is one in-process function, `check_resource_access(ctx, resource, action, team) -> Decision`, that every surface (REST, MCP) converges on. Permissions live in the database as `grants`, never in the token. (`crates/fp-core/src/authz.rs`)

---

## 1. Identity model

### 1.1 Entities

Identity tables are defined in `crates/fp-storage/migrations/0002_identity.sql`. Humans and machines are **separate tables** — there is no `user_type`/`agent_context` discriminator on a shared `users` row.

| Entity | Table | Key fields | Notes |
|---|---|---|---|
| **Organization** | `organizations` | `id` (UUID), `name` (NOT NULL UNIQUE), `display_name`, `status` (`active`/`suspended`) | Governance layer above teams. A special org named **`platform`** holds platform operators (`crates/fp-storage/src/repos/identity.rs::set_platform_org`). (`0002_identity.sql:8-15`) |
| **Team** | `teams` | `id` (UUID), `org_id` (NOT NULL FK), `name` (unique **per org**), `status`; `UNIQUE(org_id, name)` and `UNIQUE(id, org_id)` | Tenancy unit. The `UNIQUE(id, org_id)` lets composite FKs prove team ∈ org at the schema level. (`0002_identity.sql:17-29`) |
| **User** (human) | `users` | `id` (UUID), `subject` (TEXT NOT NULL UNIQUE — the OIDC `sub` claim, provider-agnostic), `email`, `name`, `status` (`active`/`suspended`) | `subject` is the bridge from OIDC identity to local permissions. No password storage, ever — there is no `password_hash`, `is_admin`, `user_type`, or `agent_context` column. (`0002_identity.sql:31-41`) |
| **Agent** (machine) | `agents` | `id` (UUID), `org_id` (NOT NULL FK), `name`, `kind` (`cp-tool`/`gateway-tool`/`api-consumer`, CHECK), `token_hash`, `status`; `UNIQUE(org_id, name)`, `UNIQUE(token_hash)` | Machine identity, org-owned. Authenticates with a locally-issued bearer token whose SHA-256 hash is stored. `kind` structurally bounds what the agent can ever reach (§3.3). (`0002_identity.sql:43-56`, `migrations/0028_agents_token_hash_unique.sql`) |
| **Org membership** | `org_memberships` | `(user_id, org_id)` UNIQUE, `role` ∈ {`owner`, `admin`, `member`, `viewer`} (CHECK) | Source of a user's org role. (`0002_identity.sql:58-65`) |
| **Team membership** | `team_memberships` | `(user_id, team_id)` UNIQUE | Records which teams a user belongs to. Membership alone grants **nothing** — grants do. (`0002_identity.sql:67-73`) |
| **Grant** | `grants` | `id`, `principal_type` (`user`/`agent`), `principal_id`, `org_id`, `team_id`, `resource`, `action` (`read`/`create`/`update`/`delete`/`execute`, CHECK), `created_by`; composite FK `(team_id, org_id) → teams(id, org_id)`; `UNIQUE(principal_type, principal_id, team_id, resource, action)` | The team-level permission row — a `(principal × resource × action × team)` tuple. The composite FK makes a grant whose team is outside its org unrepresentable. (`0002_identity.sql:75-91`) |

There is no `scopes` table and no scope-string column anywhere. Authorization is the `grants` rows plus the principal's typed org role (§3).

### 1.2 Org roles

Org-level access is a **typed role** (`OrgRole`) loaded from `org_memberships`, not a scope string computed at request time. The DB roles are `owner`, `admin`, `member`, `viewer` (`0002_identity.sql:62`). Platform-admin status is a boolean derived once per request — it is `true` iff the user is an `owner` of the `platform` org (`crates/fp-api/src/auth.rs:177-185`, `crates/fp-core/src/authz.rs:55-77`). There is no `admin:all` / `org:{name}:{role}` string representation at runtime (the v1 scope-string vocabulary is gone — `crates/fp-domain/src/authz.rs`).

### 1.3 Principal kinds

- **human** — interactive operator (CLI/UI). Authenticates via OIDC JWT; resolves to `PrincipalCtx::User`.
- **agent** — machine identity (service account / AI tool). Authenticates via a `fpat_…` bearer token; resolves to `PrincipalCtx::Agent`. `kind` partitions what the agent may touch (§3.3):
  - `cp-tool`: control-plane access; uses the same `(resource, action)` grants as humans, with no governance or org-admin arm.
  - `gateway-tool`: structurally limited to the `McpTools` resource; access there still requires a matching grant. Denied every other resource.
  - `api-consumer`: data-plane only via Envoy RBAC; structurally denied every control-plane resource.

(`crates/fp-core/src/authz.rs:55-77,145-200`)

### 1.4 Provisioning flows

- **JIT (every authenticated request, humans):** after JWT validation the middleware upserts the user by `subject` (`crates/fp-storage/src/repos/identity.rs::upsert_user_by_subject`, called from `crates/fp-api/src/auth.rs:156-162`): `INSERT ... ON CONFLICT (subject) DO UPDATE`. `email`/`name` are taken from the token claims. JIT creates the user row only — no memberships, no grants; a fresh JIT user can do nothing until granted.
- **Members:** a user is added to an org/team by an org admin via the identity API (`crates/fp-api/src/identity_api.rs`, registered in `crates/fp-api/src/routes.rs:65-68`). Membership is by `subject`/email lookup — there is **no** invite-token email flow and **no** IdP Management-API call. (Invite tokens: ABSENT.)
- **Agent provisioning:** an org admin creates an agent (`crates/fp-core/src/services/agents.rs:26-43`). A fresh token `fpat_{uuid}{uuid}` is minted, returned **once**, and only its SHA-256 hash is persisted (`agents.rs:82-87,132-174`; `identity.rs::hash_agent_token:39`). The token is rotated with `rotate_agent_token` (new token, replaces hash) and revoked by setting `status='suspended'` (`agents.rs:192-228`, `identity.rs:368-410`). No OIDC client, no `client_credentials`, no Dynamic Client Registration. (Zitadel / Management API / DCR / `client_credentials`: ABSENT.)
- **Bootstrap (first platform admin):** `POST /api/v1/bootstrap/initialize`, off the JWT path, gated by a one-shot operator token. See §1.5.
- **Dev seeding (boot, dev mode):** `crates/fp-storage/src/seed.rs::seed_dev` — idempotent (`ON CONFLICT DO NOTHING`) inserts: org `dev-org`, team `default`, user with `subject = "dev-user"`, an `org_memberships` row with role **`owner`** in `dev-org`, and a team membership (`seed.rs:13-79`). Dev mode has **no platform org and no platform admin** (`seed.rs:6`) — the dev user is an org owner of the single dev org only.

### 1.5 Bootstrap

The first platform admin is created before any JWT-authenticated principal exists, via a one-shot operator-supplied token (`crates/fp-storage/src/repos/bootstrap.rs`, table `bootstrap_tokens` in `0002_identity.sql:114-121`).

- **Token:** format `fpboot_{uuid}{uuid}`; only its SHA-256 hash is stored, never logged (`bootstrap.rs:23-49`). Expires 24 h after seeding (`bootstrap.rs:55,165`). Single-use — `used_at` is stamped on consume (`bootstrap.rs:230`).
- **Provenance (`operator_seeded`, `migrations/0029_bootstrap_token_operator_seeded.sql`):** `true` = operator-supplied via `seed_token_hash_if_uninitialized` (the normal path); `false` = the legacy generate-and-log path, behind the `FLOWPLANE_ALLOW_LOGGED_BOOTSTRAP_TOKEN` opt-in. A successful operator seed invalidates prior unused *legacy* tokens; a different live *operator-seeded* token fails closed rather than being clobbered.
- **Endpoint:** `POST /api/v1/bootstrap/initialize` with `Authorization: Bearer fpboot_*` and body `{org_name, org_display_name, admin_subject, admin_email}` (`crates/fp-api/src/routes.rs:224-227`, handler `bootstrap.rs:181+`). Serialized and made idempotent across replicas by `pg_advisory_xact_lock(BOOTSTRAP_LOCK_KEY)` (`bootstrap.rs:21,210`). In one transaction it consumes the token, creates the `platform` org, creates the admin user, adds an `owner` membership, and audits.
- `GET /api/v1/bootstrap/status` (unauthenticated) reports whether the instance is initialized (`routes.rs:223`).

---

## 2. Authentication

### 2.1 Mode selection

Auth mode is selected by **`FLOWPLANE_DEV_MODE`** (`true`/`false`), not a `FLOWPLANE_AUTH_MODE` enum (`crates/fp-core/src/config.rs:263-266`). Dev mode and prod OIDC config are **mutually exclusive**: setting `FLOWPLANE_DEV_MODE=true` together with `FLOWPLANE_OIDC_*` is a hard config error (`config.rs:298-303`). Dev mode requires the `dev-oidc` cargo feature (a default feature of the `flowplane` crate, `crates/flowplane/Cargo.toml:10`); a build without it has `setup_dev_mode` reject dev mode at startup (`crates/flowplane/src/serve.rs:344`). Build the release artifact without default features to harden it against dev mode.

Critical property: **dev and prod share one auth code path.** Dev differs only in who signs the JWT (an in-process issuer vs the external IdP). There is no `if dev { skip_auth() }` branch — the same `validate → JIT upsert → load_principal → check_resource_access` chain runs in both modes (`crates/fp-api/src/auth.rs`). When no OIDC issuer is configured and dev mode is off, authenticated endpoints fail closed (`config.rs:43-44`).

### 2.2 OIDC JWT validation (`crates/fp-core/src/oidc.rs`)

Provider-agnostic: the validator works against any compliant IdP (e.g. Auth0, Keycloak, Okta, Entra) or the dev mock. Configuration (`OidcConfig`, `config.rs:54-59`):

| Env var | Meaning | Default |
|---|---|---|
| `FLOWPLANE_OIDC_ISSUER` | must equal JWT `iss` | required (in prod) |
| `FLOWPLANE_OIDC_AUDIENCE` | expected `aud` | required (in prod) |
| `FLOWPLANE_OIDC_JWKS_URI` | JWKS endpoint | discovered from `{issuer}/.well-known/openid-configuration` when unset |

`FLOWPLANE_OIDC_ISSUER` and `FLOWPLANE_OIDC_AUDIENCE` must be set together (`config.rs:268-288`). There is no project-id or admin-PAT env var.

JWT validation (`OidcValidator::validate`):
1. Decode header; require `kid`.
2. Look up `kid` in the cached JWKS; on miss, refresh (single-flight, cooldown-bounded) then retry (`oidc.rs:170-173`).
3. Build the decoding key from the RSA JWK `(n, e)`.
4. Validation pins **RS256** — `alg=none` and HS256 confusion are rejected at the validation config, not by string checks (`oidc.rs:158-161`). Validates `iss` (`oidc.rs:186`), `aud` (`:187`), requires `exp` and `sub` (`:188`), and **enables `nbf`** (`validation.validate_nbf = true`, `:189`; the jsonwebtoken crate leaves this `false` by default — a regression to avoid), with **30 s** clock-skew leeway (`:185`).
5. Extract **only** `subject` (required), `email?`, `name?` into `ValidatedClaims` (`oidc.rs:35-39`). Role claims, scopes, and everything else are ignored at request time — permissions come from the DB.

**JWKS cache / rotation:** keys cached in `KeyCache` (`oidc.rs:58-61`); an unknown `kid` triggers a refresh guarded by a per-issuer **30 s cooldown** (`oidc.rs:86`) and a single-flight mutex (`oidc.rs:103-110`) so a flood of bogus tokens cannot hammer the IdP. Discovery via `{issuer}/.well-known/openid-configuration` (`oidc.rs:116-132`).

**Userinfo fallback:** ABSENT — `email`/`name` come from the token claims only; there is no secondary userinfo call.

**Revocation latency (accepted risk):** validation is stateless — no `jti` blocklist, no introspection. A revoked session's access token remains valid until `exp`. Grant/permission changes take effect on the **next request**, because the principal (memberships + grants) is loaded fresh from the DB every request (§2.3) — there is no permission cache to stale out.

### 2.3 Request authentication middleware (`crates/fp-api/src/auth.rs`)

Applied to the secured router (all `/api/v1/*` except the public set in §2.5). Flow:

1. **Bearer extraction:** read `Authorization: Bearer <token>`; missing → 401 (`auth.rs:21-27,107-114`).
2. **Principal-kind split** on token prefix (`auth.rs:116-118`):
   - token starts with `fpat_` → **agent** path.
   - otherwise → **user (OIDC)** path.
3. **User path** (`auth.rs:96-195`): `OidcValidator::validate` → `ValidatedClaims`; on failure, a best-effort audit row (`actor=anonymous`, `action="authn.failed"`, `outcome=failure`) then 401 (`auth.rs:129-151`). Then JIT upsert by `subject` (`upsert_user_by_subject`) → `load_principal` (memberships + grants from DB) → active-org resolution (D-014, §2.4) → build `PrincipalCtx::User` (`auth.rs:156-185`).
4. **Agent path** (`auth.rs:197-224`): SHA-256-hash the token and look it up in `agents` by `token_hash` (`load_agent_principal_by_token_hash`, `identity.rs:152-204`); not found or suspended → best-effort audit + 401; else build `PrincipalCtx::Agent` (`auth.rs:206-211`).
5. The `PrincipalCtx` is inserted into request extensions (`auth.rs:185,211`).

There is **no permission cache** — the principal is loaded from the DB on every authenticated request. **Rate limiting** is not in the auth middleware (it is a separate write-path layer, `routes.rs:202-205`). **CSRF**, **cookie/session auth**, and **CORS preflight handling** are ABSENT — authentication is bearer-token only.

### 2.4 Active-org resolution (D-014, `crates/fp-api/src/auth.rs:39-94`)

A user may belong to more than one org, so the active org for a tenant-scoped request is resolved explicitly and fails closed on ambiguity:

- **With selector** (`X-Flowplane-Org` header / CLI `--org`): must resolve to an org the user is a member of, else fail closed (`auth.rs:52-63`).
- **No selector, exactly one non-platform membership:** use it (`auth.rs:89-90`).
- **No selector, zero or ≥2 candidates:** no active org; `org_selector_required = true` is recorded on the principal and tenant-scoped operations fail until the caller names an org (`auth.rs:91-93`).
- The `platform` org is never implicitly selectable as a tenant context (`auth.rs:71`).

`GET /api/v1/auth/whoami` echoes the validated principal, its membership set, the org-selector state, and grant count so a CLI can offer/select an org (`crates/fp-api/src/routes.rs:258-310`).

### 2.5 Dev mode and public endpoints

- **Dev OIDC:** in dev mode the CP runs an in-process issuer (feature `dev-oidc`) and points the same `OidcValidator` at it; the seeded dev user's `subject` is `dev-user` so the normal JIT/authorization path resolves to org-owner of `dev-org` (`crates/fp-storage/src/seed.rs:23-79`).
- **Public (unauthenticated) routes** (`crates/fp-api/src/routes.rs:212-227`): `GET /healthz`, `GET /readyz`, `GET /metrics`, `GET /api-docs/openapi.json`, `GET /api/v1/bootstrap/status`, and `POST /api/v1/bootstrap/initialize` (itself gated by the one-shot `fpboot_*` token, §1.5).

There is **no** BFF/cookie/session surface in v2 — `/auth/login`, `/auth/callback`, `/auth/logout`, `/auth/session`, `/auth/mode`, and `/oauth/register` (DCR) are all ABSENT (`routes.rs`). Browser clients that need an interactive login obtain a JWT from the IdP directly and present it as a bearer token.

### 2.6 MCP client auth (`crates/fp-api/src/mcp_api.rs`)

MCP is served as JSON-RPC 2.0 over `POST /api/v1/mcp`, behind the **same** auth middleware (§2.3) — humans use bearer JWTs, agents use `fpat_*` tokens; there is no MCP-only credential type. `api-consumer` agents are rejected at `initialize` (`mcp_api.rs:126-140`). Additional MCP-layer protection: an **Origin allowlist** (`mcp_api.rs:1797-1826`) — env `FLOWPLANE_MCP_ALLOWED_ORIGINS` (comma-separated), default `http://localhost,http://127.0.0.1,http://[::1]`, matched on scheme+host ignoring port; a present, non-allowed Origin → JSON-RPC error `-32600 "origin is not allowed"`. Sessions are in-memory, keyed by principal, with a `connection_id` UUID and a 3600 s TTL (`mcp_api.rs:24-37`). The origin check is defense-in-depth for browser clients, not the primary boundary (bearer auth is).

---

## 3. Authorization

### 3.1 The single gate: `check_resource_access(ctx, resource, action, team) -> Decision`

(`crates/fp-core/src/authz.rs:138-279`.) Inputs: the request `PrincipalCtx`; a typed `Resource`; a typed `Action` (`Read`/`Create`/`Update`/`Delete`/`Execute`); an optional `TeamRef` (`{id, org_id}` — so the cross-org check is part of the decision, not a separate handler call). It returns a `Decision` — `Allow(Reason)` or `Deny(Reason)` (`authz.rs:120-135`) — where `Reason` is a stable wire string for audit (`authz.rs:81-115`):

- Allow reasons: `platform_governance`, `grant_match`, `org_admin_implicit_team`, `any_team_grant`, `governance_read`, `org_admin_tenant_default`.
- Deny reasons: `agent_structurally_denied`, `cross_org`, `governance_write_requires_platform_admin`, `tenant_resource_invisible_to_platform_admin`, `no_matching_grant`.

Decision logic (order matters):

```
fn check_resource_access(ctx, resource, action, team) -> Decision:

  # 0. Agent structural guard — evaluated FIRST, short-circuits everything below
  if ctx is Agent(kind, org_id, grants):
      match kind:
        CpTool      -> grants-only: cross-org guard (team.org == agent.org), then
                       Allow(grant_match) iff a grant matches (resource, action, team)
                       [team=None: Allow(any_team_grant) iff any-team grant];
                       else Deny(no_matching_grant). No governance, no org-admin arm.
        GatewayTool -> if resource == McpTools: same grants-only path as CpTool;
                       else Deny(agent_structurally_denied).
                       (kind limits the *resource* to McpTools; a grant is still required —
                        the read/execute action limit is enforced at grant creation, not here)
        ApiConsumer -> Deny(agent_structurally_denied)   # never reaches any resource

  # 1. Platform-admin governance bypass (governance resources ONLY)
  if ctx.platform_admin and resource.is_governance():
      return Allow(platform_governance)

  if team is Some(team_ref):
      # 2a. Cross-org guard before grants
      if ctx.org is None or ctx.org.id != team_ref.org_id:
          return Deny(cross_org)
      # 2b. Exact grant row
      if ctx.grants has (resource, action, team_ref.id):
          return Allow(grant_match)
      # 2c. Org-admin implicit team access (tenant resources only)
      if ctx.org.role == Admin/Owner:
          return Allow(org_admin_implicit_team)
      return Deny(no_matching_grant)

  else:  # team is None
      # 3a. ANY-team grant — lets team-scoped users hit list endpoints,
      #     which then filter rows to their teams
      if ctx.grants has any (resource, action, *):
          return Allow(any_team_grant)
      # 3b. Governance resources: reads via any org membership; writes need platform admin
      if resource.is_governance():
          return action == Read and ctx.org.is_some() ? Allow(governance_read)
                                                       : Deny(governance_write_requires_platform_admin)
      # 3c. Tenant resource without team: org admin/owner only
      if ctx.org.role == Admin/Owner:
          return Allow(org_admin_tenant_default)
      return Deny(no_matching_grant / tenant_resource_invisible_to_platform_admin)
```

(`authz.rs:145-277`.) The function is pure — no IO, no clock, no globals — so it is exhaustively property-testable and a decision is predictable from the principal's grant set.

### 3.2 Resource / Action vocabulary

`Resource` and `Action` are typed enums with one source of truth (`crates/fp-domain/src/authz.rs:25-50,102,130-136`). `Resource::is_governance()` (`fp-domain/src/authz.rs:57-62`) classifies the closed governance set; everything else is tenant.

- **Governance resources:** `Organizations`, `Users`, `Teams`, `Audit`, `Platform`.
- **Tenant resources:** `Clusters`, `RouteConfigs`, `Listeners`, `Filters`, `Secrets`, `Dataplanes`, `ProxyCertificates`, `Agents`, `Grants`, `ApiDefinitions`, `LearningSessions`, `McpTools`, `RateLimits`, `AiProviders`, `AiRoutes`, `AiBudgets`, `AiUsage`, `Stats`.
- **Actions:** `Read`, `Create`, `Update`, `Delete`, `Execute`.

A grant is one `(principal, resource, action, team)` row (`0002_identity.sql:75-91`); there is no scope-string grammar, no `resource_type`/`route_id`/`allowed_methods`/`expires_at` on the grant, and no separate gateway-tool/route grant type — gateway-tool reach is enforced structurally by agent `kind` (§3.1, §3.3), not by a grant subtype.

### 3.3 Three hard invariants

1. **Platform admin is governance, not a resource bypass.** A platform admin (`platform_admin == true`) can read/write governance resources only. A pure platform-admin context is *denied* tenant resources outright — it cannot read another tenant's clusters by virtue of being admin (`authz.rs` step 1 gates on `resource.is_governance()`; the `team`-present path returns `cross_org`/`no_matching_grant`). Cross-tenant reads go through an explicit `TeamScope::PlatformAdmin { reason }` at the storage layer (spec/08a), not through the authorization gate.
2. **Cross-org access requires an in-org grant.** The active org (`ctx.org`) is loaded from the DB, never from the URL. With a target team, the gate denies `cross_org` before consulting grants when `ctx.org.id != team.org_id`. The API renders an org-boundary denial as **404** (anti-enumeration), not 403 — see spec/08a.
3. **Team isolation is enforced in the typed `team` argument and in DB queries.** Every tenant decision carries a `TeamRef` whose `org_id` is checked against the principal's org; storage reads scope by team via `TeamScope` (spec/03 §… repository layer). There is no API path returning a tenant resource without a team check.

### 3.4 Role summary

| Role / context | Can |
|---|---|
| Platform admin (`owner` of `platform` org) | All governance reads/writes (orgs, users, teams, audit, platform). **Cannot** touch any tenant resource (clusters/routes/secrets/…), which are invisible to a pure platform-admin context. |
| Org admin/owner (`org_memberships.role` ∈ {`owner`,`admin`} in the active org) | Implicit access to every team in their own org (tenant resources) and `org_admin_tenant_default` for team-less tenant ops; governance **reads**. Nothing in other orgs; no platform governance writes. |
| Org member/viewer | Governance **reads** (`governance_read`) and any tenant access explicitly granted by a `grants` row. (member vs viewer is not differentiated at the gate; intended read-only-ness for viewers is by convention.) |
| Team-scoped user (grants only) | Exactly the `(resource, action, team)` triples in their grant rows. |
| `cp-tool` agent | Grant-based like a human, but with no governance bypass and no org-admin arm; same-org check applies. |
| `gateway-tool` agent | `McpTools` resource only, and only with a matching grant; structurally denied all other resources. |
| `api-consumer` agent | Data-plane only via Envoy RBAC; structurally denied all control-plane resources. |

### 3.5 Where the gate is invoked, per surface

- **Tenant resource services** call `check_resource_access(ctx, resource, action, team)` with the concrete resource/action and, where a team is in scope, a resolved `TeamRef`; a `Deny` maps to HTTP 403 (or 404 for an org-boundary denial, §3.3). (`crates/fp-core/src/services/*.rs`.)
- **Governance / org-admin flows** (team, org-member, grant, agent management) do **not** go through the resource gate; they use org-role helpers — e.g. `authorize_member_admin` — that check the caller's `org_memberships` role directly. (`crates/fp-core/src/services/teams.rs:18`, `services/agents.rs:26`, `services/orgs.rs:130`.)
- **MCP** `tools/call` resolves each tool to its `(resource, action)` and calls the same gate; `api-consumer` agents are excluded at `initialize` (`crates/fp-api/src/mcp_api.rs:126-140`).

---

## 4. Token / credential lifecycle

| Credential | Issued by | Lifetime | Refresh | Revocation |
|---|---|---|---|---|
| Access JWT (human) | the configured OIDC IdP (or dev in-process issuer) | provider-set | provider's refresh flow (client-side) | stateless CP keeps accepting until `exp` (accepted risk); permission changes apply next request (DB-loaded principal) |
| Agent token (`fpat_*`) | Flowplane (`crates/fp-core/src/services/agents.rs:82-87`) | until rotated/suspended | `rotate_agent_token` (new token, replaces hash) | suspend agent (`status='suspended'`) or rotate; only the SHA-256 hash is stored |
| Bootstrap token (`fpboot_*`) | operator-supplied (or legacy generate-and-log) | 24 h, single-use | n/a | expiry, single-use `used_at`, or operator re-seed (§1.5) |
| Grants (authz rows) | org admin | n/a (no `expires_at` column) | n/a | delete the row; effective next request |

There are **no** OIDC `client_credentials` machine tokens, no Dynamic Client Registration, no IdP admin PAT, and no `password_hash` (the column does not exist). Every human credential is an OIDC token; every machine credential is a Flowplane-issued `fpat_*` token.

---

## 5. Audit (`crates/fp-storage/src/repos/audit.rs`, table `audit_log` `0002_identity.sql:95-112`)

Audit covers mutations, authorization denials, and authentication failures.

- **Entry shape** (`audit.rs:72-87`): `request_id?`, `actor_type` (`user`/`agent`/`dataplane`/`system`/`anonymous`), `actor_id?`, `actor_label`, `surface` (`rest`/`mcp`/`cli`/`xds`/`system`), `action` (verb-style, e.g. `team.create`, `authz.denied`, `authn.failed`), `resource`, `org_id?`, `team_id?`, `outcome` (`success`/`denied`/`failure`), `detail` (JSONB structured context — must not contain secrets/tokens/payloads). The table has **no FKs**, so rows survive subject deletion.
- **Write paths:** transactional (`record_in_tx` — the audit row commits with the mutation or rolls back with it) and best-effort (`record_best_effort` — never fails the caller; errors are logged and counted) (`audit.rs:156-176`).
- **Authorization denials** from the resource gate are recorded via `fp_core::services::record_authz_denial` (`crates/fp-core/src/services/mod.rs:56,80`), which builds a best-effort `audit_log` row carrying the gate's `Reason` (`AuditEntry::denial`, `audit.rs:103-127`). Note two current limitations: the shared helper hardcodes `surface = Rest` even for MCP callers, and denials raised by the org-role helpers (§3.5) are not consistently audited. **Authn failures** are recorded best-effort from the middleware (`auth.rs:132-151,226-244`).

Severity is derived at read time from the action verb. (Detailed query surfaces live with the REST API reference.)

---

## 6. Dataplane identity (cross-ref spec/04)

Operator identity (JWT / agent token) and dataplane identity (mTLS client cert) are disjoint; neither credential works on the other surface. xDS mTLS is unconditional — there is no plaintext gRPC path in prod (`crates/fp-xds/src/ads.rs`; TLS material via `FLOWPLANE_XDS_TLS_CERT`/`_KEY`/`_CLIENT_CA`, `config.rs:202-225`).

Authz-relevant part (`crates/fp-xds/src/ads.rs:41-150`):
- The Envoy client cert's URI SAN (SPIFFE) is the identity; it is parsed from the **verified certificate**, never from node metadata.
- Two resolvers: a **NodeIdTeamResolver** for dev/test (trusts `team=<uuid>` in `node.id`, `ads.rs:65-87`) and a **CertRegistryResolver** for production that looks the SPIFFE URI up in the certificate registry (`fp_storage::repos::dataplanes::find_active_certificate`, `ads.rs:93-135`).
- A match yields a `PeerIdentity { team_id, dataplane_id?, certificate_id? }`; that team scopes every xDS read — the connection only ever receives that team's resources. A missing/revoked/expired certificate returns one uniform error (`ads.rs:125-132`); revoking a certificate ends the live stream via a broadcast watch (`ads.rs:138-157`).

The exact SPIFFE URI template, trust-domain handling, and CA issuance-policy invariant are specified in spec/04.

---

## 7. Notes and accepted risks

1. **Stateless revocation window.** A revoked/rotated OIDC session's access token stays valid until `exp` — no `jti` blocklist or introspection. Permission (grant/membership) changes apply on the next request because the principal is DB-loaded each request. Accepted; revisit if a hard logout is required.
2. **Origin allowlist is defense-in-depth.** The MCP Origin check ignores port and is not the security boundary — bearer auth is. Documented so it is not mistaken for one.
3. **member vs viewer is not gate-differentiated.** Both reach governance reads; read-only-ness for viewers holding grants is by convention, not enforcement.
4. **JWKS cold-start.** A failed first JWKS fetch consumes the 30 s cooldown; logins can 401 for that window on a slow IdP at boot. The single-flight + cooldown bound deliberately trade a short brownout for not hammering the IdP.
