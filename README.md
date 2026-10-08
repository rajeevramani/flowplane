# Flowplane

[![CI](https://github.com/rajeevramani/flowplane/actions/workflows/ci.yml/badge.svg)](https://github.com/rajeevramani/flowplane/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.94.1-orange.svg)](https://www.rust-lang.org/)

**Flowplane** — an API gateway built for humans and AI agents.

Publish your APIs through a multi-tenant control plane and get governance (OIDC auth, grant-based RBAC, audit), a deterministic Envoy data plane driven over xDS, schema learning that infers OpenAPI from live traffic, and an AI gateway that fronts LLM providers with token budgets. Drive it from a CLI or REST API.

> A ground-up Rust/PostgreSQL rebuild. PostgreSQL is the source of truth, Envoy is the only data plane, xDS/SDS is the config channel, and every product mutation goes through `fp-core` services.

## Quick Start (no clone, no Rust toolchain)

Install local evaluation infrastructure with **no exposed APIs**, then explicitly expose and call a backend through Envoy. No checkout or `cargo build` is needed for the published-artifact path.

> **3.2.0 release preparation:** use these commands only after `v3.2.0` and its matching eval image are published. Packaged-candidate qualification is pending. Older `3.1.x` bundles automatically expose the demo and do not implement this empty-install/shared-exposure journey. Docker Compose is used below; Podman/runtime and host-networking differences need their own verification.

### Install and define the CLI shortcut

Use a fresh directory, then keep this shell open:

```sh
mkdir flowplane-evaluation
cd flowplane-evaluation
VER=3.2.0
curl -fsSLO "https://raw.githubusercontent.com/rajeevramani/flowplane/v${VER}/compose.eval.yml"
export FLOWPLANE_EVAL_IMAGE="ghcr.io/rajeevramani/flowplane:${VER}-eval"
docker compose -f compose.eval.yml up -d --no-build

fp() {
  docker compose -f compose.eval.yml exec -T flowplane-eval \
    sh -ec 'export FLOWPLANE_SERVER=http://127.0.0.1:8080 FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default; FLOWPLANE_TOKEN="$(cat /shared/dev-token)"; export FLOWPLANE_TOKEN; exec flowplane "$@"' sh "$@"
}

# Infrastructure readiness is separate from traffic readiness.
fp auth whoami
fp dataplane get dp-eval
fp cluster list
fp route list
fp listener list
```

Wait for authenticated readiness and non-null `last_heartbeat_at`. For a fresh stack, all three gateway lists are empty. Port `10000` is published but has no Envoy listener yet; a gateway request must not return the sample body before you expose it. The [tutorial](docs/tutorials/evaluate-no-clone.md#1-install-infrastructure-not-apis) explains readiness and diagnostics.

### Deploy the first service

```sh
fp expose http://demo-upstream:5678 --name demo --path / --port 10000 \
  --public-base-url http://127.0.0.1:10000
# After xDS converges, expect: hello from the flowplane eval demo upstream
curl --max-time 5 -fsS http://127.0.0.1:10000/
```

Continue the [no-clone tutorial](docs/tutorials/evaluate-no-clone.md) for a bounded expected-body check, a real host backend, and `fp expose ... --listener demo` to share the existing port. The bundle publishes only gateway port `10000` by default. For an additional independent listener, [publish another container port explicitly](docs/how-to/expose-an-api.md#publish-an-additional-evaluation-port); auto-allocation does not change Compose mappings.

### Next steps

- **Add a local rate limit:** the [tutorial's policy experiment](docs/tutorials/evaluate-no-clone.md#4-add-a-local-rate-limit-and-prove-it-works) checks actual 429 and recovery.
- **Protect traffic with OIDC/JWT:** [install the JWT filter and route requirement](docs/how-to/jwt-auth-rate-limit-route.md). Management authentication does not automatically protect your API, and the dev management token is not a backend credential.
- **Optional dashboard:** `docker compose -f compose.eval.yml exec -T flowplane-dashboard cat /shared/dashboard-url` prints its nonce-protected loopback URL. It is not required for traffic.
- **Optional API-to-MCP:** [import and publish a spec](docs/how-to/import-and-publish-openapi-spec.md), with explicit route bindings for execution. Tool generation/status alone does not prove a backend call.

### Remove or stop

For the single untouched sample exposure, deliberately confirm removal:

```sh
fp --yes unexpose demo
# Stop infrastructure while preserving state/volumes.
docker compose -f compose.eval.yml down
```

Shared exposures retain their surviving routes/listener/config. Final managed cleanup also removes later policy edits; manual/legacy scaffolds are not adopted by matching names. See [safe removal](docs/how-to/expose-an-api.md#remove-the-exposure-without-deleting-somebody-elses-infrastructure). Use `docker compose -f compose.eval.yml down -v` only to intentionally destroy this evaluation's database and PKI volumes.

> The eval image runs dev mode, with an in-process issuer, seeded identities and a bearer token on disk. Host ports bind to `127.0.0.1`; xDS uses mTLS. It is **never** a production base and is never tagged `:latest`. The hardened image `ghcr.io/rajeevramani/flowplane:${VER}` is built `--no-default-features` and refuses dev mode.

## Build from source (contributors)

Working on Flowplane itself? Build the binary and run the control plane directly.

> **Toolchain:** build through [rustup](https://rustup.rs) so the `rust-toolchain.toml` pin (1.94.1) is applied automatically. A distro-packaged `cargo` may be too old to read this repo's version-4 `Cargo.lock`.
>
> **Prerequisites:** a reachable PostgreSQL (`postgres://postgres:postgres@127.0.0.1:5432/flowplane_dev`), a local `envoy` binary on your `PATH`, and a Rust toolchain via rustup. On macOS/Homebrew create the `postgres` role first — see the [tutorial](docs/tutorials/getting-started.md#1-prerequisites).

```bash
# Build (the default `dev-oidc` feature enables dev mode)
cargo build --bin flowplane

# 1. Start the control plane in dev mode (in-process OIDC + seeded resources)
FLOWPLANE_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/flowplane_dev \
  FLOWPLANE_DEV_MODE=true \
  FLOWPLANE_API_INSECURE=true \
  FLOWPLANE_API_ADDR=127.0.0.1:8096 \
  FLOWPLANE_XDS_ADDR=0.0.0.0:18000 \
  ./target/debug/flowplane serve
```

Dev mode logs a 24-hour `dev_token` once at boot (configurable with
`FLOWPLANE_DEV_TOKEN_TTL`). In a second terminal:

```bash
export FLOWPLANE_SERVER=http://127.0.0.1:8096
export FLOWPLANE_ORG=dev-org
export FLOWPLANE_TEAM=default
export FLOWPLANE_TOKEN='<paste the dev_token from the server log>'

./target/debug/flowplane auth whoami        # confirm authentication
```

Start a trivial upstream, expose it, point Envoy at the control plane, and verify:

```bash
# Trivial upstream (third terminal)
mkdir -p /tmp/fp-upstream && cd /tmp/fp-upstream
printf 'hello-flowplane\n' > index.html && python3 -m http.server 3001

# Expose it — creates cluster + route config + listener in one command
./target/debug/flowplane expose http://127.0.0.1:3001 \
  --name local --path / --port 10001 \
  --public-base-url http://127.0.0.1:10001

# Register a dataplane and generate the dev Envoy bootstrap (--out is global, before the subcommand)
./target/debug/flowplane dataplane create dp-local --description "local Envoy"
./target/debug/flowplane --out /tmp/flowplane-envoy.yaml \
  dataplane bootstrap dp-local --mode dev \
  --xds-host 127.0.0.1 --xds-port 18000 --admin-port 9901

# Start Envoy (its own terminal)
envoy -c /tmp/flowplane-envoy.yaml --log-level info

# Verify: this request flows through Envoy (:10001) to your upstream (:3001)
curl -i http://127.0.0.1:10001/        # -> 200 OK, body: hello-flowplane
```

Tear down the exposure's route/upstream with `flowplane unexpose local`. Other routes keep their listener/configuration; final managed-scaffold cleanup deletes its listener/configuration and subsequent policy edits. Legacy/manual matching names are never shortcut-deleted. The full walkthrough with every check is in the [Getting Started tutorial](docs/tutorials/getting-started.md).

> Dev mode runs an in-process identity issuer over plaintext — local exploration only, never production. The published release container is built `--no-default-features` and rejects dev mode entirely.

## Architecture

```mermaid
graph LR
    subgraph Configure["Configure (control plane)"]
        Op[Developer / Operator / AI Agent]
    end
    subgraph Call["Call (data plane)"]
        Cl[Service / Client]
    end

    Op -->|REST · CLI| FP[Flowplane control plane]
    FP <--> PG[(PostgreSQL)]
    FP -->|gRPC xDS / SDS| Envoy[Envoy data plane]

    Cl -->|HTTP| Envoy
    Envoy -->|HTTP| US[Upstream services / LLM providers]
```

Flowplane is the **control plane**: it stores gateway configuration (clusters, routes, listeners, filters, secrets) in PostgreSQL and pushes it to Envoy over xDS. It is out-of-band of request traffic — clients call Envoy directly, and Envoy proxies to upstreams. PostgreSQL is the single source of truth; the same database state always produces the same Envoy configuration bytes.

## Key Features

- **Multi-tenant by construction** — organizations own teams; teams own gateway resources. Every tenant query names whose data it touches (`TeamScope`), so an unscoped cross-tenant query is not representable. Cross-tenant existence is hidden (`404`, not `403`).
- **Grant-based authorization** — one pure-function gate decides every access on every surface (REST, CLI). Access is a decision over a closed `(resource, action, team)` grant vocabulary; every decision returns a stable reason for audit.
- **Provider-neutral OIDC auth** — works with any compliant IdP (Auth0, Keycloak, Okta, Entra) plus an in-process dev mock for local runs. JWTs are identity-only; all authorization comes from the database.
- **Deterministic xDS data plane** — CDS/RDS/LDS/EDS/SDS over ADS. Stable encoding, per-type versions that bump only on real byte changes, and NACK quarantine that serves last-known-good rather than blanking a resource type. mTLS/SPIFFE identity, per-dataplane scoping.
- **HTTP filter chain** — a closed set of nine filter kinds: CORS, local and global rate limit, header mutation, health check, compressor, JWT auth, ext authz, and RBAC. Per-route overrides supported; chain order is semantic.
- **API schema learning + discovery** — capture live traffic and infer JSON schemas with confidence scoring, exported as OpenAPI 3.1. *Learning* enriches an existing API definition; *discovery* spins up a throwaway listener and creates new API definitions from observed traffic.
- **AI gateway** — register LLM providers (OpenAI / OpenAI-compatible), publish AI routes, and cap token spend with budgets that run in `shadow` (observe-only) then `enforcing`. Provider credentials are encrypted at rest.
- **REST API + CLI** — a JSON API and a full-surface `flowplane` CLI covering auth/context, org/team management, gateway resources, expose/unexpose, learning, AI, secrets, dataplane registration, and ops diagnostics. Print the exact contract with `flowplane openapi`.
- **Read-only team dashboard** — run `flowplane dashboard` for Overview, Resources, APIs, Learning, AI, MCP, and Operations views using the CLI's resolved credentials. The bearer token stays in the CLI process and every browser route requires a per-launch nonce.

> An MCP control-plane surface is present in the codebase and evolving. The dashboard is a CLI-hosted read-only presentation layer over the existing REST API, not a second control plane.

## Documentation

The [documentation home](docs/README.md) is organised by [Diátaxis](https://diataxis.fr/) mode. Start here:

| You want to… | Start here |
|--------------|------------|
| Try Flowplane without cloning the repo | [Evaluate without cloning](docs/tutorials/evaluate-no-clone.md) |
| Expose your API or share a listener | [Expose an API](docs/how-to/expose-an-api.md) |
| Evaluate a production-shaped platform setup | [Evaluate a production-shaped platform setup](docs/how-to/evaluate-platform.md) |
| Delegate API onboarding to a team | [Onboard an API team](docs/how-to/onboard-api-team.md) |
| Stand up a gateway from a clean checkout | [Getting Started](docs/tutorials/getting-started.md) |
| Protect a route with JWT auth + rate limit | [JWT auth & rate limit](docs/how-to/jwt-auth-rate-limit-route.md) |
| Cap a route globally across all Envoys | [Enable global rate limiting](docs/how-to/global-rate-limit.md) |
| Learn an API spec from live traffic | [Learn & publish an API spec](docs/how-to/learn-and-publish-api-spec.md) |
| Front an LLM with a token budget | [AI gateway route & budget](docs/how-to/ai-gateway-route-budget.md) |
| Inspect a team's gateway in the dashboard | [View your team's gateway dashboard](docs/how-to/view-team-dashboard.md) |
| Secure the data plane with mTLS | [Register a dataplane (mTLS)](docs/how-to/register-dataplane-mtls.md) |
| Understand tenancy, grants, and xDS | [Tenancy, grants & the xDS pipeline](docs/concepts/tenancy-grants-xds.md) |
| Understand global rate limiting | [Global rate limiting](docs/concepts/global-rate-limiting.md) |

Reference: [CLI](docs/reference/cli.md) · [Configuration](docs/reference/configuration.md) · [REST API](docs/reference/rest-api.md) · [Filters](docs/reference/filters.md) · [Errors](docs/reference/errors.md) · [Adoption issue map](docs/reference/adoption-evaluation-issue-map.md)

## Building and Testing

Build the main binary:

```bash
cargo build --bin flowplane
```

Run tests for the main binary:

```bash
cargo test -p flowplane
```

Run the full workspace suite with PostgreSQL-backed tests enabled. CI uses
[`cargo nextest`](https://nexte.st) (faster; the same suite); install it with
`cargo install cargo-nextest --locked` or `cargo binstall cargo-nextest`:

```bash
export FLOWPLANE_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/flowplane_test

cargo nextest run --workspace --all-features   # what CI runs (via the `ci` profile)
cargo test --workspace --all-features --doc    # doctests — nextest does not run these

# plain cargo test still works and additionally runs doctests inline:
cargo test --workspace --all-features
```

Print the generated REST contract:

```bash
./target/debug/flowplane openapi
```

> Workspace tests read the DB URL from `FLOWPLANE_TEST_DATABASE_URL`. The `scripts/ensure-postgres.sh` helper assumes a Linux/container setup and does not create the `postgres` role; on macOS/Homebrew create it yourself (see [Getting Started](docs/tutorials/getting-started.md#1-prerequisites)).

## License

Apache-2.0.
