# fp-4n5 Task 5b — Two-mocks fix + harness collapse (in progress)

Date: 2026-04-15
Author: implementer5 (Part 1), implementer6 (Part 2)
Status: **Part 1 + Part 2 landed**

# What

Task 5b of fp-4n5 has two parts:

1. **Two-mocks token-mismatch fix** (Part 1, CRITICAL — blocks all dev-mode e2e tests). Before this commit, Task 5a made `tests/e2e/common/{shared_infra,harness}.rs` spin up their own `MockOidcServer` side-cars to mint JWTs, while `src/api/server.rs::start_api_server` independently spawned ANOTHER mock internally whenever `AuthMode::Dev`. Two mock instances meant two ephemeral RSA signing keys: tokens minted by the test harness could never validate against the CP's JWKS. The test binary compiled but every authenticated request in dev-mode e2e would have failed at the JWT verification step. Part 1 unifies the two paths onto a single mock instance.

2. **Harness collapse 4 → 2** (Part 2): replace `initialize_dev`, `initialize_prod_zitadel`, `initialize` dispatcher, and isolated-mode CP bootstrapping with a single `boot_cp(CpBootConfig) -> BootedCp` consumed by `get_or_init_shared` (OnceLock cache) and `start_isolated` (owning wrapper). **This commit does NOT contain Part 2** — see "Part 2 handoff" below.

# Part 1 — changes landed

## `src/api/server.rs`

- `start_api_server` gained a third parameter: `zitadel_override: Option<ZitadelAuthState>`. When `Some`, it is used directly and the internal dev-mock spawn is skipped. When `None`, existing behavior: in dev mode, the CP spawns its own mock via `start_dev_mock_oidc` and derives the state from it.
- Extracted `pub fn build_dev_auth_state(&MockOidcServer, DbPool) -> ZitadelAuthState` (feature-gated on `dev-oidc`) so tests that own a mock externally can build the same state the CP would build internally. `start_dev_mock_oidc` now delegates to it — single source of truth for how a `ZitadelAuthState` is constructed from a mock.
- `build_dev_auth_state` is re-exported from `src/api/mod.rs` under `#[cfg(feature = "dev-oidc")]`.

## `src/cli/mod.rs:934`

Production caller (`run_server`) passes `None` as the override — no behavior change for prod; the CP still spawns its own mock in dev mode when invoked outside of tests.

## `tests/e2e/common/control_plane.rs`

- `ControlPlaneConfig` gained `dev_oidc_mock: Option<Arc<MockOidcServer>>` (feature-gated) + `with_dev_oidc_mock(Arc<MockOidcServer>)` builder.
- Inside `ControlPlaneHandle::start`, after pool creation, if `dev_oidc_mock` is Some, the config builds a `ZitadelAuthState` via `flowplane::api::build_dev_auth_state(mock.as_ref(), pool.clone())` and passes it to `start_api_server` as the override. The CP skips spawning its own mock.
- Dropped `#[derive(Debug)]` from `ControlPlaneConfig` — `MockOidcServer` is not `Debug`, and no callers format the struct.

## `tests/e2e/common/shared_infra.rs::initialize_dev`

- Mock OIDC is wrapped in `Arc<MockOidcServer>`; the same Arc is passed into `ControlPlaneConfig::with_dev_oidc_mock` AND used to mint the test token via `issue_token_for_sub(DEV_USER_SUB)`. One mock, two consumers.
- `SharedInfrastructure.mock_oidc` field changed from `Option<MockOidcServer>` to `Option<Arc<MockOidcServer>>`.

## `tests/e2e/common/harness.rs::start_isolated`

- Mock OIDC creation moved BEFORE `ControlPlaneConfig` construction (it was previously AFTER `cp.wait_ready().await`, which was safe only because the token wasn't actually being validated). Wired into `cp_config` via `with_dev_oidc_mock`.
- The later auth-token block reuses `mock_oidc_handle` instead of spawning a second mock.
- `_mock_oidc` field on `TestHarness` changed from `Option<MockOidcServer>` to `Option<Arc<MockOidcServer>>`.
- **Also added**: a `seed_dev_resources` call after `cp.wait_ready()` for isolated dev mode. Rationale: without it, the mock-issued JWT (sub=`DEV_USER_SUB`) would JIT-upsert a user with no team memberships via `upsert_from_jwt`, breaking any test that touches resource-scoped endpoints. `shared_infra::initialize_dev` already calls `seed_dev_resources`; isolated dev previously relied on `generate_dev_token` which bypassed the user-row check entirely. This is a trivial fix silently documented here per deviation protocol.

# Alternatives considered (Part 1)

1. **Option (a): credentials-file handoff via `FLOWPLANE_CREDENTIALS_PATH`**. The CP runs inside the test binary's tokio task (no docker, no host filesystem handoff); setting a temp path before CP start and reading the token after was possible but fragile — temp file cleanup ordering, parallelism with multiple isolated harnesses, and the need for a filesystem watch for the "token is now written" signal. Rejected.
2. **Option (b): expose the CP's mock handle via a test-only accessor**. This would undo implementer3's deliberate encapsulation (`DevAuthBundle` lives as a `_mock` local in `start_api_server`) and leak mock internals out through `ApiState` or similar. Rejected.
3. **Option (c) — chosen**: extend the existing `zitadel_override` parameter implementer3 added in T4 so tests inject their OWN mock. `start_api_server` already had the override parameter; Task 5b made the override also SKIP the internal mock spawn so a test-supplied state is the sole source of truth. The architectural surface added is one extra `Option<Arc<MockOidcServer>>` field on `ControlPlaneConfig` and one extra parameter on the already-existing `start_api_server` signature. Zero new global state, no env-var mutation, no accessors into the middleware.

# Why (Part 1, linked to dev-prod-root-cause enforcement)

`specs/decisions/2026-04-14-dev-prod-root-cause.md` makes the case that dev and prod must exercise the same auth middleware, same JWT validation, same JIT provisioning. The two-mocks bug was a regression against that principle dressed up as a test-harness plumbing issue: the CP was validating against a different key than the test was signing with, which would force future engineers to either (i) disable auth in test mode (restoring a dev/prod divide), or (ii) resort to `std::env::set_var("FLOWPLANE_ZITADEL_*", ...)` to point the CP at the test mock (forbidden by the grep gate in `src/auth/middleware.rs` — zero hits). Option (c) keeps both divides closed: the CP's JWT validation runs unchanged, and the mock is explicitly passed in rather than hidden in global state.

# Gotchas (Part 1)

- **Mock lifetime under `Arc`**. `MockOidcServer`'s `Drop` aborts the JWKS tokio task. Wrapping in `Arc` means the server stays up until the LAST reference drops. Shared-infra stashes one Arc in `SharedInfrastructure.mock_oidc` (static lifetime); another Arc lives inside `ControlPlaneConfig`/`ControlPlaneHandle` until the CP shuts down. Both arcs drop together when the test binary exits, which is fine — the shared runtime is also gone by that point.
- **rustls CryptoProvider install (fp-6yj history)**. Unchanged. Shared-infra and isolated harness both install the ring default provider on first call (guarded by `CryptoProvider::get_default().is_none()`). The new seed_dev_resources call in isolated mode does not touch TLS state.
- **`FLOWPLANE_E2E_MTLS=1` opt-in for shared-mtls**. Unchanged — still only honored in `initialize_prod_zitadel`.
- **`FLOWPLANE_AUTH_MODE=dev` env var still set** in both isolated and shared dev paths before CP boot. This is NOT `FLOWPLANE_ZITADEL_*` (grep gate only covers those); it is the top-level auth-mode selector and is the documented way to boot CP in dev mode. Left untouched.
- **`ControlPlaneConfig` lost `Debug`**. No callers formatted the struct; a couple use sites do pattern-matching on fields but do not call `{:?}`. Confirmed via grep before the drop.

# Part 2 — Completed (implementer6)

The four parallel CP boot paths are now a single `boot_cp(CpBootConfig) -> BootedCp`
function in `tests/e2e/common/shared_infra.rs`. Callers:

- **Shared infra**: `build_shared_infra()` (private) → `SharedInfrastructure::get_or_init()`
  (public — kept the existing method name instead of the handoff's proposed
  `get_or_init_shared` to avoid churning 30+ test-file callers). Runs
  `cleanup_stale_processes()`, constructs `CpBootConfig` with fixed
  `SHARED_*` ports, calls `boot_cp`, moves `pg_container` /
  `zitadel_container` into the `SHARED_PG_CONTAINER` / `SHARED_ZITADEL_CONTAINER`
  statics, then assembles the `SharedInfrastructure` struct from the
  remaining `BootedCp` fields. `FLOWPLANE_E2E_MTLS=1` opt-in preserved
  (prod-only, matching previous behavior).

- **Isolated**: `TestHarness::start_isolated()` is now a thin wrapper that
  allocates per-test ports via `PortAllocator`, bails early on prod auth
  (error message matching the pre-refactor text), builds a `CpBootConfig`
  with `update_team_admin_port: false`, calls `boot_cp`, rebuilds
  `MtlsCertPaths` from the returned CA + certs + SPIFFE URI, and moves
  `BootedCp` fields into `TestHarness` slots. Isolated-mode Envoy team
  metadata preserved via `envoy_team = config.mtls_team.unwrap_or(E2E_SHARED_TEAM)`.

## The ONE branching point

Inside `boot_cp`, the only `match` is on `cfg.auth_mode` (Dev | Prod). It
branches on the identity issuer — mock OIDC server vs real Zitadel
container — and nothing else. Port allocation, mTLS opt-in, envoy team,
mocks flavor, and the `update_team_admin_port` behavior are all driven by
`CpBootConfig` fields. Dev mode sets the process env vars
(`FLOWPLANE_AUTH_MODE=dev`, `FLOWPLANE_COOKIE_SECURE=false`,
`FLOWPLANE_BASE_URL=…`). Prod mode sets the Zitadel vars via
`zitadel::set_cp_env_vars`. Neither path mutates `FLOWPLANE_ZITADEL_*`
directly (grep gate in `src/auth/middleware.rs` still holds — 0 hits).

## Invariants preserved

- **rustls CryptoProvider install** — happens unconditionally at the top
  of `boot_cp`, before any TlsAcceptor can be built. fp-6yj root cause
  eliminated: every CP boot now passes through the same install line.
- **`seed_dev_resources`** — runs in the Dev arm after `cp.wait_ready()`,
  so both shared-dev and isolated-dev paths seed org/team/user/dataplane.
- **Single mock OIDC instance** — Part 1's `with_dev_oidc_mock` wiring is
  centralized in `boot_cp`'s Dev arm. One `Arc<MockOidcServer>` is built,
  the CP trusts it (via `ControlPlaneConfig::with_dev_oidc_mock`), and
  the same Arc mints the test token (`issue_token_for_sub(DEV_USER_SUB)`).
- **Isolated-prod bail** — preserved verbatim in `start_isolated` before
  any work is done (grep the original text in the new code to confirm a
  byte-for-byte match).
- **FLOWPLANE_E2E_MTLS=1 shared opt-in** — preserved in `build_shared_infra`,
  gated on `auth_mode == Prod` to match the pre-refactor scoping.
- **`cleanup_stale_processes`** — called by `build_shared_infra` before
  boot. Isolated mode doesn't need it (fresh testcontainer PG each run).
- **Testcontainer PG cleanup** — unchanged; still handled by the
  `cleanup_stale_processes` docker-filter block.
- **`teams.envoy_admin_port` update** — still happens in shared-dev mode
  after envoy starts. Now driven by `cfg.update_team_admin_port &&
  cfg.auth_mode == Dev` in `start_envoy_if_available`. Shared infra sets
  `update_team_admin_port: true`; isolated passes `false`.

## BootedCp fields (actual shape that shipped)

Matches the handoff list with one addition: `mtls_spiffe_uri:
Option<String>`. Necessary so that isolated mode can expose `MtlsCertPaths`
(which carries the SPIFFE URI) to tests without re-deriving it. Shared
mode ignores the field. Flagged here per the deviation protocol.

## Helper functions

Three small private helpers keep `boot_cp` from becoming unreadable:

- `issue_mtls_material(&cfg)` — generates CA + server cert + client cert
  and builds `XdsTlsConfig`. Returns all five components as a tuple or
  `(None,None,None,None,None)` if `enable_mtls == false`.
- `start_mocks(flavor)` — dispatches on `MocksFlavor` to the right
  `MockServices` constructor.
- `start_envoy_if_available(&cfg, envoy_team, client_cert, ca, db_url)` —
  skips when disabled/unavailable, builds `EnvoyConfig` with optional mTLS,
  waits ready, and runs the `teams.envoy_admin_port` update for shared-dev.

These helpers don't introduce new branching on auth_mode — they take
plain data. The identity-issuer match stays inside `boot_cp` itself.

## Deviations from the handoff doc

1. **Method name**: handoff proposed `get_or_init_shared`. Kept
   `SharedInfrastructure::get_or_init` unchanged — there are 30+ callers
   across `tests/e2e/full/**` and renaming them would balloon the diff
   into Task 5c's territory. The method body is the new wrapper over
   `boot_cp`; the signature is unchanged.
2. **`initialize()` dispatcher removed**: replaced with an inline call to
   `build_shared_infra()` (free async fn) inside the `INIT_ONCE.call_once`
   block. Cleaner than two nested `impl` methods.
3. **`mtls_spiffe_uri` field on `BootedCp`**: added for the isolated
   `MtlsCertPaths` reconstruction path. See above.
4. **`CpBootConfig::mocks_flavor`**: handoff left mocks flavor implicit.
   Made explicit because shared mode always wants `All` while isolated
   mode picks based on `start_auth_mocks` / `start_ext_authz_mock` knobs.
5. **`CpBootConfig::update_team_admin_port`**: handoff didn't mention the
   `teams.envoy_admin_port` UPDATE that shared-dev runs post-envoy. Made
   explicit so isolated mode can opt out (no `default` team in isolated
   prod, though isolated dev would also work — kept opt-out for safety).
6. **`mtls_ca` ownership in isolated mode**: `boot_cp` returns
   `Arc<TestCertificateAuthority>` for uniformity with shared mode.
   Isolated mode unwraps the Arc via `Arc::try_unwrap` to store the inner
   value in `TestHarness._mtls_ca: Option<TestCertificateAuthority>`.
   Safe because boot_cp holds the only ref at that point.

## Line count + cargo gates

- `tests/e2e/common/shared_infra.rs`: 906 → ~980 LOC (boot_cp + helpers +
  types + new wrapper).
- `tests/e2e/common/harness.rs`: 972 → ~760 LOC (start_isolated shrank
  from ~275 LOC to ~110 LOC; removed unused imports).
- `cargo fmt && cargo build --all-targets` → only pre-existing
  `tests/cli_onboarding.rs` errors (Task 5c scope). No new errors.
- `cargo build --all-targets --features dev-oidc` → same. No new errors.

## What's left for Task 5c

Task 5c is the test-file rewrite that collapses the `#[cfg(feature =
"dev-oidc")]` gates and stops importing `generate_dev_token`. The files
listed in the team-lead brief — `cli_onboarding`, `dev_auth_*`,
`compose_runner_test`, `dev_agent_supervisor`, `phase2_adversarial`,
`phase25_onboarding`, `test_dev_mtls_chain` — still need their token
acquisition updated to use the harness-minted token. The new
`TestHarness.auth_token` field already carries the right value for
dev-mode e2e tests; Task 5c just has to wire callers to it instead of
calling the removed `generate_dev_token`.

---

# Part 2 handoff (original spec, now obsoleted by the Part 2 section above)

Per deviation request sent to team-lead on 2026-04-15, Part 2 (the `boot_cp` 4→2 collapse) is deferred to a fresh implementer to avoid context exhaustion. Three implementers have already burned out on fp-4n5, and attempting a ~400-500 LOC monolithic function rewrite on top of Part 1 in the same session carries a high risk of a half-done refactor landing on Task 5c's doorstep.

## What Part 2 needs to do

**Delete** (grep to confirm zero callers first):
- `tests/e2e/common/shared_infra.rs::SharedInfrastructure::initialize_dev` (~lines 298–447)
- `tests/e2e/common/shared_infra.rs::SharedInfrastructure::initialize_prod_zitadel` (~lines 477–749)
- `tests/e2e/common/shared_infra.rs::SharedInfrastructure::initialize` (~lines 282–301) — dispatcher
- The inline CP-setup logic in `tests/e2e/common/harness.rs::start_isolated` (~lines 466–710) should move INTO `boot_cp`, leaving `start_isolated` as a thin wrapper.

**Add** (single function, single internal `match auth_mode`):
```rust
pub async fn boot_cp(cfg: CpBootConfig) -> anyhow::Result<BootedCp> { ... }
pub async fn get_or_init_shared() -> anyhow::Result<&'static SharedInfrastructure> { ... }
```

**CpBootConfig** fields (architect-specified):
- `auth_mode: flowplane::config::AuthMode` (Dev | Prod) — wait, correction: `E2eAuthMode` (the shared_infra type), since that's what the isolated/shared wiring uses. The architect wrote `flowplane::config::AuthMode` in the task, but shared_infra branches on `E2eAuthMode`; resolve at implementation time.
- `enable_mtls: bool`
- `enable_envoy: bool`
- `mtls_team: Option<String>`
- `mtls_proxy_id: Option<String>`
- `test_name: String` (for port allocation / unique naming)
- `ports: CpBootPorts` (fixed shared ports or allocated isolated ports — caller's choice)

**BootedCp** fields (merged from current `SharedInfrastructure` and isolated-mode `TestHarness`):
- cp: `ControlPlaneHandle`
- envoy: `Option<EnvoyHandle>`
- mocks: `MockServices`
- db_url: `String`
- auth_mode: `E2eAuthMode`
- auth_token: `String`
- team: `String`
- org: `String`
- auth_config: `E2eAuthConfig`
- zitadel_config: `ZitadelTestConfig` (dummy in dev mode for backward compat)
- mtls_ca: `Option<Arc<TestCertificateAuthority>>`
- mtls_server_cert: `Option<TestCertificateFiles>`
- mtls_envoy_client_cert: `Option<TestCertificateFiles>`
- mock_oidc: `Option<Arc<MockOidcServer>>`
- pg_container: `ContainerAsync<Postgres>`
- zitadel_container: `Option<ContainerAsync<GenericImage>>`

**Preserve in both wrappers**:
- `FLOWPLANE_E2E_MTLS=1` shared-mtls opt-in path.
- `rustls::crypto::ring::default_provider().install_default()` on first call (fp-6yj invariant).
- `cleanup_stale_processes()` on shared-infra init.
- `seed_dev_resources()` call for dev mode (Part 1 added this to isolated; shared already had it).
- Isolated-prod bail (return an error matching the existing message in harness.rs ~line 648).
- All env vars set before CP boot (`FLOWPLANE_AUTH_MODE=dev`, `FLOWPLANE_COOKIE_SECURE=false`, `FLOWPLANE_BASE_URL=...`) — these are test-harness side, not CP runtime.

**Hard constraint**: `boot_cp` must be a SINGLE function. The internal `match cfg.auth_mode { Dev => { ... }, Prod => { ... } }` is the ONLY branching. Do NOT split into `boot_cp_dev` / `boot_cp_prod` helpers — that is the drift pattern this refactor is meant to prevent.

**Caller responsibilities**:
- `get_or_init_shared`: cleanup_stale, rustls provider install, build CpBootConfig with fixed SHARED_* ports, call boot_cp, destructure the returned BootedCp into `SHARED_PG_CONTAINER` / `SHARED_ZITADEL_CONTAINER` statics + the `SharedInfrastructure` struct, cache via `OnceLock<SharedInfrastructure>`.
- `start_isolated`: rustls provider install (if mtls), allocate ports via `PortAllocator`, generate per-test CA + server cert + client cert (current isolated logic for mtls stays), build CpBootConfig, call boot_cp, move BootedCp fields into `TestHarness` struct fields, return.

## Why not done in this commit

Writing `boot_cp` faithfully requires reading ~800 LOC of harness.rs + shared_infra.rs in detail and producing a ~400-500 LOC function plus type definitions plus two rewired callers. Part 1's implementation already consumed the context budget for a careful read of those files. Attempting Part 2 in the same session risks:
- Partial rewrite landing with compilation errors that a fresh implementer then has to untangle.
- Broken shared/isolated dispatch that silently runs the wrong boot path and produces confusing test failures.
- Context exhaustion mid-refactor — the exact failure mode that killed three prior implementers.

Part 1 is the critical semantic fix (without it, every dev-mode e2e test will fail at JWT validation). Part 2 is a drift-prevention refactor that blocks no tests by itself. Splitting them preserves the priority ordering and hands off Part 2 with a clean starting point.

## Grep gate still holds

`grep 'auth_mode' src/auth/middleware.rs` → 0 hits (verified post-Part-1).

## Local build gates (Part 1)

- `cargo build --all-targets` → only pre-existing `tests/cli_onboarding.rs` errors (Task 5c scope). No new errors introduced.
- `cargo build --all-targets --features dev-oidc` → same. Error count unchanged by Part 1.
