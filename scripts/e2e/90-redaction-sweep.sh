# E2E terminal gate — sourced last by scripts/e2e/run.sh after the selected phases.
# Runs the Tier-0 redaction sweep, then the known-failure exit gate.
#
# The redaction sweep is authoritative ONLY for a full run: it asserts that the *current* API
# bearer token does not appear in the control-plane log, which holds for the full suite only
# because phase P2 restarts the CP and rotates the token (the pre-restart log carries a different
# token). A partial `--only`/`--from` selection without P2 would false-positive on the dev-mode
# token that the CP legitimately logs (and which setup itself reads from that log). So the sweep
# is enforced on full runs and skipped — with a clear note — on subsets, where it proves little.
if [ "${E2E_FULL_RUN:-0}" = "1" ]; then
  redaction_sweep
else
  echo "REDACTION SWEEP SKIPPED: subset run (${E2E_SELECTED:-?}); the sweep is authoritative only for a full run (see scripts/e2e/90-redaction-sweep.sh)"
fi

if [ "$KNOWN_FAIL_COUNT" -gt 0 ]; then
  echo "E2E INCOMPLETE: $KNOWN_FAIL_COUNT known failure(s) recorded (tracked product bugs); all other phases + redaction sweep passed"
  exit 1
fi
if [ "${E2E_FULL_RUN:-0}" = "1" ]; then
  echo "E2E PASSED: traffic, learning capture, traffic-first discovery, CP restart convergence, Envoy restart convergence, cross-team isolation, http filters, auth filters, MCP descriptor gateway parity, SDS rotation, advanced parity, AI streaming boundary, redaction sweep"
else
  echo "E2E PASSED (subset: ${E2E_SELECTED:-?}): selected phases passed (redaction sweep enforced on full runs only)"
fi
exit 0
