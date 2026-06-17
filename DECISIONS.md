# Flowplane v2 — Architectural Decisions

Every divergence from v1, every borrowed idea from prior art, every cut or reshape. Format per
entry: **Context** → **Decision** → **Why it's better than the v1 approach** (or why borrowed /
rejected). Decisions made without founder response to a question in `QUESTIONS.md` are marked
**provisional** until approved or vetoed.

---

## D-001: Spec-first rewrite process

- **Context:** v2 is a ground-up rewrite; v1 is read-only reference.
- **Decision:** Extract behavioral specs (Phase 0) before any v2 code; implement only from specs;
  return to v1 source only when a spec is ambiguous, and fix the spec when that happens.
- **Why:** Prevents verbatim porting of v1's coupling problems; makes the contract reviewable by
  the founder before implementation cost is sunk.

## D-002: Every v1 UI workflow gets a CLI/MCP path or a recorded cut

- **Context:** v2 drops the SvelteKit UI. spec/07 §4–5 inventories all 62 UI pages (~38
  workflows): 21 already CLI-covered, 10 partial, 7 with no CLI path at all.
- **Decision:** All 7 zero-coverage workflow families become v2 CLI commands (team/org
  membership + grants, scoped filter configuration, single-route edit + bulk MCP ops, MCP tool
  update/apply-learned, MCP connections, secret update/references, org update + member roles),
  and the 10 partial gaps are closed in their owning slices. The spec/07 §5 fates table is the
  binding inventory; the four visualization-only workflows are proposed cuts (Q-002).
- **Why better than v1:** v1's CLI was a subset of the UI; with no UI, CLI parity is the
  product floor, not a nice-to-have.

## D-003 (approved by founder, Q-002): Cut dashboard rendering, keep the data

- **Context:** Four v1 UI workflows are visualization-only: stats dashboard (30 s polling
  charts), platform-admin KPI dashboard, per-org governance drill-in widgets, profile/password
  page (an IdP deep-link).
- **Decision (provisional):** Keep every underlying data endpoint (`stats *`, `admin
  resources/audit`, new `admin health` for the xDS rollup) with `--json` + `--watch`; cut the
  chart/dashboard rendering — point operators at Prometheus/Grafana for visualization;
  `auth whoami` prints the IdP account-console URL replacing the password page.
- **Why:** A control plane without a UI shouldn't own dashboard rendering; operators already
  run metric stacks. Recorded as removing real (cosmetic) user value → founder veto in Q-002.

## D-004: Environment-agnostic deployment (founder non-negotiable, 2026-06-12)

- **Context:** Founder directive: control plane and data plane must be deployable in any
  environment — bare metal, VMs, plain containers, managed container platforms (ECS/Fargate,
  Nomad, Cloud Run and similar), or Kubernetes — never *designed for* Kubernetes or any other
  specific orchestrator (founder clarification 2026-06-12: ECS-class environments are explicitly
  in scope).
- **Decision:** No Kubernetes API dependency anywhere: PostgreSQL (not CRDs) is the source of
  truth; identity via OIDC + mTLS certs (not ServiceAccounts); deployment artifacts are a
  static binary, an OCI image, a compose bundle, and systemd guidance — plus deployment notes
  per environment class (ECS task definitions, K8s manifests) offered as packagings among
  equals, never required. Concretely this forbids: orchestrator-specific service discovery,
  sidecar injection assumptions, orchestrator secret stores as the only secrets path, and any
  health/identity mechanism that doesn't work over plain TCP/HTTP. Prior-art borrowings (Envoy
  Gateway's IR pipeline, AI Gateway's metering) are adopted as mechanisms, stripped of their
  CRD/controller substrate (spec/09 rejects already aligned with this).
- **Why better than v1:** v1 was already environment-agnostic (compose-first); this locks the
  property in as a reviewed constraint so no v2 design step regresses it.

## D-005: CLI precedence (Q-001 approved): server env-first, CLI flag-first

- **Decision:** Server config: env > config file > defaults. CLI client: explicit flag > env >
  config file > defaults — uniformly for every value (token, team, org, base-url, timeout).
- **Why better than v1:** v1 had three contradictory precedence orders across values; explicit
  flags silently losing to ambient env vars violates least surprise (gh/kubectl convention).

## D-006: Discovery listener is a forwarding proxy with mandatory destination constraints

- **Context:** Founder design review of the traffic-first transcript asked what the discovery
  listener listens to and whether routes must exist first.
- **Decision:** No user routes required. `learn discover start` materializes a Flowplane-owned
  Envoy listener whose chain is capture + catch-all forwarding (explicit `--upstream`, or
  host-routed dynamic forward proxy). Forwarding is mandatory (responses must be observed to
  learn schemas/status/auth). Because a host-routed forwarder is an open-proxy/SSRF surface,
  discovery refuses to start without `--upstream` or a destination allowlist; CP, loopback,
  link-local, and cloud-metadata ranges are always denied; sessions have TTL + capture quotas.
- **Why better than v1:** v1 could only black-hole unmatched traffic (request-only, useless
  responses, spec/06 §9); v2 observes full exchanges safely and feeds upstream provenance into
  route generation.

## D-007: Dataplane analytics via agent telemetry relay (not CP admin-scrape)

- **Context:** v1 serves `stats *` by scraping each Envoy's admin API over the network
  (`stats_data_source.rs`) — requires CP→dataplane inbound reachability and off-box admin-port
  exposure; breaks NAT'd/ECS environments (D-004) and is a security liability. The v1 agent
  proto already reserves field ranges for heartbeats (20–29) and telemetry relay (40–49).
- **Decision:** fp-agent scrapes Envoy admin on loopback only and streams a curated metric set
  (request/response counters, response-code classes, latency histograms, connection gauges,
  listener/cluster health) + liveness heartbeats to the CP over the existing outbound mTLS
  diagnostics stream, using the reserved proto ranges. CP aggregates per team → powers
  `flowplane stats`, `dataplane status`, `ops doctor`, and learning-loop usage insights.
  Envoy's native /stats/prometheus stays available for customer monitoring stacks
  (complementary). Envoy admin ports are never exposed off-box in v2.
- **Why better than v1:** outbound-only connectivity works on bare metal/VM/ECS/K8s alike;
  removes an entire attack surface; heartbeats give real liveness (v1 inferred it from xDS
  stream state); founder-raised gap (2026-06-12).

## D-008: Native TLS on the API listener; bearer-only auth (no cookie/CSRF surface)

- **Context:** v1 served REST/MCP on plaintext :8080 assuming LB termination, and carried a
  BFF/cookie/CSRF stack for the SvelteKit UI. D-004 deployments (bare metal, ECS) may have no
  fronting LB; v2 has no UI.
- **Decision:** The v2 API listener supports native TLS (cert/key via config or secrets
  subsystem); plaintext requires explicit opt-in that warns at startup. All auth is bearer
  tokens on every surface — v1's BFF/cookie/CSRF machinery is deleted, not ported (MCP Origin
  allowlist retained). Consolidated security architecture recorded as spec/10 §4a
  (founder-raised, 2026-06-12).
- **Why better than v1:** removes an entire browser-attack surface class; works without an LB
  in any environment; smaller auth codebase with one path to test.

## D-009: CP and DP independently deployable (founder-confirmed constraint, 2026-06-12)

- **Context:** Founder requires v1's property that control plane and data plane deploy,
  scale, and upgrade independently.
- **Decision:** Codified as spec/10 §10.1. The DP unit (Envoy + fp-agent) couples to the CP
  through outbound mTLS gRPC only; separate deployment artifacts per plane; independent
  lifecycle guarantees (DP serves last config through CP outages); documented Envoy
  version-skew range; additive-only agent proto. S12 failure-mode suite explicitly tests
  traffic through a full CP outage; operator docs ship separate CP and DP install guides.
- **Why better than v1:** v1 had the property de facto (compose-dp bundle) minus one leak —
  the CP dialed Envoy admin ports for stats (removed by D-007, which makes independence
  strictly stronger: outbound-only connectivity from the DP).

## D-010: v2 REST contract — systematic divergences from v1 (S4 diff)

- **Context:** S4 exit requires diffing the generated v2 OpenAPI document against v1's
  (committed as spec/01-api-contract.v1-openapi.json). v1 documented 85 of ~187 real
  operations with 4 wrong methods; v2 documents 100% by construction (router and document
  split from one `routes!` declaration; parity pinned by test).
- **Intentional differences (apply to every endpoint):**
  1. Errors: `{code, message, hint?, details?, request_id}` with a stable closed code set —
     replaces v1's `{error, message}`.
  2. Optimistic concurrency everywhere: PATCH/DELETE require `If-Match: <revision>`; 409
     `revision_mismatch` reports the current revision (v1: last-writer-wins, PUT/PATCH mixed).
  3. PATCH uniformly for updates (v1 mixed PUT/PATCH; its doc claimed PUT for 4 PATCH routes).
  4. Per-team resource names (v1: global uniqueness — cross-tenant name oracle, 08a §2.2.2).
  5. Uniform `{items,total,limit,offset}` list envelope (v1: partial adoption).
  6. Team/member/grant management initially moved from `/orgs/{org_name}/…` to
     caller-org-implied `/api/v1/teams…` under the one-org-per-user assumption. D-014
     supersedes that assumption: tenant-scoped operations must now carry an explicit org
     context, while platform admins are still NOT admitted to tenant administration merely by
     platform role (invariant 1).
  7. `/api/v1/auth/whoami` replaces v1's UI-session-oriented `/api/v1/auth/session`; the
     BFF/cookie endpoints are deleted, not ported (D-008).
- **Not-yet-mapped v1 surface:** secrets/dataplanes/certs (S6), learning/schemas (S8-S9),
  MCP management (S11), ops/stats (S6/S12), filters (S5), org-admin CRUD + agents (S4 wrap) —
  each lands with its owning slice and extends this diff; the systematic rules above bind them.

## D-011: xDS listener is mTLS-or-off (no plaintext production mode, no boot hard-fail)

- **Context:** v1 hard-failed boot when any xDS TLS path was missing (ADR
  "cp-xds-mtls-non-negotiable"). v2 keeps mTLS non-negotiable but must also support
  bring-up: the API has to be reachable to create dataplanes and certificates before any
  dataplane exists (bootstrap ordering), and API-only deployments are legitimate.
- **Options:** (a) hard-fail boot without xDS TLS (v1); (b) serve xDS plaintext with a
  warning (rejected outright — violates spec/04 §1.2); (c) with TLS material configured →
  mTLS listener; without it → no xDS listener at all, loud startup warning naming the
  three FLOWPLANE_XDS_TLS_* variables; dev mode (triple-gated) keeps its plaintext
  listener with node-id resolution.
- **Decision:** (c). Fail-closed is preserved (no listener ≠ insecure listener) while the
  control plane stays operable for bring-up. Partial TLS config (1 or 2 of the 3 paths) is
  an invalid_config boot error, so a typo cannot silently disable xDS.
- **Status:** decided (S5.4), founder can revisit at the S7 CLI review.

## D-012: HTTP filters are typed IR on the spec, translated at build time (no v1-style injection)

- **Context:** v1 assembled filters by post-hoc protobuf surgery — decoding built listeners,
  walking HCM filter chains, and mutating them (spec/04 §4.4), plus a JWT "merger" because
  filters could be attached from many places and had to be coalesced into one
  `JwtAuthentication`. That pipeline was a recurring source of warm-and-skip failures and
  silent no-ops.
- **Decision (v2):** filters are a closed, validated IR. `ListenerSpec.http_filters` is the
  ordered chain; `VirtualHost`/`RouteRule.filter_overrides` carry per-scope behavior. The xDS
  translator emits the HCM chain (router auto-appended last) and `typed_per_filter_config`
  directly — no decode/mutate. **At most one filter of each type per chain**, which removes
  the need for v1's JWT provider-merge entirely (a JWT config simply declares all its
  providers/requirements in one place).
- **Per-route capability matches Envoy's** (spec/04 §4.1 column): `Disable` is universal
  (except health_check, which is listener-only); cors and local_rate_limit take full
  per-scope config; jwt is reference-only (`PerRouteConfig.requirement_name`); oauth2 has no
  per-route form and is therefore not expressible as an override.
- **Type-URL fidelity:** wire type URLs must match the proto message name, not the prost Rust
  type — e.g. RBAC's message is `RBAC` (all-caps) though the Rust type is `Rbac`. A real-Envoy
  E2E phase (jwt_auth + rbac) plus a unit assertion guard this class of NACK.
- **Catalog status:** shipped {cors, local_rate_limit, header_mutation, health_check,
  compressor, jwt_auth, ext_authz, rbac}. Deferred to their dependency slices:
  rate_limit/rate_limit_quota (RLS), ext_proc, oauth2/credential_injector (SDS secrets, S6),
  custom_response, mcp (S11), wasm (custom-binary storage).
- **Status:** decided (S5.8), founder can revisit at the S7 CLI review.

## D-013: target Envoy 1.37.x (one release before the latest stable line)

- **Context:** the data plane is Envoy; we test the translated xDS against a real proxy. As of
  2026-06, the latest stable line is 1.38.x (1.38.1). Founder directive: track one release
  before the latest stable — i.e. the 1.37.x line (latest patch 1.37.4).
- **Rationale:** N-1 trades the newest features for a line that has had a full quarter of patch
  hardening, while staying well inside Envoy's 4-minor support window. Flowplane targets the
  stable xDS **v3** API, which 1.37 speaks; the live E2E confirms every resource type and all
  eight shipped filters ACK on 1.37.4.
- **What moved:** `scripts/e2e-envoy.sh` docker tag → `envoyproxy/envoy:v1.37-latest`; sandbox
  binary → archive-envoy 1.37.4. `envoy-types` stays at 0.7.4 (xDS v3 is API-stable across
  these minors; the E2E proves compatibility — no proto-path churn, so no reason to bump and
  risk breakage).
- **Revisit:** bump the pin one line behind whenever Envoy cuts a new stable (so we'd move to
  1.38.x once 1.39.x ships), and re-bump `envoy-types` only if a NACK ever shows an API gap.
- **Status:** decided (post-S5.8, founder directive).

## D-014: human users may belong to multiple orgs; request org context is explicit

- **Context:** Earlier v2 authz kept v1's one-org-per-user simplification and the loader chose
  one membership with `ORDER BY created_at LIMIT 1`. Founder direction is to support users that
  can work across multiple customer orgs, e.g. consultants, operators, and design-partner
  admins. That makes implicit membership selection a tenant-isolation risk.
- **Decision:** v2 supports multiple org memberships per human user. Do **not** add a
  uniqueness constraint on `org_memberships(user_id)`. Instead, every tenant-scoped operation
  must be authorized against a validated request org context. The v2 selector contract is
  `X-Flowplane-Org` for REST/MCP and `--org` / active config context for the CLI. When no
  selector is supplied, the server may infer the org only if the user has exactly one active
  non-platform org membership; zero or multiple tenant memberships produce no active org and
  tenant-scoped operations fail closed.
- **Identity resolution:** user lookup by email must not search globally and attach whichever
  row appears first. Prefer immutable subject/user-id based selection. Where email remains a
  UX affordance, resolve it inside the target org or reject ambiguous matches.
- **API implication:** existing caller-org-implied routes such as `/api/v1/teams...` remain
  viable because the server constructs a validated active org from the selector before
  authorization. Path-scoped org endpoints such as `/api/v1/orgs/{org}/members...` validate
  membership against the path org directly. This is v2-native; v1 routes are input for desired
  functionality, not a surface to port wholesale.
- **Status:** decided (post-S5.8, founder directive).

### D-014 addendum: resolved org-context contract (founder-confirmed)

The mechanism left open in D-014 is now decided (founder, 2026-06-13):

- **Transport:** active-org *selector*, not route nesting. REST/MCP carry `X-Flowplane-Org`
  (org name or UUID); the CLI uses `--org` / config. URLs stay `/api/v1/teams/...` — no
  `/orgs/{org}/...` nesting, no route duplication. The server resolves and validates the
  active org BEFORE authorization runs.
- **Resolution policy (exact):**
  1. Selector present → resolve to an org the user is an active member of, then authorize in
     that org. Unknown org or non-member → fail closed (`org_selector_required`, no existence
     disclosure).
  2. Selector absent + exactly one active **non-platform** membership → use it.
  3. Selector absent + zero or ≥2 candidate orgs → fail closed (`org_selector_required`).
  4. The platform org is NEVER an inferred or selectable tenant context.
  5. Never choose by `created_at` / `LIMIT 1` / any arbitrary ordering.
- **Carrier:** `PrincipalCtx::User.org` becomes "the validated active request org" (set by the
  auth middleware, never by the loader). A new `org_selector_required` flag distinguishes
  "no active org because the selector is needed" (→ 400 `org_selector_required`) from "genuinely
  no access" (→ 404). The pure authz engine is unchanged — it still authorizes against the
  single active org, so cross-org/org-admin semantics and invariant tests hold.
- **Identity resolution (R6):** prefer immutable subject/user-id; email is a UX affordance only
  and must reject ambiguous (>1) matches rather than `LIMIT 1`. Global email uniqueness is not
  an isolation boundary.

## D-015: local/dev ports are defaults with explicit overrides, never fixed assumptions

- **Context:** Flowplane local workflows bind several ports at once: API, xDS, Envoy admin,
  gateway listeners, upstream fixtures, Postgres, and later fp-agent health/diagnostics. v1
  examples frequently used literals such as `8080`, `18000`, `9901`, and `10001`; that is fine
  for documentation but brittle for real developer machines, CI shards, and parallel E2E runs.
- **Decision:** every local/dev bind must have an override path. Defaults remain stable and
  copy-pasteable, but they are not architecture: CLI/dev commands accept flags and env vars
  for API, xDS, Postgres, Envoy admin, agent health, and gateway listener ranges. Scripts use
  `FLOWPLANE_E2E_*` variables; product CLI flows use named flags such as `--api-port`,
  `--xds-port`, `--postgres-port`, `--admin-port`, and `--gateway-port-range`.
- **Allocation rule:** multi-listener workflows allocate from a caller-provided gateway port
  range, checking availability before writing control-plane state. If no port is available,
  fail before mutation with the exact occupied range and the override flag/env var to change.
  Do not silently skip, retry random ports, or leave the operator guessing which port was used.
- **Config propagation:** generated Envoy bootstrap and dataplane compose/systemd/K8s
  artifacts must carry the resolved ports explicitly. Runtime code should read resolved config,
  not assume the documented defaults.
- **Status:** decided (S6.4/S6.5 local-run hardening).

## D-016: crate boundaries are layered, with explicit read-side exceptions

- **Context:** The v2 workspace intentionally separates pure domain types, storage, core
  services, REST transport, xDS serving, and binaries. Current dependency direction is acyclic
  and mostly layered: `fp-domain` is pure; `fp-storage` owns SQL/migrations/outbox; `fp-core`
  owns service mutations and authorization; `fp-api` exposes REST/OpenAPI; `fp-xds` owns Envoy
  translation, ADS, diagnostics, and snapshot serving; binaries compose the pieces. However,
  `fp-api` and `fp-xds` both have narrow direct storage reads for request context and
  dataplane read models.
- **Decision:** Keep the pragmatic layered model, but make the exceptions explicit. All
  state-changing product behavior must go through `fp-core::services`; transport crates must
  not create alternate mutation paths. `fp-api` may call `fp-storage` directly only for
  authentication/request-context read helpers: JIT user provisioning, principal loading,
  audit of authn failures, org selector resolution, and team path resolution. `fp-xds` may call
  `fp-storage` directly only for read-side projection/snapshot assembly, certificate-registry
  identity binding, NACK persistence, diagnostics telemetry persistence, and outbox event
  consumption.
- **Why:** Strict ports/traits everywhere would add ceremony before there is a real alternate
  backend or test seam need. Unbounded direct storage access would erode the core service layer
  and re-create v1-style coupling. This rule keeps the fast read paths close to their owning
  runtime surfaces while preserving the important invariant: business mutations, authz checks,
  audit/outbox writes, and tenant isolation rules live in core services.
- **Revisit trigger:** If S8-S12 add another externally callable surface (MCP, learning workers,
  admin jobs) that wants direct storage access, either route it through `fp-core::services` or
  update this decision with a named read-side exception before implementation.
- **Status:** decided (post-S7 crate-boundary review).

## D-017: Learning is an API lifecycle aggregate, not a sidecar schema pipeline

- **Context:** v1's learning loop works, but it is stitched together by string conventions:
  learning sessions, inferred rows, aggregated schemas, route metadata, OpenAPI imports, and
  MCP tools all live as separate records with manual export/import/enable hops between them.
  That creates drift: exports are not re-importable without editing, tools go stale, matching
  depends on exact path strings, and capture is listener-wide with CP-side filtering.
- **Decision:** S8 introduces a typed, team-owned `ApiDefinition` aggregate as the spine for
  the loop: route bindings, capture sessions, observations, immutable `SpecVersion`s, publish
  state, and generated tool rows all reference the same API identity. Learning remains data
  until an operator reviews/publishes a spec version. Publishing is the integration point:
  it regenerates the API's tool projection and, in later traffic-first work, produces route
  materialization plans. The v1 algorithms are reference material only; the v2 shape is
  stricter and more integrated.
- **Accuracy improvements over v1:**
  - Observations are keyed by `(team, api_definition, capture_scope, host, method, normalized_path)`
    so two hosts or route bindings with the same path cannot collapse into one schema.
  - Capture is scoped to team-owned listeners/routes at injection time, not all listeners with
    CP-side path regex filtering. Per-message team/API/session ids are validated before ingest.
  - Required fields use frequency thresholds with minimum sample counts rather than v1's brittle
    100% rule; optional-rich APIs should still earn high confidence.
  - Header export uses allowlist/frequency thresholds and length caps. One hostile request must
    not become an OpenAPI parameter.
  - Path templating keeps v1's useful heuristics, but adds cardinality caps and an outlier bucket
    so random-word path floods do not create unbounded endpoints.
- **Efficiency improvements over v1:**
  - ALS sample counts are batched; hot-path capture does not update one DB row per request.
  - The worker pipeline is sharded by `(team, session, request_id)` and bounded at every queue;
    drops are accounted against session health instead of silently disagreeing with counters.
  - Raw inference intermediates have TTL/retention after aggregation, so transient enum
    candidates and header examples do not persist forever.
  - Aggregation is incremental per API endpoint and spec version, not a full ad hoc export/import
    round trip.
- **Integration rule:** OpenAPI import, config-first learning, traffic-first discovery, and AI
  gateway APIs all produce or update `ApiDefinition`/`SpecVersion` records. MCP tools are a
  generated projection of the published spec version, not manually managed schema copies.
- **Status:** decided for S8 (founder direction, 2026-06-13).

## D-018: S10 AI gateway uses OpenAI chat-completions as the v1.0 IR

- **Context:** Earlier S10 notes borrowed AI Gateway's translator matrix too directly:
  OpenAI, Anthropic, Bedrock, Vertex, and OpenAI-compatible providers appeared to be parallel
  day-one translator work, and token budgets were described as using an existing
  `flowplane-rls`-style cost path. In v2, no RLS/cost-settlement service exists yet, and most
  target providers already expose an OpenAI-compatible chat-completions surface.
- **Decision:** S10 v1.0 standardizes on OpenAI chat-completions as the canonical request,
  response, streaming, and usage shape. `openai` and `openai-compatible` providers are the
  critical path and use passthrough plus credential/header/prefix handling. Native Anthropic,
  Bedrock, Vertex, and other dialect translators are demand-pulled behind a `Translator`
  trait when a user need appears; they are not required for the v1.0 exit. The Go AI Gateway
  ExtProc is prior art and a golden-fixture generator, not a Flowplane runtime dependency.
- **Credential delivery:** AI provider records reference S6 `Secret`s; secret values are never
  serialized into xDS snapshots, route config, logs, usage rows, or plan artifacts. The AI
  processor fetches the credential over Flowplane's authenticated control-plane/data-plane
  channel and keeps a bounded in-memory cache so failover can re-sign with the selected
  backend's credential. SDS for AI credentials is deferred until there is a concrete need.
- **Gateway processor:** The AI path uses the same Flowplane-owned ExtProc infrastructure as
  learning/discovery, with an explicit upstream-position AI processing stage. If capture is
  also active, the filter chain must define ordering and buffering so request bodies are read
  once and neither processor inherits the other's size limits by accident.
- **Budget enforcement:** S10 does not assume an existing RLS/Redis cost path. The first
  implementation uses check-then-settle in the AI processor/control-plane path with the
  authoritative counter in Postgres, updated atomically so multiple Envoy/processor instances
  cannot double-spend a team's bucket. External Envoy RLS with metadata costs remains a future
  substrate only if explicitly sized.
- **Budget semantics:** Budgets use fixed windows and fixed weighted token units
  (`sum(weight[token_type] * tokens[token_type])`) before CEL or price tables. A request is
  admitted when the relevant enforcing budget has at least one unit remaining, then settled
  after provider usage is known; bounded overdraft on the last in-flight request is accepted
  and charged to the settlement-time fixed window. Shadow budgets meter without rejection.
  Missing or unparseable terminal usage is currently best-effort/fail-open: no usage event is
  recorded, no counter moves, and subsequent budgeted requests are not blocked. Fail-closed
  missing-usage handling is deferred hardening, not v1.0 behavior.
- **Streaming and failover:** The processor may force `stream_options.include_usage=true` for
  OpenAI streams. If it injected that option, it must strip the synthetic terminal usage chunk
  from the client-facing stream while still using it for settlement. Failover is allowed only
  before the first response byte; once streaming starts, backend failure is terminal for that
  request and partial usage is attributed to the backend actually used.
- **Status:** decided for S10a (2026-06-15), implemented through S10 exit (2026-06-16).
  This overrides the broader translator-matrix and "existing RLS cost path" wording in older
  spec notes.

## D-019: S11 MCP v2 contract replaces the v1 MCP auth/tool model

- **Context:** `spec/02-mcp-tools.md` is a faithful v1 extraction, but v2 has diverged in
  security-sensitive ways. The v1 scope-string model, static "82 tools" count, route exposure
  column, and phantom authz entries do not exist in v2. Current v2 code has typed
  `Resource`/`Action`, service-layer authz, generated `api_tools` rows, and downstream
  `PrincipalCtx::Agent` support, but no agent-token authentication path and no shared
  REST/CLI/MCP declaration registry yet.
- **Decision:** S11 implements MCP from the v2 contract below. v1 is reference material for
  workflows and failure modes, not a source of truth for authorization, tool counts, or storage
  shape.
  **Protocol versions:** advertised/preferred MCP protocol version is `2025-11-25`; accepted
  request versions are exactly `2025-11-25` and `2025-03-26`.
- **Transport and sessions:** v1.0 supports Streamable HTTP `POST /api/v1/mcp` with
  `initialize`, `notifications/initialized`, `ping`, `tools/list`, and `tools/call`.
  `GET` SSE streams, client `DELETE` session termination, resumability, resources, prompts,
  logging, and server-to-client notifications are deferred for v1.0 and must not be advertised.
  Sessions expire by server TTL cleanup only. MCP sessions cache no principal, grants, team list,
  or authorization result: every non-initialize request re-authenticates the bearer token and
  re-evaluates grants so agent disable/rotate and grant revocation take effect on the next call,
  not at session TTL. The Origin allowlist is a browser defense: absent `Origin` is allowed for
  headless agents; present `Origin` must match `FLOWPLANE_MCP_ALLOWED_ORIGINS`.
- **Authentication:** human callers keep using OIDC/JWT through the existing middleware. S11 also
  adds agent bearer tokens: use a distinct `fpat_` prefix, return the clear token only once, store
  only a SHA-256 hash of the token on the agent row, check `status = active`, and resolve to
  `PrincipalCtx::Agent` with kind and grants loaded. Middleware dispatch checks `fpat_` tokens
  through the agent-token path first and all other bearer tokens through JWT validation.
- **Authorization:** MCP uses the existing v2
  `check_resource_access(&PrincipalCtx, Resource, Action, Option<TeamRef>)` model. Platform-admin
  bypass is governance-only, org-admins get implicit same-org team access, `CpTool` agents are
  grants-only, and `GatewayTool`/`ApiConsumer` agents are structurally denied control-plane
  resources. `cp_*` tools are internal control-plane tools and are never externally exposable
  through MCP.
- **Session team resolution:** `tools/list` considers explicit session/request team when present,
  plus granted teams and org-admin implicit teams. Team-scoped tool calls require an explicit team
  when the caller resolves to more than one possible team; there is no "first team" fallback.
- **Dynamic `api_*` tools:** `api_tools` rows generated from published/current specs are the
  source of truth. `tools/list` reads live enabled/current rows from Postgres unless S11 records a
  measured caching reason; without caching, spec republish staleness is handled by the database.
  #78 owns the service, REST, and OpenAPI mutation for toggling an individual
  `api_tools.enabled` flag for `flowplane mcp enable|disable`. `api_*` execution resolves through
  v2 bindings (`api_tools` → `api_definitions`/published spec binding → listener/dataplane route
  information); missing listener/dataplane resolution fails closed with a structured
  configuration error.
- **`api_*` exposure and agent classes:** v2 has no v1 `external/internal` route column. For v1.0,
  MCP callers are internal principals/agents; "external" means dataplane exposure, not a
  cross-org MCP caller. `GatewayTool` agents may consume authorized `api_*` tools through MCP.
  `ApiConsumer` agents are for direct dataplane consumption and are rejected at MCP
  `initialize`.
- **Static registry:** S11 does not build the shared REST/CLI/MCP declaration layer. #74
  hand-authors the MCP registry and adds a drift gate proving the registry `(Resource, Action)`
  matches the service function's enforced `(Resource, Action)`. The weaker "every exposed tool
  has metadata" parity test is necessary but not sufficient.
- **Resource mapping for S11:** v1 `routes:*` maps to `RouteConfigs`; generated gateway tools map
  to `McpTools:read/execute` for listing/execution and may use `ApiDefinitions:read` for backing
  spec metadata; ops/xDS/status diagnostics map to `Stats:read` or `Audit:read` when reading
  persisted diagnostics/audit; learning/API tools map to `ApiDefinitions` and
  `LearningSessions`; shipped AI tools map to `AiProviders`, `AiRoutes`, `AiBudgets`, and
  `AiUsage`; secrets tools are metadata-only unless they go through the existing write-only
  secret service; v1 aggregate-schema, generate-envoy-config, certificates/WASM/reports/RLS
  families are deferred unless a current v2 resource/service path exists.
- **Tools and errors:** `tools/list` is an executable-tool view, not a catalog. A listed tool
  without executable authz, or authz metadata without a real exposed tool, is drift. v1.0
  deliberately returns all tools without cursor pagination for bounded S11 counts; add cursor
  pagination only after measured size pressure. Tool params use camelCase. Authz failures use a
  stable JSON-RPC `error.data.kind = "authz"` discriminator even if the outer JSON-RPC code is
  reused for compatibility. JSON-RPC errors use `{code,message,data}` with stable
  `data.kind`, optional `data.resource`, optional `data.action`, optional `data.requestId`, and
  optional `data.fix` as a human-readable recovery hint. Tool-call business failures return
  normal MCP tool results with `isError: true` and text content containing the same stable
  message plus recovery hint.
- **CLI/operator surface:** #77 `mcp status` reports the POST-only MCP server mode, advertised
  protocol version, supported version list, and enabled tool counts. #77 `mcp connections`
  reports active in-memory sessions only; SSE connection reporting waits until SSE exists.
- **AI tools:** S11 includes read-only AI inspection tools over existing service paths:
  list/get AI providers, list/get AI routes, list/get AI budgets, and AI usage/status summaries.
  AI create/update/delete mutations remain REST/CLI-only in v1.0 because credential and budget
  mutation through MCP is not needed for the S11 agent-usability exit.
- **Audit:** MCP tools call service functions rather than storage directly, so mutation audit
  should match REST by construction. S11 still tests at least one MCP mutation produces the same
  actor/resource/action semantics as the REST-equivalent service path.
- **Test matrix and exit flow:** #73 covers protocol negotiation, POST-only method handling,
  no-Origin allowed, disallowed-Origin denied, session/token mismatch, and per-request re-auth.
  #79 covers `fpat_` issuance, SHA-256 hash lookup, active/disabled/rotated token behavior, and
  agent kind loading. #74 covers static registry parity, registry-vs-service authz drift,
  `tools/list == executable`, and MCP mutation audit parity. #78 covers live `api_tools` reads,
  per-tool enable/disable, routing-resolution failure, Envoy execution, and republish staleness.
  #77 covers CLI status/connections/enable/disable against real endpoints. #76 owns the exit E2E:
  an MCP client initializes, lists tools, completes one control-plane workflow through `cp_*`,
  calls one dynamic `api_*` tool through Envoy, proves cross-team list/call denial, proves
  mid-session agent/grant revocation fails on the next call, and proves spec republish updates
  tool visibility.
- **Status:** decided for S11a / #75 (2026-06-17). This overrides any older spec text that implies
  v1 scope strings, v1 tool counts, cached MCP authz sessions, separate v1 route exposure columns,
  or already-existing shared declaration generation.
