#!/usr/bin/env bash
# Back-compat shim. The live Envoy E2E suite was split into scripts/e2e/ (run.sh + lib.sh +
# ordered NN-*.sh phase files) for maintainability — see issue #71. This path is preserved
# because it is the documented/manual-gate entrypoint (README, release-walkthrough, COVERAGE,
# failure-mode-matrix). All arguments pass through, so `--from`/`--only`/`--list` work here too.
exec "$(dirname "$0")/e2e/run.sh" "$@"
