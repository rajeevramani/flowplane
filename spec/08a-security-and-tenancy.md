# 08a — Security & Tenancy: Threat Model and Isolation Spec

Synthesized from specs 01–07 and 09 plus direct v1 source reading. This file is both a faithful
record of v1's security posture (what is enforced, where, and where it is missing) and the
binding requirements input for v2's security design. Sections marked **[V2 REQUIREMENT]** are
normative for the rewrite.

## 1. Tenant boundary definition

- **Tenant = team.** Every gateway resource (cluster, route config, listener, filter, secret,
  dataplane, learning session, schema, MCP tool, rate-limit domain/policy, proxy certificate)
  is owned by exactly one team. Orgs group teams; org isolation is transitive via
  `teams.org_id` except for grants/rate-limit/audit rows which carry `org_id` directly.
- **Shared-resource escape hatch:** `team IS NULL` on clusters/route_configs/listeners means
  "visible to every team" (`list_by_teams … OR team IS NULL`). Implicit and undocumented.
  **[V2 REQUIREMENT]** sharing must be explicit (e.g. a `visibility` attribute owned by the
  platform), never an absent FK.
- **Data plane:** each dataplane (Envoy) belongs to one team and must only ever receive that
  team's resources. The xDS stream's team is derived from the client cert's SPIFFE URI looked
  up in the `proxy_certificates` registry (DB row is authoritative, never the SAN string or
  node metadata). Node id format `team={team}/dp-{uuid}` is informational only.
- **Identity tiers:** platform admin (`admin:all`, governance-only), org admin/member/viewer
  (scope strings), team-scoped principals (`grants` rows: principal × resource × action ×
  team), machine agents with structural partitions (cp-tool / gateway-tool / api-consumer).

## 2. Where isolation is enforced in v1 — and where it is missing

### 2.1 Enforcement inventory (v1 facts)

| Layer | Mechanism | Coverage |
|---|---|---|
| REST | `authenticate` middleware → `ensure_dynamic_scopes` (path-derived, team=None) → per-handler `check_resource_access(…, team)` → row-level `verify_team_access` (404 on cross-team) | All `/teams/{team}` routes; admin routes gate on `admin:all` |
| MCP | per-tool-call `(resource, action)` from registry → same `check_resource_access`; gateway tools re-check exposure + grant at execution | All `tools/call`; **not** `tools/list` content, prompts, ping |
| SQL | scoped predicates **only** for rate-limit tables (`WHERE … AND org=$ AND team=$` + team∈org trigger) and `get_by_id_scoped`; everything else relies on handler checks over unscoped `get_by_id`/`get_by_name`/`list` | Partial — the two-layer model is the core structural weakness |
| xDS | mandatory mTLS; SPIFFE URI → `proxy_certificates` row → team; all LDS/RDS/CDS/SDS responses filtered to that team (+ NULL-team shared rows) | Stream establishment only; no mid-stream revocation |
| Learning capture | session rows carry team; ALS attributes entries to the matched session's team | **Broken at capture time** — see §4.1 |
| Audit | governance actions, provisioning, secrets access | No authn failures, no authz denials, no login events |

### 2.2 Missing or weak (v1 facts, each → v2 requirement)

1. **Handler-enforced tenancy** (spec/03 §6.2): one forgotten `verify_team_access` call is a
   cross-tenant read/write. **[V2 REQUIREMENT]** team predicate lives in SQL for every query on
   team-owned tables (rate-limit style), with the repository API making it impossible to omit
   (scoped methods take a typed `TeamId`; no unscoped variants outside an `admin` module).
2. **Global resource-name uniqueness** (clusters/route-configs/listeners): tenant A can probe
   or squat tenant B's names; 409 on create is a cross-tenant existence oracle.
   **[V2 REQUIREMENT]** per-team name uniqueness; any global namespace (listener ports) is
   platform-allocated.
3. **Learning capture has no team affinity** — the highest-severity tenancy finding; §4.1.
4. **MCP session binding not re-checked** on POST; session ids function as a weak secondary
   credential. `tools/list` leaks internal-exposure tool names. Origin validation exists but is
   unwired. **[V2 REQUIREMENT]** session bound to (token, team) and re-validated per request;
   list == executable set; origin allowlist enforced.
5. **`ops_*` authorization piggybacks on `clusters:read`** and 17 phantom registry entries make
   phantom grants valid. **[V2 REQUIREMENT]** every tool/endpoint declares a real (resource,
   action) that exists; registry is the single source and is test-pinned.
6. **No audit of authn/authz failures**; revocation window ≈ JWT lifetime (~1 h) with no
   denylist. **[V2 REQUIREMENT]** auth failures and denials are audited (rate-limited, no
   secrets in entries); revocation propagates ≤ 60 s (short tokens + permission-cache
   invalidation + session kill).
7. **xDS side doors:** ALS/ExtProc/diagnostics services accept any cert-holding dataplane
   without checking the cert's team against the data it submits (warming reports, access logs);
   allowlist scope is opt-in via unauthenticated node metadata; CDS/RDS fall back to
   config-based static resources on DB error (LDS fails closed) — inconsistent and can serve
   non-tenant config. **[V2 REQUIREMENT]** every gRPC service authorizes per-message against
   the stream's cert-derived team; uniform fail-closed semantics.
8. **Mid-stream cert revocation gap** — revoking a proxy certificate does not terminate live
   xDS streams. **[V2 REQUIREMENT]** revocation check on push (cheap: in-memory revoked set)
   and periodic re-validation.
9. **Org-boundary trigger only on rate-limit tables** — a `grants` row can pair a team with the
   wrong org if app code errs. **[V2 REQUIREMENT]** team∈org enforced by schema for every
   (org_id, team_id) pair.
10. **Dev-mode artifacts**: fixed IDs, static bootstrap bearer token, plaintext initial
    credentials in `.env.zitadel`, permissive MCP origin defaults. **[V2 REQUIREMENT]** dev
    mode is a build/runtime flag that cannot be enabled by accident in a release artifact;
    bootstrap tokens are one-shot and expire.

## 3. AuthN/AuthZ matrix per surface

| Surface | AuthN (v1) | AuthZ (v1) | v1 gaps |
|---|---|---|---|
| REST `/api/v1/**` | Zitadel JWT (RS256, JWKS cached); BFF cookie (AEAD-sealed, CSRF double-submit) for the UI | `check_resource_access` decision tree (spec/05 §3.1): agent guard → admin:all governance-only → grant match → org-admin implicit → default deny; 404-hides cross-org/team | hand-maintained middleware skip-list; semantic-read path rules fail-open for new endpoints; `viewer` accepted at runtime, rejected by validator |
| MCP `POST /api/v1/mcp` | same bearer token | per-call (resource, action) + exposure & grant re-check for `api_*` | §2.2(4,5); Forbidden → -32600 |
| CLI | OIDC PKCE / device-code; creds 0600 in `~/.flowplane/credentials` | = REST (CLI is a REST client) | token precedence env>file>flag (Q-001); bootstrap token bypass path |
| xDS gRPC | mandatory mTLS, SPIFFE SAN → cert registry row | team-filtered resource responses | §2.2(7,8); plaintext ALS/ExtProc payloads if dataplane TLS off |
| Swagger/docs | none (accepted risk H9) | n/a | static doc only — keep in v2 only if still true |
| SDS (secrets to Envoy) | rides the mTLS ADS stream | team-filtered like other resources | secret values transit only to the owning team's dataplanes — preserve |

**[V2 REQUIREMENT]** one authorization path for REST + MCP + CLI (CLI via REST), decision
logic table-driven and property-tested; per-surface bypass lists eliminated (public endpoints
are an explicit allowlist in one place).

## 4. Abuse cases

### 4.1 Malicious tenant (tenant A attacks tenant B)

- **Capture poaching (v1-exploitable):** A starts a learning session with pattern `^/.*`.
  v1 injects ALS+ExtProc into **every listener** and matches sessions by path regex,
  first-match-wins, no team check — A receives B's headers and request/response bodies (10 KB
  each) attributed to A's session, exportable as A's "learned spec".
  **[V2 REQUIREMENT]** capture is scoped at injection time to the session team's
  listeners/routes; the capture services verify (session.team == resource.team) per entry;
  cross-team match is dropped + alerted.
- **Name/port oracles:** global name uniqueness (§2.2.2); `occupied_ports()` reveals port
  occupancy across teams; listener `(address,port)` 409s leak existence. **[V2 REQUIREMENT]**
  errors never distinguish "exists in another tenant" from "invalid".
- **Resource exhaustion:** unbounded ALS→worker channel; per-entry sample-count UPDATE on the
  hot path; broadcast(128) lag drops xDS pushes; one team's NACK-ing listener blocks the whole
  team's LDS but a flood of config churn degrades the shared CP. v1 mitigates writes with the
  per-org write throttle (B11). **[V2 REQUIREMENT]** per-tenant quotas on resources, learning
  sessions, capture volume, schema cardinality; backpressure that degrades the offender first.
- **Grant escalation probes:** phantom grant pairs (§2.2.5); `agents:create` checked on first
  team only while joining all listed teams (spec/05 gap). **[V2 REQUIREMENT]** every
  team-affecting argument is individually authorized.

### 4.2 Compromised operator token

- v1 blast radius: whatever the token's grants/org role allow, for the remaining JWT lifetime
  (~1 h); no auth-failure audit to notice probing; MCP sessions opened with the token survive
  conceptually (in-memory, 1 h TTL). Secrets: values are write-only via API (reads redacted),
  but a token with `secrets:update` can rotate to attacker material; dataplane bootstrap +
  cert generation allows joining a rogue Envoy that receives the team's config and secrets
  via SDS.
- **[V2 REQUIREMENT]** short access tokens + refresh; revocation ≤ 60 s (§2.2.6); cert
  issuance audited with rate limit (exists in v1 — keep); secret rotation and proxy-cert
  generation flagged as high-risk audit events; per-token anomaly counters (denied-call rate)
  surfaced in ops.

### 4.3 Hostile observed traffic (the learning loop as attack surface)

v1 facts (spec/06 §10): header names embed verbatim into specs (no allowlist/frequency bar);
one request lacking a field demotes `required`; >10 distinct values suppress enum detection;
alphabetic path segments are never collapsed → path-cardinality explosion; raw enum candidates
(≤100 chars of real payload data) persist indefinitely in `inferred_schemas`; truncated bodies
silently yield no schema; anyone who can reach the listener contributes with equal weight.

**[V2 REQUIREMENTS]**
1. Observed traffic is **data, never config**: nothing learned becomes a route, cluster,
   filter, tool, or tool-schema change without an explicit operator approval step; CLI
   `--dry-run` previews exactly what would be created (founder-stated invariant).
2. Spec-poisoning resistance: min-frequency thresholds for headers/fields/paths before they
   enter an exported spec; per-session caps on distinct paths with alerts; outlier/quarantine
   bucket rather than equal-weight merging; provenance retained per learned element (sample
   counts, first/last seen) so a reviewer can see *why* a field exists.
3. Captured-data hygiene: capture buffers and inference intermediates are team-scoped rows
   with TTL; enum/example values redacted by default in exports; sensitive-header list
   extensible per team.
4. Traffic-first generation (new in v2) inherits all of the above **plus**: generated
   routes/clusters are proposals in `reviewed` state, validated against the team's quota and
   the platform's port/name policies, and can never reference another team's clusters or
   listeners.

### 4.4 Rogue/compromised dataplane

- v1: a valid team cert yields that team's full config + SDS secrets (by design), and
  unauthenticated-metadata allowlist opt-in, ALS/ExtProc submission without per-message team
  checks, diagnostics reports accepted from any cert holder.
- **[V2 REQUIREMENT]** per-message team binding on all dataplane-facing services; secrets
  delivered only to dataplanes whose cert team owns them (v1 does this — preserve); revocation
  kills streams (§2.2.8); warming/diagnostics reports validated against the reporter's team.

## 5. Secrets handling (v1 baseline → v2 posture)

- v1: secrets encrypted at rest (AES-GCM via `secret_encryption.rs`, KEK from env), pluggable
  backends (env, Vault) with caching + audited access, delivered to Envoy exclusively via SDS
  over the mTLS ADS stream; API reads return metadata only; rotation endpoint exists; OAuth
  client secrets surfaced once at creation (one-shot) — but the Zitadel admin PAT lands in
  `.env.zitadel` plaintext and initial user passwords are printed.
- **[V2 REQUIREMENT]** carry forward: encrypt-at-rest with KEK rotation procedure, SDS-only
  delivery, redacted reads, one-shot reveals, audited access. Fix: no plaintext credential
  files in any setup path; document KEK rotation in the runbook. **AI-provider credentials**
  (new in v2) are secrets of a distinct type flowing through this same subsystem — never
  inline in provider/cluster config, redacted everywhere, rotatable without route downtime,
  and never delivered to dataplanes except as the filter/SDS material that the AI-gateway data
  path needs.

## 6. Audit requirements

v1 audits governance verbs, provisioning, secrets access, and xDS NACK/warming events;
misses authn failures, authz denials, logins, and has no retention policy (unbounded
`audit_log`, no FKs by design).

**[V2 REQUIREMENT]** every mutating operation on every surface writes one audit row
(actor, surface, resource ref, team/org, outcome, request id); denials and auth failures
audited with rate-limiting; learning lifecycle transitions (created → capturing → learned →
reviewed → published) and AI-gateway budget/failover events are audit events; retention is
configurable with a documented default (founder question if pricing-relevant); admin feed
stays redacted (v1's `/admin/audit/feed` pattern is good).

## 7. Residual v1 findings index (traceability)

Full per-subsystem lists live in: spec/02 §7 (19 items), spec/03 §8 (20), spec/04 §8 (23),
spec/05 §7 (15), spec/06 §10–11 (15+). The v2 architecture (spec/10) must map each
security-relevant item to a design element or an explicit accepted risk; the hardening slice's
adversarial pass tests every §4 abuse case end-to-end across REST, MCP, CLI, learned specs,
generated routes, and xDS.
