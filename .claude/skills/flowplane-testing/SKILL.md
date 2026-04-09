---
name: flowplane-testing
description: How to write and run tests for Flowplane code. Three test layers, anti-patterns, test framework, and running commands. Use when writing tests, adding test coverage, running tests, understanding test infrastructure, E2E tests, integration tests, or fixing test failures.
license: Apache-2.0
metadata:
  author: rajeevramani
  version: "0.1.0"
---

# Flowplane Testing

Every change must have three test layers. Do not mark work complete until all three are addressed.

## 1. Three Test Layers

### Layer 1: Unit Tests
Written alongside the code in the same file.

```bash
cargo test                    # Run all unit tests
cargo test <test_name>        # Run specific test
```

**Rules:**
- Cover the function's contract: valid inputs, invalid inputs, boundary values, error paths
- Test with the format users actually type (e.g., URLs with schemes, strings with whitespace)
- Same agent that wrote the code can write these

### Layer 2: Integration Tests
Spec-driven, not implementation-driven. Go through the real database.

```bash
cargo test --features postgres_tests    # Needs running PostgreSQL or testcontainers
```

**Rules:**
- The agent writing integration tests must NOT read the implementation source first
- Write from the API contract, not the code
- Must go through real DB with real FK constraints — no mocks
- Must verify error responses are correct content type (JSON, not HTML)
- Must include adversarial inputs: wrong HTTP methods, misspelled paths, malformed payloads, auth edge cases
- A different agent than the implementer must write these

**Test files:** `tests/` directory (e.g., `tests/dev_auth_integration.rs`, `tests/storage_team_isolation.rs`)

### Layer 3: E2E Smoke Tests
Black-box, against the running Docker stack.

```bash
make test-e2e-dev             # Dev mode E2E (bearer token, no Zitadel)
make test-e2e-prod            # Prod mode E2E (real Zitadel, multi-user)
make test-e2e                 # Both modes
FLOWPLANE_E2E_MTLS=1 make test-e2e-prod   # Prod + mTLS
```

**Rules:**
- Run against real Docker stack, not `tower::oneshot`
- Cover full user journey: setup → action → verify → teardown
- Must test both dev and prod modes where applicable
- Use the Rust framework in `tests/e2e/` with `TestHarness` + `CliRunner`

## 2. E2E Test Framework

### Directory Structure
```
tests/e2e/
  common/
    harness.rs        — TestHarness (HTTP client + auth + assertions + Envoy helpers)
    cli_runner.rs     — CliRunner (subprocess flowplane commands)
    envoy.rs          — EnvoyHandle (Envoy process + admin API + config_dump)
    shared_infra.rs   — SharedInfrastructure (shared test setup)
    mod.rs            — Module declarations
  smoke/              — Smoke tests (quick, both modes)
    test_bootstrap.rs
    test_dev_mode_smoke.rs
    test_prod_mode_smoke.rs
    test_routing.rs
    test_sds_delivery.rs
    test_cli_subprocess.rs
    test_cli_phase3.rs
    test_cli_phase3_prod.rs
    test_cli_phase5.rs    — Scaffold E2E tests (30 tests: cluster, route, listener, 10 filter types)
  full/               — Full E2E tests (feature-specific)
    test_11_bootstrap.rs
    test_12_header_mutation.rs
    test_13_jwt_auth.rs
    test_14_retry_policies.rs
    test_15_circuit_breakers.rs
    test_16_rate_limit.rs
    test_17_compression.rs
    test_18_outlier_detection.rs
    test_19_custom_response.rs
    test_20_ext_authz.rs
    test_21_cors.rs
    test_22_wasm.rs
    test_23_oauth2.rs
    test_24_mtls.rs
    test_25_organizations.rs
    test_26_grant_enforcement.rs
```

### Conventions
- **Dev mode tests:** Use `dev_harness()` (API-only) or `quick_harness()` (with Envoy), test name must start with `dev_`
- **Prod mode tests:** Use `TestHarnessConfig::new(...)` with real Zitadel
- **Gating:** All E2E tests use `#[ignore]` + `RUN_E2E=1` env var
- **Filtering:** `make test-e2e-dev` filters by `dev_` prefix, `make test-e2e-prod` filters non-`dev_` tests

### Harness Types
- **`dev_harness()`** — API-only, no Envoy. Use for tests that only verify CLI → API (error handling, basic CRUD).
- **`quick_harness()`** — Starts Envoy with dedicated ports. Use for tests that verify the full pipeline: CLI → API → xDS → Envoy config_dump → traffic.

### TestHarness Pattern (API-only)
```rust
#[tokio::test]
#[ignore]
async fn dev_my_feature_test() {
    let harness = dev_harness("test_name").await.expect("harness should start");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let output = cli.run(&["cluster", "get", "my-cluster"]).unwrap();
    output.assert_success();
}
```

### TestHarness Pattern (with Envoy verification)
```rust
#[tokio::test]
#[ignore]
async fn dev_my_feature_with_envoy() {
    let harness = quick_harness("test_name").await.expect("harness should start");
    let cli = CliRunner::from_harness(&harness).unwrap();

    // Create resource via CLI
    let output = cli.run(&["cluster", "create", "-f", file_path]).unwrap();
    output.assert_success();

    // Verify resource reached Envoy via xDS
    verify_in_config_dump(&harness, "my-cluster").await;

    // Verify traffic flows through Envoy
    verify_traffic(&harness, "my-domain.local", "/test").await;
}
```

### Scaffold E2E Helpers (in test_cli_phase5.rs)
- **`scaffold_and_create()`** — Scaffolds resource, replaces placeholders, writes temp file, runs create. Returns `CliOutput` without asserting.
- **`verify_in_config_dump(harness, name)`** — Polls Envoy admin config_dump until named resource appears (30s timeout).
- **`verify_traffic(harness, domain, path)`** — Sends traffic through Envoy, asserts 200 response.
- **`create_chain_for_cluster()`** — Creates route + listener to complete Envoy chain for a CLI-created cluster.
- **`create_chain_for_route()`** — Creates listener for a CLI-created route.
- **`create_filter_test_chain()`** — Creates cluster + route + listener chain for filter E2E tests.
- **`attach_filter_to_listener()`** — Attaches filter to listener via CLI.

## 3. Anti-Patterns — DO NOT DO THESE

These violations get tests rejected in code review.

1. **Do not write tests designed to pass.** Tests exist to catch bugs, not confirm implementation. If every test passes on first run, your tests are too weak.

2. **Do not choose inputs because you know the implementation handles them.** Use inputs a real user would provide. If you see the code splits on `:`, test with `http://host:port` not `host:port`.

3. **Do not change the test when it fails — fix the code.** If a test reveals a bug, fix the bug. The test was right.

4. **Do not mock the database in integration tests.** Integration tests must hit real DB constraints (FK violations, unique constraints, cascade deletes).

5. **Do not skip error paths.** Every API endpoint needs tests for: wrong input format, missing required fields, wrong HTTP method, wrong auth, misspelled paths.

6. **Do not let the implementer be the sole integration test author.** The implementer has blind spots — they test what they built, not what users do.

## 4. Running Tests

| Command | What It Tests | Needs |
|---|---|---|
| `cargo test` | Unit tests | Nothing |
| `cargo test --features postgres_tests` | Integration tests | PostgreSQL (testcontainers or local) |
| `make test-e2e-dev` | Dev mode E2E | Running Docker stack |
| `make test-e2e-prod` | Prod mode E2E | Running Docker stack + Zitadel |
| `make test-e2e` | Both E2E modes | Running Docker stack + Zitadel |
| `npm run test` (from `ui/`) | Frontend unit tests | Node.js |
| `npx playwright test` (from `ui/`) | Frontend E2E | Running stack |
| `make test-cleanup` | Remove orphaned testcontainers | Docker |

### Quality Gate (before every commit)
```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings && cargo test
```

## 5. Test File Conventions

| Test Type | Location | Gate |
|---|---|---|
| Unit tests | Same file as code (`#[cfg(test)]`) | `cargo test` |
| Integration tests | `tests/*.rs` | `--features postgres_tests` |
| E2E smoke tests | `tests/e2e/smoke/` | `#[ignore]` + `RUN_E2E=1` |
| E2E full tests | `tests/e2e/full/` | `#[ignore]` + `RUN_E2E=1` |
| Frontend unit | `ui/src/**/*.test.ts` | `npm run test` |
| Frontend E2E | `ui/tests/` | `npx playwright test` |

## 6. What to Always Test

- Unit logic and domain transformations
- API handler responses (success + error paths)
- MCP tool authorization (scope checking)
- Team isolation (can user A see user B's resources?)
- Filter create → attach → verify → detach lifecycle
- Both dev and prod auth modes where applicable
- **Scaffold roundtrip**: scaffold → create -f → get → Envoy config_dump → traffic (for any scaffold changes)
- **YAML/JSON parity**: both formats accepted by create -f and update -f (shared loader in `src/cli/config_file.rs`)
