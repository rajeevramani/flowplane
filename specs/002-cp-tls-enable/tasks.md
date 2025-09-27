# Tasks: TLS Bring-Your-Own-Cert MVP

These implementation tasks track what is required for the 002-cp-tls-enable feature branch to ship a usable "bring your own certificate" TLS capability for the admin API. Items are grouped so we can commit at natural checkpoints.

## 1. Configuration & Validation
- [x] Add `ApiTlsConfig` loader gated by `FLOWPLANE_API_TLS_ENABLED` with PEM path parsing (`src/config/tls.rs`).
- [x] Surface TLS configuration on the API server struct and propagate through `Config::from_env` (`src/config/mod.rs`).
- [x] Define a focused `TlsError` enum for certificate/Key validation failures (`src/errors/tls.rs`).
- [x] Implement `load_certificate_bundle` with metadata extraction, expiry checks, and key pair validation (`src/utils/certificates.rs`).

## 2. Server Wiring
- [x] Introduce optional TLS startup path in `start_api_server`, logging subject/expiry on success (`src/api/server.rs`).
- [x] Wrap the TCP listener with a rustls acceptor that retries on handshake errors and preserves graceful shutdown (`src/api/server.rs`).
- [ ] Mirror HTTP and HTTPS listener metrics/log counters once observability surfaces those hooks (follow-up).

## 3. Testing
- [x] Add unit coverage for TLS config parsing and certificate loader edge cases (`tests/tls/unit/*`).
- [x] Add integration coverage proving HTTP fallback and HTTPS happy-path requests using fixtures (`tests/tls/integration/test_api_tls.rs`).
- [x] Add an integration test exercising broken certificate/key pairs using the ephemeral fixture helper.

## 4. Tooling & Fixtures
- [x] Provide helper to generate ephemeral TLS certificate/key fixtures for tests without committing private keys.
- [x] Document OpenSSL command used to refresh local fixtures for manual testing under `docs/dev/tls-fixtures.md`.

## 5. Documentation & Release Checklist
- [ ] Author "Bring Your Own Certificate" guide covering ACME vs corporate PKI, renewal cadence, and local dev tips (`docs/tls.md`).
- [ ] Update quickstart/README with TLS enablement section and environment variable table.
- [ ] Capture operational follow-up items (rotation automation, observability hooks) in `specs/002-cp-tls-enable/spec.md` notes.

## 6. Validation Before Tagging v0.0.1
- [ ] Re-run `cargo fmt` and `CARGO_NET_OFFLINE=true cargo test --tests` before each checkpoint commit.
- [ ] Smoke test binary locally to confirm HTTPS listener works with regenerated fixtures.
- [ ] Stage commits logically: (1) config + utils + unit tests, (2) server wiring + integration tests, (3) docs/update guides.

> Remaining tasks marked `[ ]` are blockers for the MVP release. Completed items keep the history of what landed on this branch so far.
