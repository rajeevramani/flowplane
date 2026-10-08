# Evaluate Flowplane without cloning the repo

> Audience: newcomers, api-teams · Status: draft — 3.2.0 release preparation

Start with an empty gateway, explicitly expose the supplied sample, then route your own backend through the same listener. Add a local rate limit, observe HTTP 429 and recovery, and remove the exposures safely. Dashboard and API-to-MCP exploration are optional continuations, not prerequisites for your first successful request.

**Prerequisites:** Docker with Compose, or a Podman VM with Docker Compose already configured against its engine socket; `curl`, Python 3, and a POSIX shell. The commands use the `docker compose` spelling. Only the explicitly labelled local Podman variant has supporting runtime evidence in this draft. Python supplies the prerequisite assertions, the tiny own backend, and the policy-file edit. Matching v3.2.0 container images are not yet published. These commands target the **3.2.0** bundle: use the published-artifact path only after `v3.2.0` and its matching eval image are published. Older `3.1.x` bundles seed demo traffic and do not implement this empty-install/shared-exposure journey. This draft has not yet qualified the packaged 3.2.0 release candidate.

The evaluation bundle is local-only dev mode: in-process OIDC identity issuer, seeded `dev-org` / `default` identities, a management bearer token, Postgres, demo upstream, Envoy, and a dataplane agent. It automatically registers and bootstraps the dataplane over xDS mTLS, **without creating gateway clusters, route configs, listeners, or exposing an API**. Host ports bind to `127.0.0.1`. This is not a production deployment.

## 1. Install infrastructure, not APIs

Use a new directory for this evaluation; do not overwrite another stack's Compose file or reuse its volumes.

```sh
mkdir flowplane-evaluation &&
cd flowplane-evaluation &&
VER=3.2.0 &&
curl -fsSLO "https://raw.githubusercontent.com/rajeevramani/flowplane/v${VER}/compose.eval.yml" &&
export FLOWPLANE_EVAL_IMAGE="ghcr.io/rajeevramani/flowplane:${VER}-eval" &&
docker compose -f compose.eval.yml up -d --no-build || {
  printf '%s\n' 'Installation failed; stop here and diagnose. Use a new directory, not an existing evaluation.' >&2
  false
}
```

Define this short helper in the same terminal. It runs the existing CLI inside the container, reads the token there without printing it, supplies management context, and preserves each argument. `-T` also lets later commands pipe files into the container without allocating a terminal.

```sh
fp() {
  docker compose -f compose.eval.yml exec -T flowplane-eval \
    sh -ec 'export FLOWPLANE_SERVER=http://127.0.0.1:8080 FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default; FLOWPLANE_TOKEN="$(cat /shared/dev-token)"; export FLOWPLANE_TOKEN; exec flowplane "$@"' sh "$@"
}
```

Paste one block at a time and stop if it reports an error. Do not enable global `set -e` in your interactive terminal: expected startup failures must not close it or lose `fp` and the image context. Verification blocks below use their own subshell with local `set -eu`; an error stops that block and prints a diagnostic, not your terminal.

Confirm infrastructure readiness separately from gateway traffic. This block includes its own bounded startup wait; do not pre-wait elsewhere:

```sh
(
  set -eu
  docker compose -f compose.eval.yml ps -a
  infrastructure_ready=false
  for attempt in $(seq 1 30); do
    if fp auth whoami >/dev/null 2>&1 &&
       fp -o json dataplane get dp-eval 2>/dev/null | python3 -c 'import json,sys; assert json.load(sys.stdin)["data"]["last_heartbeat_at"] is not None' 2>/dev/null; then
      infrastructure_ready=true
      break
    fi
    sleep 2
  done
  [ "$infrastructure_ready" = true ] || { printf '%s\n' 'Authentication/agent heartbeat did not become ready; inspect setup and agent logs. Do not expose an API.' >&2; exit 1; }
  fp auth whoami
  fp -o json dataplane get dp-eval
  fp -o json cluster list | python3 -c 'import json,sys; assert json.load(sys.stdin)["data"]["items"] == [], "Existing clusters: stop this empty-install check"'
  fp -o json route list | python3 -c 'import json,sys; assert json.load(sys.stdin)["data"]["items"] == [], "Existing route configs: stop this empty-install check"'
  fp -o json listener list | python3 -c 'import json,sys; assert json.load(sys.stdin)["data"]["items"] == [], "Existing listeners: stop this empty-install check"'
)
```

The block waits for authenticated `whoami` and a non-null dataplane `last_heartbeat_at`. If it fails, stop and diagnose; do not proceed to exposure. Successful one-shot setup services can be `Exited (0)`; they are not failed long-running services. The three gateway inventories must be empty for a fresh stack. If they are not, stop: you are resuming existing state, not testing an empty installation. For initial diagnosis, inspect `docker compose -f compose.eval.yml logs flowplane-eval init flowplane-agent envoy`; never use Envoy admin as an operator API.

There is no gateway listener yet. This bounded request must **not** return a successful sample response:

```sh
(
set -eu
if curl --max-time 3 -fsS http://127.0.0.1:10000/; then
  printf '%s\n' 'Unexpected gateway success before exposure; inspect the stack.' >&2
  false
fi
)
```

A refusal/reset is expected here; it is not evidence that infrastructure failed. Do not mistake the published host port for an authored Envoy listener.

## 2. Explicitly expose and call the sample

The bundle publishes gateway port `10000`; use it explicitly. Do not let auto-port allocation choose an unpublished container port.

```sh
(
set -eu
fp expose http://demo-upstream:5678 --name demo --path / --port 10000 \
  --public-base-url http://127.0.0.1:10000
fp listener get demo
fp route get demo-routes
fp cluster get demo-upstream
)
```

This creates cluster `demo-upstream`, route config `demo-routes`, and listener `demo`. The control plane sends their configuration to Envoy; it does not proxy your API request. Wait a bounded time for xDS delivery and verify the actual backend body, not just HTTP 200:

```sh
(
set -eu
sample_ready=false
for attempt in $(seq 1 30); do
  body=$(curl --max-time 2 -fsS http://127.0.0.1:10000/ 2>/dev/null) || body=
  if [ "$body" = 'hello from the flowplane eval demo upstream' ]; then
    sample_ready=true
    break
  fi
  sleep 1
done
[ "$sample_ready" = true ] || { printf '%s\n' 'Sample traffic did not converge; inspect control-plane and Envoy logs.' >&2; false; }
)
```

The expected response is `hello from the flowplane eval demo upstream` followed by a newline. The command substitution above strips its trailing newline for comparison.

**Two separate security planes:** `fp` uses the management bearer token to configure Flowplane. The sample gateway request does not need that token. Dev-mode OIDC authentication of the CLI does **not** automatically protect exposed traffic. Never send `/shared/dev-token` to your backend. To protect traffic, configure the listener's JWT filter and route requirement with your own issuer, audience, and JWKS; see [JWT authentication and rate limiting](../how-to/jwt-auth-rate-limit-route.md).

## 3. Expose your own backend on the same listener

The actually exercised host-backend variant used Docker Compose connected to a **Linux-arm64 Podman VM**, `host.containers.internal`, and a Python backend bound to host loopback. This is source-bound local supporting evidence, not packaged 3.2.0 or Docker Desktop qualification. Docker Desktop/macOS and Linux Docker host-gateway alternatives below are **unverified in this draft**; select and verify your engine's setup before exposing. Docker and Podman qualification are not interchangeable.

Use an existing reachable API, or start this harmless backend in a **second terminal**. The loopback variant is the one exercised locally:

```sh
(
  set -eu
  mkdir flowplane-own-backend
  cd flowplane-own-backend
  printf 'hello from my API\n' > hello
  python3 -m http.server 3001 --bind 127.0.0.1
)
```

Stop it with Ctrl-C when finished. Reachability is from **Envoy**, not your laptop's CLI:

| Backend location | Upstream URL / qualification boundary |
|---|---|
| Locally exercised Podman VM host | `http://host.containers.internal:3001`; loopback worked in that VM. Verify it on another Podman setup; this is not a universal guarantee. |
| Docker Desktop/macOS host — unverified here | `http://host.docker.internal:3001`; verify that Docker can reach your backend before attachment. |
| Linux Docker host — unverified here | Add the override below, recreate Envoy/agent, then use `http://host.docker.internal:3001`. |
| Service on the Compose network | `http://<service-name>:<container-port>`; attach your service to that network. |

**Host-backend warning:** if another engine cannot reach a loopback-only backend, use an interface reachable from that engine and a firewall restriction appropriate for the test. Binding `0.0.0.0` makes it reachable beyond host loopback; that alternative was not exercised here. Serve only harmless sample data and stop the server afterwards. Never use `http://127.0.0.1:3001` as a host upstream when Envoy is containerized: it means the Envoy container itself.

For the **unverified Linux Docker alternative**, save this additional file as `compose.host.yml` and apply it **before** the attachment command:

```yaml
services:
  envoy:
    extra_hosts:
      - "host.docker.internal:host-gateway"
```

Remove only the agent container first: it shares Envoy's network namespace and can otherwise block Envoy replacement. Recreate both services with the override; retain the database/shared volumes. Existing evaluation traffic may briefly pause:

```sh
(
  set -eu
  docker compose -f compose.eval.yml rm -sf flowplane-agent
  docker compose -f compose.eval.yml -f compose.host.yml up -d --no-build envoy flowplane-agent
)
```

In the **first terminal**, run this attachment for the locally exercised Podman variant. On a verified Docker setup, substitute `http://host.docker.internal:3001` for the upstream; do not execute both variants or replay a failed mutation blindly:

```sh
(
  set -eu
  fp expose http://host.containers.internal:3001 --name own --path /hello --listener demo
  own_ready=false
  for attempt in $(seq 1 30); do
    own_body=$(curl --max-time 2 -fsS http://127.0.0.1:10000/hello 2>/dev/null) || own_body=
    sample_body=$(curl --max-time 2 -fsS http://127.0.0.1:10000/ 2>/dev/null) || sample_body=
    if [ "$own_body" = 'hello from my API' ] && [ "$sample_body" = 'hello from the flowplane eval demo upstream' ]; then
      own_ready=true
      break
    fi
    sleep 1
  done
  [ "$own_ready" = true ] || { printf '%s\n' 'Own-backend and surviving sample traffic did not converge; diagnose reachability and routes. Do not replay expose.' >&2; exit 1; }
  printf '%s\n' "$own_body" "$sample_body"
)
```

Only bounded traffic **reads** retry. `/hello` is forwarded unchanged; this backend implements `/hello`. TLS backends and remote APIs must satisfy their own network, trust and authentication requirements.

`--listener demo` attaches to the actual listener and its route config; it does not allocate a second port. The narrower `/hello` Prefix is inserted before `/`, with existing relative order preserved. A Prefix `/hello` also matches `/helloworld`; an earlier Exact exception can take precedence. Attachment supports a same-team user-owned HTTP listener with one wildcard virtual host and simple Prefix/Exact routes; unsupported shapes fail rather than guess.

A route-config edit affects **every listener sharing the named route config**. Inspect the reported resource names before changing a shared config. Do not combine `--listener` with `--port` or `--public-base-url`.

## 4. Add a local rate limit and prove it works

This experiment adds a **listener-wide**, per-Envoy token bucket: it affects both `/` and `/hello`. It is not a fleet-wide/global quota. The listener filter chain must contain `local_rate_limit`; a per-route override alone cannot add a missing filter.

Save the current listener and prepare an update that preserves its other fields. CLI JSON has a `data` envelope; the PATCH file contains only `spec`. The update uses the exact revision read here, so concurrent changes fail instead of being silently overwritten.

```sh
(
set -eu
fp -o json listener get demo > eval-listener-before.json
python3 - <<'PY'
import json
from pathlib import Path
resource = json.loads(Path('eval-listener-before.json').read_text())['data']
spec = resource['spec']
filters = spec.setdefault('http_filters', [])
if any(entry['filter']['type'] == 'local_rate_limit' for entry in filters):
    raise SystemExit('Local rate limit already exists; inspect it instead of adding another.')
filters.append({'filter': {
    'type': 'local_rate_limit',
    'stat_prefix': 'eval_local',
    'token_bucket': {'max_tokens': 2, 'tokens_per_fill': 2, 'fill_interval_ms': 5000}
}, 'disabled': False})
Path('eval-local-limit.json').write_text(json.dumps({'spec': spec}))
Path('eval-listener-revision.txt').write_text(str(resource['revision']))
PY
docker compose -f compose.eval.yml exec -T flowplane-eval \
  sh -ec 'cat > /tmp/eval-local-limit.json' < eval-local-limit.json
revision=$(cat eval-listener-revision.txt)
fp --revision "$revision" listener update demo --file /tmp/eval-local-limit.json
)
```

Burst until you see an actual 429, with a bounded attempt count. Keep other callers idle during this experiment. The brief pauses between bursts allow xDS delivery; HTTP 200 is not proof that the rate limit is active. Do not accept a connection error, 404, or 5xx as evidence of rate limiting:

```sh
(
set -eu
saw_429=false
for attempt in $(seq 1 30); do
  http_status=$(curl --max-time 2 -sS -o /dev/null -w '%{http_code}' http://127.0.0.1:10000/)
  case "$http_status" in
    200) ;;
    429) saw_429=true; break ;;
    *) printf 'Unexpected HTTP status: %s\n' "$http_status" >&2; exit 1 ;;
  esac
  if [ "$attempt" = 10 ] || [ "$attempt" = 20 ]; then sleep 1; fi
done
[ "$saw_429" = true ] || { printf '%s\n' 'No HTTP 429 observed within the bounded burst; inspect the listener policy.' >&2; exit 1; }
printf '%s\n' 'Observed HTTP 429 from Envoy'
)
```

Wait for one refill and verify successful traffic and the original body again:

```sh
(
set -eu
sleep 6
http_status=$(curl --max-time 5 -sS -o eval-recovered-body.txt -w '%{http_code}' http://127.0.0.1:10000/)
[ "$http_status" = 200 ] || { printf 'Recovery returned HTTP %s, expected 200\n' "$http_status" >&2; exit 1; }
[ "$(cat eval-recovered-body.txt)" = 'hello from the flowplane eval demo upstream' ] || { printf '%s\n' 'Recovery body did not match the sample upstream' >&2; exit 1; }
printf '%s\n' 'Recovered HTTP 200 with the sample body'
)
```

Envoy's data-plane 429 does not promise a `Retry-After` header here. It is different from the control-plane write throttle described in the [error reference](../reference/errors.md). See the [filter reference](../reference/filters.md) for route overrides and [global rate limiting](../how-to/global-rate-limit.md) for a quota shared across Envoys.

## 5. Remove safely, then repeat

`fp` disables terminal allocation, so pass `--yes` deliberately for destructive commands. First remove the original exposure while the attached one survives:

```sh
(
set -eu
fp --yes unexpose demo
sleep 6
curl --max-time 5 -fsS http://127.0.0.1:10000/hello
fp listener get demo
fp route get demo-routes
)
```

After removal converges, `/hello` still serves your backend. The response reports the original cluster deleted and the listener/config retained; the listener-wide policy remains active. Removing `demo` is not a promise to delete all resources named `demo-*`.

Now remove the final managed exposure:

```sh
(
set -eu
fp --yes unexpose own
fp cluster list
fp route list
fp listener list
)
```

For this otherwise untouched example, the final inventories are empty. Final cleanup also removes the shortcut-created listener/config and their later policy edits. The dataplane registration, backend process, and infrastructure are not deleted by `unexpose`. A borrowed manual/legacy scaffold is never shortcut-deleted, and live references or an invalid last-route removal can reject cleanup atomically. Resolve those dependencies through their supported lifecycle; do not force-delete named resources.

To repeat, run the sample exposure and own-backend attachment again, then remove `own` **before** `demo`: the sample should survive the first removal and the final managed inventories should again be empty. Recreated resources have new identities. Pre-3.2 legacy trios have no shortcut association; matching names do not authorize adoption or deletion. See [Expose an API](../how-to/expose-an-api.md) for these modes and limits.

## Optional: dashboard, traffic authentication, and MCP

- **Dashboard:** read its nonce-protected URL with `docker compose -f compose.eval.yml exec -T flowplane-dashboard cat /shared/dashboard-url`. Open that URL in your browser. If it is not ready, inspect `docker compose -f compose.eval.yml logs flowplane-dashboard`. Re-read the URL after dashboard restart; host port `8081` is separate from gateway port `10000`.
- **Traffic authentication:** [JWT auth and local rate limiting](../how-to/jwt-auth-rate-limit-route.md) explains issuer/audience/JWKS, listener filter installation, and per-route requirements. A configured management IdP is not an automatic API traffic policy.
- **API-to-MCP:** [Import and publish an OpenAPI spec](../how-to/import-and-publish-openapi-spec.md) explains import, publish, and route bindings. Generated tools/status are not evidence that a backend invocation succeeded; execution needs a valid binding and reachable backend. Complete the traffic journey first, and keep bindings/captures in mind before `unexpose`.

## Stop versus destructive reset

Stop the Python backend with Ctrl-C. Preserve Flowplane state and volumes when stopping the evaluation:

```sh
(
set -eu
docker compose -f compose.eval.yml down
)
```

Resume from the same directory with the same image and `up -d --no-build`; this is not another empty installation. To intentionally destroy **this evaluation's** database, identities, configuration, and PKI volumes:

```sh
(
set -eu
docker compose -f compose.eval.yml down -v
)
```

Never use the reset command against someone else's stack or as an unexplained first response to a readiness problem. For production-shaped evaluation, use [Evaluate a production-shaped platform setup](../how-to/evaluate-platform.md).
