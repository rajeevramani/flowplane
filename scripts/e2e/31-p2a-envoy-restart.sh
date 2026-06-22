# E2E phase P2a — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES="P2"

# ---- Phase 2a: Envoy restart convergence. Restart the dataplane while the CP keeps live config
# state; the restarted Envoy must reconnect over ADS, receive persisted config, and serve traffic.
if [ "$ENVOY_MODE" = "docker" ]; then
  docker restart fp-e2e-envoy >/dev/null
else
  kill "$ENVOY_PID"; wait "$ENVOY_PID" 2>/dev/null || true
  envoy -c /tmp/fp-e2e-bootstrap.yaml --log-level info >> /tmp/fp-e2e-envoy.log 2>&1 &
  ENVOY_PID=$!
fi
wait_body hello-from-upstream2- || fail "traffic broke across Envoy restart"
./target/debug/flowplane ops xds nacks >/tmp/fp-e2e-xds-nacks-after-envoy-restart.txt
grep -q "no rows" /tmp/fp-e2e-xds-nacks-after-envoy-restart.txt || fail "unexpected xDS NACKs after Envoy restart"
echo "PHASE 2a OK: Envoy restarted, reconnected, and served '$BODY'"
