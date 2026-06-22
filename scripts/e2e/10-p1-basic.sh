# E2E phase P1 — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

wait_body hello-from-upstream- || fail "initial traffic never flowed"
./target/debug/flowplane stats overview >/tmp/fp-e2e-stats.txt
grep -q "TOTAL DATAPLANES" /tmp/fp-e2e-stats.txt || fail "stats overview did not render dataplane totals"
./target/debug/flowplane ops xds status >/tmp/fp-e2e-xds-status.txt
grep -q "TOTAL DATAPLANES" /tmp/fp-e2e-xds-status.txt || fail "xds status did not render dataplane totals"
./target/debug/flowplane ops xds nacks >/tmp/fp-e2e-xds-nacks.txt
grep -q "no rows" /tmp/fp-e2e-xds-nacks.txt || fail "unexpected xDS NACKs after happy-path expose"
echo "PHASE 1 OK: '$BODY' served through Envoy via ADS-delivered config"
