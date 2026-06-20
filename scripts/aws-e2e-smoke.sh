#!/usr/bin/env bash
# AWS end-to-end smoke test, one step at a time.
#
#   scripts/aws-e2e-smoke.sh <step>
#
# Runs the full live flow against the AWS-hosted control plane and a LOCAL Envoy
# dataplane, ending with a routed API call through Envoy. Each step is independent
# and re-runnable; the flowplane CLI persists your login between steps.
#
# Run `scripts/aws-e2e-smoke.sh` with no arg to list steps.
#
# ponytail: a smoke runbook, not product code — hardcodes the demo account/zone and
# single-CA trust on purpose. Generalize only if this becomes a real test harness.
set -euo pipefail

# ---- config (override via env) ---------------------------------------------
FP="${FP:-./target/release/flowplane}"
AWS_PROFILE="${AWS_PROFILE:-rajeev-flowplane-demo}"
AWS_REGION="${AWS_REGION:-us-east-1}"

export FLOWPLANE_SERVER="${FLOWPLANE_SERVER:-https://cp.getflowplane.io}"

# Auth0 / OIDC + admin identity — provide via env; do NOT hardcode tenant identifiers here.
# Export these before running (or source a gitignored local env file of your own):
#   export FLOWPLANE_OIDC_ISSUER=https://<tenant>.us.auth0.com/
#   export FLOWPLANE_OIDC_CLIENT_ID=<auth0-native-app-client-id>
#   export ADMIN_EMAIL=<platform-admin email>
#   export ADMIN_SUBJECT='<auth0|... admin OIDC subject>'
export FLOWPLANE_OIDC_ISSUER="${FLOWPLANE_OIDC_ISSUER:-}"
export FLOWPLANE_OIDC_CLIENT_ID="${FLOWPLANE_OIDC_CLIENT_ID:-}"
export FLOWPLANE_OIDC_SCOPE="${FLOWPLANE_OIDC_SCOPE:-openid email profile}"
ADMIN_SUBJECT="${ADMIN_SUBJECT:-}"
ADMIN_EMAIL="${ADMIN_EMAIL:-}"

# tenant context (created by this script)
export FLOWPLANE_ORG="${FLOWPLANE_ORG:-acme}"
export FLOWPLANE_TEAM="${FLOWPLANE_TEAM:-payments}"

need_oidc() { # fail with guidance if required identifiers are not provided
  [ -n "$FLOWPLANE_OIDC_ISSUER" ]    || die "set FLOWPLANE_OIDC_ISSUER (e.g. https://<tenant>.us.auth0.com/)"
  [ -n "$FLOWPLANE_OIDC_CLIENT_ID" ] || die "set FLOWPLANE_OIDC_CLIENT_ID (Auth0 native-app client id)"
}

DP_NAME="${DP_NAME:-dp-local}"
UPSTREAM_PORT="${UPSTREAM_PORT:-3001}"
LISTEN_PORT="${LISTEN_PORT:-10001}"

CP_HOST="cp.getflowplane.io"
XDS_HOST="xds.getflowplane.io"
WORK="${WORK:-/tmp/fp-e2e}"
mkdir -p "$WORK"

say() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }
ok()  { printf '\033[1;32m   ok: %s\033[0m\n' "$*"; }
die() { printf '\033[1;31m   FAIL: %s\033[0m\n' "$*" >&2; exit 1; }

case "${1:-help}" in

# ---------------------------------------------------------------------------
0-preflight)
  say "Preflight: tooling + reachability"
  command -v "$FP" >/dev/null 2>&1 || [ -x "$FP" ] || die "flowplane binary not at $FP (cargo build --release --bin flowplane)"
  command -v envoy >/dev/null 2>&1 || die "envoy not installed (brew install envoy)"
  command -v python3 >/dev/null 2>&1 || die "python3 missing"
  command -v jq >/dev/null 2>&1 || echo "   note: jq not found, will use python3 for JSON"
  echo "   flowplane: $($FP --version 2>/dev/null || echo "$FP")"
  echo "   envoy:     $(envoy --version 2>&1 | head -1)"
  echo "   server:    $FLOWPLANE_SERVER"
  getent_host() { python3 -c "import socket,sys;print(socket.gethostbyname(sys.argv[1]))" "$1" 2>/dev/null || echo "UNRESOLVED"; }
  echo "   $CP_HOST  -> $(getent_host $CP_HOST)"
  echo "   $XDS_HOST -> $(getent_host $XDS_HOST)"
  echo "   (if UNRESOLVED or pointing at an old ELB, run step 1-hosts)"
  ;;

# ---------------------------------------------------------------------------
1-hosts)
  say "Fetch current ALB/NLB IPs and print /etc/hosts lines"
  ALB_IP=$(aws ec2 describe-network-interfaces --region "$AWS_REGION" --profile "$AWS_PROFILE" \
    --filters "Name=description,Values=ELB app/flowplane-api/*" \
    --query 'NetworkInterfaces[0].Association.PublicIp' --output text)
  NLB_IP=$(aws ec2 describe-network-interfaces --region "$AWS_REGION" --profile "$AWS_PROFILE" \
    --filters "Name=description,Values=ELB net/flowplane-xds/*" \
    --query 'NetworkInterfaces[0].Association.PublicIp' --output text)
  [ "$ALB_IP" != "None" ] && [ -n "$ALB_IP" ] || die "no ALB IP — is the deployment up?"
  printf '%s\n' "$ALB_IP $CP_HOST" "$NLB_IP $XDS_HOST" > "$WORK/hosts.add"
  echo "   Add these to /etc/hosts (CLI + Envoy use the OS resolver):"
  echo
  sed 's/^/      /' "$WORK/hosts.add"
  echo
  echo "   Run:"
  echo "      sudo sh -c 'grep -v getflowplane.io /etc/hosts > /tmp/h && cat /tmp/h $WORK/hosts.add > /etc/hosts'"
  echo "   (the grep drops any stale getflowplane.io lines first)"
  ;;

# ---------------------------------------------------------------------------
2-login-admin)
  say "Device-code login as ADMIN (Auth0 user 1)"
  need_oidc
  "$FP" auth login --device-code \
    --issuer "$FLOWPLANE_OIDC_ISSUER" \
    --client-id "$FLOWPLANE_OIDC_CLIENT_ID" \
    --scope "$FLOWPLANE_OIDC_SCOPE"
  ok "approve in the browser as the admin identity ($ADMIN_EMAIL)"
  ;;

3-whoami)
  say "Verify admin identity + JIT row"
  "$FP" auth whoami
  echo "   expect PLATFORM ADMIN = true"
  ;;

# ---------------------------------------------------------------------------
4-org-team)
  say "Create tenant org + team, make admin a member (D-014 context)"
  [ -n "$ADMIN_EMAIL" ] || die "set ADMIN_EMAIL to the platform-admin email"
  "$FP" org create "$FLOWPLANE_ORG" --display-name "Acme" || echo "   (org may already exist)"
  "$FP" org member add "$FLOWPLANE_ORG" --email "$ADMIN_EMAIL" --role admin || echo "   (admin may already be a member)"
  "$FP" team create "$FLOWPLANE_TEAM" --display-name "Payments" || echo "   (team may already exist)"
  echo "   org=$FLOWPLANE_ORG team=$FLOWPLANE_TEAM (org admin reaches all teams implicitly)"
  "$FP" org list
  ;;

# ---------------------------------------------------------------------------
5-dataplane)
  say "Register a dataplane record"
  "$FP" dataplane create "$DP_NAME" --description "local Envoy smoke" || echo "   (may already exist)"
  "$FP" dataplane list
  ;;

# ---------------------------------------------------------------------------
6-cert)
  say "Issue the dataplane mTLS client cert (CP cert-issuer CA)"
  "$FP" -o json dataplane cert issue "$DP_NAME" --ttl-hours 24 > "$WORK/cert.json"
  python3 - "$WORK" <<'PY'
import json,sys,os
w=sys.argv[1]
d=json.load(open(f"{w}/cert.json"))
for k,f in [("certificate_pem","client.crt"),("private_key_pem","client.key"),("ca_certificate_pem","ca.crt")]:
    open(f"{w}/{f}","w").write(d[k]);
os.chmod(f"{w}/client.key",0o600)
print("   wrote client.crt, client.key, ca.crt to",w)
PY
  ok "cert/key/ca staged in $WORK"
  ;;

# ---------------------------------------------------------------------------
7-upstream)
  say "Start a local upstream on :$UPSTREAM_PORT"
  mkdir -p "$WORK/upstream"
  printf 'hello-flowplane\n' > "$WORK/upstream/index.html"
  ( cd "$WORK/upstream" && nohup python3 -m http.server "$UPSTREAM_PORT" >"$WORK/upstream.log" 2>&1 & echo $! > "$WORK/upstream.pid" )
  sleep 1
  curl -fsS "http://127.0.0.1:$UPSTREAM_PORT/" && ok "upstream up (pid $(cat $WORK/upstream.pid))" || die "upstream not responding"
  ;;

# ---------------------------------------------------------------------------
8-expose)
  say "Expose the upstream -> cluster + route + listener (:$LISTEN_PORT)"
  "$FP" expose "http://127.0.0.1:$UPSTREAM_PORT" \
    --name local --path / --port "$LISTEN_PORT" \
    --public-base-url "http://127.0.0.1:$LISTEN_PORT"
  ;;

# ---------------------------------------------------------------------------
9-bootstrap)
  say "Generate the Envoy mTLS bootstrap (xDS -> $XDS_HOST:18000)"
  "$FP" --out "$WORK/envoy.yaml" dataplane bootstrap "$DP_NAME" \
    --mode mtls \
    --xds-host "$XDS_HOST" --xds-port 18000 --admin-port 9901 \
    --cert-path "$WORK/client.crt" \
    --key-path  "$WORK/client.key" \
    --ca-path   "$WORK/ca.crt"
  ok "wrote $WORK/envoy.yaml"
  echo "   (xds-host = $XDS_HOST matches the xDS server cert SAN; /etc/hosts maps it to the NLB)"
  ;;

# ---------------------------------------------------------------------------
10-envoy)
  say "Run local Envoy (foreground — watch for xDS connect, Ctrl-C to stop)"
  echo "   expect: cluster/listener warmed, no NACKs"
  exec envoy -c "$WORK/envoy.yaml" --log-level info
  ;;

# ---------------------------------------------------------------------------
11-curl)
  say "THE TEST: call through Envoy"
  curl -i "http://127.0.0.1:$LISTEN_PORT/" || die "no response — check Envoy logs + step 12-diag"
  echo
  ok "expect body: hello-flowplane"
  ;;

# ---------------------------------------------------------------------------
12-diag)
  say "Diagnostics (CP-side, the operator path)"
  "$FP" stats overview || true
  "$FP" ops xds status || true
  "$FP" ops xds nacks || true
  ;;

# ---- optional: onboard the SECOND Auth0 user ------------------------------
u2-login)
  say "Login as Auth0 user 2 (separate context to avoid clobbering admin)"
  need_oidc
  echo "   NOTE: this overwrites the stored CLI token. Re-run 2-login-admin to switch back."
  "$FP" auth login --device-code --issuer "$FLOWPLANE_OIDC_ISSUER" \
    --client-id "$FLOWPLANE_OIDC_CLIENT_ID" --scope "$FLOWPLANE_OIDC_SCOPE"
  "$FP" auth whoami   # JIT-creates the user 2 row; copy its USER ID / subject
  ;;

u2-add)
  say "As ADMIN, add user 2 to the org (re-login as admin first!)"
  echo "   usage: U2_SUBJECT='auth0|...' scripts/aws-e2e-smoke.sh u2-add"
  : "${U2_SUBJECT:?set U2_SUBJECT to the user-2 OIDC subject from u2-login whoami}"
  "$FP" org member add "$FLOWPLANE_ORG" --subject "$U2_SUBJECT" --role member
  "$FP" org member list "$FLOWPLANE_ORG"
  ;;

# ---------------------------------------------------------------------------
cleanup)
  say "Tear down local bits (leaves AWS infra alone)"
  [ -f "$WORK/upstream.pid" ] && kill "$(cat $WORK/upstream.pid)" 2>/dev/null && echo "   stopped upstream" || true
  "$FP" unexpose local 2>/dev/null && echo "   unexposed listener" || true
  echo "   (stop Envoy with Ctrl-C in its terminal; AWS teardown: tofu -chdir=deploy/aws destroy)"
  ;;

*)
  cat <<EOF
AWS end-to-end smoke — run one step at a time:

  0-preflight   tooling + DNS check
  1-hosts       fetch live ALB/NLB IPs, print /etc/hosts lines  (run the sudo cmd it prints)
  2-login-admin device login as Auth0 user 1 (admin)
  3-whoami      verify PLATFORM ADMIN true
  4-org-team    create org '$FLOWPLANE_ORG' + team '$FLOWPLANE_TEAM', add admin
  5-dataplane   register dataplane '$DP_NAME'
  6-cert        issue mTLS client cert -> $WORK/{client.crt,client.key,ca.crt}
  7-upstream    start local upstream on :$UPSTREAM_PORT
  8-expose      create cluster+route+listener on :$LISTEN_PORT
  9-bootstrap   generate Envoy mTLS bootstrap -> $WORK/envoy.yaml
  10-envoy      run local Envoy (foreground)
  11-curl       >>> curl through Envoy, expect 'hello-flowplane'   (new terminal)
  12-diag       CP-side xDS diagnostics

  optional:
  u2-login      login as Auth0 user 2 (clobbers admin token)
  u2-add        admin adds user 2 to the org (U2_SUBJECT=...)
  cleanup       stop upstream + unexpose

Typical order: 0 1 (sudo) 2 3 4 5 6 7 8 9 10(keep running) then 11 in a new terminal.
EOF
  ;;
esac
