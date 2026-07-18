# Flowplane

[![CI](https://github.com/rajeevramani/flowplane/actions/workflows/ci.yml/badge.svg)](https://github.com/rajeevramani/flowplane/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.94.1-orange.svg)](https://www.rust-lang.org/)

**Flowplane** — an API gateway built for humans and AI agents.

Publish your APIs through a multi-tenant control plane and get governance (OIDC auth, grant-based RBAC, audit), a deterministic Envoy data plane driven over xDS, schema learning that infers OpenAPI from live traffic, and an AI gateway that fronts LLM providers with token budgets. Drive it from a CLI or REST API.

> A ground-up Rust/PostgreSQL rebuild. PostgreSQL is the source of truth, Envoy is the only data plane, xDS/SDS is the config channel, and every product mutation goes through `fp-core` services.

## Quick Start (no clone, no Rust toolchain)

Evaluate Flowplane on a clean machine with only a container engine (Docker or Podman). This pulls the
published **evaluation** image and stands up the whole stack — Postgres, the dev-mode control plane, a
demo upstream, and Envoy — then routes a real request through the gateway. No repo checkout, no
`cargo build`.

> Set `VER` to a published release. The example below uses `3.1.0`, whose evaluator bundle and
> `:${VER}-eval` image are published for `linux/amd64` and `linux/arm64` (the dashboard step
> needs `3.1.0` or newer). For newer releases, use the version shown on the GitHub Releases
> page. The image is **multi-arch**: a plain `docker pull` resolves the native variant — no
> `--platform` flag, no emulation.

```bash
VER=3.1.0

# 1. Fetch the evaluator bundle at the matching release tag (the only file you need)
curl -fsSLO https://raw.githubusercontent.com/rajeevramani/flowplane/v${VER}/compose.eval.yml

# 2. Bring up the whole stack against the published eval image (no --build)
FLOWPLANE_EVAL_IMAGE=ghcr.io/rajeevramani/flowplane:${VER}-eval \
  docker compose -f compose.eval.yml up -d --no-build

# 3. A request flows through Envoy (:10000) to the demo upstream
curl http://127.0.0.1:10000/        # -> hello from the flowplane eval demo upstream

# 4. Open the read-only dashboard (the URL carries a per-launch security nonce)
docker compose -f compose.eval.yml exec flowplane-dashboard cat /shared/dashboard-url
# -> open the printed http://127.0.0.1:8081/<nonce>/ in your browser

# 5. (optional) confirm authentication from inside the control-plane container
docker compose -f compose.eval.yml exec flowplane-eval \
  sh -c 'FLOWPLANE_TOKEN=$(cat /shared/dev-token) flowplane auth whoami'

# Tear down
docker compose -f compose.eval.yml down -v
```

Next, continue the no-clone evaluation with [Evaluate Flowplane without cloning the repo](docs/tutorials/evaluate-no-clone.md) to try the CLI, import an OpenAPI document, publish it, and verify the generated API tools.

> The `:${VER}-eval` image is **for evaluation only** — it runs dev mode (in-process OIDC issuer +
> seeded resources + a dev bearer token on disk) and binds every port to `127.0.0.1`. It is **never**
> an operator/production base and is never tagged `:latest`. The hardened, publishable image is
> `ghcr.io/rajeevramani/flowplane:${VER}` (built `--no-default-features`, which refuses dev mode).

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

Tear it down with `flowplane unexpose local`. The full walkthrough with every check is in the [Getting Started tutorial](docs/tutorials/getting-started.md).

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

> An MCP control-plane surface is present in the codebase and evolving; a web dashboard is planned (the REST API already backs one).

## Documentation

The [documentation home](docs/README.md) is organised by [Diátaxis](https://diataxis.fr/) mode. Start here:

| You want to… | Start here |
|--------------|------------|
| Try Flowplane without cloning the repo | [Evaluate without cloning](docs/tutorials/evaluate-no-clone.md) |
| Evaluate a production-shaped platform setup | [Evaluate a production-shaped platform setup](docs/how-to/evaluate-platform.md) |
| Delegate API onboarding to a team | [Onboard an API team](docs/how-to/onboard-api-team.md) |
| Stand up a gateway from a clean checkout | [Getting Started](docs/tutorials/getting-started.md) |
| Protect a route with JWT auth + rate limit | [JWT auth & rate limit](docs/how-to/jwt-auth-rate-limit-route.md) |
| Cap a route globally across all Envoys | [Enable global rate limiting](docs/how-to/global-rate-limit.md) |
| Learn an API spec from live traffic | [Learn & publish an API spec](docs/how-to/learn-and-publish-api-spec.md) |
| Front an LLM with a token budget | [AI gateway route & budget](docs/how-to/ai-gateway-route-budget.md) |
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
