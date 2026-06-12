# 08 — Architecture Critique of v1

An honest assessment of what made v1 hard to change, where its layering breaks down, and —
most importantly — a trace of the route → observe → learn → spec → tools loop showing every
manual step, glue handoff, duplicated representation, and drift point. This file drives the v2
design (spec/10). Per-subsystem detail lives in specs 01–07; this synthesizes.

## 1. What v1 got right (keep the behavior, keep the ideas)

- **Deterministic projection contract:** every Envoy snapshot is a pure function of DB state;
  no out-of-band config. This is the single most valuable invariant in the system.
- **One authorization gate** (`check_resource_access`) genuinely shared by REST and MCP, with
  well-chosen invariants (admin:all ≠ tenant bypass; 404-hides cross-tenant; structural agent
  partitions). The *decision logic* is sound; the enforcement *placement* is not (§3.1).
- **mTLS-mandatory xDS with DB-bound SPIFFE identity** — cert tells you who, DB row tells you
  what team; never trusting node metadata.
- **Single-binary operational simplicity**: CP + CLI in one artifact, agent as sidecar; sane
  for the deployment story.
- **Hard-won security fixes** encoded as middleware (CORS credential gating, tenant write
  throttle, body limits, cookie-secure boot check) — the *findings* transfer to v2 even where
  the mechanisms will differ.
- The **drift-probe test pattern** (`b1_admin_routes` asserts list ↔ router parity) — right
  idea, applied to 6 of 187 operations. v2 generalizes it.

## 2. The central finding: subsystems communicate through storage and conventions

v1's docs claim "three surfaces, one service layer." The write path mostly honors it. The
*propagation* path does not — there are no domain events anywhere:

- **DB-polling as the integration bus.** The xDS layer discovers config changes by polling
  `COUNT(*) + MAX(updated_at)` per resource type every 500 ms. Services write rows; the poller
  notices; the snapshot rebuilds; streams get pushed. Consequences: missed-delete edge cases,
  propagation latency, 4 polls/replica/500 ms of dead DB load, and no ordering across types
  (a route can reach Envoy before the cluster it references; NACKs are the safety net).
- **Name-string conventions as foreign keys.** Learning sessions correlate to Envoy config via
  `name.contains(session_id)` on injected filters; ALS↔ExtProc entries correlate via
  `x-request-id` with a 15 s TTL; learned schemas attach to routes by **byte-exact string
  equality** of paths; NACKs attribute to resources by regex-scanning Envoy error text.
- **Generated artifacts detached from their source of truth:** OpenAPI doc covers 85 of 187
  operations with 4 wrong methods (spec/01 §4); the "auto-generated" CLI reference misses real
  flags; the MCP authz registry carries 17 phantom entries that make phantom grants valid.
  Same disease: a hand-fed generator beside the real definition.

**v2 consequence:** a shared domain model with explicit lifecycle states and transactional
domain events (outbox) is not a style preference — it is the fix for the entire class of bugs
above.

## 3. Layering violations and duplication

### 3.1 Tenancy enforced at the wrong layer

Repositories expose unscoped `get_by_id`/`get_by_name`/`list`/`delete_by_name`; isolation
depends on every handler remembering `verify_team_access()`. The rate-limit subsystem (newest
code) shows the correct pattern — scoped predicates in SQL plus a team∈org trigger — proving
v1's own authors converged on it late. (Full inventory: spec/03 §6.2; requirements: 08a §2.2.)

### 3.2 Validation in three-and-a-half places

REST handlers and MCP tools call `validation/business_rules/*`; `internal_api` validates via a
*different* rule set (`ClusterSpec::validate_model`); `src/validation/requests/` is dead
scaffolding referencing functions that don't exist, with limits contradicting the live rules;
the UI re-encodes the name rules as constants (pinned by parity tests — the one bright spot).
Same payload can pass one path and fail another depending on the surface.

### 3.3 Dual representations without transactions

`route_configs.configuration` JSON **and** normalized `virtual_hosts`/`routes` projection
tables; `clusters.configuration` JSON **and** `cluster_endpoints`; filter attachments **and**
`listener_auto_filters` **and** the listener's own config JSON — each synchronized by
sequential statements, crash-windows leave them disagreeing, and different readers trust
different copies (the MCP `routes` resource reader still queries pre-rename legacy columns —
likely a runtime SQL error today, spec/02 §7.14).

### 3.4 Duplication catalog

Two singularization implementations; two sensitive-header knowledge bases; three learning-DTO
mappings (REST, internal_api, MCP); two MCP dispatchers (one vestigial); two PUT/PATCH
conventions; three addressing schemes (name, UUID, numeric id) plus dual name-or-UUID team
params; `repository_old.rs` shipped dead. Each pair has already diverged at least once.

### 3.5 Concurrency as an afterthought

No optimistic locking on gateway resources (last-writer-wins; rate-limit alone does
`WHERE version=$n` → 409); check-then-insert races on cluster-name case-insensitivity and
`envoy_admin_port = MAX+1` (rescued by unique indexes, surfaced as raw 409s); xDS dirty-check
races with injection nondeterminism (HashMap ordering churns versions and JWT per-route
provider selection).

## 4. The loop, traced: route → observe → learn → spec → tools

The pipeline works end-to-end in v1 — as five subsystems holding hands. Every numbered seam
below is a manual step (M), glue convention (G), duplicated representation (D), or drift risk
(R):

1. **Route exists → observation.** Operator creates a learning session (M) with a
   path regex they must write themselves to match the route they already created (G — nothing
   links session→route; `cluster_name` filter is stored but never enforced). Activation
   injects ALS+ExtProc into **every listener on every team** (R — blast radius;
   first-match-wins attribution with zero team affinity = the 08a §4.1 cross-tenant capture).
2. **Observation → learned schema.** Automatic and well-built (worker pool, batching,
   inference, aggregation, confidence) — the strongest stretch of the loop. But sample counts
   are written per-request on the hot path (R: throughput ceiling) and intermediates persist
   raw payload fragments forever (R: hygiene).
3. **Learned schema → spec.** Aggregation is automatic; **export to OpenAPI is manual** (M)
   and lossy — exports lack `servers`, so the export cannot be re-imported (G/R: the loop's
   "closed" claim breaks here; identity is re-keyed at every hop:
   session_id → schema id → export → import id).
4. **Spec → tools.** MCP enablement is **per-route and manual** (M: `mcp/enable`, then
   optionally `apply-learned`), requires confidence ≥ 0.8 AND byte-exact path equality between
   the route's `path_pattern` and the learned template (G/R — `{id}` vs `{customerId}` silently
   yields nothing). New aggregations never refresh existing tools (R: stale tools forever);
   `mcp/refresh` is manual per route (M).
5. **Tools → serving.** Gateway tool execution re-resolves the route and proxies via
   `gateway_host:port` (G: host header convention). Deleting upstream resources does not
   cascade to tools or schemas; `learned_schema_id` is SET NULL but schemas are never deleted
   (R: orphans by design).
6. **The reverse direction barely exists.** Traffic with no matching route *is* visible to the
   capture path (catch-all to a black-hole cluster), but learns only useless responses; there
   is **no path from a learned spec to generated routes/clusters** — only export→import, which
   §3 above shows isn't round-trippable. (Full gap analysis + v2 design: spec/06 §9.)

**Net:** five identity systems, four manual gates in the forward direction, no freshness
propagation, and the two ends of the "loop" connected by a file download. The v2 integration
design (one entity carrying lifecycle state observed → learned → reviewed → published →
served, with events driving tool and xDS updates) eliminates seams 1, 3, 4, 5 structurally
and makes 6 a first-class feature.

## 5. API and error-contract critique

- Error bodies are `{error, message}` with coarse kinds — no stable per-failure code, no
  remediation, no request id; agents and the CLI both resort to string-matching.
- OpenAPI doc is <50 % true (spec/01 §4) — disqualifying for the "operate from the contract
  alone" goal.
- Pagination wrapper exists but adoption is partial; PUT/PATCH split is arbitrary; idempotency
  is absent everywhere; `expose` creates three resources with undocumented partial-failure
  semantics.

## 6. CLI critique (summary of spec/07 §3/§6)

36 top-level commands (vs ~8-noun ideal); `-o` means format except where it means file; ~20
commands have no structured output (unscriptable mutations); three contradictory precedence
rules across token/team/base-url; raw HTTP error dumps with exit-code monoculture; no
completions; `route` CRUDs route-configs while actual routes are UI-only; `apply` is
half-declarative (6 kinds, no dry-run/diff/prune). The v2 CLI is a redesign, not a port —
design doc is spec/12.

## 7. Operational posture gaps

- **Multi-replica CP is accidental**: independent in-memory snapshot caches + version counters
  work only because ACK matching is nonce-based; broadcast(128) lag silently starves slow
  streams; in-memory MCP sessions/connections don't survive restart or load-balance.
- Failure semantics are inconsistent (LDS fails closed on DB error; CDS/RDS fall back to
  static config — could serve test resources to production dataplanes).
- No retention policy on audit, inferred schemas, or aggregated versions; docs recommend
  `flowplane down --volumes` as cleanup (!).
- EDS vestigial (endpoint churn = cluster reconnect); delta-xDS is fake (full sets always).
- Observability: OTel wiring exists, but the alerts-that-matter, capacity guidance, and
  backup/restore are undocumented — the production-readiness review (v2 deliverable) starts
  from zero here.

## 8. Why this happened (pattern diagnosis, for v2 discipline)

Reading 100 migrations and the bead/finding-tagged comments chronologically: v1 grew
feature-first with security and consistency retrofitted in waves (the `fp-*` finding tags are
literally an audit trail of retrofits). Each retrofit was local — rate-limit got SQL scoping,
optimistic locking, and org triggers; nothing older was migrated. The lesson for v2 is not
"v1's authors were wrong" — it's that **cross-cutting invariants (tenancy, events, identity,
error contract) must be load-bearing infrastructure from slice 1**, because retrofitting them
produces exactly this two-generation codebase.

## 9. Change-difficulty index (what to never rebuild)

| v1 element | Verdict for v2 |
|---|---|
| DB-polling xDS integration | Replace: transactional outbox → event bus → snapshot rebuild |
| Handler-level tenancy | Replace: SQL-scoped repositories, typed TeamId (08a §2.2.1) |
| JSON config + projection-table dual writes | Replace: one normalized model, generated JSON views |
| Name-convention correlation (sessions, NACKs, schemas↔routes) | Replace: real FKs on one shared entity |
| Manual spec→tool hops with re-keying | Replace: lifecycle state machine + events |
| Three validation layers | Replace: one domain-validation path, all surfaces converge |
| Hand-fed OpenAPI/CLI-doc/authz registries | Replace: generated from the single definition, CI-gated parity |
| `check_resource_access` decision logic | Keep semantics; re-house enforcement |
| Deterministic DB→xDS projection | Keep, strengthen with ordering guarantees |
| SPIFFE cert binding, SDS-only secrets | Keep |
| Worker-pool inference design | Keep shape; fix contention, batching, hygiene |
| Single-gate auth invariants (admin≠tenant, 404-hiding) | Keep, property-test |
