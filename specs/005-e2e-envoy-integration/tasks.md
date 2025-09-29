# Tasks: End-to-End Envoy Integration Tests

**Input**: Design documents from `specs/005-e2e-envoy-integration/`
**Prerequisites**: spec.md (required), plan.md, existing Envoy fixtures (`envoy-non-tls-test.yaml`, `envoy-tls-test.yaml`), platform API examples (`.local/api-tests/api-definition.http`)

## Execution Flow (main)
```
1. Confirm scope in spec.md (smoke, updates, TLS/mTLS, resilience, negative, optional multi-Envoy)
2. Decide harness: programmatic (testcontainers) for CI; docker-compose for local convenience
3. Scaffold harness utilities and fixtures layout
4. Add smoke suite tests (create → route → config assertions)
5. Expand with scenario suites (updates, deletes, restarts, TLS/mTLS, negatives)
6. Wire CI: smoke on PR, full on main/nightly; artifacts on failure
7. Document quickstart and troubleshooting
```

## Format: `[ID] [P?] Description`
- **[P]**: Task can be executed in parallel (different files, independent work)
- Always include full relative file path(s) in each description

## Path Conventions
- E2E tests live in `tests/e2e/`
- Helpers live in `tests/e2e/support/`
- Fixtures live in `tests/e2e/fixtures/` (API payloads, TLS certs/keys); reuse root `envoy-*.yaml` as needed
- CI config under `.github/workflows/`

## Phase 1: Harness & Fixtures

- [ ] E2E-001 Create `tests/e2e/support/env.rs` for environment boot: start control plane with per-test DB path; wait for readiness; capture logs
- [ ] E2E-002 Create `tests/e2e/support/envoy.rs` to run Envoy container with pinned version; load `envoy-non-tls-test.yaml` or TLS variant; helpers for `/config_dump` and `/stats`
- [ ] E2E-003 Create `tests/e2e/support/echo.rs` to run an upstream echo container; expose port and simple probe
- [x] E2E-001 Create `tests/e2e/support/env.rs` for environment boot: start control plane with per-test DB path; wait for readiness; capture logs (in-process servers)
- [x] E2E-002 Create `tests/e2e/support/envoy.rs` to run Envoy process with generated config; helpers for `/config_dump` and `/stats`
- [x] E2E-003 Create `tests/e2e/support/echo.rs` to run an upstream echo server; expose port and simple probe
- [x] E2E-004 [P] Add `tests/e2e/support/api.rs` helpers to hit Platform API (create API via PAT token)
- [x] E2E-005 [P] Add `tests/e2e/fixtures/api_payloads.json` with deterministic example payloads (shared listener, isolated listener)
- [x] E2E-006 [P] Add `tests/e2e/fixtures/tls/` with documented cert/key generation and placeholders; ready for local generation and use by TLS tests

- [x] E2E-007 [P] Add `tests/e2e/support/naming.rs` unique name/path helpers: derive `test_id` from test name; generate unique domain `test-<shortid>.e2e.local` and path `/e2e/<test_id>/<route>`; ensure determinism within a test run
- [x] E2E-008 [P] Add `tests/e2e/support/teardown.rs` drop-guard that always stops containers/processes, removes temp DB, and exports artifacts on failure only; callable explicit cleanup for shared-CP runs
- [x] E2E-009 [P] Add `tests/e2e/support/ports.rs` ephemeral, per-test port allocator with reservation file to avoid collisions in parallel runs

## Phase 2: Smoke Suite (PR gate)

- [x] E2E-010 Add `tests/e2e/smoke_boot_and_route.rs`: boot CP (in-process) + echo, optionally Envoy if present; create API using `naming` unique domain/path; perform proxy request through Envoy (Host=domain, path=route) and assert 200 echo; fetch `/config_dump` and assert presence of platform resources (domain, upstream port); ensure teardown runs
- [x] E2E-011 Add `tests/e2e/smoke_no_nacks.rs`: assert zero xDS NACKs and healthy LDS/RDS/CDS stats after convergence

## Phase 3: Configuration & Delta Updates

- [x] E2E-020 Add `tests/e2e/config_update_add_route.rs`: append unique sub-path → assert both old and new routes work; config_dump contains both paths
- [x] E2E-021 Add `tests/e2e/config_update_change_upstream.rs`: change upstream target (cluster endpoints) → assert traffic shifts and config updates
- [x] E2E-022 Add `tests/e2e/config_delete_cleanup.rs`: create dedicated route/listener → delete route; assert removal in config_dump and routing no longer returns 200

## Phase 4: Resilience

- [ ] E2E-030 Add `tests/e2e/resilience_restart_cp.rs`: restart control plane → Envoy serves last-good config; resync on recovery
- [ ] E2E-031 Add `tests/e2e/resilience_restart_envoy.rs`: restart Envoy → refetch snapshot; routing recovers
- [x] E2E-030 Add `tests/e2e/resilience_restart_cp.rs`: restart control plane → Envoy serves last-good config; resync on recovery
- [x] E2E-031 Add `tests/e2e/resilience_restart_envoy.rs`: restart Envoy → refetch snapshot; routing recovers

## Phase 5: Security (TLS/mTLS)

- [~] E2E-040 Add `tests/e2e/tls_client_edge.rs`: client↔Envoy TLS success with valid cert; failure cases with invalid/no cert
  - Implemented TLS listener in Envoy helper and HTTPS client with custom CA
  - Test creates API, routes over TLS with CA (success), and verifies failure without CA
  - Additional cases (hostname SAN mismatch) can be added after expanding fixtures
- [~] E2E-041 Add `tests/e2e/mtls_xds.rs`: Envoy↔xDS mTLS success/failure using TLS fixtures
  - xDS server TLS wiring added to harness; Envoy ADS mTLS config implemented
  - Matrix cases scaffolded and ignored until fixtures present; verifies ADS success/failure via stats and optional routing

## Phase 6: Negative Cases

- [x] E2E-050 Add `tests/e2e/negative_invalid_payload.rs`: invalid API rejected; assert no Envoy changes; confirm teardown cleaned resources
- [x] E2E-051 Add `tests/e2e/negative_conflicts.rs`: intentionally craft duplicates using `naming` helpers to simulate conflicts; assert deterministic error and no partial materialization

## Phase 7: Multi-Instance (optional)

- [x] E2E-060 Add `tests/e2e/multi_envoy_consistency.rs`: boot two Envoys → both receive consistent config; both route traffic

## Phase 8: CI & Docs

- [x] E2E-070 CI: integrate e2e smoke into existing `.github/workflows/ci.yml` (avoid duplicating pipeline). Runs smoke tests on PR/push and uploads artifacts on failure.
- [x] E2E-071 CI: integrate full E2E job into existing `.github/workflows/ci.yml` (main + nightly schedule), best-effort Envoy install, runs extended suites with artifacts on failure
- [ ] E2E-072 Add `specs/005-e2e-envoy-integration/quickstart.md` with local run instructions and troubleshooting
- [x] E2E-073 Add artifact collection helpers (dump Envoy admin endpoints) in `tests/e2e/support/artifacts.rs`; integrate in CI job for failures

## Dependencies

**Critical**:
- Harness (E2E-001..006) before smoke (E2E-010..011)
- Uniqueness + teardown + ports (E2E-007..009) before any tests that create routes
- Smoke before extended suites
- TLS fixtures before TLS/mTLS tests

**Parallel**:
- API helpers and fixtures (E2E-004..006)
- Uniqueness/teardown/ports (E2E-007..009)
- Config update scenarios (E2E-020..022) can be authored in parallel after smoke harness exists

## MVP Coverage Mapping
- FR-004 Smoke → E2E-010, E2E-011
- FR-005 Config validation → E2E-010, E2E-020..022
- FR-007 Resilience → E2E-030, E2E-031
- FR-008/009 TLS/mTLS → E2E-040, E2E-041
- FR-018 Negative → E2E-050, E2E-051
- FR-011 CI → E2E-070, E2E-071
- FR-012 Docs → E2E-072
