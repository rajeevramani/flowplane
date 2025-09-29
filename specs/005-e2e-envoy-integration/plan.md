# Implementation Plan: End-to-End Envoy Integration Tests

**Branch**: `005-e2e-envoy-integration` | **Date**: 2025-09-29 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `specs/005-e2e-envoy-integration/spec.md`

## Approach
We will add a deterministic E2E harness that orchestrates Flowplane, Envoy, and a simple upstream echo service; drives API workflows through the Platform API; and validates traffic and Envoy admin state. We’ll keep a fast PR smoke suite and expand to deeper suites for main/nightly.

Phases:
1. Research & decisions: test harness choice (testcontainers vs docker-compose), Envoy image pin, fixture layout.
2. Harness scaffolding: process/container orchestration, per-test DB isolation, admin endpoints helpers.
3. Smoke suite: boot → create API → route traffic → config_dump checks.
4. Scenario suites: updates/deletes, TLS/mTLS, resilience (restarts), negative/error cases, optional multi-Envoy.
5. CI integration: jobs, caching, artifacts on failure; local one-command entrypoints.
6. Documentation: quickstart, troubleshooting, artifacts.

## Key Decisions
- Harness: Prefer programmatic orchestration (e.g., Rust testcontainers) for isolation and artifact control; provide a parallel docker-compose path for local dev convenience.
- Envoy version: Pin a stable Envoy image tag and document upgrade testing.
- DB isolation: Per-test SQLite file in a temp dir; never mutate `data/flowplane.db`.
- TLS/mTLS fixtures: Include static certs/keys for client↔Envoy and Envoy↔xDS.

## Constitution Check
- Test-first development: Smoke and scenario tests created before expanding implementation details; for this feature we mainly add tests+harness.
- Security-by-default: TLS/mTLS scenarios included; negative cases ensure no accidental exposure.
- Stability: Isolation and deterministic fixtures mitigate flakiness.

## Deliverables
- E2E harness utilities (helpers to boot services, hit admin endpoints, assert signals).
- Smoke and scenario tests grouped by suite.
- CI workflows to run smoke on PRs and full suite on main/nightly with artifacts.
- Quickstart docs for local runs and failure triage.

## Risks & Mitigations
- Flakiness due to startup timing → add robust readiness checks with bounded retries.
- Long CI times → keep smoke minimal; parallelize where safe; cache builds.
- Envoy image changes → pin version and test upgrades separately.

## Next Step
Generate tasks in `tasks.md` with concrete, file-scoped work items, dependencies, and suite structure.

