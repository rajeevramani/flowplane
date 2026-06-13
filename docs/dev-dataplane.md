# Dev Control Plane and Dataplane Runbook

This is the current manual V2 path for validating the core gateway loop before S8 learning work
continues. It intentionally documents the rough edges as they exist today.

Do not use this as a production deployment guide. Dev mode uses plaintext API/xDS and a boot-local
development token. Non-dev xDS is mTLS-only.

## Prerequisites

- PostgreSQL reachable as `postgres://postgres:postgres@127.0.0.1:5432/flowplane_dev`
- `cargo`
- `curl`
- `python3`
- Envoy, either as a local `envoy` binary or Docker running an Envoy image

On macOS, prefer a local Envoy binary for this manual path. Docker host networking is not reliable
across all Docker Desktop setups.

## Dataplane Lifecycle Decision

Before S8 learning resumes, the supported V2 dataplane lifecycle is manual local Envoy started from
`flowplane dataplane bootstrap` output. V2 does not currently provide `dataplane up/down/status`.

That is intentional for this phase: it keeps the product contract on registered dataplanes, xDS/SDS,
and persisted diagnostics instead of porting V1's compose orchestration. A V2-native lifecycle
wrapper can be added later in packaging/S12 if the validated smoke path needs it.

## 1. Start PostgreSQL

The helper is idempotent:

```bash
scripts/ensure-postgres.sh
```

It ensures the `flowplane_dev` database exists and sets the local `postgres` password to
`postgres`.

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

Copy the full `dev_token` value from the CP logs. The token is valid for this CP process only; if
you restart the CP, export the new token.

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

## 7. Create Gateway Resources

The current CLI accepts JSON files for resource create/update. These shapes are the V2 REST shapes.

Create `/tmp/fp-cluster.json`:

```json
{
  "name": "local-upstream",
  "spec": {
    "endpoints": [
      {
        "host": "127.0.0.1",
        "port": 3001
      }
    ]
  }
}
```

Create `/tmp/fp-route.json`:

```json
{
  "name": "local-routes",
  "spec": {
    "virtual_hosts": [
      {
        "name": "default",
        "domains": ["*"],
        "routes": [
          {
            "name": "all",
            "match": {
              "prefix": {
                "prefix": "/"
              }
            },
            "action": {
              "cluster": "local-upstream"
            }
          }
        ]
      }
    ]
  }
}
```

Create `/tmp/fp-listener.json`:

```json
{
  "name": "local-edge",
  "spec": {
    "address": "0.0.0.0",
    "port": 10001,
    "route_config": "local-routes"
  }
}
```

Apply them:

```bash
./target/debug/flowplane cluster create -f /tmp/fp-cluster.json
./target/debug/flowplane route create -f /tmp/fp-route.json
./target/debug/flowplane listener create -f /tmp/fp-listener.json
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

In this manual dev path there is no `fp-agent` sidecar yet, so request counters may not increase.
The command should still return a valid overview. S7.7c/S7.7e cover the agent-backed lifecycle and
validated stats path.

Recent xDS NACKs:

```bash
./target/debug/flowplane ops xds nacks
```

Envoy config dump:

```bash
curl -fsS http://127.0.0.1:9901/config_dump
```

If traffic does not flow, check in this order:

1. CP logs show Envoy connected to xDS.
2. `ops xds nacks` is empty.
3. Envoy admin `config_dump` contains `local-edge`, `local-routes`, and `local-upstream`.
4. The upstream is reachable at `http://127.0.0.1:3001/` from the same host/network namespace as
   Envoy.
5. Port `10001` is not already occupied.

## Current Gaps Captured by S7.7

These are known gaps, not operator mistakes:

- There is no V2-native `dataplane up/down/status` command yet.
- There is no `flowplane expose` shortcut yet; resources must be created manually.
- This runbook should be pinned by an S7.7e transcript/e2e test.

Relevant tracking:

- `PROGRESS.md` -> `S7.7 Core gateway parity before learning`
- `spec/13-basics-before-learning-mindmap.md`
- `spec/14-architecture-integrity.md`
