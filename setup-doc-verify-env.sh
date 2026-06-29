#!/usr/bin/env bash
#
# setup-doc-verify-env.sh
# -----------------------------------------------------------------------------
# Prepares this VM to run the "runtime-verify Flowplane v2.1.0 documentation"
# mission. It only installs/starts TOOLING and pre-pulls artifacts; it does NOT
# run the verification itself and makes no product-code or doc changes.
#
# Idempotent: safe to re-run. Each step skips if already satisfied.
#
# Verified facts this script is built on (checked on Ubuntu 24.04 / x86_64):
#   - Docker 29.3.1 client + compose v2 plugin are installed, but the daemon is
#     NOT auto-started and does not persist across shells -> we start it here.
#   - GHCR public images pull through the egress proxy with no login.
#   - The v2.1.0 GitHub *release* ships ONLY compose.eval.yml -- there is NO host
#     CLI binary asset -> we copy `flowplane` out of the :2.1.0-eval image.
#   - The in-image flowplane binary is a glibc x86_64 ELF and runs natively here.
#   - api.github.com / github.com auth is injected by the proxy, so `gh` and the
#     raw API authenticate as the repo owner even though $GH_TOKEN is a placeholder.
# -----------------------------------------------------------------------------
set -euo pipefail

# ---- Configurable knobs ------------------------------------------------------
REPO_OWNER="rajeevramani"
REPO_NAME="flowplane"
DOCS_BRANCH="feature/fpv2-0ym-adoption-evaluation-spine"
EVAL_IMAGE="ghcr.io/${REPO_OWNER}/${REPO_NAME}:2.1.0-eval"
PROD_IMAGE="ghcr.io/${REPO_OWNER}/${REPO_NAME}:2.1.0"
DEX_IMAGE="${DEX_IMAGE:-ghcr.io/dexidp/dex:latest}"      # throwaway local IdP for Track 2
REPO_DIR="${REPO_DIR:-$(pwd)}"                            # assumes you run from the repo
DOCS_WORKTREE="${DOCS_WORKTREE:-${REPO_DIR}/../flowplane-docs-${DOCS_BRANCH##*/}}"
ENV_FILE="${ENV_FILE:-${REPO_DIR}/.uat.env}"             # generated secrets/OIDC env
VAULT_DESIGN_DST="/Users/rajeevramani/workspace/projects/flowplane-private-vault/releases/2.1.1/features/2026-06-adoption-evaluation-spine/10-design.md"
VAULT_DESIGN_SRC="${VAULT_DESIGN_SRC:-}"                  # optional: path to the uploaded 10-design.md

log()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m  ok\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m  !!\033[0m %s\n' "$*" >&2; }

[ "$(id -u)" -eq 0 ] || warn "not running as root; some installs may need sudo"
[ "$(uname -m)" = "x86_64" ] || warn "expected x86_64; download URLs below assume amd64"

# ---- 1. Docker daemon (start + persist) -------------------------------------
log "Docker daemon"
if docker info >/dev/null 2>&1; then
  ok "daemon already running"
else
  command -v dockerd >/dev/null || { warn "dockerd binary missing; install docker first"; exit 1; }
  nohup dockerd >/var/log/dockerd.log 2>&1 &
  disown || true
  for _ in $(seq 1 30); do docker info >/dev/null 2>&1 && break; sleep 1; done
  docker info >/dev/null 2>&1 && ok "daemon started (log: /var/log/dockerd.log)" \
                              || { warn "daemon failed to start; see /var/log/dockerd.log"; exit 1; }
fi
docker compose version >/dev/null 2>&1 && ok "compose v2 plugin present" \
                                       || warn "docker compose plugin missing"

# ---- 2. Base apt packages ----------------------------------------------------
log "Base packages (curl, jq, ca-certificates, git, openssl)"
need_apt=()
for p in curl jq ca-certificates git openssl; do
  dpkg -s "$p" >/dev/null 2>&1 || need_apt+=("$p")
done
if [ "${#need_apt[@]}" -gt 0 ]; then
  apt-get update -qq && apt-get install -y -qq "${need_apt[@]}" && ok "installed: ${need_apt[*]}"
else
  ok "all present"
fi

# ---- 3. GitHub CLI (gh) ------------------------------------------------------
# The mission text says "use gh for all issue ops". gh authenticates here because
# the proxy injects real credentials for api.github.com. (The sanctioned
# alternative in this environment is the GitHub MCP tools -- either works.)
log "GitHub CLI (gh)"
if command -v gh >/dev/null 2>&1; then
  ok "gh already installed: $(gh --version | head -1)"
else
  GH_VER="$(curl -fsSL https://api.github.com/repos/cli/cli/releases/latest | jq -r .tag_name | sed 's/^v//')"
  GH_VER="${GH_VER:-2.63.2}"
  tmp="$(mktemp -d)"
  curl -fsSL "https://github.com/cli/cli/releases/download/v${GH_VER}/gh_${GH_VER}_linux_amd64.tar.gz" \
    | tar -xz -C "$tmp"
  install -m0755 "$tmp/gh_${GH_VER}_linux_amd64/bin/gh" /usr/local/bin/gh
  rm -rf "$tmp"
  ok "installed gh ${GH_VER}"
fi
# sanity: confirm API write identity (auth is proxy-injected)
if gh api user -q .login >/dev/null 2>&1; then
  ok "gh authenticated as $(gh api user -q .login)"
else
  warn "gh not authenticated via proxy; use the GitHub MCP tools for issue ops instead"
fi

# ---- 4. OpenTofu (tofu) for deploy/aws validate ------------------------------
log "OpenTofu (tofu) -- needed for deploy/aws terraform/tofu validate"
if command -v tofu >/dev/null 2>&1; then
  ok "tofu already installed: $(tofu version | head -1)"
else
  TF_VER="$(curl -fsSL https://api.github.com/repos/opentofu/opentofu/releases/latest | jq -r .tag_name | sed 's/^v//')"
  TF_VER="${TF_VER:-1.9.0}"
  tmp="$(mktemp -d)"
  curl -fsSL "https://github.com/opentofu/opentofu/releases/download/v${TF_VER}/tofu_${TF_VER}_linux_amd64.tar.gz" \
    | tar -xz -C "$tmp"
  install -m0755 "$tmp/tofu" /usr/local/bin/tofu
  rm -rf "$tmp"
  ok "installed tofu ${TF_VER} (the AWS docs may say 'terraform'; tofu is CLI-compatible for validate)"
fi

# ---- 5. Pre-pull runtime images ---------------------------------------------
log "Pulling runtime images (public GHCR; no login needed)"
for img in "$EVAL_IMAGE" "$PROD_IMAGE" "$DEX_IMAGE"; do
  if docker image inspect "$img" >/dev/null 2>&1; then
    ok "present: $img"
  else
    docker pull "$img" >/dev/null && ok "pulled: $img" || warn "pull FAILED: $img"
  fi
done

# ---- 6. Host flowplane CLI (copied out of the eval image) --------------------
# No release binary exists; the CLI lives only inside the images. Copy it to the
# host so doc steps that call a host-installed `flowplane` work as written.
log "Host flowplane CLI"
if command -v flowplane >/dev/null 2>&1 && flowplane --version 2>/dev/null | grep -q '2.1.0'; then
  ok "flowplane 2.1.0 already on PATH"
else
  cid="$(docker create "$EVAL_IMAGE" sh)"
  docker cp "$cid":/usr/local/bin/flowplane /usr/local/bin/flowplane
  docker rm "$cid" >/dev/null
  chmod +x /usr/local/bin/flowplane
  ok "installed host CLI: $(flowplane --version)"
fi

# ---- 7. Secrets / OIDC env file ---------------------------------------------
log "Secrets & OIDC env file -> ${ENV_FILE}"
if [ -f "$ENV_FILE" ]; then
  ok "exists (left untouched): $ENV_FILE"
else
  ENC_KEY="$(openssl rand -hex 16)"   # 32 hex chars, matches the documented key shape
  cat >"$ENV_FILE" <<EOF
# Generated by setup-doc-verify-env.sh -- throwaway local values, do NOT commit.
# Secret encryption key (required by the production-shaped control plane).
FLOWPLANE_SECRET_ENCRYPTION_KEY=${ENC_KEY}

# Track 2 OIDC -- fill these AFTER dex is up. Point issuer at your local dex,
# audience at the client/audience dex emits, admin_subject at the test user's
# decoded 'sub'. Examples (adjust host/port to your dex config):
#   FLOWPLANE_OIDC_ISSUER=http://127.0.0.1:5556/dex
#   FLOWPLANE_OIDC_AUDIENCE=flowplane-cli
# CLI OIDC callback is http://127.0.0.1:8976/callback (dex staticClient redirect).
EOF
  ok "wrote $ENV_FILE (source it: 'set -a; . $ENV_FILE; set +a')"
fi

# ---- 8. Docs branch worktree (read-only source of truth) --------------------
# Keep your current working branch intact; check out the docs-under-test branch
# in a separate worktree so you can READ the docs without switching branches.
log "Docs branch worktree -> ${DOCS_WORKTREE}"
git -C "$REPO_DIR" fetch origin "$DOCS_BRANCH" --quiet
if [ -d "$DOCS_WORKTREE" ]; then
  ok "worktree already exists"
else
  git -C "$REPO_DIR" worktree add "$DOCS_WORKTREE" "origin/$DOCS_BRANCH" --quiet \
    && ok "worktree at $DOCS_WORKTREE"
fi
DOCS_SHA="$(git -C "$DOCS_WORKTREE" rev-parse HEAD 2>/dev/null || echo '?')"
ok "docs HEAD commit: $DOCS_SHA   (cite this sha when closing issues)"

# ---- 9. Vault issue-mapping doc (optional placement) -------------------------
# The authoritative #199-#216 mapping table lives in 10-design.md at a macOS-only
# path that does not exist on this VM. If you have the file, point VAULT_DESIGN_SRC
# at it and this drops a copy where the mission expects it.
log "Vault issue-mapping doc"
if [ -n "$VAULT_DESIGN_SRC" ] && [ -f "$VAULT_DESIGN_SRC" ]; then
  mkdir -p "$(dirname "$VAULT_DESIGN_DST")"
  cp "$VAULT_DESIGN_SRC" "$VAULT_DESIGN_DST"
  ok "placed mapping doc at $VAULT_DESIGN_DST"
else
  warn "no VAULT_DESIGN_SRC given; derive the #199-#216 mapping from issue text"
  warn "(or re-run with VAULT_DESIGN_SRC=/path/to/10-design.md)"
fi

# ---- Summary -----------------------------------------------------------------
log "Setup complete. Quick verification:"
printf '  docker   : %s\n' "$(docker --version)"
printf '  compose  : %s\n' "$(docker compose version --short 2>/dev/null || echo '?')"
printf '  gh       : %s\n' "$(gh --version 2>/dev/null | head -1 || echo 'MISSING')"
printf '  tofu     : %s\n' "$(tofu version 2>/dev/null | head -1 || echo 'MISSING')"
printf '  flowplane: %s\n' "$(flowplane --version 2>/dev/null || echo 'MISSING')"
printf '  images   : %s\n' "$(docker images --format '{{.Repository}}:{{.Tag}}' | grep -E 'flowplane|dex' | tr '\n' ' ')"
printf '  env file : %s\n' "$ENV_FILE"
printf '  docs tree: %s @ %s\n' "$DOCS_WORKTREE" "$DOCS_SHA"
log "Note: dockerd is ephemeral (no systemd) -- re-run this script after a VM/container restart."
