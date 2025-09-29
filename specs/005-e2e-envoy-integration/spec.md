# Feature Specification: End-to-End Envoy Integration Tests

**Feature Branch**: `005-e2e-envoy-integration`
**Created**: 2025-09-29
**Status**: Draft

## 1. Objective
- Provide reliable end-to-end coverage that proves Flowplane materializes APIs into Envoy and routes traffic correctly via xDS.
- Catch regressions across API layer, persistence, xDS responses (SotW/Delta), Envoy config/state, and security settings (TLS/mTLS).
- Enable fast PR smoke checks with deeper suites on main/nightly.

## 2. Problem Statement
Recent work spans multiple layers (API → storage → materializer → xDS → Envoy). Manual verification is slow and error-prone, and it’s easy to ship a change that breaks convergence or routing. We need a deterministic, automated E2E harness that boots the control plane and Envoy, provisions APIs through the platform API, verifies traffic outcomes, and inspects Envoy’s rendered state for correctness.

## 3. Guiding Principles
- Black-box first: assert via client requests and Envoy admin signals, not control-plane internals.
- Deterministic fixtures: stable inputs (API payloads, certs) and fixed ports to reduce flakiness.
- Isolated runs: per-test DB and container network to avoid inter-test interference.
- Fast feedback: keep a <2 min smoke test for PRs; run deeper suites on main/nightly.
- Artifacts on failure: always collect config_dump, stats, logs, and test DB for triage.

## 4. Personas
- Platform Engineer: needs confidence that creating/updating/deleting APIs affects traffic as intended.
- SRE/Operator: needs to verify Envoy connects to xDS, accepts updates, and keeps serving after restarts.
- Security Engineer: needs TLS/mTLS flows validated and misconfiguration caught.

## 5. User Scenarios
### 5.1 Smoke (PR gate)
1. Boot control plane and Envoy; Envoy connects to xDS without NACKs.
2. Create a simple API via Platform API.
3. Send a client request through Envoy → upstream echo; receive 200 OK.
4. Envoy config_dump contains expected listener/route/cluster names.

### 5.2 Config & Delta Updates
1. Append a route and verify new matchers appear and route works.
2. Update upstream target; traffic shifts accordingly; stale config removed.
3. Delete route/definition; Envoy removes resources; traffic fails as expected.

### 5.3 Resilience
1. Restart control plane; Envoy keeps serving last-good config; resyncs on recovery.
2. Restart Envoy; refetches correct snapshot; routing recovers.

### 5.4 Security
1. Client↔Envoy TLS success with valid cert; fail with invalid/no cert when required.
2. Envoy↔xDS mTLS handshake succeeds with valid CA/certs; fails with invalid credentials.

### 5.5 Negative Cases
1. Invalid API payload rejected; no Envoy changes.
2. Domain/path conflict returns deterministic error; no partial materialization.

### 5.6 Multi-Instance
1. Two Envoy instances connect and receive consistent config; both route traffic correctly.

## 6. Functional Requirements
- FR-001 Harness: Provide programmatic orchestration to boot Envoy and an upstream echo, and start the control plane with per-test DB isolation.
- FR-002 Determinism: Use fixed, documented ports and static fixtures for API payloads and TLS.
- FR-003 Health: Verify control plane readiness and Envoy xDS connectivity; assert zero NACKs in `/stats` during steady state.
- FR-004 Smoke: Implement a <2-minute smoke scenario covering create → route traffic → config_dump assertions.
- FR-005 Config Validation: Parse Envoy `/config_dump` to assert expected listeners, clusters, routes, names, matchers, and endpoints.
- FR-006 Delta/Updates: Apply update and delete operations; assert additions/removals are reflected in Envoy and via traffic outcomes.
- FR-007 Resilience: Validate restarts (control plane and Envoy) preserve/restore routing and resynchronize.
- FR-008 TLS Client Edge: Validate HTTPS termination at Envoy with provided certs; exercise success and failure modes.
- FR-009 xDS mTLS: Validate Envoy↔control-plane mTLS with correct CA/cert/key and failure with invalid credentials.
- FR-010 Artifacts: On failure, collect control plane logs, Envoy config_dump and stats, request/response traces, and the test DB file.
- FR-011 CI Integration: Run smoke on PRs; run full suite (incl. TLS/resilience/negatives) on main/nightly; export artifacts on failure.
- FR-012 Local Dev: Provide one-command targets for smoke and full suite; document prerequisites and how to inspect artifacts.
- FR-013 Isolation: Ensure each test uses a unique DB path and network namespace; prevent cross-test leakage.
- FR-014 Time Budgets: Apply sensible timeouts for boot, xDS convergence, and request routing to catch regressions.
- FR-015 Envoy Version Pin: Pin an Envoy image version; document upgrade testing strategy.
- FR-016 Metrics-Based Assertions: Use `/stats` counters for xDS ACK/NACK and active connections as part of assertions.
- FR-017 Multi-Envoy: Optional test to run two Envoys against the same control plane and assert consistent routing.
- FR-018 Negative Validations: Invalid payloads, port/listener conflicts, and RBAC errors do not mutate Envoy state.
- FR-019 Configuration Modes: Cover shared-listener and dedicated-listener isolation modes where applicable.
- FR-020 Documentation: Provide quickstart and CI docs with troubleshooting guides.
- FR-021 Unique Resource Names: Tests must generate unique domains and paths per test (e.g., `/e2e/<test_id>/...`) to avoid cross-test collisions.
- FR-022 Deterministic Teardown: Harness must guarantee cleanup of processes/containers, temp DB files, and network ports; collect artifacts only on failure.

## 7. Non-Functional Requirements
- NFR-001 Stability: Tests minimize flakiness via retries for external readiness checks but never mask correctness failures.
- NFR-002 Speed: PR smoke under 2 minutes on standard CI runners; full suite target under 15 minutes.
- NFR-003 Portability: Works on macOS/Linux developer machines and typical CI Docker runners.
- NFR-004 Observability: Clear, searchable logs and structured artifacts for failures.

## 8. Signals and Evidence
- HTTP outcomes: status codes and bodies from an upstream echo service.
- Envoy admin: `/config_dump` for LDS/RDS/CDS/EDS; `/stats` for xDS ACK/NACK and listener/cluster health.
- Control plane readiness/health endpoints and logs for convergence timing.
- SQLite DB presence and row counts in a per-test location (no mutation of `data/flowplane.db`).

## 9. Dependencies and Assumptions
- Docker or container runtime available for Envoy and upstream echo service.
- Control plane can be started with custom DB path and optional TLS/mTLS flags.
- Existing sample envoy configs (`envoy-non-tls-test.yaml`, `envoy-tls-test.yaml`) can be adapted as fixtures.
- Network access to pull the pinned Envoy image in CI (or pre-cache in CI if restricted).

## 10. Out of Scope (MVP)
- Full performance/load testing beyond simple latency/convergence budgets.
- Chaos-style fault injection beyond basic restarts.
- Multi-cluster, multi-tenant isolation beyond what’s needed to validate current features.

## 11. Acceptance Criteria
- A developer runs the smoke suite with one command; it boots CP+Envoy, provisions an API, validates routing, and asserts Envoy config without flakiness.
- CI runs the smoke suite on PRs and fails on regressions with actionable artifacts attached.
- The full suite (run on main/nightly) covers config updates, deletes, restarts, TLS/mTLS, and negative cases, with deterministic outcomes.
- Documentation clearly explains how to run locally and interpret failures.

## 12. Review Checklist
- [x] Requirements are testable and unambiguous
- [x] Scope and out-of-scope clearly defined
- [x] Signals and artifacts identified
- [x] CI/local workflows specified
- [x] Security flows (TLS/mTLS) explicitly covered
