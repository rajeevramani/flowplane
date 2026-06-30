# Dev Control Plane and Dataplane Runbook

This is the current manual V2 path for validating the core gateway loop before S8 learning work continues. It intentionally documents the rough edges as they exist today.

Do not use this as a production deployment guide. Dev mode uses plaintext API/xDS and a boot-local development token. Non-dev xDS is mTLS-only.

## Prerequisites

- PostgreSQL reachable as `postgres://postgres:postgres@127.0.0.1:5432/flowplane_dev`
- `cargo`
- `curl`
- `python3`
- Envoy, either as a local `envoy` binary or Docker running an Envoy image

On macOS, prefer a local Envoy binary for this manual path. Docker host networking is not reliable across all Docker Desktop setups.

## Dataplane Lifecycle Decision

Before S8 learning resumes, the supported V2 dataplane lifecycle is manual local Envoy started from `flowplane dataplane bootstrap` output. V2 does not currently provide `dataplane up/down/status`.

That is intentional for this phase: it keeps the product contract on registered dataplanes, xDS/SDS, and persisted diagnostics instead of porting V1's compose orchestration. A V2-native lifecycle wrapper can be added later in packaging/S12 if the validated smoke path needs it.

## 1. Start PostgreSQL

The helper is idempotent:

```bash
scripts/ensure-postgres.sh
```

It ensures the `flowplane_dev` database exists and sets the local `postgres` password to `postgres`.

> **Prerequisite:** `scripts/ensure-postgres.sh` targets a Linux / container PostgreSQL
> that has a `postgres` superuser role (it uses `service postgresql start` and `su postgres`).
> It does **not** create that role. On a fresh **macOS / Homebrew** install there is no
> `postgres` role (the superuser is your OS user), so the helper — and the documented
> `postgres://postgres:postgres@…` URL — will fail with `role "postgres" does not exist`.
> Create the role and database once:
>
> ```bash
> # macOS / Homebrew (brew services start postgresql first)
> createuser -s postgres                                   # superuser role named 'postgres'
> psql -d postgres -c "ALTER USER postgres PASSWORD 'postgres'"
> createdb -O postgres flowplane_dev
> ```
>
> Alternatively, skip the `postgres` role entirely and point Flowplane at your own
> superuser: `export FLOWPLANE_DATABASE_URL="postgres://$(whoami)@127.0.0.1:5432/flowplane_dev"`
> after `createdb flowplane_dev`.

## 2. Build Flowplane

```bash
cargo build --bin flowplane
```

## 3. Start the Control Plane

Run this in its own terminal:

```bash
FLOWPLANE_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/flowplane_dev \
  FLOWPLANE_API_INSECURE=true \
  FLOWPLANE_DEV_MODE=true \
  FLOWPLANE_SECRET_ENCRYPTION_KEY=12345678901234567890123456789012 \
  FLOWPLANE_API_ADDR=127.0.0.1:8096 \
  FLOWPLANE_XDS_ADDR=0.0.0.0:18000 \
  ./target/debug/flowplane serve
```

Expected log signals:

- `database connected and migrations applied`
- `DEV MODE: in-process identity, seeded resources`
- `dev resources seeded`
- `dev bearer token`
- `xDS ADS server starting (plaintext dev mode)`
- `API listener starting`

Dev mode seeds:

| Resource | Value |
| --- | --- |
| Org | `dev-org` |
| Team | `default` |
| User | `dev-user` |

## 4. Export CLI Context

Copy the full `dev_token` value from the CP logs. The token is valid for this CP process only; if you restart the CP, export the new token.

```bash
export FLOWPLANE_SERVER=http://127.0.0.1:8096
export FLOWPLANE_ORG=dev-org
export FLOWPLANE_TEAM=default
export FLOWPLANE_TOKEN='<full dev_token from the current CP logs>'
```

Verify auth:

```bash
./target/debug/flowplane auth whoami
```

If this returns `401 token validation failed`, check:

- the token was copied without dropping the final character
- the token came from the currently running CP process
- `FLOWPLANE_SERVER` points at the same CP process

## 5. Start an Upstream

Run this in another terminal:

```bash
mkdir -p /tmp/fp-upstream
printf 'hello-flowplane\n' > /tmp/fp-upstream/index.html
cd /tmp/fp-upstream
python3 -m http.server 3001
```

## 6. Create a Dataplane Record

The dev bootstrap command uses this row to generate a stable Envoy `node.id`.

```bash
./target/debug/flowplane dataplane create dp-local --description "manual local Envoy"
./target/debug/flowplane dataplane list
```

## 7. Expose the Upstream

This creates a normal cluster, route config, and listener through the V2 services:

```bash
./target/debug/flowplane expose http://127.0.0.1:3001 \
  --name local \
  --path / \
  --port 10001 \
  --public-base-url http://127.0.0.1:10001
```

The loopback `--public-base-url` is a local-dev hint for this runbook. Non-local deployments should set it to the dataplane listener address clients can actually reach, or omit it when no public endpoint is configured.

Expected table fields include:

| Field | Value |
| --- | --- |
| `curl_url` | `http://127.0.0.1:10001/` |
| `endpoint_source` | `listener.public_base_url` |
| `cluster_name` | `local-upstream` |
| `route_config_name` | `local-routes` |
| `listener_name` | `local` |

Cleanup after testing:

```bash
./target/debug/flowplane unexpose local
```

## 8. Start Envoy

Generate the dev plaintext bootstrap:

```bash
./target/debug/flowplane --out /tmp/flowplane-envoy.yaml \
  dataplane bootstrap dp-local \
  --mode dev \
  --xds-host 127.0.0.1 \
  --xds-port 18000 \
  --admin-port 9901
```

For non-dev dataplanes, use mTLS mode with paths as Envoy sees them:

```bash
./target/debug/flowplane --out /tmp/flowplane-envoy.yaml \
  dataplane bootstrap dp-local \
  --mode mtls \
  --xds-host cp.example.internal \
  --cert-path /etc/flowplane/tls/client.crt \
  --key-path /etc/flowplane/tls/client.key \
  --ca-path /etc/flowplane/tls/ca.crt
```

Start local Envoy:

```bash
envoy -c /tmp/flowplane-envoy.yaml --log-level info
```

Or, on Linux hosts where Docker host networking works:

```bash
docker run --rm --name flowplane-envoy --network host \
  -v /tmp/flowplane-envoy.yaml:/etc/envoy/envoy.yaml:ro \
  envoyproxy/envoy:v1.37-latest \
  -c /etc/envoy/envoy.yaml --log-level info
```

## 9. Curl Through Envoy

Once Envoy has connected to xDS and warmed the listener:

```bash
curl -i http://127.0.0.1:10001/
```

Expected body:

```text
hello-flowplane
```

## 10. Inspect Diagnostics

Stats:

```bash
./target/debug/flowplane stats overview
```

xDS delivery status:

```bash
./target/debug/flowplane ops xds status
```

Recent xDS NACKs:

```bash
./target/debug/flowplane ops xds nacks
```

This manual dev path starts Envoy directly, without `flowplane-agent`. Request counters may not increase until the agent-backed diagnostics path is included in a later local lifecycle wrapper. The normal operator path is still Flowplane diagnostics (`stats`, `ops xds status`, `ops xds nacks`), not direct Envoy admin access.

Envoy admin is loopback-only local debugging. Do not treat `curl :9901/config_dump` as a product or operator workflow; in the intended DP unit, `flowplane-agent` is the only component that scrapes Envoy admin and relays curated telemetry to the CP.

If traffic does not flow, check in this order:

1. CP logs show Envoy connected to xDS.
2. `ops xds nacks` is empty.
3. `ops xds status` shows the expected dataplane and no recent NACKs.
4. The upstream is reachable at `http://127.0.0.1:3001/` from the same host/network namespace as Envoy.
5. Port `10001` is not already occupied.

## Current Gaps Captured by S7.7

These are known gaps, not operator mistakes:

- There is no V2-native `dataplane up/down/status` command yet.
- This runbook should be pinned by an S7.7e transcript/e2e test.

Relevant tracking:

- `../../flowplane-private-vault/archive/repo-import-2026-06-24/internal/PROGRESS.md` -> `S7.7 Core gateway parity before learning`
- `spec/13-basics-before-learning-mindmap.md`
- `../../flowplane-private-vault/constitution.md`
