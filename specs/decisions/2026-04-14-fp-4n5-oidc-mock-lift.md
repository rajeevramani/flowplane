# fp-4n5 Task 2: Lift Mock OIDC Server to Production Module

**Date**: 2026-04-14
**Bead**: fp-4n5 Task 2
**Author**: implementer2
**Status**: Implemented

## What

Lifted `tests/common/mock_oidc.rs` to `src/dev/oidc_server.rs`, gated behind a
new `dev-oidc` Cargo feature. Replaced the `openssl` CLI shell-out keygen with
pure-Rust 2048-bit RSA generation via the `rsa` crate. Retained all 15
test-module cases in the new location. `tests/common/mock_oidc.rs` is left
unchanged — Task 4 handles the import redirect.

Files changed:

- `Cargo.toml` — added `dev-oidc` feature; added `rsa = { version = "0.9", optional = true }` as a `dep:rsa` activation.
- `src/lib.rs` — declared `#[cfg(feature = "dev-oidc")] pub mod dev;`.
- `src/dev/mod.rs` (new) — module root, cfg-gated upstream in `lib.rs`.
- `src/dev/oidc_server.rs` (new) — production copy of the mock.
- `specs/decisions/2026-04-14-fp-4n5-oidc-mock-lift.md` (this doc).

## Alternatives considered

### A. `rsa = { features = ["pkcs1"] }` as specified in the brief

**Rejected** — first `cargo build --features dev-oidc` failed with:

    package `flowplane` depends on `rsa` with feature `pkcs1` but
    `rsa` does not have that feature.

In `rsa` 0.9.8, the `pkcs1` module (`EncodeRsaPrivateKey`, etc.) is always
compiled and exposed at the crate root. There is no `pkcs1` cargo feature to
opt into. The brief specified this feature flag because it was plausible from
the docs, but the crate's feature list doesn't include it. Dropping the
`features = ["pkcs1"]` specifier fixed the resolve and produced a working
build. All `rsa::pkcs1::EncodeRsaPrivateKey` and `rsa::traits::PublicKeyParts`
imports resolve as expected.

This was a trivial build-resolve fix, not an architectural deviation — the
functional shape the brief asked for (`to_pkcs1_der` → `EncodingKey::from_rsa_der`)
is preserved exactly.

### B. `GeneratedRsaKey` struct vs 3-tuple return

**Chose struct.** The brief permitted either, and a 2048-bit keygen helper
that returns `(Vec<u8>, String, String)` is genuinely unreadable at the call
site — the fields are not positional in any obvious way. The struct names
`private_key_der` / `n_b64` / `e_b64` cost ~8 lines and remove a guessing
hazard for future readers. Destructured at the single call site in
`start_on`, so no ergonomic cost.

### C. Keep the original signature `fn generate_rsa_keypair() -> (Vec<u8>, String, String)`

**Rejected.** See B. Also, the original returned infallibly (via `.expect`)
because it could panic — that is not acceptable in production code per
CLAUDE.md. The new signature is `Result<GeneratedRsaKey>` so bind/keygen/DER
failures surface as `FlowplaneError::Internal`, consistent with the rest of
the crate's error handling.

### D. `MockOidcServer::start` returning `Self` vs `Result<Self>`

**Chose `Result<Self>`.** The original returned `Self` and panicked on bind
failure via `.expect("failed to bind mock OIDC server")`. Production callers
(CP startup in dev mode) must be able to handle "port 0 bind failed" without
crashing the process. All tests updated to `.await.unwrap()` at their call
sites (acceptable in `#[cfg(test)]`).

### E. Leave `issue_token` as infallible by swallowing the build_jwt error

**Rejected.** `build_jwt` legitimately fails on JWT encoding errors; the
original used `.expect` which is forbidden. New signature is
`pub async fn issue_token(&self) -> Result<String>`. Callers (currently only
tests; Task 4 wires CLI init) will propagate the error.

### F. Remove the `set_failure_mode` no-op method

**Chose remove.** The original had a stub method with a `let _ = mode;` body
that did nothing, kept around as a test API placeholder. Dead code is
prohibited in the new module (no `#![allow(dead_code)]` at the top), so I
deleted the no-op entirely. No test referenced it — grep verified.

## Why

1. **Feature gating**: `dev-oidc` ensures production release builds that do
   not opt in pay zero compile cost for the mock. `rsa`, `pkcs1`,
   `num-bigint-dig` all pull into the dep graph only when the feature is
   active. Verified via the clean default-features build path.

2. **Pure-Rust keygen**: the previous openssl CLI shell-out was the primary
   reason the mock was test-only. The three subprocess calls are:
   - fragile (assumes `openssl` on PATH; assumes the specific output format
     of `openssl rsa -modulus`)
   - slow (three fork+exec)
   - not portable to stripped container images
   The `rsa` crate replacement is ~15 lines, deterministic, and has no
   runtime dependencies beyond what the binary already ships with.

3. **Constants alignment**: `UserInfo::default()` now reads
   `DEV_USER_SUB` / `DEV_USER_EMAIL` from `crate::auth::dev_token`, so tokens
   the mock issues match the row seeded by `seed_dev_resources` — no more
   drift between `test-user-001` and `dev-sub`. The two test assertions that
   asserted the old hard-coded literals were updated to compare against the
   constants.

4. **Ephemeral port gotcha**: `MockOidcState.base_url` is still written
   **after** `TcpListener::bind` returns and **before** `tokio::spawn` of the
   serve loop, so any caller (including `ZitadelConfig::from_mock` in Task 4)
   that reads `mock.jwks_url()` immediately after `start()` returns sees the
   correctly-populated URL. This is called out in a load-bearing comment in
   the source so future edits do not reorder it.

## Gotchas

### G1: `rsa` 0.9 has no `pkcs1` cargo feature

The brief specified `features = ["pkcs1"]` but that feature does not exist
in `rsa` 0.9.8. Use `rsa = { version = "0.9", optional = true }` with no
features list. The `EncodeRsaPrivateKey` trait is re-exported at the crate
root unconditionally. Verified by successful build.

### G2: Warning on `num-bigint-dig` future-incompat

`cargo build --features dev-oidc` emits:

    warning: the following packages contain code that will be rejected by a
    future version of Rust: num-bigint-dig v0.8.4

This is a transitive dependency of `rsa`, not code we control. The warning
is informational and does not affect the build. If a future Rust release
breaks this, the fix is a `rsa` crate bump. Not actionable now.

### G3: Old file stays put

Per brief §9, `tests/common/mock_oidc.rs` is NOT deleted or rewritten in
this commit. `tests/phase25_onboarding.rs` still imports it via
`tests/common/mod.rs`. Task 4 will handle the redirect. A verifier who
searches for the pattern will find two copies; that is expected and
temporary.

### G4: `#[cfg(feature = "dev-oidc")]` applied at module root only

I gate only at `src/lib.rs` (`#[cfg(feature = "dev-oidc")] pub mod dev;`).
I do NOT re-apply the cfg on every function inside `oidc_server.rs`. The
entire module is gated by the declaration in `lib.rs` — if the feature is
off, the module file is never parsed, so per-item gating is redundant.
Verified: `cargo build` (no features) succeeds; `cargo build --features dev-oidc`
succeeds.

### G5: Test `#[cfg(test)] mod tests` still uses `unwrap()`

Production code in this module has zero `unwrap`/`expect`. The 15 lifted
unit tests keep the original `.unwrap()` style — tests are allowed to panic
on failure. `grep 'unwrap\|expect' src/dev/oidc_server.rs` returns matches
only in the `#[cfg(test)]` block, which is the project-wide convention.
