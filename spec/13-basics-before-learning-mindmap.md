# 13 — Core Gateway Parity Before Learning

Purpose: align the practical "can I run the CP, start a dataplane, expose traffic, and verify it"
work with `PROGRESS.md` before S8 learning continues. V1 is inspiration for the operator journey
only (`init`, `expose`, `dataplane up`, runbooks, smoke tests); V2 keeps the existing Rust
workspace boundaries, PostgreSQL source of truth, REST/CLI surfaces, xDS snapshot cache, and
agent telemetry design. Architecture integrity rules for the seams and domain boundaries are in
`spec/14-architecture-integrity.md`.

Principle: **V1 defines the user outcome; V2 defines the architecture and experience.** The goal is
not feature-for-feature porting. The goal is core gateway workflow parity with a simpler, safer,
faster V2-native UX.

## V1 vs V2 Gap

V1 is ahead in operability and first-user experience. V2 is ahead in the control-plane foundation:
tenant isolation, mTLS registry binding, xDS/SDS internals, telemetry direction, and cleaner
workspace boundaries. The pre-S8 work is to close the product workflow gap without porting V1's
implementation.

| Area | V1 has | V2 has | Gap before S8 |
| --- | --- | --- | --- |
| First run | `flowplane init` brings up PG + CP + Envoy + agent | Manual `flowplane serve` works with dev mode | Documented/manual path first; later `stack up` |
| Dataplane bootstrap | Generated bootstrap wrapped by CLI/scripts | REST `/envoy-config` + CLI call exist | Better CLI naming, `--out`, explicit dev/prod bootstrap modes |
| Dataplane lifecycle | `dataplane up/down`, bundle `dataplane-up.sh` | E2E script proves Envoy can connect | Decide manual Envoy vs V2-native lifecycle command |
| Traffic shortcut | `flowplane expose` creates cluster/route/listener | Low-level CRUD exists | Add `expose`/`unexpose` over V2 services |
| Docs | README, quickstarts, dataplane runbook | Specs and progress notes | Add operator runbook/README path |
| Validation | Smoke tests cover the full CLI journey | API/unit/e2e pieces exist | Add transcript/e2e for CP + DP + expose + curl |
| Packaging | Platform evaluation bundle | No bundle yet | Defer until wrappers sit on a clean CLI contract |

## Core Gateway Capability Parity

S7.7 is the immediate **deployment and route-to-traffic gate**. It should not absorb every remaining
gateway feature from V1. The table below separates pre-S8 blockers from broader parity items.

| Capability | V1 outcome | V2 status | Focus next | Progress anchor |
| --- | --- | --- | --- | --- |
| Dev CP bring-up | `flowplane init` starts an immediately usable local stack | Manual `serve` path works; no README/runbook | **Pre-S8 blocker:** document current manual path; later decide `stack up` | S7.7a, S12 packaging |
| Production CP bring-up | Platform bundle starts prod-mode evaluation stack | Core server config exists; no bundle/runbook | Document non-dev mTLS/env contract after dev path; full packaging later | S7.7a, S12 |
| Dataplane registration | Default dataplane provisioned in bundle/dev flows | `dataplane create/list/get` exists | Keep; document expected dev/prod sequence | S6.1, S7.7a |
| Dataplane certs | Bundle mints certs and writes files | cert issue/register/revoke API/CLI exists | Document one-time PEM response and local file layout; improve CLI output only if needed | S6.2, S7.7a |
| Envoy bootstrap | CLI/scripts generate usable bootstrap | API/CLI exists, but naming and dev plaintext UX are rough | **Pre-S8 blocker:** `dataplane bootstrap`, `--out`, explicit dev/prod mode | S7.7b |
| Dataplane process | `dataplane up/down`, bundle scripts | e2e script only | **Pre-S8 blocker:** choose manual Envoy or V2-native lifecycle command | S7.7c |
| Route-to-traffic shortcut | `expose` creates cluster/route/listener | Low-level CRUD only | **Pre-S8 blocker:** implement `expose`/`unexpose` on V2 services | S7.7d |
| Gateway CRUD | cluster/listener/route CRUD | Implemented with V2 REST/CLI and xDS | Keep as substrate for `expose`; no parity work now | S3, S4, S5, S7 |
| xDS delivery | ADS, ACK/NACK visibility | Implemented with stronger quarantine + persistence | Surface in runbook and diagnostics path | S5.5, S7.7a/e |
| SDS/secrets | TLS secrets to Envoy | Implemented, stronger write-only API | Operator workflow docs; not a pre-S8 learning blocker unless used by examples | S6.3, S6.4, S12 |
| Agent telemetry/stats | Agent reports warming/status; stats commands | Foundation implemented; CLI has stats overview | Pre-S8: prove happy path or clearly document dev plaintext limitations | S6.5, S7.7e |
| Ops summary | `xds status`, `ops doctor`-style flow | NACKs/stats exist; no rich doctor | Not required before S8; record as S12 hardening unless needed for tests | S7.7e, S12 |
| Filters | Broad V1 filter catalog | V2 typed IR subset shipped; some filters deferred | Not part of core deployment parity; continue in owning slices | S5.8, S10, S11, S12 |
| Rate limiting | local/RLS workflows | local_rate_limit filter shipped; RLS/domain/policy deferred | Not a pre-S8 blocker; belongs with rate-limit/AI budget work | S5.8, S10/S12 |
| MCP gateway tools | api_* through Envoy | Deferred | Must wait for published specs/tools | S11 |
| Learning capture | learn from live traffic | Deferred | Must use the S7.7-proven route-to-traffic loop | S8/S9 |
| Packaging | release installer/platform bundle | None in V2 | Defer until CLI contract is clean; avoid premature V1 script port | S12 |

Pre-S8 blockers are intentionally narrow: CP runbook, bootstrap UX, dataplane lifecycle decision,
`expose`/`unexpose`, and workflow validation. Everything else is either already implemented enough
for learning, belongs to a later feature slice, or should wait for the V2 CLI contract to stabilize.

## Execution Breakdown

This table is the pre-S8 work queue. `PROGRESS.md` owns status; this spec explains why each item
exists and what "done" means.

| Progress item | Focus | User outcome | Implementation shape | Done criteria |
| --- | --- | --- | --- | --- |
| S7.7a | Dev runbook | A developer can manually start CP, authenticate CLI, start Envoy, create traffic, and troubleshoot failures | `README.md` plus `docs/dev-dataplane.md` or equivalent | Commands work from a fresh DB without reading source or `scripts/e2e-envoy.sh` |
| S7.7b | Bootstrap UX | A dataplane bootstrap is generated with one clear command and written to a file | Add `dataplane bootstrap` alias/rename over existing `/envoy-config`; add `--out`; make dev/prod mode explicit | Generated YAML works for dev plaintext and non-dev mTLS; errors explain missing cert/xDS inputs |
| S7.7c | Dataplane lifecycle | Operator knows the supported way to run Envoy in V2 | Decide manual local Envoy first vs V2-native `dataplane up/down/status`; do not port V1 compose internals | Decision recorded; chosen path documented or implemented; macOS path avoids Docker host-network dependency |
| S7.7d | Expose shortcut | One command turns an upstream into curlable traffic through Envoy | Implement `expose`/`unexpose` using V2 cluster/route-config/listener services and existing xDS propagation | `flowplane expose <url> --name demo` prints a listener port and curl hint; cleanup works |
| S7.7e | Workflow validation | The CP + DP + expose loop cannot regress silently | Add transcript/parser test and one live e2e/smoke path around the happy path and key diagnostics | Test proves CP start, DP bootstrap/connect, expose, curl, stats/NACK checks |

After S7.7e, S8 may resume. S8 should depend on the core gateway loop instead of carrying its own
bring-up logic.

## Mindmap

```text
Flowplane v2 product loop
├── 0. Operator-facing docs and runbooks
│   ├── Status: gap after S7
│   ├── Progress anchor: S7 follow-up, S12 operator docs
│   ├── Need: README quickstart for source/dev users
│   ├── Need: manual CP runbook
│   ├── Need: manual dataplane runbook
│   ├── Need: troubleshooting for token, org selector, xDS, Envoy admin, stats
│   └── Done when: a fresh session can run CP + DP + curl without reading source or e2e scripts
│
├── 1. Control plane local bring-up
│   ├── Status: implemented but not packaged
│   ├── Progress anchor: S1, S2, S5, S6, S7
│   ├── Existing: `flowplane serve`, dev mode seed, dev token log, Postgres migrations
│   ├── Need: one documented happy path
│   ├── Need: clearer token ergonomics after restart
│   ├── Need: `auth whoami` troubleshooting docs
│   └── Done when: CP starts manually and CLI auth works predictably
│
├── 2. Dataplane registration and bootstrap
│   ├── Status: APIs exist; UX is rough
│   ├── Progress anchor: S6.1, S6.2, S7
│   ├── Existing: dataplane CRUD REST/CLI
│   ├── Existing: `/dataplanes/{name}/envoy-config`
│   ├── Existing: cert issue/register/revoke
│   ├── Need: dev plaintext bootstrap path for local CP runs
│   ├── Need: CLI naming alignment (`dataplane bootstrap` or alias from `envoy-config`)
│   ├── Need: `--out` support for writing bootstrap YAML
│   └── Done when: one command emits a bootstrap usable by local Envoy in dev mode
│
├── 3. Dataplane process lifecycle
│   ├── Status: e2e script proves it; product CLI absent
│   ├── Progress anchor: S7 follow-up, S12 packaging
│   ├── Existing: `scripts/e2e-envoy.sh` with Docker/local Envoy fallback
│   ├── Need: V2-native `dataplane up/down/status` decision
│   ├── Option A: manual local Envoy first, documented
│   ├── Option B: CLI-managed local Envoy/agent stack like V1, but implemented fresh
│   ├── Need: macOS-friendly path that does not depend on Docker host networking
│   └── Done when: Envoy connects to xDS and appears in status without hand-written YAML
│
├── 4. Resource-to-traffic happy path
│   ├── Status: low-level resources exist; shortcut absent
│   ├── Progress anchor: S3, S4, S5, S7; proposed pre-S8 basics checkpoint
│   ├── Existing: cluster/listener/route-config CRUD
│   ├── Existing: xDS pushes resources and live Envoy E2E proves traffic
│   ├── Need: `flowplane expose <url> --name <name>`
│   ├── Need: port allocation or explicit `--port`
│   ├── Need: clear output with curl hint
│   ├── Need: `unexpose` or equivalent cleanup
│   └── Done when: upstream -> expose -> curl through Envoy works from CLI
│
├── 5. Observability and diagnostics baseline
│   ├── Status: partial
│   ├── Progress anchor: S5.5, S6.5, S7, S12
│   ├── Existing: xDS NACK API/CLI
│   ├── Existing: stats overview API/CLI, agent telemetry foundation
│   ├── Need: `dataplane status` or `ops status` user-facing summary
│   ├── Need: docs for Envoy config dump and CP logs
│   ├── Need: CLI hints that point to the next diagnostic command
│   └── Done when: a failed dataplane start has an obvious first diagnostic command
│
├── 6. Generated API contract and CLI coverage
│   ├── Status: route/CLI coverage tests exist for shipped paths
│   ├── Progress anchor: S4, S7
│   ├── Existing: OpenAPI generation, OpenAPI-vs-CLI path coverage
│   ├── Need: keep docs/CLI aligned when bootstrap/expose paths change
│   ├── Need: transcript tests for CP + DP + expose happy path
│   └── Done when: examples are pinned by tests, not just prose
│
└── 7. Learning readiness gate
    ├── Status: S8 planned but should pause until basics are usable
    ├── Progress anchor: S8.2, S8.3, S8.4
    ├── Requires: stable API definition/import surface
    ├── Requires: stable route binding and traffic path
    ├── Requires: dataplane capture injection target is observable
    ├── Requires: diagnostics can explain no samples / no traffic / NACKs
    └── Done when: learning can start from a known-good route rather than debugging bring-up
```

## Priority Order

1. Documentation first: write the manual path while the rough edges are visible.
2. Bootstrap UX: make the generated config match dev and production modes explicitly.
3. Dataplane lifecycle: choose manual-local first or CLI-managed `dataplane up`.
4. Expose shortcut: create the simplest route-to-traffic loop.
5. E2E transcript: pin the workflow so it does not regress.
6. Resume S8 config-first learning on top of that baseline.

## Open Questions

1. Should the immediate target be documented manual local Envoy, or a V2-native `dataplane up/down`
   command?
2. Should `flowplane expose` land before learning starts? Recommendation: yes.
3. Should the dev bootstrap endpoint generate plaintext xDS when `FLOWPLANE_DEV_MODE=true`, or
   should plaintext remain a CLI-only local convenience? Recommendation: API supports an explicit
   `mode=dev-plaintext` query only in dev mode, so the contract is testable and fail-closed.
