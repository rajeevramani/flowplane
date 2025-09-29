# Quickstart: End-to-End Envoy Integration Tests

## Prerequisites
- Docker (or compatible runtime)
- Rust toolchain (for running tests)
- Network access to pull the pinned Envoy image (or pre-cached CI runner)

## Local Smoke Run
- Goal: boot control plane + Envoy + echo, create one API, route traffic, and check Envoy admin state.
- Command (to be added with harness): `cargo test -p flowplane --test e2e -- smoke` (example; finalize with actual test names)

## Full Suite
- Goal: run config updates, deletes, restarts, TLS/mTLS, and negative scenarios.
- Command: `cargo test -p flowplane --test e2e` (or `make e2e-full`)

## Inspecting Failures
When a test fails, the harness collects:
- Envoy `/config_dump` and `/stats`
- Control plane logs
- Request/response traces
- Per-test SQLite DB file path

Artifacts are written to a per-test directory under `target/e2e-artifacts/<test-name>/` (finalize in harness).

## Uniqueness & Teardown
- Routes and domains are automatically namespaced with a per-test ID (e.g., `/e2e/<test_id>/...`) to avoid collisions.
- The harness tears down containers/processes and cleans the per-test DB; artifacts are preserved only on failures.

## Troubleshooting
- Ensure no local process binds to the test ports (documented in harness output).
- If Envoy fails to start, check the test’s Envoy stdout/stderr logs.
- If xDS shows NACKs, review control plane logs and the diff between expected and actual resources.
- For TLS/mTLS issues, open the collected cert details and validate SAN/SNI and trust chain.

## Notes
- Do not use `data/flowplane.db` for tests. The harness creates a unique SQLite db per test.
- Envoy version is pinned for determinism; see spec for upgrade policy.
