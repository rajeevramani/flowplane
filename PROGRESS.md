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
  - [x] S1.7 flowplane bin: serve + db migrate; graceful shutdown
  - [x] S1.8 CI: fmt, clippy -D warnings, tests w/ Postgres, cargo audit/deny
  - [x] S1 exit: request_id in error body + log + trace; traceparent inherited; boots on fresh PG; TLS verified
- [x] S2 Identity, teams, authz backbone
  - [x] S2.1 schema 0002 (orgs/teams/users/agents/memberships/grants/audit/bootstrap) + domain types
  - [x] S2.2 authz decision engine (pure, table-driven, exhaustive invariant tests vs spec/05 §3.1)
  - [x] S2.3 OIDC JWT validation (provider-agnostic, JWKS cache) + dev issuer (same validation path) + triple gating + seeding
  - [x] S2.4 TeamScope pattern, identity repos + principal loader, audit writer (incl. denials), keystone tenancy integration tests
  - [x] S2.5 auth middleware, whoami, OIDC config, authn-failure audit, one-shot bootstrap flow (live-verified)
  - [x] S2.6 per-tenant write throttle (org-keyed, fail-closed keying, tenant-isolation test)
  - [x] S2 exit: invariant tests green, cross-org denied pre-grant, denial+authn audit rows, real-PG integration, live E2E (whoami + bootstrap)
- [ ] S3 Gateway domain + storage + outbox events
  - [x] S3.1 outbox: events table, transactional append, dispatcher (LISTEN/NOTIFY + poll, SKIP LOCKED cursors, trace ctx), crash-redelivery test
  - [~] S3.2 gateway domain types: cluster DONE (validated business rules); listener/route-config NEXT
  - [~] S3.3 clusters: schema 0004, TeamScope repo, optimistic locking, per-team uniqueness DONE; listener/route-config NEXT
  - [x] S3.4 per-tenant quotas (framework + clusters limit, quota test)
  - [~] S3 exit tests for clusters vertical GREEN (concurrent 409 + no lost update, event emission, cross-org 404, quota); repeat for listener/route-config
- [ ] S4 REST API core + OpenAPI generation (+ v1 contract diff)
- [ ] S5 xDS: IR pipeline, ADS, mTLS, quarantine
- [ ] S6 Secrets/SDS, proxy certs, dataplanes
- [ ] S7 CLI core (+ commands for S2–S6)
- [ ] S8 Learning config-first
- [ ] S9 Learning traffic-first
- [ ] S10 AI gateway
- [ ] S11 MCP server + tools
- [ ] S12 Hardening, production readiness, v1.0.0 tag

## Notes

- v1 layout: main crate `src/{api,auth,cli,config,domain,errors,internal_api,mcp,observability,openapi,schema,secrets,services,storage,utils,validation,xds}`, plus `crates/{flowplane-agent,flowplane-docs-gen,flowplane-rls}`, `migrations/`, `ui/` (SvelteKit — feature inventory only), `filter-schemas/`, `proto/`.
- v1 version at clone: 0.2.10 (commit 3a510a4).
