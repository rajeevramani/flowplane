#!/usr/bin/env bash
# Standalone P2b: exercise agent/control-plane recovery in compose.eval.yml's real xDS mTLS
# split topology. This deliberately does not use scripts/e2e/run.sh: that harness runs the CP's
# plaintext loopback development xDS listener and cannot authenticate diagnostics identity.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

for command_name in curl python3; do
  command -v "$command_name" >/dev/null 2>&1 || {
    echo "P2b FAILED: required command not found: $command_name" >&2
    exit 1
  }
done

PROVIDER=${FLOWPLANE_RECOVERY_PROVIDER:-auto}
case "$PROVIDER" in
  auto)
    if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
      PROVIDER=docker
    elif command -v podman >/dev/null 2>&1 && podman compose version >/dev/null 2>&1; then
      PROVIDER=podman
    else
      echo "P2b FAILED: neither 'docker compose' nor 'podman compose' is available" >&2
      exit 1
    fi
    ;;
  docker)
    if ! command -v docker >/dev/null 2>&1 || ! docker compose version >/dev/null 2>&1; then
      echo "P2b FAILED: FLOWPLANE_RECOVERY_PROVIDER=docker but Docker Compose is unavailable" >&2
      exit 1
    fi
    ;;
  podman)
    if ! command -v podman >/dev/null 2>&1 || ! podman compose version >/dev/null 2>&1; then
      echo "P2b FAILED: FLOWPLANE_RECOVERY_PROVIDER=podman but Podman Compose is unavailable" >&2
      exit 1
    fi
    ;;
  *)
    echo "P2b FAILED: FLOWPLANE_RECOVERY_PROVIDER must be auto, docker, or podman" >&2
    exit 1
    ;;
esac

DEFAULT_PROJECT=$(printf 'flowplane-recovery-%s-%s' "${USER:-user}" "$$" | tr '[:upper:]' '[:lower:]')
PROJECT=${FLOWPLANE_RECOVERY_PROJECT:-$DEFAULT_PROJECT}
case "$PROJECT" in
  *[!a-z0-9_-]*|'')
    echo "P2b FAILED: invalid Compose project '$PROJECT' (use only lowercase letters, digits, _ and -)" >&2
    exit 1
    ;;
esac
[ "$PROJECT" != flowplane ] || {
  echo "P2b FAILED: project 'flowplane' is reserved for the normal evaluator stack" >&2
  exit 1
}

FLOWPLANE_EVAL_IMAGE=${FLOWPLANE_EVAL_IMAGE:-flowplane:eval-local}
FLOWPLANE_EVAL_API_PORT=${FLOWPLANE_RECOVERY_API_PORT:-18080}
FLOWPLANE_EVAL_GATEWAY_PORT=${FLOWPLANE_RECOVERY_GATEWAY_PORT:-20000}
export FLOWPLANE_EVAL_IMAGE FLOWPLANE_EVAL_API_PORT FLOWPLANE_EVAL_GATEWAY_PORT
API="127.0.0.1:$FLOWPLANE_EVAL_API_PORT"
OUTAGE_SUCCESSES=${FLOWPLANE_RECOVERY_REQUESTS:-5}
OUTAGE_ERRORS=1
ERROR_LISTENER_PORT=10080
RECOVERED_LISTENER_PORT=10081

case "$FLOWPLANE_EVAL_API_PORT:$FLOWPLANE_EVAL_GATEWAY_PORT:$OUTAGE_SUCCESSES" in
  *[!0-9:]*|*::*|:*)
    echo "P2b FAILED: recovery ports and request count must be decimal integers" >&2
    exit 1
    ;;
esac
[ "$OUTAGE_SUCCESSES" -gt 0 ] || {
  echo "P2b FAILED: FLOWPLANE_RECOVERY_REQUESTS must be greater than zero" >&2
  exit 1
}

PROBE_IMAGE=${FLOWPLANE_RECOVERY_PROBE_IMAGE:-curlimages/curl:8.10.1@sha256:d9b4541e214bcd85196d6e92e2753ac6d0ea699f0af5741f8c6cccbfcf00ef4b}
OVERRIDE="${TMPDIR:-/tmp}/flowplane-recovery-${PROJECT}-$$.yml"
LOG="${TMPDIR:-/tmp}/flowplane-recovery-${PROJECT}.log"
RESPONSE="${TMPDIR:-/tmp}/flowplane-recovery-${PROJECT}-response.json"
KEEP=${FLOWPLANE_RECOVERY_KEEP:-0}
STACK_CREATED=0

cat >"$OVERRIDE" <<EOF
services:
  # The dashboard is irrelevant to this drill and its fixed Host/Origin port would prevent
  # concurrent evaluator stacks. It remains available only if the caller explicitly enables it.
  flowplane-dashboard:
    profiles: ["recovery-dashboard"]
  flowplane-agent:
    environment:
      # Leave a wide observation window between accepted reports. Each /stats scrape increments
      # Envoy's own admin downstream-request counter, so one-second polling cannot produce a
      # stable request total even when no gateway traffic is flowing.
      FLOWPLANE_AGENT_POLL_INTERVAL_SECS: "10"
      FLOWPLANE_AGENT_HEALTH_BIND_ADDR: "127.0.0.1:19902"
  recovery-probe:
    image: "$PROBE_IMAGE"
    entrypoint: ["/bin/sh", "-c"]
    command: ["trap : TERM INT; sleep infinity & wait"]
    network_mode: "service:envoy"
    depends_on:
      envoy:
        condition: service_started
  recovery-upstream:
    image: hashicorp/http-echo:1.0.0
    command: ["-text=hello from recovered control plane", "-listen=:5679"]
EOF

if [ "$PROVIDER" = docker ]; then
  COMPOSE=(docker compose -p "$PROJECT" -f compose.eval.yml -f "$OVERRIDE")
  RUNTIME=docker
else
  COMPOSE=(podman compose -p "$PROJECT" -f compose.eval.yml -f "$OVERRIDE")
  RUNTIME=podman
fi
compose() { "${COMPOSE[@]}" "$@"; }

cleanup() {
  status=$?
  trap - EXIT INT TERM
  if [ "$status" -ne 0 ]; then
    echo "P2b FAILED (exit $status); collecting Compose logs in $LOG" >&2
  fi
  if [ "$STACK_CREATED" = 1 ]; then
    compose logs --no-color >"$LOG" 2>&1 || true
    if [ "$KEEP" = 1 ]; then
      echo "P2b stack retained: project=$PROJECT override=$OVERRIDE logs=$LOG" >&2
      echo "cleanup: ${COMPOSE[*]} down -v --remove-orphans" >&2
    else
      compose down -v --remove-orphans >/dev/null 2>&1 || true
      rm -f "$OVERRIDE" "$RESPONSE"
      echo "P2b cleanup: removed only Compose project '$PROJECT' and its volumes; logs: $LOG" >&2
    fi
  else
    rm -f "$OVERRIDE" "$RESPONSE"
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

fail() {
  echo "P2b FAILED: $*" >&2
  exit 1
}

# Refuse to adopt or later delete an existing project. The eval image must already exist: this
# drill never rebuilds or silently substitutes source artifacts.
[ -z "$(compose ps -a -q 2>/dev/null)" ] || fail "Compose project '$PROJECT' already exists"
"$RUNTIME" image inspect "$FLOWPLANE_EVAL_IMAGE" >/dev/null 2>&1 \
  || fail "eval image '$FLOWPLANE_EVAL_IMAGE' is absent; build or pull it before running this drill"
EVAL_IMAGE_ID=$("$RUNTIME" image inspect --format '{{.Id}}' "$FLOWPLANE_EVAL_IMAGE")
[ -n "$EVAL_IMAGE_ID" ] || fail "could not record eval image identity"

# --no-build is load-bearing: all Flowplane processes in the drill come from the one pre-existing
# eval image named above. Compose may pull pinned third-party fixture images if they are absent.
echo "P2b: provider=$PROVIDER project=$PROJECT image=$FLOWPLANE_EVAL_IMAGE ($EVAL_IMAGE_ID)"
STACK_CREATED=1
compose up -d --no-build

container_id() {
  compose ps -q "$1" | tr -d '\r\n'
}
container_identity() {
  local id
  id=$(container_id "$1")
  [ -n "$id" ] || return 1
  "$RUNTIME" inspect --format '{{.Id}}|{{.State.StartedAt}}|{{.Image}}|{{.RestartCount}}' "$id"
}
container_running() {
  local id
  id=$(container_id "$1")
  [ -n "$id" ] && [ "$("$RUNTIME" inspect --format '{{.State.Running}}' "$id")" = true ]
}
probe_curl() {
  compose exec -T recovery-probe curl "$@"
}
agent_health_code() {
  probe_curl -sS --max-time 2 -o /dev/null -w '%{http_code}' \
    http://127.0.0.1:19902/healthz 2>/dev/null || true
}
read_token() {
  compose exec -T flowplane-eval sh -c 'cat /shared/dev-token' 2>/dev/null | tr -d '\r\n'
}
api_ready() {
  curl -fsS --max-time 3 -H "Authorization: Bearer $TOKEN" "http://$API/api/v1/teams" >/dev/null 2>&1
}
refresh_token_and_wait_api() {
  TOKEN=""
  for _ in $(seq 1 120); do
    TOKEN=$(read_token || true)
    if [ -n "$TOKEN" ] && api_ready; then
      auth=(-H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json")
      return 0
    fi
    sleep 1
  done
  return 1
}
read_dataplane_snapshot() {
  curl -fsS "${auth[@]}" "http://$API/api/v1/teams/default/dataplanes/dp-eval" | python3 -c '
import datetime, json, sys
value = json.load(sys.stdin)
required = ("last_heartbeat_at", "total_requests", "total_errors", "id")
missing = [key for key in required if key not in value]
if missing:
    raise SystemExit("missing dataplane fields: " + ",".join(missing))
heartbeat = value["last_heartbeat_at"]
if not isinstance(heartbeat, str) or not heartbeat:
    raise SystemExit("last_heartbeat_at is not a non-empty string")
datetime.datetime.fromisoformat(heartbeat.replace("Z", "+00:00"))
for key in ("total_requests", "total_errors"):
    if not isinstance(value[key], int) or isinstance(value[key], bool) or value[key] < 0:
        raise SystemExit(key + " is not a non-negative integer")
if not isinstance(value["id"], str) or not value["id"]:
    raise SystemExit("id is not a non-empty string")
print(heartbeat, value["total_requests"], value["total_errors"], value["id"], sep="\t")'
}
read_envoy_snapshot() {
  probe_curl -fsS --max-time 5 'http://127.0.0.1:9901/stats?format=json' | python3 -c '
import json, sys
value = json.load(sys.stdin)
stats = value.get("stats")
if not isinstance(stats, list):
    raise SystemExit("Envoy stats response has no stats array")
requests = errors = 0
for entry in stats:
    if not isinstance(entry, dict):
        continue
    name, counter = entry.get("name"), entry.get("value")
    if not isinstance(name, str) or not isinstance(counter, int) or isinstance(counter, bool):
        continue
    if name.endswith(".downstream_rq_total"):
        requests += counter
    elif name.endswith(".downstream_rq_5xx"):
        errors += counter
print(requests, errors, sep="\t")'
}
heartbeat_advanced() {
  python3 - "$1" "$2" <<'PY'
import datetime
import sys
before = datetime.datetime.fromisoformat(sys.argv[1].replace("Z", "+00:00"))
after = datetime.datetime.fromisoformat(sys.argv[2].replace("Z", "+00:00"))
raise SystemExit(0 if after > before else 1)
PY
}
reconcile_cp_with_next_envoy_poll() {
  local require_newer_than=$1 envoy_snapshot envoy_requests envoy_errors
  local target_requests target_errors snapshot heartbeat requests errors id
  envoy_snapshot=$(read_envoy_snapshot) || fail "authoritative Envoy counter snapshot unavailable"
  IFS=$'\t' read -r envoy_requests envoy_errors <<<"$envoy_snapshot"

  # The external /stats request itself completes after its response snapshot, and the agent's
  # next /stats request contributes one more admin request. Therefore the next accepted report
  # must carry exactly Envoy snapshot requests + 1 and the same error total. Sampling at 250 ms
  # catches that ten-second-wide state before another scheduled scrape can legitimately advance it.
  target_requests=$((envoy_requests + 1))
  target_errors=$envoy_errors
  for _ in $(seq 1 80); do
    snapshot=$(read_dataplane_snapshot 2>/dev/null || true)
    if [ -n "$snapshot" ]; then
      IFS=$'\t' read -r heartbeat requests errors id <<<"$snapshot"
      [ "$id" = "$DP_ID" ] || fail "dataplane API identity changed during reconciliation"
      [ "$requests" -le "$target_requests" ] \
        || fail "CP request counter exceeded authoritative Envoy total ($requests > $target_requests)"
      [ "$errors" -le "$target_errors" ] \
        || fail "CP error counter exceeded authoritative Envoy total ($errors > $target_errors)"
      if [ "$requests" -eq "$target_requests" ] \
        && [ "$errors" -eq "$target_errors" ] \
        && heartbeat_advanced "$require_newer_than" "$heartbeat"; then
        RECONCILED_SNAPSHOT=$snapshot
        return 0
      fi
    fi
    sleep 0.25
  done
  return 1
}
create_resource() {
  local segment=$1 payload=$2 code
  code=$(curl -sS "${auth[@]}" -o "$RESPONSE" -w '%{http_code}' -X POST \
    "http://$API/api/v1/teams/default/$segment" -d "$payload")
  [ "$code" = 201 ] || fail "create $segment returned $code: $(tr '\n' ' ' <"$RESPONSE")"
}
wait_config_names() {
  local first=$1 second=$2 third=$3 dump
  for _ in $(seq 1 90); do
    dump=$(probe_curl -fsS --max-time 5 http://127.0.0.1:9901/config_dump 2>/dev/null || true)
    if [[ $dump == *"$first"* && $dump == *"$second"* && $dump == *"$third"* ]]; then
      return 0
    fi
    sleep 1
  done
  return 1
}
assert_dataplane_unchanged() {
  [ "$(container_identity flowplane-agent)" = "$AGENT_IDENTITY" ] \
    || fail "agent container identity/start time/image/restart count changed"
  [ "$(container_identity envoy)" = "$ENVOY_IDENTITY" ] \
    || fail "Envoy container identity/start time/image/restart count changed"
  container_running flowplane-agent || fail "agent container is not running"
  container_running envoy || fail "Envoy container is not running"
}

refresh_token_and_wait_api || fail "initial control plane/API never became ready"
for _ in $(seq 1 120); do
  [ "$(agent_health_code)" = 200 ] && break
  sleep 1
done
[ "$(agent_health_code)" = 200 ] || fail "agent health never reached HTTP 200 in the mTLS topology"

AGENT_IDENTITY=$(container_identity flowplane-agent) || fail "could not record agent container identity"
ENVOY_IDENTITY=$(container_identity envoy) || fail "could not record Envoy container identity"
AGENT_IMAGE=$(container_id flowplane-agent)
AGENT_IMAGE=$("$RUNTIME" inspect --format '{{.Image}}' "$AGENT_IMAGE")
[ "$AGENT_IMAGE" = "$EVAL_IMAGE_ID" ] || fail "agent did not start from requested eval image"
CP_IMAGE=$(container_id flowplane-eval)
CP_IMAGE=$("$RUNTIME" inspect --format '{{.Image}}' "$CP_IMAGE")
[ "$CP_IMAGE" = "$EVAL_IMAGE_ID" ] || fail "control plane did not start from requested eval image"

DP_ID=$(curl -fsS "${auth[@]}" "http://$API/api/v1/teams/default/dataplanes/dp-eval" \
  | python3 -c 'import json,sys; value=json.load(sys.stdin).get("id"); assert isinstance(value,str) and value; print(value)')

# Add a deterministic error listener before the outage. It points at an unbound loopback port in
# Envoy's network namespace, so exactly one request below deterministically contributes one 5xx.
create_resource clusters \
  '{"name":"recovery-dead","spec":{"endpoints":[{"host":"127.0.0.1","port":1}],"lb_policy":"round-robin","connect_timeout_secs":1,"use_tls":false}}'
create_resource route-configs \
  '{"name":"recovery-error-routes","spec":{"virtual_hosts":[{"name":"default","domains":["*"],"routes":[{"name":"dead","match":{"prefix":{"prefix":"/"}},"action":{"cluster":"recovery-dead"}}]}]}}'
create_resource listeners \
  "{\"name\":\"recovery-error\",\"spec\":{\"address\":\"0.0.0.0\",\"port\":$ERROR_LISTENER_PORT,\"protocol\":\"http\",\"route_config\":\"recovery-error-routes\"}}"
wait_config_names recovery-dead recovery-error-routes recovery-error \
  || fail "deterministic error configuration never reached Envoy"

LAST_GOOD_BODY=$(probe_curl -fsS --max-time 5 http://127.0.0.1:10000/) \
  || fail "could not capture last-good response"
[ -n "$LAST_GOOD_BODY" ] || fail "last-good response was empty"

BASELINE=""
for _ in $(seq 1 60); do
  SNAPSHOT=$(read_dataplane_snapshot 2>/dev/null || true)
  if [ -n "$SNAPSHOT" ]; then
    BASELINE=$SNAPSHOT
    break
  fi
  sleep 1
done
[ -n "$BASELINE" ] || fail "initial accepted dataplane snapshot unavailable"
IFS=$'\t' read -r HEARTBEAT_BEFORE REQUESTS_BEFORE ERRORS_BEFORE API_DP_ID <<<"$BASELINE"
[ "$API_DP_ID" = "$DP_ID" ] || fail "dataplane API identity changed before outage"

# Stop ONLY the CP container. Compose start below restarts that same stopped CP container; no other
# service is stopped, started, recreated, or replaced during the drill.
echo "P2b: stopping only control-plane service flowplane-eval"
compose stop -t 20 flowplane-eval
container_running flowplane-eval && fail "control plane remained running after Compose stop"
assert_dataplane_unchanged

for _ in $(seq 1 120); do
  assert_dataplane_unchanged
  [ "$(agent_health_code)" = 503 ] && break
  sleep 1
done
[ "$(agent_health_code)" = 503 ] || fail "agent readiness never degraded from 200 to 503"

for _ in $(seq 1 "$OUTAGE_SUCCESSES"); do
  body=$(probe_curl -fsS --max-time 5 http://127.0.0.1:10000/) \
    || fail "Envoy stopped serving last-good traffic during CP outage"
  [ "$body" = "$LAST_GOOD_BODY" ] \
    || fail "outage response was not byte-for-byte equal to the last-good response"
done
ERROR_CODE=$(probe_curl -sS --max-time 5 -o /dev/null -w '%{http_code}' \
  "http://127.0.0.1:$ERROR_LISTENER_PORT/" 2>/dev/null || true)
[ "$ERROR_CODE" = 503 ] || fail "deterministic outage error returned $ERROR_CODE instead of 503"
assert_dataplane_unchanged

# Restore ONLY the CP. The agent and Envoy must recover in place.
echo "P2b: restoring only control-plane service flowplane-eval"
compose start flowplane-eval
refresh_token_and_wait_api || fail "restored control plane/API never became ready"
for _ in $(seq 1 180); do
  assert_dataplane_unchanged
  [ "$(agent_health_code)" = 200 ] && break
  sleep 1
done
[ "$(agent_health_code)" = 200 ] || fail "agent readiness did not recover from 503 to 200"
assert_dataplane_unchanged

RECONCILED_SNAPSHOT=""
reconcile_cp_with_next_envoy_poll "$HEARTBEAT_BEFORE" \
  || fail "recovered CP counters did not reconcile with authoritative Envoy counters"
IFS=$'\t' read -r HEARTBEAT_AFTER REQUESTS_AFTER ERRORS_AFTER _api_dp_id_after <<<"$RECONCILED_SNAPSHOT"
[ "$ERRORS_AFTER" -gt "$ERRORS_BEFORE" ] \
  || fail "known outage error traffic did not advance Envoy/CP error counters"
[ "$REQUESTS_AFTER" -ge $((REQUESTS_BEFORE + OUTAGE_SUCCESSES + OUTAGE_ERRORS)) ] \
  || fail "outage request counters omitted known gateway traffic"

# Reconcile again after another report cycle. Any non-idempotent replay exceeds Envoy's
# authoritative cumulative counters even though legitimate admin scrapes continue.
sleep 11
RECONCILED_SNAPSHOT=""
reconcile_cp_with_next_envoy_poll "$HEARTBEAT_AFTER" \
  || fail "post-replay CP counters did not reconcile with authoritative Envoy counters"
STABLE=$RECONCILED_SNAPSHOT
IFS=$'\t' read -r HEARTBEAT_STABLE REQUESTS_STABLE ERRORS_STABLE API_DP_ID_STABLE <<<"$STABLE"
[ "$ERRORS_STABLE" -eq "$ERRORS_AFTER" ] \
  || fail "error counter changed without new error traffic ($ERRORS_STABLE != $ERRORS_AFTER)"
[ "$REQUESTS_STABLE" -ge "$REQUESTS_AFTER" ] \
  || fail "request counter regressed after replay observation"
[ "$API_DP_ID_STABLE" = "$DP_ID" ] || fail "dataplane identity changed after replay observation"

# Mutate persisted config through the restored CP and prove the same Envoy receives and serves it.
create_resource clusters \
  '{"name":"recovery-new-upstream","spec":{"endpoints":[{"host":"recovery-upstream","port":5679}],"lb_policy":"round-robin","connect_timeout_secs":2,"use_tls":false}}'
create_resource route-configs \
  '{"name":"recovery-new-routes","spec":{"virtual_hosts":[{"name":"default","domains":["*"],"routes":[{"name":"all","match":{"prefix":{"prefix":"/"}},"action":{"cluster":"recovery-new-upstream"}}]}]}}'
create_resource listeners \
  "{\"name\":\"recovery-new-listener\",\"spec\":{\"address\":\"0.0.0.0\",\"port\":$RECOVERED_LISTENER_PORT,\"protocol\":\"http\",\"route_config\":\"recovery-new-routes\"}}"
wait_config_names recovery-new-upstream recovery-new-routes recovery-new-listener \
  || fail "post-restart configuration update never reached Envoy"
RECOVERED_BODY=$(probe_curl -fsS --max-time 5 "http://127.0.0.1:$RECOVERED_LISTENER_PORT/") \
  || fail "post-restart listener did not serve through Envoy"
[ "$RECOVERED_BODY" = "hello from recovered control plane" ] \
  || fail "post-restart listener returned unexpected body: $RECOVERED_BODY"
assert_dataplane_unchanged

echo "P2b OK: mTLS Compose recovery preserved agent='$AGENT_IDENTITY' Envoy='$ENVOY_IDENTITY'"
echo "P2b OK: health 200->503->200; heartbeat $HEARTBEAT_BEFORE->$HEARTBEAT_STABLE"
echo "P2b OK: exact outage traffic success/error=$OUTAGE_SUCCESSES/$OUTAGE_ERRORS; counters requests=$REQUESTS_BEFORE->$REQUESTS_STABLE errors=$ERRORS_BEFORE->$ERRORS_STABLE"
echo "P2b OK: restored CP pushed and served recovery-new-listener without replacing agent or Envoy"
