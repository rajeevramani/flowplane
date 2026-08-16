#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)

assert_exec_server() {
  local file=$1 pattern=$2
  if ! grep -Eq "$pattern" "$root/$file"; then
    echo "$file must exec the HTTP server inside the background subshell so cleanup owns the real PID" >&2
    exit 1
  fi
}

assert_exec_server scripts/e2e/lib.sh '\(cd /tmp/fp-e2e-www && exec python3 -m http\.server'
assert_exec_server scripts/e2e/30-p2-cp-restart.sh '\(cd /tmp/fp-e2e-www2 && exec python3 -m http\.server'

echo "qualification E2E PID ownership contract: PASS"
