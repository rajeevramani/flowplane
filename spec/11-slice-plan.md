# 11 — Implementation Slice Plan

Ordered vertical slices. Every slice ends green (`cargo fmt && cargo clippy --all-targets
--all-features -- -D warnings`, full tests incl. real-PostgreSQL integration), updates
PROGRESS/DECISIONS, and is committed+pushed. From slice 7 onward, **every API capability ships
its CLI command in the same slice**. Spec coverage column proves 100% of Phase 0 surface is
owned by some slice.

| # | Slice | Covers |
|---|---|---|
| 1 | Skeleton & quality gates | 10 §2,§8,§10 |
| 2 | Identity, teams, authz backbone | 05; 08a §1–3 |
| 3 | Domain + storage for gateway resources, events/outbox | 03; 10 §3 |
| 4 | REST API core + OpenAPI generation | 01; 10 §9 |
| 5 | xDS: IR pipeline, ADS, snapshots, mTLS identity | 04; 10 §5 |
| 6 | Secrets & SDS, proxy certs, dataplane lifecycle | 04; 08a §5 |
| 7 | CLI core | 12; 07 fates |
| 8 | Learning config-first (capture → spec → publish → tools) | 06 §1–8; 10 §3,§6 |
| 9 | Learning traffic-first (discover → generate → approve) | 06 §9; 10 §6 |
| 10 | AI gateway | 09 §2; 10 §7 |
| 11 | MCP server + tools | 02; 10 §9 |
| 12 | Hardening & production readiness | 08a §4 all; 10 §10–11 |

## Slice details & exit criteria

**S1 — Skeleton.** Workspace per 10 §2; config loading (env-first server config, validated);
error taxonomy + `{code,message,hint,request_id}` envelope; sqlx migrations runner;
`/healthz`+`/readyz`; observability foundation per 10 §8a (JSON `tracing` logging with
request_id/team_id fields + redaction boundary, OTel tracing init + W3C propagation,
Prometheus `/metrics` endpoint); native-TLS-capable API listener (plaintext = explicit opt-in
with warning, 10 §4a); CI: fmt, clippy `-D warnings`, tests, sqlx-offline check, deny(unwrap),
cargo audit + cargo deny. **Exit:** server boots against fresh Postgres, health green, a request's
request_id appears in error body + log line + trace, CI green on a PR.

**S2 — Identity & tenancy backbone.** Orgs/teams/users/agents/memberships/grants schema
(`team_id` naming, composite org/team FKs); Zitadel JWT validation + JWKS cache + dev-mode mock
OIDC + seeding; `check_resource_access` as table-driven pure function (property tests vs
spec/05 §3.1 decision table); `TeamScope`-typed repository pattern established; audit writer
(incl. denials); per-tenant write throttle; bootstrap flow (one-shot expiring token); dev-mode feature+runtime gating per 10 §4a. **Exit:**
authz property tests pass incl. the three §3.2 invariants; cross-org returns 404; audit rows
for denials; integration tests against real Postgres.

**S3 — Gateway domain + storage + events.** Outbox rows carry trace context (10 §8a) so
config-change traces span the async hop. Cluster/RouteConfig/VirtualHost/Route/Listener/
Filter domain types + validation (carry spec/03 §7 business rules); normalized schema (no
JSON+projection duals); per-team name uniqueness; optimistic locking everywhere; outbox table +
dispatcher (LISTEN/NOTIFY + poll fallback, persisted cursors, SKIP LOCKED); quotas. **Exit:**
concurrent-update test yields 409 not lost write; outbox survives kill -9 mid-publish with no
event loss (test); quota enforcement test.

**S4 — REST API core.** All gateway-resource endpoints + org/team/agent/grant endpoints +
pagination (cursor) + filtering, per spec/01 conventions with v2 fixes (PATCH everywhere,
idempotent creates via per-team names, uniform list envelope); OpenAPI generated from route
declarations; CI parity gate router↔doc = 100%. **Exit:** OpenAPI diff vs
`01-…v1-openapi.json` produced and every difference recorded in DECISIONS; agent-usability
smoke: scripted client driven only by the generated doc completes CRUD journey.

**S5 — xDS.** IR pipeline; ADS SOTW per-team/per-type versioned snapshots; make-before-break
ordering; ACK/NACK with per-resource quarantine + `degraded` status; real EDS; deterministic
encoding (BTreeMap); mandatory mTLS + SPIFFE cert registry binding; revocation kills streams;
event-driven rebuild (no polling); NACK/warming surfaced via API. Filter catalog: the 16 v1
filter types re-specced through IR; per-route overrides. **Exit:** live Envoy E2E (docker):
boot, join, route traffic; NACK quarantine test; restart CP under churn → Envoy converges, no
orphan resources; cross-team isolation test on xDS (team A's dataplane never sees B's
resources, by construction test + adversarial cert test).

**S6 — Secrets/SDS + dataplanes.** Encrypted-at-rest secrets (KEK + rotation procedure),
write-only API, rotate; external references (env/Vault backends); SDS delivery; proxy-cert
issue/revoke (rate-limited); dataplane registration + bootstrap generation + status;
`fp-agent` warming reports + telemetry relay + heartbeats per D-007 (per-message team
binding); per-team stats aggregation powering `/stats/*` and dataplane liveness. **Exit:** TLS
listener E2E with SDS rotation without restart; revoked cert stream-kill test; secret never
appears in logs/responses (grep test); stats E2E (traffic through Envoy → agent relay →
`flowplane stats overview` shows it; kill agent → dataplane marked stale, no CP errors).

**S7 — CLI core.** Framework per spec/12 §1–4: contexts/auth (PKCE+device-code), precedence
D-005, output engine (table/json/yaml/wide, TTY detection), error rendering + exit codes,
completions, `--dry-run` plumbing, `apply` for all kinds with `--diff`; commands for everything
S2–S6 shipped. **Exit:** CLI E2E journey (login → team → cluster → listener → secret →
dataplane → traffic); transcript tests assert exact output shapes; every S2–S6 endpoint has a
command (coverage test enumerates OpenAPI ops vs CLI calls).

**S8 — Learning, config-first.** ApiDefinition + SpecVersion + lifecycle; capture sessions
scoped to team listeners (Envoy-side ALS filters); ALS+ExtProc ingest with per-message team
binding; worker pipeline (bounded, batched, sharded); inference/aggregation/path-templating
(v1 algorithms + 06 §10–11 fixes incl. frequency thresholds, cardinality caps, TTL'd
intermediates); spec review/publish/diff; MCP toolset generation on publish (tools exist as
data; serving comes in S11); `expose`/`api create --from-openapi`. CLI: `api *`, `learn *`.
**Exit:** config-first E2E from CLI per definition-of-done (create → traffic → learned →
publish → tools rows exist → delete upstream → zero orphans); cross-team capture adversarial
test (A's broad session never receives B's traffic); poisoning tests (header flood, path
explosion capped).

**S9 — Learning, traffic-first.** Discovery capture (opt-in listener); candidate ApiDefinition
clustering; `route generate --from-spec` planner (shared with import) with `--dry-run` plan ==
applied result guarantee; approval gating (nothing < `reviewed` touches config). CLI:
`learn discover *`, `route generate`. **Exit:** traffic-first E2E per definition-of-done
(unmatched traffic → spec → dry-run preview matches creation → traffic flows → tools
published); approval-bypass adversarial test fails closed.

**S10 — AI gateway.** AiProvider/AiRoute/AiBudget/AiUsage; OpenAI chat-completions as the
canonical v1.0 IR; OpenAI/OpenAI-compatible passthrough providers first, with native
Anthropic/Bedrock/Vertex translators demand-pulled behind the trait seam; Flowplane-owned AI
ExtProc using S6 secret references fetched over the authenticated control channel; token
metering incl. streaming (force-`include_usage` when needed and strip synthetic usage chunks);
append-only usage events + `ai usage` query surface; shadow + enforcing budgets settled through
atomic Postgres counters; AI route materialization through existing gateway primitives; priority
failover before first response byte. CLI: `ai *`. **Exit:** AI E2E per definition-of-done
(mock OpenAI-compatible provider: credential → routed traffic → per-team tokens tracked →
budget trips → pre-byte failover engages); budget isolation tests for team A vs team B and
same-team concurrent settlement; usage metrics exported.

**S11 — MCP server + tools.** Streamable HTTP per spec/02 (sessions bound to token+team,
origin allowlist enforced, list==executable); cp/ops tool registry generated from the shared
declarations (no phantom entries — parity gate); gateway `api_*` tools from published
SpecVersions, refreshed by events; structured tool errors with recovery hints. CLI: `mcp *`.
**Exit:** MCP agent-usability E2E — an MCP client completes a full workflow using only
tools/list descriptions; cross-tenant tool access adversarial suite; tool staleness test (spec
republish regenerates).

**Depth knobs (pre-agreed, recorded here so schedule pressure cuts depth, never capability or
security):** S5 filter catalog may ship as the ~8 demonstrably-used filters with the rest
following behind the same IR seam; S10 non-OpenAI translators trail until a user pulls them;
external Envoy RLS/Redis budget enforcement trails the Postgres-backed S10 counter unless
explicitly re-sized. The loop, tenancy, and the adversarial exits are never the knob.

**S12 — Hardening & v1.0.** Observability completion per 10 §8a (alert pack over the metric
families fixed in S1–S11, dashboards-as-docs, GenAI semconv verification); failure-mode suite (kill Postgres mid-write, restart Envoy, restart CP under
load — no loss/orphans/panics); load sanity; full adversarial pass per 08a §4 across REST, MCP,
CLI, learned specs, generated routes, xDS; `docs/production-readiness.md`; operator docs
(separate CP and DP-bundle install/upgrade guides incl. bare-metal/VM/compose/ECS/K8s parity
per D-004 and 10 §10.1, config reference,
runbook, backup/restore drill, CLI workflow guide); scripted seeded demo walkthrough
(first-contact → full loop → MCP client, doubles as smoke test); `REWRITE-REPORT.md`; tag
`v1.0.0`.
**Exit:** definition-of-done list in the mission prompt — every box checked.

## Coverage check

01→S4(+S7); 02→S11; 03→S2,S3; 04→S5,S6; 05→S2; 06→S8,S9; 07 fates→S7–S11 (each workflow's
owning slice), cuts ratified in DECISIONS; 08 §9 table→S1–S5; 08a→S2 (gate), S5/S6 (xDS,
secrets), S8/S9 (capture), S12 (adversarial pass); 09→S5 (IR), S10 (AI), S12 (semconv);
10→S1–S3 (skeleton/events), all; 12→S7+every slice after. Workflows with no owning slice:
none.
