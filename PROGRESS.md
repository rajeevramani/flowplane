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
- [ ] `spec/03-domain-model.md` — entities, invariants, team isolation, DB schema (`migrations/`)
- [ ] `spec/04-xds.md` — listener/route/cluster/filter generation, SDS/secrets
- [x] `spec/05-auth.md` — identity model, JWT flows, check_resource_access decision table
- [ ] `spec/06-learning.md` — learning pipeline end to end + traffic-first gap analysis
- [x] `spec/07-cli-and-workflows.md` — v1 CLI surface + UI workflow inventory with v2 fates
- [ ] `spec/09-prior-art.md` — Envoy AI Gateway + Envoy Gateway survey; AI-gateway requirements input
- [ ] `spec/08a-security-and-tenancy.md` — threat model, tenancy spec, authn/authz matrix, abuse cases
- [ ] `spec/08-architecture-critique.md` — v1 critique; trace the learning↔MCP↔xDS seams
- [ ] Phase 0 exit: all specs done → **STOP, notify founder for review gate**

## Phase 1 — Target architecture (after founder gate)

- [ ] `spec/10-v2-architecture.md` — modules, layering, loop integration design, lifecycle state machine
- [ ] `spec/12-cli-design.md` — command tree, global flags, error style, worked transcripts (both loop directions)
- [ ] `spec/11-slice-plan.md` — ordered vertical slices with exit criteria
- [ ] Phase 1 exit: slice plan covers 100% of Phase 0 surface → **STOP, founder gate**

## Phase 2..N — Implementation

Slices to be enumerated in `spec/11-slice-plan.md` (suggested shape: skeleton → auth/teams →
domain+storage → REST → xDS → secrets/SDS → CLI core → learning both directions → AI gateway →
MCP server → hardening). This section gets one checkbox per slice once the plan is written.

## Notes

- v1 layout: main crate `src/{api,auth,cli,config,domain,errors,internal_api,mcp,observability,openapi,schema,secrets,services,storage,utils,validation,xds}`, plus `crates/{flowplane-agent,flowplane-docs-gen,flowplane-rls}`, `migrations/`, `ui/` (SvelteKit — feature inventory only), `filter-schemas/`, `proto/`.
- v1 version at clone: 0.2.10 (commit 3a510a4).
