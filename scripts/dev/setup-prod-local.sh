#!/usr/bin/env bash
# Prepare a local production-mode Flowplane control plane backed by Auth0 OIDC.
set -euo pipefail

# scripts/dev/ -> repo root
cd "$(dirname "$0")/../.."

ENV_FILE=${FLOWPLANE_PROD_LOCAL_ENV_FILE:-internal/.env.prod-local}
DB_NAME=${FLOWPLANE_PROD_LOCAL_DB_NAME:-flowplane_prod_local}
API_ADDR=${FLOWPLANE_PROD_LOCAL_API_ADDR:-127.0.0.1:8080}
XDS_ADDR=${FLOWPLANE_PROD_LOCAL_XDS_ADDR:-127.0.0.1:18000}
SKIP_BUILD=0
SKIP_MIGRATE=0
FORCE=0

usage() {
  cat <<'EOF'
Usage: scripts/dev/setup-prod-local.sh [options]

Creates internal/.env.prod-local for local production-mode Auth0 testing.

Options:
  --force          Overwrite an existing env file.
  --skip-build     Do not run cargo build --release --bin flowplane.
  --skip-migrate   Do not run flowplane db migrate.
  -h, --help       Show this help.

Optional environment inputs:
  AUTH0_DOMAIN              e.g. my-tenant.us.auth0.com
  AUTH0_CLIENT_ID           Auth0 Native Application client id
  AUTH0_ADMIN_SUBJECT       Auth0 user_id / OIDC sub, e.g. auth0|...
  AUTH0_ADMIN_EMAIL         First admin email
  FLOWPLANE_DATABASE_URL    Defaults to postgres://$USER@127.0.0.1:5432/flowplane_prod_local
  FLOWPLANE_PROD_LOCAL_DB_NAME
  FLOWPLANE_PROD_LOCAL_API_ADDR
  FLOWPLANE_PROD_LOCAL_XDS_ADDR
  FLOWPLANE_PROD_LOCAL_ENV_FILE
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --force) FORCE=1 ;;
    --skip-build) SKIP_BUILD=1 ;;
    --skip-migrate) SKIP_MIGRATE=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage; exit 2 ;;
  esac
  shift
done

prompt() {
  local name=$1
  local label=$2
  local default=${3:-}
  local value=${!name:-}
  if [ -n "$value" ]; then
    printf '%s' "$value"
    return
  fi
  if [ -n "$default" ]; then
    read -r -p "$label [$default]: " value
    printf '%s' "${value:-$default}"
  else
    while [ -z "$value" ]; do
      read -r -p "$label: " value
    done
    printf '%s' "$value"
  fi
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1" >&2
    exit 1
  }
}

need_cmd openssl
need_cmd psql
need_cmd createdb
need_cmd pg_isready

if [ "$SKIP_BUILD" = 0 ]; then
  need_cmd cargo
fi

if [ -f "$ENV_FILE" ] && [ "$FORCE" != 1 ]; then
  echo "$ENV_FILE already exists; pass --force to overwrite" >&2
  exit 1
fi

AUTH0_DOMAIN=$(prompt AUTH0_DOMAIN "Auth0 domain, e.g. tenant.us.auth0.com")
AUTH0_DOMAIN=${AUTH0_DOMAIN#https://}
AUTH0_DOMAIN=${AUTH0_DOMAIN#http://}
AUTH0_DOMAIN=${AUTH0_DOMAIN%/}
AUTH0_CLIENT_ID=$(prompt AUTH0_CLIENT_ID "Auth0 Native App Client ID")
AUTH0_ADMIN_SUBJECT=$(prompt AUTH0_ADMIN_SUBJECT "Auth0 admin user_id / OIDC sub, e.g. auth0|...")
AUTH0_ADMIN_EMAIL=$(prompt AUTH0_ADMIN_EMAIL "Admin email" "admin@example.com")

DEFAULT_DB_URL="postgres://${USER:-$(whoami)}@127.0.0.1:5432/${DB_NAME}"
DATABASE_URL=${FLOWPLANE_DATABASE_URL:-$(prompt FLOWPLANE_DATABASE_URL "Postgres URL" "$DEFAULT_DB_URL")}
SECRET_KEY=$(openssl rand -base64 32)

mkdir -p "$(dirname "$ENV_FILE")"
umask 077
{
  printf 'export FLOWPLANE_DATABASE_URL=%q\n' "$DATABASE_URL"
  printf 'export FLOWPLANE_API_ADDR=%q\n' "$API_ADDR"
  printf 'export FLOWPLANE_XDS_ADDR=%q\n' "$XDS_ADDR"
  printf 'export FLOWPLANE_API_INSECURE=%q\n' "true"
  printf 'export FLOWPLANE_LOG_FORMAT=%q\n' "pretty"
  printf 'export FLOWPLANE_LOG=%q\n' "info"
  printf 'export FLOWPLANE_SECRET_ENCRYPTION_KEY=%q\n' "$SECRET_KEY"
  printf '\n'
  printf 'export FLOWPLANE_OIDC_ISSUER=%q\n' "https://${AUTH0_DOMAIN}/"
  printf 'export FLOWPLANE_OIDC_AUDIENCE=%q\n' "$AUTH0_CLIENT_ID"
  printf '\n'
  printf 'export FLOWPLANE_SERVER=%q\n' "http://${API_ADDR}"
  printf 'export FLOWPLANE_OIDC_CLIENT_ID=%q\n' "$AUTH0_CLIENT_ID"
  printf 'export FLOWPLANE_OIDC_SCOPE=%q\n' "openid email profile"
  printf '\n'
  printf 'export FLOWPLANE_BOOTSTRAP_ADMIN_SUBJECT=%q\n' "$AUTH0_ADMIN_SUBJECT"
  printf 'export FLOWPLANE_BOOTSTRAP_ADMIN_EMAIL=%q\n' "$AUTH0_ADMIN_EMAIL"
} > "$ENV_FILE"

echo "wrote $ENV_FILE"

if ! pg_isready -q -h 127.0.0.1 -p 5432 >/dev/null 2>&1; then
  if command -v brew >/dev/null 2>&1; then
    echo "Postgres is not ready on 127.0.0.1:5432; trying Homebrew postgresql@16"
    brew services start postgresql@16 >/dev/null 2>&1 || true
    for _ in $(seq 1 15); do
      pg_isready -q -h 127.0.0.1 -p 5432 >/dev/null 2>&1 && break
      sleep 1
    done
  fi
fi

if psql "$DATABASE_URL" -tc 'select 1' >/dev/null 2>&1; then
  echo "database reachable"
else
  if [ "$DATABASE_URL" = "$DEFAULT_DB_URL" ]; then
    echo "creating database $DB_NAME"
    createdb "$DB_NAME" 2>/dev/null || true
    psql "$DATABASE_URL" -tc 'select 1' >/dev/null
  else
    echo "database is not reachable: $DATABASE_URL" >&2
    echo "create it or set FLOWPLANE_DATABASE_URL to a reachable database, then rerun with --force" >&2
    exit 1
  fi
fi

if [ "$SKIP_BUILD" = 0 ]; then
  cargo build --release --bin flowplane
fi

if [ "$SKIP_MIGRATE" = 0 ]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
  ./target/release/flowplane db migrate
fi

cat <<EOF

Prod-local setup is ready.

Next terminal 1:
  set -a; source $ENV_FILE; set +a
  ./target/release/flowplane serve

Copy the logged bootstrap_token=fpboot_... value.

Next terminal 2:
  set -a; source $ENV_FILE; set +a
  curl -fsS -X POST http://$API_ADDR/api/v1/bootstrap/initialize \\
    -H "Authorization: Bearer fpboot_xxxxxxxx" \\
    -H "Content-Type: application/json" \\
    -d '{"org_name":"platform","org_display_name":"Platform","admin_subject":"'"$AUTH0_ADMIN_SUBJECT"'","admin_email":"'"$AUTH0_ADMIN_EMAIL"'"}'

Then login:
  ./target/release/flowplane auth login --device-code \\
    --issuer "\$FLOWPLANE_OIDC_ISSUER" \\
    --client-id "\$FLOWPLANE_OIDC_CLIENT_ID" \\
    --scope "\$FLOWPLANE_OIDC_SCOPE"
  ./target/release/flowplane auth whoami
EOF
