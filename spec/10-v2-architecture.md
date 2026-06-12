# 10 — Flowplane v2 Target Architecture

The design authority for all implementation slices. It must be read with: spec/08 (the v1
failures it fixes), spec/08a (binding security requirements), spec/09 (adopted/rejected prior
art), and D-001..D-005. Constraint D-004 applies globally: **no orchestrator dependency anywhere**;
every mechanism below runs identically on bare metal, VMs, plain containers, ECS-class managed
platforms, or K8s.

## 1. System shape

Unchanged from v1 at the process level (it was right): one control-plane binary (`flowplane`)
serving REST+MCP (:8080) and xDS-family gRPC (:18000) out-of-band of traffic; PostgreSQL as the
single source of truth; Envoy as the only data plane; `flowplane-agent` sidecar for dataplane
diagnostics; optional standalone RLS for global/token rate limiting. The CLI is the same binary
(`flowplane` client subcommands talk REST).

What changes is the **interior**: one shared domain model with explicit lifecycle, transactional
domain events instead of DB polling, tenancy in the type system and the SQL, and contracts
(OpenAPI, MCP registry, CLI reference) generated from the same definitions that serve traffic.

## 2. Workspace and dependency rules

Cargo workspace; layering enforced by crate boundaries (a violation is a compile error, not a
review comment):

```
crates/
  fp-domain      # entities, newtype IDs, lifecycle state machines, validation,
                 # event TYPES, quota policy types. Depends on: nothing internal.
  fp-storage     # SQLx repos (scoped-by-construction), migrations, outbox writer,
                 # event reader. Depends on: fp-domain.
  fp-core        # use-cases/services: the ONLY mutation path; transaction +
                 # outbox orchestration; authorization decision engine.
                 # Depends on: fp-domain, fp-storage.
  fp-xds         # domain → IR → Envoy protos; ADS server; SDS; capture ingest
                 # (ALS/ExtProc); diagnostics. Depends on: fp-domain, fp-core (read).
  fp-learning    # inference, aggregation, spec generation, route materialization
                 # planning. Depends on: fp-domain, fp-core.
  fp-ai          # AI gateway: provider model, request/response translation contract,
                 # token metering, budgets, failover policy. Depends on: fp-domain, fp-core.
  fp-api         # axum REST + OpenAPI generation from the same route definitions.
                 # Depends on: fp-core (+ feature views of others via fp-core only).
  fp-mcp         # MCP server; tool registry generated from definitions shared with
                 # fp-api. Depends on: fp-core.
  fp-cli         # human surface; REST client only (no direct DB/core linkage except
                 # `flowplane db migrate`). Depends on: fp-domain (types only).
  flowplane      # bin: wires server (api+mcp+xds+workers) and CLI dispatch.
  fp-agent       # bin: dataplane sidecar (as v1, re-specified in spec/04 §6).
```

Rules: surfaces (`fp-api`, `fp-mcp`, `fp-cli`) never touch `fp-storage` directly; `fp-xds`,
`fp-learning`, `fp-ai` never call each other — they communicate only through domain events and
`fp-core` services (kills 08 §2). `fp-domain` has no async, no IO, no SQLx.

## 3. The shared domain model (fixes 08 §4)

### 3.1 Central entity: `ApiDefinition`

The loop's five views — routes serving traffic, observed traffic, learned schema, published
spec, MCP tools — become **one aggregate** with linked projections, not five records joined by
string conventions:

- **`ApiDefinition`** (team-owned): a named API surface. Origin ∈ {`declared` (operator-created
  routes), `imported` (OpenAPI), `discovered` (traffic-first)}. Owns:
  - **`SpecVersion`s** — immutable versions of the API's schema (source: imported file or
    aggregation over observations), each carrying lifecycle state and provenance (sample
    counts, confidence, first/last seen — per element).
  - **Gateway bindings** — FKs to the routes/clusters that serve it (a route may exist without
    an ApiDefinition; attaching one is cheap and automatic for `expose`/import/generate paths).
  - **MCP tool set** — generated projection of the *published* SpecVersion; never hand-edited
    structurally (metadata like description/enabled is editable; schema is derived).
- Gateway resources (`Cluster`, `RouteConfig`/`VirtualHost`/`Route`, `Listener`, `Filter`,
  `Secret`, `Dataplane`) remain independently addressable team-scoped entities (v1's chain,
  normalized — single representation, no JSON+projection dual writes; views are generated).
- AI gateway entities (§7): `AiProvider`, `AiRoute`, `AiBudget` — same domain model, same
  tenancy, same events.

Identity: one `domain_id!`-style newtype per entity (UUIDv7 for index locality), `team_id`
typed as `TeamId` everywhere. One human handle per resource type (name, unique **per team** —
fixes 08a §2.2.2), UUID accepted as an alternate key on the API.

### 3.2 Lifecycle state machine (per SpecVersion)

```
observed ──aggregation──▶ learned ──operator review──▶ reviewed ──publish──▶ published ──xDS ACK──▶ served
   │                        │                             │                     │
   └────── discard ◀────────┴──────────── reject ◀────────┘     unpublish ◀─────┘
```

- Transitions are domain methods emitting events; illegal transitions are unrepresentable
  (compile-time enum methods, DB CHECK on state column).
- `published` is the only state that generates MCP tools and (traffic-first) route proposals
  → live config. Everything left of `reviewed` is **data, never config** (08a §4.3.1).
- `served` is asserted by xDS ACK feedback, making "is my API actually live?" a first-class
  queryable fact end-to-end (CLI: `flowplane api status`).

### 3.3 Domain events and the outbox (fixes 08 §2)

- Every `fp-core` mutation writes its rows **and** its event(s) to an `events` outbox table in
  the same transaction. Event types (closed enum, versioned payloads): resource CRUD
  (`ClusterUpserted`, `RouteConfigDeleted`, …), lifecycle (`SpecVersionLearned`,
  `SpecVersionPublished`, …), learning (`CaptureSessionStarted/Stopped`), AI
  (`BudgetThresholdCrossed`, `ProviderFailedOver`), security (`CertRevoked`).
- Delivery: in-process dispatcher consumes the outbox (Postgres `LISTEN/NOTIFY` for wakeup,
  poll fallback for missed notifications — works on any Postgres, no K8s anything), with
  at-least-once semantics and idempotent consumers. Consumer cursors are persisted → CP restart
  resumes cleanly; multi-replica CPs coordinate via `FOR UPDATE SKIP LOCKED` on the cursor.
- Consumers: xDS snapshot rebuilder (per team, per type), MCP tool generator, learning session
  manager, audit fan-in, budget evaluator. No subsystem ever polls another's tables.

### 3.4 Consistency rules (no orphans, no stale artifacts)

1. All FKs real, all with explicit ON DELETE (no name-string FKs; kills the v1
   cluster-name CASCADE surprise — referenced clusters RESTRICT delete with an actionable
   error listing dependents, `--cascade` opt-in does an ordered, evented teardown).
2. Deleting an upstream (cluster) or ApiDefinition emits events that retire its tools,
   unpublish its spec versions, and rebuild xDS — verified by the definition-of-done E2E.
3. MCP tools carry the `spec_version_id` they were generated from; publishing a newer version
   regenerates tools (with a diff surfaced for review when breaking) — kills 08 §4.4 staleness.
4. Optimistic concurrency on **every** mutable resource (`version` column, `If-Match`/CLI
   `--revision`, 409 with current revision on mismatch).
5. Retention is a first-class policy: inference intermediates TTL after aggregation; spec
   versions pruned by count; audit by age (configurable; defaults documented).

## 4. Tenancy architecture (fixes 08a §2)

- `TeamId`/`OrgId` newtypes; `fp-storage` repository methods **require** a `TeamScope`
  parameter (`Team(TeamId)` or `PlatformAdmin(reason)`) — there is no unscoped query API;
  the scope lands in the SQL predicate, not the handler (rate-limit pattern universalized).
- Schema: `team_id` column name everywhere; `(org_id, team_id)` composite FKs validate team∈org
  by construction wherever both appear; per-team unique indexes `(team_id, name)`.
- Authorization: v1's `check_resource_access` **semantics** preserved verbatim (decision table
  in spec/05 §3.1 — including admin:all≠tenant-bypass, 404-hiding, agent partitions),
  re-housed in `fp-core::authz` as a pure, table-driven, property-tested function; one public
  allowlist; surfaces declare `(resource, action)` in their route/tool definitions so the
  OpenAPI doc, MCP registry, and enforcement derive from the same table (kills phantom
  entries).
- Per-tenant quotas (resources, sessions, capture volume, schema cardinality, write rate) in
  `fp-domain`, enforced in `fp-core`, configurable per team by platform admin.

## 4a. Security architecture (consolidated; binding requirements in spec/08a)

**Defense-in-depth summary** — where each property is enforced and where it is proven:

| Property | Mechanism | Proven by |
|---|---|---|
| Tenant isolation (data) | `TeamScope`-typed repos, SQL predicates, composite FKs (§4) | per-slice adversarial exits + S12 full 08a §4 pass |
| Tenant isolation (data plane) | per-team xDS snapshots from cert-bound identity (§5) | S5 adversarial cert test |
| Tenant isolation (capture) | team-scoped injection + per-message team binding (§6) | S8 capture-poaching test |
| AuthN | OIDC JWT (Zitadel; mock OIDC in dev) — bearer-only | S2 |
| AuthZ | one table-driven decision engine, declared `(resource, action)` per route/tool (§4, §9) | S2 property tests vs spec/05 §3.1 |
| Audit | every mutation + denials + auth failures, request-id linked (08a §6) | S2 + each slice's mutations |
| Approval gates | lifecycle: nothing < `reviewed` becomes config/tools (§3.2); `--dry-run` everywhere | S9 approval-bypass test |

**Transport security:**
- xDS-family gRPC: mandatory mTLS, SPIFFE cert registry binding, live-stream revocation (§5).
- REST/MCP API: **native TLS support on the API listener** (cert/key via config or the secrets
  subsystem), because D-004 deployments may have no fronting LB; running plaintext requires an
  explicit `--insecure-api` style opt-in that logs a startup warning. LB/proxy termination
  remains the documented default for fleet deployments.
- **No cookies, no sessions, no CSRF surface:** dropping the web UI removes v1's BFF/cookie/
  CSRF machinery entirely; v2 is bearer-token-only on every surface (CLI, agents, MCP). MCP
  Origin allowlist still enforced (08a §2.2.4).

**Edge hardening (carried from v1's findings, on by default):** request body limits
(per-route overrides where needed), per-tenant write throttle, security response headers,
JSON-only error bodies, fail-closed degraded mode when auth infrastructure is unavailable.

**Secrets:** encrypted at rest (KEK from env/file with documented rotation), write-only API,
SDS-only delivery to owning team's dataplanes, one-shot reveals, audited access; AI provider
credentials are typed secrets in the same subsystem (§7); never-log redaction boundary (§8a).

**Dev-mode safety (08a §2.2.10):** dev auth/seeding compiled behind a feature flag *and*
runtime-gated; a release build refuses dev mode unless an explicit
`FLOWPLANE_DEV_MODE_ACK=yes-this-is-not-production` is set; bootstrap tokens one-shot+expiring.

**Supply chain & release:** locked dependencies, `cargo audit`/`cargo deny` in CI (S1 gates),
musl static binary, image signing + SBOM at release (S12); no telemetry/phone-home.

## 5. xDS subsystem (fixes 08 §2, spec/04 §8)

- **Pipeline (adopted from Envoy Gateway, de-K8s'd per D-004):** domain snapshot → typed IR →
  Envoy protos. The IR seam is where filters, learning capture config, and AI-gateway filter
  chains inject — deterministically (BTreeMap everywhere; no HashMap iteration in any encoded
  output).
- Event-driven rebuilds (outbox consumer) with per-team, per-type-URL versioned snapshots;
  ordering: make-before-break (CDS/EDS before RDS before LDS; deletes reversed) within a team's
  push sequence.
- ACK/NACK: per-type versions; NACK quarantines the offending resource (serve last-ACKed
  snapshot for that type, flag resource `degraded`, surface in CLI/API/audit) instead of v1's
  silent wait-for-fix.
- Real EDS (endpoints from their own table; endpoint churn ≠ cluster rebuild). Delta xDS
  dropped in v1.0 (SOTW only — honest, vs v1's fake delta); revisit post-1.0.
- Uniform **fail-closed**: DB error = serve last good snapshot, never config fallback.
- mTLS identity as v1 (SPIFFE SAN → cert registry row → team) plus: per-message team binding on
  ALS/ExtProc/diagnostics, revocation enforced on live streams, allowlist-scope via
  authenticated registry not node metadata.
- **Dataplane analytics (D-007):** fp-agent relays curated Envoy stats (loopback admin scrape)
  and liveness heartbeats over the outbound mTLS diagnostics stream, using the proto's reserved
  telemetry/lifecycle ranges; the CP aggregates per team to power `stats *`, `dataplane
  status`, `ops doctor`, and usage insights. CP never dials dataplane admin ports (v1 did —
  removed); Envoy's /stats/prometheus remains for customer monitoring.
- Capture injection scoped to the session's team listeners/routes only, with Envoy-side ALS
  filters where possible (kills the all-listener blast radius and 08a §4.1).

## 6. Learning subsystem (fixes spec/06 §9–11)

- Both directions on the shared model: **config-first** (session targets an ApiDefinition or
  route set; capture → inference → aggregation → SpecVersion `learned`) and **traffic-first**
  via a *discovery listener*:
  - `learn discover start` materializes a **Flowplane-owned Envoy listener** (not a user
    Listener resource; owned by the session, excluded from user resource lists, torn down on
    stop). Filter chain: capture (team-scoped ALS+ExtProc) + a catch-all **forwarding stage** —
    forwarding is mandatory because response observation (schemas, status codes, auth-scheme
    detection) requires proxying traffic onward, not black-holing it (v1's gap, spec/06 §9).
  - Two forwarding modes: `--upstream host:port` (recording reverse proxy in front of one
    service) or host-routed via Envoy dynamic forward proxy (Host/SNI-keyed, runtime DNS) —
    observations cluster by observed Host into separate candidate ApiDefinitions.
  - **Open-proxy/SSRF hardening (binding):** discovery refuses to start without `--upstream`
    or an explicit destination allowlist (`--to-host`/`--to-cidr`); the CP address,
    loopback, link-local, and cloud-metadata ranges (169.254.0.0/16 incl. 169.254.169.254)
    are always denied; sessions carry TTL (`--max-duration`) and capture quotas.
  - Operators direct traffic at the discovery port themselves (DNS/LB cutover or client
    config) — Flowplane observes only traffic addressed to it.
  - Learned specs store **upstream provenance** (observed Host, SNI, resolved address, TLS)
    per endpoint; `flowplane route generate --from-spec` uses it to plan the durable
    cluster/route-config/listener (same planner as OpenAPI import), shown via `--dry-run`,
    applied only on approval, entering the lifecycle at `reviewed`.
- Poisoning resistance per 08a §4.3: frequency thresholds, cardinality caps + alerts,
  provenance per element, quarantine bucket, TTL'd intermediates, redaction by default.
- Pipeline mechanics keep v1's good bones (worker pool, batched transactional writes,
  confidence scoring, path templating) with the fixes: per-queue bounds end-to-end, batched
  sample counting, sharded workers (no single-Mutex pool), ExtProc honoring method filters,
  template-aware (not byte-exact) route↔spec matching.

## 7. AI gateway (requirements from spec/09; same domain model)

- **`AiProvider`** (team-scoped): provider kind (anthropic | openai | bedrock | self-hosted |
  openai-compatible(prefix)), endpoints, credential = reference to a `Secret` (never inline),
  model catalog. Compiled to a flavored Cluster in the IR.
- **`AiRoute`**: unified inbound API (OpenAI-compatible chat-completions shape for v1.0),
  model-based routing rules, backend list with `weight` + `priority` + optional
  `model_override` (failover vocabulary borrowed from AIGW; retries re-translate/re-sign per
  selected backend).
- **Translation + metering**: ExtProc service in `fp-xds` hosting `fp-ai` translators
  (ingress↔provider); token usage parsed from provider `usage` accounting — streaming included
  by force-injecting `include_usage` — emitted as dynamic metadata costs and ingested as usage
  events (per team, provider, model, token type).
- **`AiBudget`**: per-team/per-provider token & request budgets with windows; enforced via
  rate-limit costs charged post-response (overdraft-on-last-request semantics, documented);
  `shadow_mode` for meter-without-enforce (fits the dry-run culture); breaches emit events →
  audit + CLI visibility.
- Observed AI traffic is still traffic: usage records feed the same stats/insights surface, and
  AI endpoints are learnable ApiDefinitions like any other.

## 8. Error handling strategy

- One `fp-domain::Error` taxonomy; every API failure renders as
  `{code, message, hint, request_id, details?}` where `code` is a stable machine string from a
  published closed set (OpenAPI-documented), `hint` says what to do next, `request_id`
  correlates to logs/traces. CLI renders the same object as colored text + remediation, exit
  codes: 0 ok, 1 generic, 2 usage, 3 auth, 4 not-found, 5 conflict/revision, 6 server, 7
  unavailable.
- No `unwrap`/`expect` outside tests; panics = bugs; `#![deny(clippy::unwrap_used)]`.

## 8a. Observability architecture (logging, tracing, metrics)

Cross-cutting infrastructure established in **slice S1** (not retrofitted at hardening; S12
only completes the alert pack and dashboards). All three signals are OTLP-exportable to any
collector (D-004: no vendor or platform assumption); a deployment with no collector still gets
JSON logs on stdout and a Prometheus `/metrics` endpoint.

**Structured logging** (`tracing` crate, JSON to stdout):
- Every log line carries: timestamp, level, target, `request_id`, and where present `team_id`,
  `org_id`, `actor`, `trace_id`/`span_id`, `resource` ref. The `request_id` in every API error
  body (§8) greps straight to its log lines — that correlation is the operator contract.
- Levels are disciplined: ERROR = operator action likely required; WARN = degraded/retried;
  INFO = lifecycle and mutation summaries; DEBUG/TRACE = development. `RUST_LOG`-style filter
  via `FLOWPLANE_LOG` (env-first, D-005).
- **Never logged:** secret values, credentials, captured request/response bodies, full JWTs.
  Enforced by redaction at the serialization boundary plus a CI grep-test over log calls
  (carried from S6 exit criteria).
- Logs ≠ audit: the audit trail (08a §6) is a DB-backed, queryable, retained record of *who did
  what*; logs are operational diagnostics. Events appear in both where they overlap, linked by
  `request_id`.

**Distributed tracing** (OpenTelemetry):
- Spans cover the full causal chain, *including the async hops*: REST/MCP request → core
  service → DB transaction → **outbox event (trace context stored on the event row)** →
  consumer (xDS rebuild, tool generation, budget evaluation) → xDS push. A config change is
  one trace from `PUT /clusters/x` to the Envoy ACK — this is the primary debugging story for
  "why isn't my route live?".
- Inbound `traceparent` honored (W3C trace context); CLI sends one per invocation so a failed
  command's trace id is printable with `--verbose`.
- Data-plane traffic tracing (Envoy → Jaeger/OTLP) is *Envoy's* concern, configured via
  listener tracing settings the CP can enable per listener (as v1 supported); CP traces and
  data-plane traces share nothing but the collector.
- Learning/AI pipelines: capture-batch and inference spans sampled (head sampling, configurable
  rate) to avoid per-request span cost on hot paths; AI-gateway requests follow GenAI OTel
  semconv (`gen_ai.*` attributes, TTFT, time-per-output-token — spec/09 borrow).

**Metrics** (Prometheus exposition + OTLP):
- Golden set defined with the subsystem that owns it, label discipline `team` (bounded), never
  per-request-path unbounded labels. Core families: API (rate/errors/latency by route class),
  outbox (depth, consumer lag seconds — the readiness signal), xDS (connected dataplanes,
  pushes, ACK/NACK, quarantined resources), learning (capture rate, queue depths, drops,
  inference latency), AI (tokens by team/provider/model/type, budget utilization, failovers),
  DB pool, auth (denials by code).
- The **alert pack** (S12 deliverable, but metric names fixed from each owning slice):
  outbox lag, NACK/quarantine count, dataplane disconnects, capture drop rate, budget
  near-exhaustion, auth-denial spike, DB pool saturation.

## 9. Contract generation (fixes 08 §2 "detached artifacts")

Route definitions (REST), tool definitions (MCP), and CLI command metadata each carry their
`(resource, action)`, schemas, and docs **in one declaration site**; OpenAPI doc, MCP
`tools/list`, the authz table, and reference docs are generated from those declarations at
build time, with CI parity gates (router↔OpenAPI 100%, registry↔tools 100%, docs↔flags 100%).
The v1↔v2 OpenAPI diff (required by the plan) runs against `spec/01-api-contract.v1-openapi.json`.

## 10. Operations (D-004; full review in docs/production-readiness.md at hardening)

Single static binary (musl) + OCI image + compose bundle + systemd unit examples; ECS task
definition and K8s manifests as optional extras (D-004: ECS-class managed container platforms
are first-class deployment targets — nothing may assume orchestrator-specific discovery,
sidecar injection, or secret stores; everything works over plain TCP/HTTP + env vars). `/healthz` (liveness) and `/readyz` (DB + outbox lag + xDS serving)
endpoints; observability per §8a; graceful
shutdown (drain xDS streams, flush outbox consumers, stop workers); migrations forward-only,
run by `flowplane db migrate` or auto-on-boot (flag); backup = Postgres backup (documented
restore drill); degradation: DB down → serve last xDS snapshots read-only, REST 503 with hint;
Envoy down → no impact on CP, dataplane status visible.

## 11. Traceability: 08-critique items → design elements

| 08 item | Fixed by |
|---|---|
| §2 DB-polling integration | §3.3 outbox events |
| §2 name-convention FKs | §3.1 real FKs on shared aggregate |
| §2 detached generated artifacts | §9 single-declaration generation + CI gates |
| §3.1 handler tenancy | §4 TeamScope-typed repos |
| §3.2 triple validation | §2 rules: domain validation only in fp-domain, all surfaces converge via fp-core |
| §3.3 dual representations | §3.1 normalized single model, generated views |
| §3.5 concurrency | §3.4.4 universal optimistic locking; DB-allocated ports/uniqueness |
| §4 loop seams 1–6 | §3.1–3.4, §6 (traffic-first), lifecycle §3.2 |
| §5 error contract | §8 |
| §6 CLI | spec/12 |
| §7 multi-replica/failure semantics | §3.3 persisted cursors, §5 fail-closed, §10 |

Adopted from spec/09: EG IR pipeline (§5), AIGW resource split + failover vocabulary +
ExtProc metering (§7), metadata-driven rate-limit cost (§7), shadow mode (§7), GenAI OTel
semconv (§10), 3-condition status model (adapted to lifecycle states §3.2). Rejected (with
09's reasoning): EnvoyPatchPolicy-style raw overrides, header-based tenancy, secrets inlined
into filter config, K8s-native anything (D-004).
