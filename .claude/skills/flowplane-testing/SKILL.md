---
name: flowplane-testing
description: How to write and run tests for Flowplane code. Three test layers, anti-patterns, test framework, and running commands. Use when writing tests, adding test coverage, running tests, understanding test infrastructure, E2E tests, integration tests, or fixing test failures.
license: Apache-2.0
metadata:
  author: rajeevramani
  version: "0.2.0"
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
    harness.rs        — TestHarness (orchestrator: CP, Envoy, mocks, DB)
    cli_runner.rs     — CliRunner (subprocess flowplane commands)
    envoy.rs          — EnvoyHandle (Envoy process + admin API + config_dump)
    shared_infra.rs   — SharedInfrastructure (singleton shared test setup)
    test_helpers.rs   — Shared E2E helpers (verify_in_config_dump, verify_traffic, etc.)
    resource_setup.rs — ResourceSetup builder (clusters, routes, listeners, filters)
    zitadel.rs        — Zitadel container lifecycle + OIDC token acquisition
    api_client.rs     — Typed HTTP client for Flowplane API
    mocks.rs          — Mock services (Echo, Auth0, ext_authz)
    ports.rs          — Port allocation (PortAllocator with bind-testing)
    stats.rs          — Envoy stats parsing
    filter_configs.rs — Type-safe filter configuration builders
    timeout.rs        — Timeout utilities and retry logic
    mod.rs            — Module declarations and re-exports
  smoke/              — Smoke tests (quick, both modes)
    test_bootstrap.rs
    test_dev_mode_smoke.rs
    test_prod_mode_smoke.rs
    test_routing.rs
    test_sds_delivery.rs
    test_cli_subprocess.rs
    test_cli_expose.rs      — Expose, unexpose, list, status, filter list, learn list, doctor (dev mode)
    test_cli_expose_prod.rs — Same as expose but prod mode JWT
    test_cli_views.rs       — Dataplane, vhost, filter types, import, stats, route-views, schema, reports
    test_cli_scaffold.rs    — mTLS/cert, admin, MCP tools, WASM, apply, scaffold
    test_cli_ops.rs         — Ops diagnostic commands (trace, topology, validate, xds, audit, admin)
    test_cli_ops_envoy.rs   — Ops commands requiring Envoy (config_dump verification)
    test_cli_deletes.rs     — Delete/cleanup operations
    test_cli_learn.rs       — Learning session lifecycle
    test_cli_reads.rs       — Read-only CLI commands
    test_cli_secrets.rs     — Secret management
    test_cli_import.rs      — Import operations
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

**File renames (2026-04):** `test_cli_phase3.rs` -> `test_cli_expose.rs`, `test_cli_phase3_prod.rs` -> `test_cli_expose_prod.rs`, `test_cli_phase4.rs` -> `test_cli_views.rs`, `test_cli_phase5.rs` -> `test_cli_scaffold.rs`.

### Conventions
- **Dev mode tests:** Use `dev_harness()` (API-only) or `envoy_harness()` (with per-test Envoy), test name **must** start with `dev_`
- **Prod mode tests:** Use shared harness (default), test name **must** start with `prod_`
- **Gating:** All E2E tests use `#[ignore]` + `RUN_E2E=1` env var
- **Filtering:** `make test-e2e-dev` filters to `dev_` prefix only, `make test-e2e-prod` filters to `prod_` prefix only
- **Isolated mode:** Only supported for dev auth. Prod isolated mode is explicitly unsupported (will bail with error)
- **Resource naming:** Tests MUST use unique resource names with UUID prefix to avoid collisions in the shared DB. See "Resource Naming" section below.

### Harness Types — Three Modes

| Harness | Shared CP | Per-test Envoy | Use When |
|---|---|---|---|
| `dev_harness(name)` | Yes | No | API-only tests: CLI -> API error handling, basic CRUD, reads |
| `envoy_harness(name)` | Yes | Yes | Tests that verify config_dump or traffic through Envoy |
| `TestHarnessConfig::new(name).isolated()` | No (own CP) | Yes (own Envoy) | Tests needing their own DB/CP (rare, slowest) |

- **`dev_harness(name)`** — Shared control plane, no Envoy. Use for tests that only verify CLI -> API (error handling, basic CRUD, reads). Fastest.
- **`envoy_harness(name)`** — Shared control plane + fresh Envoy spawned per test. Use for tests that verify the full pipeline: CLI -> API -> xDS -> Envoy config_dump -> traffic. Replaces the old `quick_harness()` (which has been removed).
- **Isolated mode** via `TestHarnessConfig::new(name).isolated()` — Own CP, own Envoy, own DB. Slowest. Use only when tests require a clean database state.

**`quick_harness()` is removed.** All tests have been migrated to either `dev_harness()` or `envoy_harness()`. Do not use `quick_harness()`.

### Dev Mode Constraints

Dev mode runs a single team (`default`) with a single dataplane (`dev-dataplane`). Important rules:
- Do NOT create new teams in dev mode — there is only `default`.
- Per-test Envoy (from `envoy_harness`) connects to the shared xDS server with the shared team's metadata.
- All tests share the same DB, so resource name uniqueness is critical.

### Resource Naming (Required)

All E2E tests MUST use unique resource names to avoid collisions when tests run in parallel against the shared team's DB. Use a UUID prefix:

```rust
let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();
let cluster_name = format!("{id}-my-cluster");
let route_name = format!("{id}-my-route");
let listener_name = format!("{id}-my-listener");
```

Do NOT use hardcoded names like `"test-cluster"` or `"my-route"` — these will collide with other tests.

### Port Allocation

`envoy_harness()` uses `PortAllocator::for_test()` with bind-testing to avoid port collisions. No hardcoded port ranges. The allocator binds to port 0, reads the OS-assigned port, then releases it for Envoy to use.

Do NOT hardcode ports in E2E tests. Always get ports from the harness or allocator.

### TestHarness Pattern (API-only)
```rust
#[tokio::test]
#[ignore]
async fn dev_my_feature_test() {
    let harness = dev_harness("test_name").await.expect("harness should start");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let name = format!("{id}-my-cluster");

    let output = cli.run(&["cluster", "get", &name]).unwrap();
    output.assert_success();
}
```

### TestHarness Pattern (with Envoy verification)
```rust
#[tokio::test]
#[ignore]
async fn dev_my_feature_with_envoy() {
    let harness = envoy_harness("test_name").await.expect("harness should start");
    let cli = CliRunner::from_harness(&harness).unwrap();

    let id = uuid::Uuid::new_v4().as_simple().to_string()[..8].to_string();

    // Create resource via CLI
    let output = cli.run(&["cluster", "create", "-f", file_path]).unwrap();
    output.assert_success();

    // Verify resource reached Envoy via xDS
    verify_in_config_dump(&harness, &format!("{id}-my-cluster")).await;

    // Verify traffic flows through Envoy
    verify_traffic(&harness, &format!("{id}-my-domain.local"), "/test").await;
}
```

### Shared E2E Helpers (`tests/e2e/common/test_helpers.rs`)

These are re-exported from `crate::common` — import via `use crate::common::test_helpers::*`:

- **`write_temp_file(content, extension)`** — Creates a named temp file for CLI input (`-f` flag). Caller must hold the return value to keep file alive.
- **`verify_in_config_dump(harness, name)`** — Polls Envoy admin config_dump until named resource appears (30s timeout, 500ms interval). Skips gracefully if Envoy unavailable.
- **`verify_not_in_config_dump(harness, name)`** — Polls Envoy admin config_dump until named resource is absent (30s timeout, 500ms interval). Use after delete operations.
- **`verify_traffic(harness, domain, path)`** — Sends traffic through Envoy, asserts 200 + non-empty body. Skips gracefully if Envoy unavailable.
- **`create_chain_for_cluster(harness, cluster, prefix)`** — Creates route config + listener via API to complete Envoy chain for a CLI-created cluster. Returns `(route_name, domain)`.
- **`create_chain_for_route(harness, route, prefix, domain)`** — Creates listener for a CLI-created route config. Returns domain.

### Test-file-local Helpers (in `test_cli_scaffold.rs`)
- **`scaffold_and_create()`** — Scaffolds resource, replaces placeholders, writes temp file, runs create.
- **`create_filter_test_chain()`** — Creates cluster + route + listener chain for filter E2E tests.
- **`attach_filter_to_listener()`** — Attaches filter to listener via CLI.

### Multi-User CLI Testing

For prod-mode tests that need multiple users with different team roles:

```rust
#[tokio::test]
#[ignore]
async fn prod_team_isolation_via_cli() {
    let harness = envoy_harness("isolation_test").await.unwrap();
    if harness.is_dev_mode() { return; }

    let shared = harness.shared_infra().expect("prod uses shared mode");

    // Create two users with different team roles
    shared.create_test_user("alice@test.com", "Alice", "Test", "pass123").await.unwrap();
    let alice_token = shared.get_user_token("alice@test.com", "pass123").await.unwrap();

    // Run CLI as alice targeting team-a
    let alice_cli = CliRunner::with_token_and_team(&harness, &alice_token, "team-a", "acme-corp").unwrap();
    let output = alice_cli.run(&["cluster", "list"]).unwrap();
    output.assert_success();
}
```

**Key methods:**
- `CliRunner::with_token(harness, token)` — Same team/org as harness, different auth token
- `CliRunner::with_token_and_team(harness, token, team, org)` — Different team, org, and auth token
- `harness.shared_infra()` — Access `SharedInfrastructure` for `create_test_user()`, `get_user_token()`, `get_agent_token()`

## 3. Anti-Patterns — DO NOT DO THESE

These violations get tests rejected in code review.

1. **Do not write tests designed to pass.** Tests exist to catch bugs, not confirm implementation. If every test passes on first run, your tests are too weak.

2. **Do not choose inputs because you know the implementation handles them.** Use inputs a real user would provide. If you see the code splits on `:`, test with `http://host:port` not `host:port`.

3. **Do not change the test when it fails — fix the code.** If a test reveals a bug, fix the bug. The test was right.

4. **Do not mock the database in integration tests.** Integration tests must hit real DB constraints (FK violations, unique constraints, cascade deletes).

5. **Do not skip error paths.** Every API endpoint needs tests for: wrong input format, missing required fields, wrong HTTP method, wrong auth, misspelled paths.

6. **Do not let the implementer be the sole integration test author.** The implementer has blind spots — they test what they built, not what users do.

7. **Do not use hardcoded resource names in E2E tests.** Always use UUID-prefixed names. Hardcoded names cause flaky failures when tests run in parallel.

8. **Do not hardcode ports in E2E tests.** Use `PortAllocator::for_test()` or get ports from the harness.

9. **Do not use `quick_harness()`.** It has been removed. Use `dev_harness()` or `envoy_harness()` instead.

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
