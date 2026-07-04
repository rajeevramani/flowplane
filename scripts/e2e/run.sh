#!/usr/bin/env bash
# Live Envoy E2E (S5.6/S7.7e, dev-mode path): boot CP -> configure via the CLI expose shortcut
# -> generate dataplane bootstrap -> a real Envoy joins over ADS -> traffic flows end to end.
#
# Runner for the split suite. Sources lib.sh, runs the shared setup_harness, then sources the
# selected ordered phase files, then the terminal redaction/exit gate. The legacy entrypoint
# scripts/e2e-envoy.sh is a thin shim that execs this with the same arguments.
#
# Usage:
#   scripts/e2e/run.sh                 # full suite (all phases, in order) — identical to the old run
#   scripts/e2e/run.sh --from P5       # P5 and every later phase, in canonical order
#   scripts/e2e/run.sh --only "P4 P5"  # just those phases; prerequisites are auto-included
#   scripts/e2e/run.sh --list          # list phase IDs -> files and exit
#   scripts/e2e/run.sh --plan --only P7  # print the resolved selection (with prereqs) and exit, no boot
#   scripts/e2e/run.sh -h | --help
#
# --only auto-includes the transitive prerequisites (REQUIRES) of each requested phase and
# always runs the resulting set in canonical order, so a selection like `--only P7` faithfully
# runs `P2 P7` (P7 needs the upstream2 server + cluster rewrite that P2 establishes).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$HERE/../.."   # repo root: all setup paths (./target/debug/flowplane, scripts/, cargo) are repo-relative
source "$HERE/lib.sh"

# Canonical phase order (index-aligned). File names carry the NN- ordering + phase ID; this table
# is the single source of truth for ordering and ID->file resolution.
PHASE_IDS=(P1 P1a P1d P1e P1f P1g P1b P1c P2 P2a P3 P4 P5 P5a P6 P7 P8)
PHASE_FILES=(
  10-p1-basic.sh 15-p1a-ai.sh 16-p1d-ai-stream.sh 17-p1e-ai-malformed.sh
  18-p1f-ai-trace.sh 19-p1g-ai-trace-failures.sh
  20-p1b-learning.sh 25-p1c-discovery.sh 30-p2-cp-restart.sh 31-p2a-envoy-restart.sh
  40-p3-isolation.sh 50-p4-http-filters.sh 60-p5-auth-filters.sh 65-p5a-mcp.sh
  70-p6-sds-rotation.sh 80-p7-advanced-parity.sh 82-p8-rls-enforcement.sh
)
TOTAL=${#PHASE_IDS[@]}

usage() { sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

idx_of() {  # echo the canonical index of phase id $1, or nothing if unknown
  local want="$1" i
  for i in "${!PHASE_IDS[@]}"; do [ "${PHASE_IDS[$i]}" = "$want" ] && { echo "$i"; return 0; }; done
  return 1
}

requires_of() {  # echo the REQUIRES list for phase id $1 (read without sourcing the phase)
  local i; i=$(idx_of "$1") || return 0
  sed -n 's/^REQUIRES="\(.*\)"/\1/p' "$HERE/${PHASE_FILES[$i]}"
}

list_phases() {
  local i
  echo "phase  file"
  for i in "${!PHASE_IDS[@]}"; do printf '%-6s %s\n' "${PHASE_IDS[$i]}" "${PHASE_FILES[$i]}"; done
}

# --- argument parsing ---
MODE=full ONLY_IDS="" FROM_ID="" PLAN_ONLY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --list) list_phases; exit 0 ;;
    --plan) PLAN_ONLY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    --only) MODE=only; ONLY_IDS="${2:-}"; shift $(( $# >= 2 ? 2 : 1 )) ;;
    --only=*) MODE=only; ONLY_IDS="${1#*=}"; shift ;;
    --from) MODE=from; FROM_ID="${2:-}"; shift $(( $# >= 2 ? 2 : 1 )) ;;
    --from=*) MODE=from; FROM_ID="${1#*=}"; shift ;;
    *) echo "error: unknown argument '$1' (try --help)" >&2; exit 2 ;;
  esac
done

# --- resolve the selected set of phase ids (SELECTED, in canonical order) ---
# WANT is a space-delimited membership set (bash 3.2 compatible — no associative arrays, so the
# suite still runs under macOS's stock /bin/bash).
WANT=" "
want_has() { case "$WANT" in *" $1 "*) return 0 ;; *) return 1 ;; esac; }
want_add() { want_has "$1" || WANT="$WANT$1 "; }

case "$MODE" in
  full)
    for id in "${PHASE_IDS[@]}"; do want_add "$id"; done
    ;;
  only)
    [ -n "$ONLY_IDS" ] || { echo "error: --only needs at least one phase id" >&2; exit 2; }
    # seed with requested ids (validate), then close over REQUIRES transitively
    queue=""
    for id in $ONLY_IDS; do
      idx_of "$id" >/dev/null || { echo "error: unknown phase id '$id' (try --list)" >&2; exit 2; }
      want_add "$id"; queue="$queue $id"
    done
    while [ -n "${queue// /}" ]; do
      set -- $queue; cur="$1"; shift; queue="$*"
      for dep in $(requires_of "$cur"); do
        if ! want_has "$dep"; then want_add "$dep"; queue="$queue $dep"; fi
      done
    done
    ;;
  from)
    start=$(idx_of "$FROM_ID") || { echo "error: unknown phase id '$FROM_ID' (try --list)" >&2; exit 2; }
    for i in "${!PHASE_IDS[@]}"; do [ "$i" -ge "$start" ] && want_add "${PHASE_IDS[$i]}"; done
    ;;
esac

# materialize SELECTED in canonical order
SELECTED=() SELECTED_FILES=()
for i in "${!PHASE_IDS[@]}"; do
  id="${PHASE_IDS[$i]}"
  want_has "$id" && { SELECTED+=("$id"); SELECTED_FILES+=("${PHASE_FILES[$i]}"); }
done

export E2E_SELECTED="${SELECTED[*]}"
if [ "${#SELECTED[@]}" -eq "$TOTAL" ]; then E2E_FULL_RUN=1; else E2E_FULL_RUN=0; fi
export E2E_FULL_RUN

if [ "$MODE" = only ]; then
  # show the expanded set so auto-included prerequisites are visible
  echo "selected (expanded for prerequisites): ${SELECTED[*]}"
fi

if [ "$PLAN_ONLY" = 1 ]; then
  echo "plan: mode=$MODE full_run=$E2E_FULL_RUN phases=${SELECTED[*]}"
  exit 0
fi

# --- run: shared setup -> selected phases -> terminal redaction/exit gate ---
trap cleanup EXIT
setup_harness
for f in "${SELECTED_FILES[@]}"; do source "$HERE/$f"; done
source "$HERE/90-redaction-sweep.sh"
