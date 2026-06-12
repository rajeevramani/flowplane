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

- [ ] S1 Skeleton & quality gates
- [ ] S2 Identity, teams, authz backbone
- [ ] S3 Gateway domain + storage + outbox events
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
