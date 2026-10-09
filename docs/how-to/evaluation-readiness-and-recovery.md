# Diagnose and recover the local evaluator

> Audience: newcomers, api-teams · Status: draft — local supporting procedure validated; release/platform qualification pending

Use this guide for the local Compose evaluator, not production. Start with [Evaluate without cloning](../tutorials/evaluate-no-clone.md): use its image, directory and `fp` helper in the same terminal. Matching 3.2.0 published artifacts are not yet available. Infrastructure readiness is management authentication plus an agent heartbeat; API traffic requires an explicit exposure and successful xDS delivery. A published host port is not an Envoy listener.

Only the **source-built supporting image**, not a published immutable 3.2.0 artifact, has local evidence on a **macOS arm64 host / Linux-arm64 Podman VM through Docker Compose**. The preserve/resume/reset procedure, control-plane restart and agent-first Envoy replacement have supporting local execution evidence on that same host/runtime, including retained resource identities/spec/revisions, actual rate-limit enforcement after resume and empty reset inventories. On that stack, the post-disruption check observed strictly advancing same-identity heartbeats after resume, control-plane restart and Envoy replacement. This does not qualify another platform or release artifact. Docker Desktop, a Linux Docker host, separate native Mac ARM/Linux release artifacts, remote CI, an unfamiliar-developer trial and the immutable 3.2.0 candidate remain unqualified here. Do not infer minimum RAM/CPU requirements or universal host-backend reachability from one successful local run.

## Initial readiness before exposure

Delegate initial readiness to the [initial tutorial](../tutorials/evaluate-no-clone.md#1-install-infrastructure-not-apis): its bounded authenticated startup check and fresh empty gateway inventories run before explicit exposure. No sample response is expected on a fresh install. Recovery below additionally requires a heartbeat that advances beyond the first post-disruption read, not merely a non-null value or one newer than a pre-stop snapshot.

## Diagnose before changing state

**Sensitive output:** control-plane startup logs include the dev management bearer token in a `dev_token` field, even though `fp` reads its token inside the container. Inspect logs locally; redact that value, nonce-protected dashboard URLs and other credentials before copying, archiving or sharing. Never assume stopped evaluation credentials are harmless. Run from the evaluation directory, using the same Compose project and image:

```sh
(
  set -eu
  docker compose -f compose.eval.yml ps -a
  docker compose -f compose.eval.yml logs --tail 80 flowplane-eval postgres shared-init init pki pki-client-verify flowplane-agent envoy
)
```

Logs contain operational identifiers and may contain the dev management token and the optional dashboard nonce URL. Never copy `/shared/dev-token`, private keys or connection strings into logs/reports. The diagnostic command intentionally shows local service logs; it is not a secret-free export. Do not use Envoy admin endpoints as an operator API or disable mTLS to make startup pass.

| Observation | Meaning and next action |
|---|---|
| `shared-init`, `pki`, `init`, `pki-client-verify` are `Exited (0)` | These are successful one-shot jobs, not unhealthy daemons. `up` may rerun them; retained PKI/setup guards validate or reuse existing state. |
| A setup job has a nonzero exit | Read that job's logs first. Missing/expired/inconsistent PKI is a fail-closed error. Do not delete volumes or regenerate files blindly. Stop and preserve data for investigation: this evaluation bundle has no in-place expired/inconsistent certificate recovery procedure. If the data is disposable, explicitly choose the evaluation reset below; production certificate lifecycle is a separate procedure. |
| `fp auth whoami` fails | The control plane/token may not be ready. Use the tutorial's bounded wait; inspect control-plane, Postgres and setup state. The helper reads the current management token inside the container. |
| Management works, agent heartbeat does not advance | Inspect agent/Envoy logs and certificate/setup state. A previously non-null heartbeat is not current liveness. Do not expose another API to repair connectivity. |
| Gateway request is refused/reset before first exposure | Expected on a fresh empty install. Verify empty gateway inventories and management/agent readiness separately. |
| Gateway request fails after exposure | Verify the exact listener/config/upstream with supported `fp ... get` commands. Check host/container ports, paths, backend reachability and xDS delivery; retry bounded traffic reads, not mutations. A 404/5xx/connection failure is not rate-limit enforcement. |
| Port is already allocated | Do not kill an unknown process or reset the database. Choose a free host port as described below. |

The management token authenticates `fp`, not API traffic. Do not forward it to the upstream. Configure traffic authentication independently if needed.

## Host ports are not container listener ports

Set overrides before initial startup and retain them in the same terminal for all later Compose commands. These example host ports must be free:

```sh
export FLOWPLANE_EVAL_API_PORT=18080
export FLOWPLANE_EVAL_GATEWAY_PORT=11000
```

The management REST API mapping becomes host `18080` to container `8080`; the `fp` helper still uses container-local `8080`. The gateway mapping becomes host `11000` to container `10000`. Author the listener with **`--port 10000`**, not `11000`, and use `http://127.0.0.1:11000` for host traffic and the listener's public base URL. Adapt every tutorial curl consistently. A host-port override does not publish arbitrary new container listener ports.

The optional dashboard uses fixed host/container port `8081`: its Host/Origin validation does not support arbitrary remapping. If that port is occupied, resolve ownership before starting the dashboard; there is no dashboard-port environment override in this bundle. Do not silently stop someone else's service.

Changing an existing Envoy container's published ports can require replacement. The agent shares its network namespace: remove only the agent before replacing Envoy, then recreate both; retain all named volumes:

```sh
(
  set -eu
  docker compose -f compose.eval.yml rm -sf flowplane-agent
  docker compose -f compose.eval.yml up -d --no-build envoy flowplane-agent
)
```

Traffic may pause. One-shot dependencies can rerun validation. See [Expose an API](expose-an-api.md#publish-an-additional-evaluation-port) for an additional container listener port rather than a host-port override.

## Capture the state you intend to retain

These examples assume the tutorial's `demo` exposure still exists. Do this **before its final removal**. If you also have `own` attached, keep its backend running and verify that traffic separately. Choose a quiet window with no other callers changing resources; snapshots are verification records, not an atomic backup or restore mechanism.

CLI envelopes carry `data.id`; product records also carry `data.spec` and `data.revision`. Save the current dataplane identity/heartbeat and the sample resources, including policy and revision, without copying credentials:

```sh
(
  set -eu
  umask 077
  fp -o json dataplane get dp-eval > eval-before-dataplane.json
  fp -o json listener get demo > eval-before-listener.json
  fp -o json route get demo-routes > eval-before-route.json
  fp -o json cluster get demo-upstream > eval-before-cluster.json
  python3 -c 'import json; d=json.load(open("eval-before-dataplane.json"))["data"]; assert d["last_heartbeat_at"] is not None, "No prior heartbeat: diagnose before stopping"'
)
```

Define this bounded recovery check in the same terminal; call it **after** the disruption and restart/resume commands. It captures the first authenticated post-disruption read as its heartbeat baseline, then requires the **same dataplane ID with a strictly later heartbeat**. The earlier snapshot checks identity/configuration only: a heartbeat written between that snapshot and shutdown is not proof of reconnect. Each call replaces its post-disruption baseline and prints both baseline and advanced timestamps for verification. Archive both the post-disruption baseline and advanced read in private verification records; a stale timestamp cannot pass merely because it is newer than the earlier snapshot:

```sh
eval_ready() {
  (
    set -eu
    umask 077
    eval_ready_ok=false
    eval_baseline_captured=false
    for attempt in $(seq 1 30); do
      if fp auth whoami >/dev/null 2>&1 &&
         fp -o json dataplane get dp-eval > eval-now-dataplane.json 2>/dev/null; then
        if [ "$eval_baseline_captured" = false ]; then
          if python3 -c 'import json; expected_id=json.load(open("eval-before-dataplane.json"))["data"]["id"]; b=json.load(open("eval-now-dataplane.json"))["data"]; assert b["id"] == expected_id and b["last_heartbeat_at"] is not None' 2>/dev/null; then
            cp eval-now-dataplane.json eval-after-disruption-baseline.json
            eval_baseline_captured=true
            python3 -c 'import json; b=json.load(open("eval-after-disruption-baseline.json"))["data"]; print(json.dumps({"phase":"post-disruption-baseline","id":b["id"],"last_heartbeat_at":b["last_heartbeat_at"]}))'
          fi
        elif python3 -c 'import json,datetime; expected_id=json.load(open("eval-before-dataplane.json"))["data"]["id"]; baseline=json.load(open("eval-after-disruption-baseline.json"))["data"]; b=json.load(open("eval-now-dataplane.json"))["data"]; assert expected_id == baseline["id"] == b["id"]; parse=lambda s: datetime.datetime.strptime(s, "%Y-%m-%dT%H:%M:%S.%f%z" if "." in s else "%Y-%m-%dT%H:%M:%S%z"); assert b["last_heartbeat_at"] is not None; assert parse(b["last_heartbeat_at"]) > parse(baseline["last_heartbeat_at"]); print(json.dumps({"phase":"post-disruption-advanced","id":b["id"],"last_heartbeat_at":b["last_heartbeat_at"]}))' 2>/dev/null; then
          eval_ready_ok=true
          break
        fi
      fi
      sleep 2
    done
    [ "$eval_ready_ok" = true ] || { printf '%s\n' 'Recovery readiness failed: authentication or a newer same-identity post-disruption heartbeat is missing; inspect logs, preserve volumes' >&2; exit 1; }
    printf '%s\n' 'Management recovered with a newer same-identity post-disruption agent heartbeat'
  )
}
```

There are at most 30 attempts with two-second gaps; individual CLI calls can extend elapsed time. A failure stops this check, not the interactive terminal. Do not enable global interactive `set -e` or replay `expose` on recovery.

## Stop and resume without deleting state

Use `down` **without `-v`** to remove this project's containers/network while retaining its named `pgdata`, `shared`, `pki-cp` and `pki-dp` volumes. Setup guards may rerun validation against retained state, without reseeding or deleting existing user-authored resources:

```sh
(
  set -eu
  docker compose -f compose.eval.yml down
)
```

Resume from the same directory/project, with the same image and port overrides. This is not a fresh installation; existing gateway resources should remain, and setup must not seed another API:

```sh
(
  set -eu
  docker compose -f compose.eval.yml up -d --no-build
  eval_ready
)
```

Verify the retained sample IDs, specs and revisions after recovery, then perform bounded exact-body traffic reads:

```sh
(
  set -eu
  umask 077
  fp -o json listener get demo > eval-now-listener.json
  fp -o json route get demo-routes > eval-now-route.json
  fp -o json cluster get demo-upstream > eval-now-cluster.json
  python3 - <<'PY'
import json
for kind in ('listener', 'route', 'cluster'):
    before = json.load(open('eval-before-' + kind + '.json'))['data']
    after = json.load(open('eval-now-' + kind + '.json'))['data']
    for field in ('id', 'revision', 'spec'):
        assert before[field] == after[field], 'Retained ' + kind + ' changed: ' + field
print('Retained resource identities, revisions and policy/specs verified')
PY
  resumed_traffic=false
  for attempt in $(seq 1 30); do
    resumed_body=$(curl --max-time 2 -fsS "http://127.0.0.1:${FLOWPLANE_EVAL_GATEWAY_PORT:-10000}/" 2>/dev/null) || resumed_body=
    if [ "$resumed_body" = 'hello from the flowplane eval demo upstream' ]; then
      resumed_traffic=true
      break
    fi
    sleep 1
  done
  [ "$resumed_traffic" = true ] || { printf '%s\n' 'Resumed traffic did not return the sample body; diagnose routes/upstream, do not replay expose' >&2; exit 1; }
  printf '%s\n' 'Resumed sample traffic with retained configuration'
)
```

With a local rate-limit policy, one successful resumed call plus an unchanged policy spec proves neither token-bucket persistence nor renewed enforcement. Repeat the tutorial's bounded actual-429/refill-200/body experiment to verify enforcement; buckets are per Envoy process and are not durable PostgreSQL counters.

For a targeted control-plane restart, refresh the same snapshots first, then use `docker compose -f compose.eval.yml restart flowplane-eval`, `eval_ready`, and the retained-state/traffic check above. The agent should reconnect with existing identity/material; restart is not certificate rotation. If Envoy is replaced rather than restarted, recreate its namespace-sharing agent as above. A reconnect claim requires archived baseline and advanced heartbeat values from after the disruption; earlier pre-stop comparisons are insufficient, even when retained traffic succeeds.

## Deliberately reset only your disposable evaluation

**Destructive:** `down -v` deletes this Compose project's database, configuration, identities, shared state and PKI volumes. It is not an upgrade, backup or first-line readiness fix. Reset requires your explicit data-loss consent, only for your own disposable evaluation. Confirm the exact directory/project and that all resources belong to it. Do not use it on someone else's stack or retain old certificates against a reset database.

```sh
(
  set -eu
  docker compose -f compose.eval.yml ps -a
  printf '%s\n' 'Inspect this project first; reset requires explicit ownership and data-loss consent'
)
```

Only after accepting that loss, run this block separately:

```sh
(
  set -eu
  docker compose -f compose.eval.yml down -v
)
```

Start again with the same image via `up -d --no-build`. Repeat the tutorial's bounded **initial** readiness check, not `eval_ready`: the old dataplane identity was intentionally destroyed. Verify empty inventories before any new exposure:

```sh
(
  set -eu
  docker compose -f compose.eval.yml up -d --no-build
)
```

After the tutorial's authenticated startup/heartbeat check:

```sh
(
  set -eu
  fp -o json cluster list | python3 -c 'import json,sys; assert json.load(sys.stdin)["data"]["items"] == [], "Reset has existing clusters"'
  fp -o json route list | python3 -c 'import json,sys; assert json.load(sys.stdin)["data"]["items"] == [], "Reset has existing route configs"'
  fp -o json listener list | python3 -c 'import json,sys; assert json.load(sys.stdin)["data"]["items"] == [], "Reset has existing listeners"'
  if curl --max-time 3 -fsS "http://127.0.0.1:${FLOWPLANE_EVAL_GATEWAY_PORT:-10000}/"; then
    printf '%s\n' 'Unexpected traffic before explicit exposure after reset; stop and inspect' >&2
    exit 1
  fi
  printf '%s\n' 'Reset evaluator is ready for a new explicit exposure; gateway inventories are empty'
)
```

The old exposure/policy is gone. Create a new exposure explicitly if continuing; do not assume a successful seeded response. Stop any separately launched own backend when finished. Keep verification snapshots private and delete them when no longer needed; they are not a backup. For real deployment/certificate/backup procedures use [production readiness](production-readiness.md) and [dataplane mTLS registration](register-dataplane-mtls.md), not evaluator reset.
