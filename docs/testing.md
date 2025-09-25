# Testing & Validation

Flowplane ships with unit tests, integration checks, and helper scripts to validate configuration flows.

## Unit Tests
Run the full suite:

```bash
cargo test
```

The tests cover:

* Filter conversions (`src/xds/filters/http/*::tests`) ensuring structured configs map to Envoy protos and reject invalid payloads.
* Route and listener translation to Envoy types.
* API handler contract tests verifying REST endpoints persist resources correctly.
* Storage layer helpers (migrations, pools).

Clippy is enforced with warnings-as-errors:

```bash
cargo clippy -- -D warnings
```

## Smoke Testing
`scripts/smoke-listener.sh` orchestrates a full control-plane workflow:

1. Creates a cluster pointing at `httpbin.org` with TLS metadata.
2. Registers a listener + route through the REST API.
3. Issues a curl against Envoy (`http://localhost:10000/status/200`).

Use it after major changes to ensure ADS propagation still works end-to-end.

## Manual Validation Tips
* Inspect generated resources via `GET /api/v1/listeners` or `GET /api/v1/routes` to confirm filter entries.
* Enable Envoy admin interface (`/config_dump`) to verify filters, JWKS metadata, and scoped overrides.
* When working with JWT auth, start with a static JWKS (`local_jwks.inlineString`) before layering remote fetch, async fetch, and retry policies.

## Database
The default SQLite database file lives under `./data`. The migrations harness (`cargo run --bin run_migrations`) ensures schema consistency during development.
