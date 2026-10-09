# Contributing to Flowplane

Run these commands from the repository root. For prerequisites and a complete local gateway walkthrough, follow [Build and run from source](docs/tutorials/build-and-run-from-source.md). That tutorial is the canonical from-source setup; this page covers contributor build and verification commands.

## Build

Use [rustup](https://rustup.rs) so the repository's `rust-toolchain.toml` pin is applied. An older distro-packaged Cargo may not read the version-4 lockfile.

```sh
cargo build --bin flowplane
```

Expected: a successful build with the debug binary at `target/debug/flowplane` when using the default target directory. The default `dev-oidc` feature enables local dev mode; this is not a production configuration. See the from-source tutorial for starting the control plane, configuring authentication and connecting Envoy.

## Test

Run tests for the main package:

```sh
cargo test -p flowplane
```

For the full workspace suite, use a **dedicated disposable PostgreSQL test database**, not a production or shared customer database. Prepare a reachable database and role before running. On macOS/Homebrew, see the [PostgreSQL prerequisites](docs/tutorials/build-and-run-from-source.md#1-prerequisites); `scripts/ensure-postgres.sh` assumes a Linux/container setup and does not create the `postgres` role on macOS.

The fault-injection suites create and drop uniquely named scratch databases, so the disposable test role needs `CREATEDB` and ownership of those databases. Most database-backed suites also require `FLOWPLANE_SECRET_ENCRYPTION_KEY`.

Workspace database-backed tests read `FLOWPLANE_TEST_DATABASE_URL`. Replace this local example with your own test database connection if needed; do not publish credentials:

```sh
export FLOWPLANE_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/flowplane_test
```

Set a fresh local test encryption key for this terminal session (requires OpenSSL; do not print or publish the key):

```sh
export FLOWPLANE_SECRET_ENCRYPTION_KEY="$(openssl rand -hex 16)"
```

Install [cargo-nextest](https://nexte.st) if it is not already available:

```sh
cargo install cargo-nextest --locked
```

Run the workspace suite with the repository's CI profile:

```sh
cargo nextest run --profile ci --workspace --all-features
```

Nextest does not run doctests. Run them separately:

```sh
cargo test --workspace --all-features --doc
```

Alternatively, plain Cargo runs workspace tests and doctests together:

```sh
cargo test --workspace --all-features
```

Inspect the test output and failures. A successful run without the test database configured is not evidence that PostgreSQL-backed test bodies ran. Resolve failures before claiming verification; do not suppress configured database failures.

## Inspect the generated API contract

After building the main binary with the default target directory:

```sh
./target/debug/flowplane openapi
```

Expected: the generated REST API contract is printed to standard output. For the user-facing API documentation, see the [REST API reference](docs/reference/rest-api.md).
