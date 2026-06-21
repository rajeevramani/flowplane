# S1–S10 Build Certification Report

Status: **GO (Tier 0)** for S1–S10, with the residual items below routed to S12 and explicitly accepted. Date: 2026-06-16. Charter: #66. Functional backbone: #65. Branch: `claude/e2e-exhaustive` (historical certification branch; current default branch is `main`).

## Scope

S1–S10 of Flowplane v2: control plane, REST/CLI, xDS + real Envoy dataplane, filters/quarantine, secrets/SDS, dataplane + agent telemetry, config-first learning, traffic-first discovery, and the AI gateway (providers/routes/usage/budgets/failover). Not in scope: S11 (MCP), S12 (production packaging/hardening), non-OpenAI translators, UI.

## Method (independent reproduction)

All results below were reproduced on a clean environment — fresh Postgres + a real Envoy 1.37.4 — not taken from upstream claims. Guiding rule: a skipped test counts as **not tested**; every `cargo:` row was run with `FLOWPLANE_TEST_DATABASE_URL` + `FLOWPLANE_SECRET_ENCRYPTION_KEY` set so nothing silently skipped.

- `cargo fmt --check` — pass
- `cargo clippy --workspace --all-targets -- -D warnings` — pass
- `cargo test --workspace` (real Postgres) — pass (25 test binaries, 0 failures)
- `bash scripts/e2e-envoy.sh` (real Envoy) — pass, **5 consecutive runs**, 12 phases each, 0 known failures, redaction sweep green

## Coverage

Full traceability in `COVERAGE.md`: every S1–S10 capability and every abuse-matrix row maps to a passing `live:` phase, a named `cargo:` test, or a named `out-of-scope` boundary. Live phases (P1 basic ADS, P1a AI inject/failover/budget/usage, P1d streaming-failover boundary, P1e malformed-provider, P1b learning, P1c discovery, P2 CP-restart, P3 cross-team isolation, P4 local RL/header-mutation, P5 JWT/RBAC/ext_authz, P6 SDS rotation, P7 advanced parity + global-RLS ACK) plus the Tier-0 redaction sweep.

Crown-jewel results:
- **Multi-tenant isolation** — cross-org/team denial (`cargo` tenancy/gateway) + live xDS isolation (P3); cross-team budget/usage isolation under concurrency (`cargo:ai_budgets`).
- **Credential non-leakage** — `redaction_sweep` confirms no provider credential or API token in CP/Envoy logs, config dumps, access logs, or usage rows; per-phase config-dump checks too.
- **Control loop** — config→xDS→Envoy→traffic and CP-restart convergence (P1, P2), 5× stable.
- **AI budgets** — shadow non-blocking, enforcing 429 at request start, atomic concurrent settlement, usage attributed to the backend actually used (P1a + `cargo:ai_budgets`).
- **Capture safety** — redacted/bounded observations + poisoning/oversize handling (`cargo` + P1b); discovery SSRF guard (`cargo` + P1c).

## Defects found during certification

| ID | Severity | Finding | Status |
|---|---|---|---|
| D8 | Minor | Phase 7 global-RLS check flaked ~1/3 even after #64 (unbounded single-shot curl) | **Fixed** (consistent-snapshot + `--max-time`; 5× green) |
| D9 | Major | AI gateway 500 on `stream:true` (stale `content-length` after include_usage body rewrite + SSE forwarding gap) — **streaming, the primary LLM pattern, was broken; missed by all prior tests** | **Fixed** (#67, `80f86c7`; verified live by P1d + 18 fp-xds tests) |

Both were caught only by reproducing on a clean environment. D9 in particular was invisible to the prior suite because no test exercised `stream:true`.

(Earlier S10-review defects D1–D7 — credential SQL blocker, quota bypass, env-test race, etc. — were fixed and regression-tested during S10 review and are green here.)

## Residual risk (accepted for Tier 0, routed to S12)

- **R2 — service-layer authz-denial audit — fixed after certification (#69).** Denial enforcement already worked; #69 pins that service-layer authz denials emit `authz.denied` audit rows with request id, target org/team, resource, and reason. This is no longer a residual risk.
- **Global rate-limit (RLS) enforcement** — filter ACKs live (P7); live enforcement needs an external RLS+Redis service. `out-of-scope`; owner S12 / when an RLS server ships.
- **OTLP wire export** — OTel layer init + `traceparent` covered; span export on the wire needs a collector. `out-of-scope`; owner S12 (GenAI semconv).
- **Heavy resilience / load / soak** — kill-Postgres-mid-write, Envoy crash, CP-under-load, soak. CP-restart + secret-rotation-under-traffic + NACK recovery are covered (P2/P6 + `cargo`); the heavier failure-mode + load suite is S12.

## Verdict

**GO at Tier 0 for S1–S10.** All Tier-0 exit gates are met: traceability complete; fmt/clippy/ `cargo test --workspace` green and reproduced; live E2E 5× consecutive green including streaming; credential-redaction gate green; zero open Critical/Major defects. Residuals are documented and accepted, routed to S12. Recommended to proceed to S11 (MCP) with the residuals tracked.

## Follow-ups (do not block Tier 0)

- Split `scripts/e2e-envoy.sh` into `scripts/e2e/run.sh` + `lib.sh` + `NN-*.sh` (move `wait_converged`/`assert_status`/`redaction_sweep`/`known_fail` into `lib.sh`); maintainability, not a coverage gap.
