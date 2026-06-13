# Flowplane v2 — Progress

Resumable state for the rewrite. On session start: read this file, continue the next unchecked item.
Rules recap: v1 at `/tmp/flowplane-v1` (cloud) is read-only reference (clone from
`https://github.com/rajeevramani/flowplane.git` if missing). Never port code verbatim. Every
architectural decision goes in `DECISIONS.md`; founder questions in `QUESTIONS.md` (always with a
recommendation). Commit+push at every green checkpoint.

**Checkpoint gates:** stop and notify the founder at end of Phase 0 (review of 08, 08a, 09) and end
of Phase 1 (architecture + slice plan). Between gates, do not wait.

## Phase 0 — Behavioral spec extraction (no v2 code)

- [x] Clone v1 read-only, create PROGRESS/DECISIONS/QUESTIONS scaffolding
- [x] `spec/00-system-overview.md` — what Flowplane is, subsystems, data flow
- [x] `spec/01-api-contract.md` — every REST endpoint + exact OpenAPI artifact + drift analysis
- [x] `spec/02-mcp-tools.md` — MCP server, 82 static tools + dynamic api_*, authz, transport
- [x] `spec/03-domain-model.md` — 36 tables from 100 migrations, invariants, isolation gaps
- [x] `spec/04-xds.md` — ADS/ALS/ExtProc/diagnostics, snapshot model, 16 filters, mTLS identity
- [x] `spec/05-auth.md` — identity model, JWT flows, check_resource_access decision table
- [x] `spec/06-learning.md` — pipeline end to end, traffic-first gap analysis, capture security
- [x] `spec/07-cli-and-workflows.md` — v1 CLI surface + UI workflow inventory with v2 fates
- [x] `spec/09-prior-art.md` — Envoy Gateway/AI Gateway survey, token metering, borrow/reject calls
- [x] `spec/08a-security-and-tenancy.md` — threat model, isolation inventory, abuse cases, v2 requirements
- [x] `spec/08-architecture-critique.md` — v1 critique, loop seams trace, change-difficulty index
- [x] Phase 0 exit: all specs done → **STOPPED at founder review gate (08, 08a, 09)**

## Phase 1 — Target architecture (after founder gate)

- [x] `spec/10-v2-architecture.md` — workspace layering, ApiDefinition aggregate, lifecycle, outbox events
- [x] `spec/12-cli-design.md` — command tree, output/error contracts, transcripts (both loop directions)
- [x] `spec/11-slice-plan.md` — 12 slices with exit criteria + 100% coverage check
- [x] Phase 1 exit → **STOPPED at founder gate (10, 11, 12 review)**

## Phase 2..N — Implementation (after founder gate; details in spec/11)

- [x] S1 Skeleton & quality gates
  - [x] S1.1 workspace + rust-toolchain + fp-domain error taxonomy + ids
  - [x] S1.2 fp-core config (env>file>defaults, validated)
  - [x] S1.3 fp-storage pool + migrations runner (+ 0001)
  - [x] S1.4 fp-api: healthz/readyz, request-id middleware, error envelope, /metrics
  - [x] S1.5 observability: JSON logs w/ request_id + trace_id; OTel layer always on; OTLP export when configured; W3C traceparent honored
  - [x] S1.6 native-TLS API listener + insecure opt-in warning; TLS smoke in CI
  - [x] S1.7 flowplane bin: serve + db migrate; API graceful shutdown (xDS task drain wired in S5.6/S5.4 — see serve.rs `xds_shutdown_signal` + bounded drain)
  - [x] S1.8 CI: fmt, clippy -D warnings, tests w/ Postgres, cargo-deny (advisories + licenses + bans; no separate `cargo audit` step)
  - [x] S1 exit: request_id in error body + log + trace; traceparent inherited; boots on fresh PG; TLS verified
- [x] S2 Identity, teams, authz backbone
  - [x] S2.1 schema 0002 (orgs/teams/users/agents/memberships/grants/audit/bootstrap) + domain types
  - [x] S2.2 authz decision engine (pure, table-driven, exhaustive invariant tests vs spec/05 §3.1)
  - [x] S2.3 OIDC JWT validation (provider-agnostic, JWKS cache) + dev issuer (same validation path) + triple gating + seeding
  - [x] S2.4 TeamScope pattern, identity repos + principal loader, audit writer (success + authn-failure paths; `AuditEntry::denial` row primitive present + storage-tested, but NOT yet wired into service-layer authz denials — see Open Risk R2), keystone tenancy integration tests
  - [x] S2.5 auth middleware, whoami, OIDC config, authn-failure audit, one-shot bootstrap flow (single-use + concurrency-safe via transaction advisory lock, live-verified)
  - [x] S2.6 per-tenant write throttle (org-keyed, fail-closed keying, tenant-isolation test)
  - [x] S2 exit: invariant tests green, cross-org denied pre-grant, authn-failure audit rows (service-authz denial auditing tracked as Open Risk R2), real-PG integration, live E2E (whoami + bootstrap)
- [x] S3 Gateway domain + storage + outbox events
  - [x] S3.1 outbox: events table, transactional append, dispatcher (LISTEN/NOTIFY + poll, SKIP LOCKED cursors, trace ctx), crash-redelivery test
  - [x] S3.2 gateway domain types + validation (cluster, listener, route-config w/ rewrite rules)
  - [x] S3.3 schemas 0004/0005, repos for all three resources, normalized reference tracking, optimistic locking, per-team uniqueness + port uniqueness
  - [x] S3.4 per-tenant quotas (framework + clusters limit, quota test)
  - [x] S3 exit: concurrent 409 + no lost update, transactional events, cross-org 404, quota, referential guards (cluster/rc deletion blocked with dependents named), no orphaned refs
- [x] S4 REST API core + OpenAPI generation (+ v1 contract diff)
  - [x] S4.1 gateway-resource endpoints (clusters/listeners/route-configs CRUD, If-Match revisions, uniform Page envelope)
  - [x] S4.2 OpenAPI generated from routes! registrations (single declaration site); /api-docs/openapi.json; parity pin test
  - [x] S4.3 HTTP CRUD integration test through real bearer auth (201/409/400-hint/409-revision/200/204/404 envelopes)
  - [x] S4.4 team/member/grant endpoints + org CRUD/member endpoints (32 ops total); agents endpoints deferred to S11 with the MCP agent auth path (one coherent unit)
  - [x] S4.5 contract diff recorded (D-010); agent smoke PASSED (doc-only workflow incl. If-Match discovery); `flowplane openapi` dump command
  - [x] S4 milestone ping to founder (Q-007) — sent 2026-06-12
- [x] S5 xDS: IR pipeline, ADS, mTLS, quarantine
  - [x] S5.1 envoy-types translation: cluster (sorted endpoints, explicit TLS, HC/CB/outlier), route-config (exact/prefix/template, order-preserving), listener (HCM+RDS over ADS); determinism tests
  - [x] S5.2 per-team snapshot cache: outbox-driven rebuilds, per-type versions with byte-diff suppression, team isolation test, watch-channel change signal
  - [x] S5.3 ADS SOTW server: subscribe/ACK/NACK state machine, live pushes from snapshot watch, make-before-break type order, honest delta-unimplemented; gRPC stream integration tests
  - [x] S5.4 mTLS + SPIFFE cert registry binding + revocation stream-kill
    - [x] migration 0006: dataplanes + proxy_certificates (TIMESTAMPTZ, composite FKs,
          globally-unique spiffe_uri); repos with fail-closed active-cert lookup in SQL
    - [x] services: create_dataplane / register_certificate / revoke_certificate
          (tx = row + event + audit; events: dataplane.created, proxy_certificate.{registered,revoked})
    - [x] fp-xds: serve_mtls (tonic ServerTlsConfig + client CA), SPIFFE URI SAN extraction
          (x509-parser), CertRegistryResolver (full-URI registry binding; SAN team text and
          node.id never trusted), revocation broadcast bus kills live streams (fail-closed on lag)
    - [x] serve wiring: xds_tls configured → mTLS listener any mode; else dev plaintext; else
          listener off with warning (D-011); FLOWPLANE_XDS_TLS_{CERT,KEY,CLIENT_CA} all-or-none
    - [x] integration tests over real TLS (openssl-minted PKI): registry team binding wins over
          SAN/node-id claims; mid-stream revocation kill + reconnect rejection; unregistered,
          expired, and certificate-less connections rejected
  - [x] S5.5 ACK/NACK + per-resource quarantine + degraded status surfaced
    - [x] snapshot cache holds named resources + one generation of history; NACK quarantines
          exactly what changed (serves last-good bytes, or holds out new resources); operator
          fix (any byte change) clears quarantine; first-generation NACKs persist-only (no
          blanket quarantine); corrected set pushed on the same stream
    - [x] migration 0007 xds_nack_events + repo (stream-side insert, org resolved in SQL);
          GET /api/v1/teams/{team}/xds/nacks (Stats:Read; 33 documented ops)
    - [x] tests: quarantine semantics unit test; live-gRPC NACK → corrected push →
          persistence → fix-rejoins; metric fp_xds_quarantined_resources_total
  - [x] S5.7 real EDS: IP-endpoint clusters reference EDS over ADS (hostname clusters stay
        STRICT_DNS inline — Envoy never DNS-resolves EDS endpoints); ClusterLoadAssignment is a
        fourth snapshot type with the same quarantine machinery; make-before-break order
        CDS→EDS→RDS→LDS; endpoint churn bumps ONLY the endpoints version (test-pinned); live
        Envoy E2E re-passed with EDS warming + post-restart pure-EDS endpoint switch
  - [x] S5.8 filter catalog: the 16 v1 filter types re-specced through IR + per-route overrides
        (spec/04 §4; v2 = typed IR on listener/route specs, translated at build time — NO v1-style
        post-hoc protobuf surgery)
    - [x] S5.8a domain: `HttpFilterSpec` {cors, local_rate_limit, header_mutation,
          health_check, compressor} + validation per spec/04 §4.1; `ListenerSpec.http_filters`
          chain (order semantic, one per type); `filter_overrides` on VirtualHost AND RouteRule
          (tagged enum: Disable | Cors policy | LocalRateLimit replace; health_check is
          listener-only by construction, one override per type per scope)
    - [x] S5.8b translate: chain assembly (declared order, router auto-appended, disabled flag
          carried); typed_per_filter_config emitted on vhosts/routes (Disable →
          route.v3.FilterConfig{disabled}, Cors → cors.v3.CorsPolicy, LocalRateLimit → same
          type URL as chain); translation + override decode tests. PROTOCOL FIX found by the
          live E2E: a request echoing the last nonce with CHANGED resource_names is a
          subscription update and is now answered (was swallowed as ACK → warming listener
          stalled on RDS; regression test over real gRPC)
    - [x] S5.8c auth-grade filters DONE: jwt_auth (providers w/ remote-JWKS-via-named-cluster
          OR inline JWKS, requirement_map + rules, allow_missing/any_of; ONE filter per chain
          so v1's provider-merge is unnecessary — D-012), ext_authz (gRPC, fail-closed default),
          rbac (Allow/Deny, header/url_path/port permissions, source-CIDR/header principals).
          Per-route: jwt is reference-only (PerRouteConfig.requirement_name); all three
          disablable per-scope. Domain + translation + live-Envoy ACK/enforce tests.
          REMAINING {rate_limit (RLS), rate_limit_quota, ext_proc, oauth2, credential_injector,
          custom_response, mcp, wasm} — the SDS/secrets-coupled and external-service ones;
          deferred into S6/S10/S11 where their dependencies land (record per slice).
    - [x] S5.8d live Envoy E2E phase 4 + 5: phase 4 local_rate_limit + header_mutation (429
          enforced, /quiet exempt via per-route Disable, header applied); phase 5 jwt_auth
          (allow_missing flows) + rbac (DENY enforced on /blocked) — real Envoy ACKs both.
          Found+fixed: rbac type URL must be all-caps `.v3.RBAC` (prost type is `Rbac`), now
          unit-pinned.
  - [x] S5.6 live Envoy E2E: join, route traffic, restart convergence, cross-team isolation
    - [x] xDS pipeline wired into `flowplane serve` (outbox consumer + dev-mode plaintext ADS listener)
    - [x] target Envoy line: **1.37.x** (latest patch 1.37.4) — one release before the 1.38
          stable line (D-013); docker tag `envoyproxy/envoy:v1.37-latest`, sandbox binary 1.37.4
    - [x] `scripts/e2e-envoy.sh` (real Envoy via docker, or Tetrate static binary fallback) — three
          phases all PASSED: ADS join + traffic; CP restart with snapshot prime-from-DB (found and
          fixed the empty-snapshot-wipes-dataplane restart bug); cross-team isolation (config_dump
          clean + foreign listener port closed)
    - [x] `SnapshotCache::prime_all` + regression test (fresh cache primed from DB byte-identical
          to the event-driven one)
- [x] S6 Secrets/SDS + dataplane & proxy-cert management surface (REST/CLI) + fp-agent telemetry
  - NOTE: the dataplane + proxy_certificate **internals** (migration 0006, repos, services,
    mTLS cert-registry binding, revocation) already shipped in S5.4 — do NOT rebuild them.
    S6's remaining scope: encrypted-at-rest secrets + SDS delivery; the REST/CLI surface for
    dataplane registration + bootstrap generation + cert issue/revoke; fp-agent telemetry
    relay + heartbeats (D-007); per-team stats aggregation.
  - [x] S6.1 dataplane REST surface: list/create/get registered dataplanes at
        `/api/v1/teams/{team}/dataplanes[/{name}]` over the existing S5.4 service internals;
        OpenAPI operation pin updated to 36.
  - [x] S6.2 dataplane bootstrap generation + proxy-cert issue/revoke REST surface
    - [x] S6.2a proxy-certificate REST registry surface: list/register/revoke at
          `/api/v1/teams/{team}/proxy-certificates[/{serial}/revoke]`, backed by the S5.4
          registry/revocation services; OpenAPI operation pin updated to 39.
    - [x] S6.2b dataplane Envoy bootstrap generation at
          `/api/v1/teams/{team}/dataplanes/{name}/envoy-config` with explicit xDS mTLS
          cert/key/CA file paths; OpenAPI operation pin updated to 40.
    - [x] S6.2c actual certificate/key issuance: local CA-backed
          `/api/v1/teams/{team}/proxy-certificates/issue` mints a SPIFFE SAN client cert/key,
          registers the cert binding, and returns PEM material only in the one-time issue response.
  - [x] S6.3 encrypted-at-rest secrets + write-only API + rotation: AES-256-GCM
        `FLOWPLANE_SECRET_ENCRYPTION_KEY`, metadata-only read/list/get responses, create/rotate
        REST surface at `/api/v1/teams/{team}/secrets[/{name}/rotate]`, OpenAPI operation pin 44.
  - [x] S6.4 SDS delivery over ADS + live rotation E2E
    - [x] S6.4a SDS resource type wired into ADS snapshots: active encrypted secrets decrypt
          into Envoy `tls.v3.Secret` resources, SDS subscriptions are name-filtered
          (empty subscription returns zero secrets), and secret upserts trigger xDS rebuilds.
    - [x] S6.4b live Envoy SDS rotation E2E: listener `tls_context` can reference SDS TLS
          certificate secrets over ADS; `scripts/e2e-envoy.sh` proves HTTPS serves from an
          SDS secret and rotates to a new cert without restarting Envoy.
  - [x] S6.5 fp-agent telemetry relay, heartbeats, liveness, per-team stats aggregation
    - [x] S6.5a dataplane telemetry/liveness foundation: heartbeat/config-verify timestamps
          and request/error/warming counters, REST telemetry ingest, and
          `/api/v1/teams/{team}/stats/overview` aggregation.
    - [x] S6.5b diagnostics gRPC service mounted beside ADS with certificate-registry
          dataplane binding; `fp-agent` sidecar keeps one outbound diagnostics stream, uses a
          bounded report queue, exposes loopback health, scrapes Envoy admin stats, and relays
          heartbeat deltas (mTLS or dev plaintext); real mTLS integration test verifies wrong
          dataplane claims are rejected and accepted heartbeats update live stats.
- [x] S6 exit: SDS rotation E2E passed in S6.4b; revoked cert stream-kill covered by S5.4;
      secret values are write-only over HTTP; stats relay covered by S6.5b.
- [x] S7 CLI core (+ commands for S2–S6)
  - [x] S7.1 CLI foundation: shared global flags/config precedence (`--server`,
        `--org`, `--team`, contexts, token file/env), REST client with bearer and
        `X-Flowplane-Org`, table/json/yaml-ish output, API error rendering, dry-run plan
        output, and typed commands for shipped S2–S6 REST endpoints (org/team/member/grant,
        cluster/listener/route-config, secrets, dataplanes/certs, stats, xDS NACKs).
        OpenAPI-vs-CLI path coverage test pins that every current secured S2–S6 path has a
        CLI template.
  - [x] S7.2 CLI UX pass: real `clap_complete` shell completions; safer
        `auth login --token-stdin`; aligned table output with stable high-signal column order;
        mutation commands print concise created/updated/deleted summaries unless JSON/YAML is
        requested; `config get-contexts` renders aligned context rows and remains compatible with
        v1-era scalar config fields (`base_url`, `org`, `team`, `token`) so existing local
        installs do not fail on first run.
  - [x] S7.3 Declarative apply/diff: `flowplane apply -f manifest.json --diff` plans JSON
        manifests for clusters/listeners/route-configs, write-only secrets, and dataplanes;
        non-diff apply creates missing resources and uses live revisions for gateway PATCH.
  - [x] S7.4 Device-code auth: `auth login --device` discovers OIDC provider metadata,
        starts the OAuth device-code flow, polls token exchange with pending/slow_down handling,
        stores `id_token` when present (else access token), and promotes `oidc_issuer`,
        `oidc_client_id`, and `oidc_scope` to first-class CLI config/env inputs. `--device-code`
        is accepted as the spec/v1-compatible alias.
  - [x] S7.5 Transcript polish: top-level help now carries the S7 happy-path examples (device-code
        login, context setup, `apply --diff`, resource list), and parser tests pin those command
        forms so future CLI refactors cannot silently break the documented workflows.
  - [x] S7.6 Browser PKCE loopback: configured OIDC defaults to PKCE login, `auth login --pkce`
        prints the authorization URL, listens only on an explicit loopback callback URL, validates
        state, exchanges the code with S256 verifier, and stores `id_token` when present (else
        access token). `--device-code` remains the headless path.
- [ ] S8 Learning config-first
- [ ] S9 Learning traffic-first
- [ ] S10 AI gateway
- [ ] S11 MCP server + tools
- [ ] S12 Hardening, production readiness, v1.0.0 tag

## Known Corrections / Open Risks

Cold-start handoff safety: items surfaced by review that the checklist above must not paper
over. "RESOLVED" items were fixed in the same review pass (with tests); "OPEN" items are real
and scheduled — read these before trusting a green checkbox.

- **R1 — Bootstrap concurrent-init race — RESOLVED.** `initialize` used `FOR UPDATE` on a
  not-yet-existing `instance_meta` row (locks nothing) + `ON CONFLICT DO NOTHING` on the
  marker, so two concurrent calls with two different valid tokens could both commit (two
  orgs, lost marker). Fixed with a transaction-scoped advisory lock serializing the critical
  section; regression test `concurrent_initialize_*` asserts exactly one winner.
- **R3 — Team create/delete transaction boundary — RESOLVED.** `create_team`/`delete_team`
  inserted/deleted the row in a separate transaction *before* the event+audit tx, so a
  mid-call crash could leave a team with no `TeamCreated`/`TeamDeleted` event or audit row
  (transactional-outbox invariant violated). Fixed with `identity::create_team_tx` /
  `delete_team_tx`; the service now does row+event+audit in one transaction (pool-based
  wrappers retained for test fixtures).
- **R4 — xDS task graceful shutdown — RESOLVED.** xDS server tasks were spawned with
  `std::future::pending()` shutdown futures (never drained); only the API drained. Fixed:
  `serve.rs::xds_shutdown_signal` feeds each task a real shutdown future off the watch
  channel, and shutdown now awaits all xDS task handles with a 10s bound.
- **R2 — Service-layer authz denials are NOT audited — RESOLVED.** Authn *failures* are audited
  (auth middleware) and the `AuditEntry::denial` row primitive exists + is storage-tested,
  but no service/middleware path wrote a denial row when `check_resource_access` denied.
  Fixed: every service `authorize()` helper that wraps `check_resource_access` now writes a
  best-effort `authz.denied` audit row with request id, actor, resource, action, org/team, and
  reason before returning the existing 403/404 error.
- **R5 — multi-org users require explicit request org context — RESOLVED.** Founder direction is
  to allow a human user to belong to multiple orgs, so `org_memberships` must keep only
  `UNIQUE (user_id, org_id)` and must **not** gain a `UNIQUE (user_id)` constraint. The v2
  selector contract is `X-Flowplane-Org` for REST/MCP and `--org` / active config context for
  CLI. Fixed: principal loading validates a selected org, infers only exactly one active
  non-platform org, and otherwise leaves no active org for tenant-scoped APIs to fail closed;
  path-scoped org member APIs validate membership against the path org directly; `/auth/whoami`
  returns all active memberships so the CLI can build/select contexts without platform-admin
  org listing.
- **R6 — Email resolution is global and non-unique — RESOLVED.** `identity::find_user_by_email`
  selects across ALL orgs with `LIMIT 1`, and `users.email` has no `UNIQUE` constraint — so
  add-member / add-grant by email can resolve a user in another org or silently pick one of
  several duplicates. Fixed: org member add rejects duplicate active global emails instead of
  picking one and also accepts immutable `subject`/`user_id` selectors; team member/grant add
  resolve email inside the selected org.
- **R7 — OIDC JWKS fetch holds the cache write-lock across an untimed network call — RESOLVED.**
  `refresh_keys` takes `cache.write()` *then* does the JWKS HTTP fetch while holding it, and
  `reqwest::Client::new()` sets no timeout — so a slow/hung IdP stalls every token validation
  (head-of-line blocking) indefinitely. Fixed: the OIDC client has a 5s request timeout,
  refreshes are single-flighted by a dedicated mutex, and the cache write lock is held only
  while swapping parsed keys.

## Notes

- v1 layout: main crate `src/{api,auth,cli,config,domain,errors,internal_api,mcp,observability,openapi,schema,secrets,services,storage,utils,validation,xds}`, plus `crates/{flowplane-agent,flowplane-docs-gen,flowplane-rls}`, `migrations/`, `ui/` (SvelteKit — feature inventory only), `filter-schemas/`, `proto/`.
- v1 version at clone: 0.2.10 (commit 3a510a4).
