---
title: FP-4N5 Task 4 — Auth Unification (dev path collapses onto Zitadel middleware)
date: 2026-04-14
status: implemented
branch: feature/fp-hsk-warming-failure-detection
bead: fp-4n5 Task 4
---

# What

Deleted the dev-only `dev_authenticate` middleware + `DevAuthState` + `FLOWPLANE_DEV_TOKEN`
env-var bearer check. Dev mode now authenticates through the same `authenticate`
(Zitadel JWT) middleware used in prod, validating real OIDC JWTs minted by an
in-process `MockOidcServer` (spawned on CP startup when built with the
`dev-oidc` cargo feature). Prod mode is unchanged.

Concrete changes:

- `src/auth/zitadel.rs`: new `ZitadelConfig::from_mock(&MockOidcServer)` behind
  `#[cfg(feature = "dev-oidc")]`. Derives issuer/audience/project_id/jwks_url/
  userinfo_url from the post-bind mock (ephemeral port).
- `src/auth/middleware.rs`: deleted `dev_authenticate`, `DevAuthState`, and the
  synthetic-context test suite. `authenticate` is now the single auth entry.
- `src/auth/dev_token.rs`: deleted `generate_dev_token` and
  `resolve_or_generate_dev_token` (plus their tests). Retained `DEV_USER_SUB`,
  `DEV_USER_EMAIL`, `write_credentials_file`, `write_credentials_to_path`,
  `read_credentials_file`, `read_credentials_from_path`.
- `src/api/server.rs`: async dev mock bootstrap lives here (new
  `start_dev_mock_oidc` helper). It spawns `MockOidcServer`, builds a
  `ZitadelAuthState`, mints a token, and (when `FLOWPLANE_CREDENTIALS_PATH` is
  set) writes the credentials file via the existing `write_credentials_to_path`
  — the mock bundle is stashed in a local `DevAuthBundle` owned for the full
  lifetime of `start_api_server` so dropping it doesn't abort the JWKS server
  mid-flight.
- `src/api/routes.rs`: `build_router_with_registry` now takes a third parameter
  `zitadel_override: Option<ZitadelAuthState>`. If `Some`, it is wired into the
  `authenticate` middleware directly; if `None`, the existing env-based prod
  resolution path runs. The old `AuthMode=="dev"` branch that read `std::env`
  is gone. `tower::util::Either<Either<dev, zitadel>, reject>` collapsed to
  `tower::util::Either<zitadel, reject>`.
- `src/cli/credentials.rs`: dropped the `resolve_or_generate_dev_token`
  re-export.
- `src/cli/compose.rs`: `handle_init_with_runner` no longer generates a token
  or passes `FLOWPLANE_DEV_TOKEN` via compose env. It waits for the CP to
  become healthy, then verifies the CP wrote the credentials file.
- `tests/phase25_onboarding.rs`: `oidc_mock_flows` module now imports
  `flowplane::dev::oidc_server` and is gated on `#[cfg(feature = "dev-oidc")]`.
  `MockOidcServer::start` returns `Result`, so call sites now `.unwrap()`.
- `tests/common/mock_oidc.rs`: deleted, plus its `pub mod mock_oidc` line in
  `tests/common/mod.rs`.
- `src/auth/authorization.rs` / `src/startup.rs`: comment drift referencing
  `dev_authenticate` / `DevAuthState` updated so the grep gate catches future
  drift.

# Alternatives considered

1. **Keep `dev_authenticate` and point it at the mock OIDC issuer.**
   Rejected: still two middlewares, still two code paths, still one
   auth contract that can silently diverge from prod. The whole point of
   Task 4 is to collapse the surface area.

2. **Put the mock OIDC spawn inside `build_router_with_registry` behind a
   `tokio::runtime::Handle::block_on`.** Rejected: `build_router_with_registry`
   is sync and called under an active tokio runtime in prod — `block_on`
   from inside a runtime panics. Making it async would ripple through every
   test helper that calls `build_router`. The clean move is to let
   `start_api_server` (already async) do the spawn and pass the ready state
   in.

3. **Mutate `FLOWPLANE_ZITADEL_*` env vars in dev startup so the existing
   `ZitadelConfig::from_env()` path picks up the mock.** Rejected per the
   architect's pre-implementation warning in
   `specs/decisions/2026-04-14-fp-4n5-pre-implementation.md` (alternative #3):
   `std::env::set_var` is not thread-safe, creates a race window against any
   other thread reading the environment, and would be a *fourth* boot path
   disguised as a config knob. The explicit `zitadel_override` parameter
   makes the dev/prod branch visible at the call site instead of hiding it
   in global state.

# Why

- **One auth contract.** Dev and prod now exercise the exact same JWT
  validation, JWKS caching, user JIT-provisioning, permission loading, and
  rate-limiting logic. A regression in `authenticate` can no longer pass
  `make test-e2e-dev` while breaking prod.
- **No env-var race.** Zero `std::env::set_var` for Zitadel config anywhere
  in the production boot path (see grep-gate note below).
- **Feature-gated cleanly.** Release builds without `dev-oidc` still compile
  (verified by `cargo build`) and will explicitly fail startup if
  `FLOWPLANE_AUTH_MODE=dev` is set — no silent fallbacks.

# Gotchas

- **Credentials handoff in compose.** The CP runs in a container; the CLI
  runs on the host. In the old path the CLI generated the token and handed
  it to the CP via `FLOWPLANE_DEV_TOKEN`. In the new path the CP mints its
  own JWT from the mock and needs to write the credentials file somewhere
  the CLI can read. This requires a new rw bind mount from
  `${HOME}/.flowplane/` into the container plus a
  `FLOWPLANE_CREDENTIALS_PATH` env var that points at the container-side
  path of `credentials`. The compose YAML + env var wiring is **not part of
  this commit** — it was flagged to team-lead as a deviation. The runtime
  code (`start_dev_mock_oidc`) already honors `FLOWPLANE_CREDENTIALS_PATH`
  and is a no-op when it is unset, so compose-side changes can land in a
  follow-up without breaking anything here.
- **Mock lifetime.** `MockOidcServer` owns a tokio task that serves JWKS +
  userinfo. Dropping it aborts that task. The `DevAuthBundle` struct holds
  the mock as `_mock` and lives as a local inside `start_api_server`, which
  awaits the HTTP server future to completion — so the mock is kept alive
  for the entire server lifetime without needing `OnceLock` or `Box::leak`.
  Adding a field to `ApiState` was considered but rejected: nothing inside
  the router needs to reach back into the mock, and `ApiState` is `Clone`,
  which would either require `Arc<MockOidcServer>` churn or a new bound
  that leaks out through every handler.
- **Grep gate — `std::env::set_var` for `FLOWPLANE_ZITADEL_*`.** The gate
  still matches 17 hits, all inside `#[cfg(test)] mod tests { ... }` blocks
  in `src/auth/zitadel_admin.rs` and `src/api/handlers/oauth.rs`. These are
  pre-existing test fixtures, not production code, and are not reachable
  from any runtime boot path. They were not introduced by this task and are
  left as-is. The boot-path constraint (zero `set_var` for Zitadel during
  CP startup) is satisfied.
- **Tests in `tests/dev_auth_*`, `tests/cli_onboarding.rs`, `tests/e2e/common/
  harness.rs`, `tests/compose_runner_test.rs`, `tests/dev_agent_supervisor.rs`,
  `tests/e2e/smoke/test_dev_mtls_chain.rs`, `tests/phase2_adversarial.rs`**
  all still reference `FLOWPLANE_DEV_TOKEN` or the deleted
  `generate_dev_token` symbol. These live in the integration/e2e test
  crate and are compiled under `cargo test`, not `cargo build`. They will
  be rewritten by **Task 5** (harness collapse) and by the verifier when
  reworking the dev-auth test suites to exercise the mock OIDC path. This
  commit intentionally does not touch them — verifier owns `cargo test` and
  harness rewrites are out of scope.

# Grep gate results

```
grep -rn FLOWPLANE_DEV_TOKEN                       src/   → 0 hits
grep -rn dev_authenticate                          src/   → 0 hits
grep -rn DevAuthState                              src/   → 0 hits
grep -rn 'std::env::set_var.*FLOWPLANE_ZITADEL'    src/   → 17 hits (all #[cfg(test)], pre-existing)
grep -rn generate_dev_token                        src/   → 0 hits
grep -rn resolve_or_generate_dev_token             src/   → 0 hits
```

# Build gate results

- `cargo build` → clean
- `cargo build --features dev-oidc` → clean (one unrelated `num-bigint-dig`
  future-incompat warning from a transitive dep)
