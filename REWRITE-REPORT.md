# Flowplane v2 Rewrite Report

Status: release-gate draft for `v1.0.0`. Q-006 is resolved (Apache-2.0; see `LICENSE`/`NOTICE`),
so public distribution is no longer license-gated; cutting the tag now needs only release approval.

## What Changed

Flowplane v2 rewrote the control plane around typed service-layer authorization, persisted
Postgres state, xDS/event-driven dataplane updates, explicit CP/DP separation, CLI-first operator
workflows, MCP tooling, AI gateway resources, and production hardening evidence.

The control plane remains the source of truth. Dataplanes dial out to the CP over xDS/diagnostics;
the CP does not use Envoy admin as a product API.

## Evidence Chain

- S12 definition of done: `DECISIONS.md` D-021.
- Observability and alert pack: `docs/observability-alerts.md`.
- Failure-mode matrix: `docs/failure-mode-matrix.md`.
- Adversarial surface map: `docs/adversarial-surface-map.md`.
- Release packaging: `docs/release-packaging.md`.
- Operator guide: `docs/production-readiness.md`.
- Release walkthrough and tag checklist: `internal/release-walkthrough.md`.
- Live Envoy certification: `scripts/e2e/CERTIFICATION-REPORT.md`.
- Live coverage matrix: `scripts/e2e/COVERAGE.md`.

## Release Gate Decision

For v1.0, live Envoy E2E is a manual recorded release gate. Existing evidence records
`bash scripts/e2e-envoy.sh` passing 5 consecutive runs, 12 phases each, with Envoy 1.37.4 and 0
known failures. CI-with-Docker promotion is deferred post-1.0.

## Accepted Risks

| Risk | Reference |
| --- | --- |
| Q-006 resolved: Apache-2.0 (`LICENSE`/`NOTICE`); public distribution no longer license-gated. | #86, #88, #93, #96 |
| Live Envoy E2E is manual recorded for v1.0, not a CI gate. | D-021, #91, #95 |
| P1d AI streaming phase is timing-sensitive and certified on Envoy 1.37.4. | #92, `scripts/e2e/CERTIFICATION-REPORT.md` |
| Native `gen_ai.*` OTel semantic-convention metrics are deferred post-1.0. | D-021, `docs/observability-alerts.md` |

## Tag Criteria

`v1.0.0` may be created only after #95 is reviewed and accepted, #86 children are closed or
accepted, required CI is green, release artifacts are reproducible, and the checklist in
`internal/release-walkthrough.md` is signed off.
