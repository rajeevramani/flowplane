# Prod-Mode Local Runbook

Run the Flowplane control plane on your machine in **production mode**: no dev issuer, no seeded dev org/team/user, real OIDC validation, one-shot bootstrap, and production authz behavior.

This runbook includes the Auth0 setup path because Auth0 is our current local prod-mode IdP test case. The Flowplane-side mechanics are provider-neutral OIDC.

## What "prod locally" means

- `FLOWPLANE_DEV_MODE` is unset/false.
- The control plane validates real OIDC JWTs from `FLOWPLANE_OIDC_ISSUER` + `FLOWPLANE_OIDC_AUDIENCE`.
- The first platform admin is created through `/api/v1/bootstrap/initialize` with the boot-logged `fpboot_...` token.
- API plaintext requires explicit local opt-in with `FLOWPLANE_API_INSECURE=true`, or real local TLS certs via `FLOWPLANE_API_TLS_CERT` + `FLOWPLANE_API_TLS_KEY`.
- xDS is production-only mTLS. If `FLOWPLANE_XDS_TLS_*` is absent, the API still runs, but the xDS listener is disabled.

## Prerequisites

- Built binary:

```bash
cargo build --release --bin flowplane
```

- PostgreSQL 15+:

```bash
brew install postgresql@16
brew services start postgresql@16
createdb flowplane_prod_local
```

Homebrew Postgres usually accepts your macOS username without a password:

```text
postgres://<your-mac-user>@127.0.0.1:5432/flowplane_prod_local
```

- An Auth0 tenant. A free tenant is enough.

## Fast Path - Scripted setup

Run:

```bash
scripts/dev/setup-prod-local.sh
```

The script prompts for:

- Auth0 domain
- Auth0 Native App Client ID
- Auth0 admin `user_id` / OIDC `sub`
- admin email
- Postgres URL, defaulting to `postgres://$USER@127.0.0.1:5432/flowplane_prod_local`

It creates `internal/.env.prod-local`, generates `FLOWPLANE_SECRET_ENCRYPTION_KEY`, creates the local database when using the default URL, builds `target/release/flowplane`, runs migrations, and prints the exact `serve`, bootstrap, and login commands.

Useful flags:

```bash
scripts/dev/setup-prod-local.sh --force
scripts/dev/setup-prod-local.sh --skip-build
scripts/dev/setup-prod-local.sh --skip-migrate
```

You can also pre-seed inputs non-interactively:

```bash
AUTH0_DOMAIN="<tenant>.us.auth0.com" \
AUTH0_CLIENT_ID="<auth0-native-app-client-id>" \
AUTH0_ADMIN_SUBJECT="auth0|6650f0..." \
AUTH0_ADMIN_EMAIL="you@example.com" \
scripts/dev/setup-prod-local.sh --force
```

## Step A - Set up Auth0

Create a local test application in Auth0:

1. Go to **Applications -> Create Application**.
2. Choose **Native**. Device Code grant requires a native app type.
3. In the app **Settings**, copy:
   - **Domain**, for example `<tenant>.us.auth0.com`
   - **Client ID**
4. Go to **Settings -> Advanced -> Grant Types** and enable:
   - **Device Code**
   - **Authorization Code**, only if you want to test `--pkce`
5. For `--pkce` only, add this allowed callback URL:

```text
http://127.0.0.1:8976/callback
```

6. Go to **Authentication -> Database -> Users -> Create User**.
7. Open the user and copy the Auth0 **user_id**. This is the OIDC `sub`, for example:

```text
auth0|6650f0...
```

You now have:

```text
Issuer:        https://<tenant>.us.auth0.com/
Client ID:     <auth0-native-app-client-id>
Admin subject: auth0|<admin-user-id>
```

Auth0-specific gotchas:

- The issuer must include Auth0's trailing slash because Auth0 tokens use it in `iss`.
- `FLOWPLANE_OIDC_AUDIENCE` should be the Auth0 application **Client ID**. The CLI stores/sends the ID token, whose `aud` is the client ID.
- Do not create/use an Auth0 API audience for this local CLI path. That produces access-token shapes the CLI is not relying on here.
- The login scope must include `openid`; `openid email profile` is fine.

Important: Flowplane authorization does **not** come from IdP groups or role claims. The IdP only proves identity. Flowplane DB memberships and grants decide access.

## Step B - Create local prod env

Create a local env file:

```bash
touch internal/.env.prod-local
```

Fill it with:

```bash
export FLOWPLANE_DATABASE_URL="postgres://<your-mac-user>@127.0.0.1:5432/flowplane_prod_local"
export FLOWPLANE_API_ADDR="127.0.0.1:8080"
export FLOWPLANE_XDS_ADDR="127.0.0.1:18000"
export FLOWPLANE_API_INSECURE="true"
export FLOWPLANE_LOG_FORMAT="pretty"
export FLOWPLANE_LOG="info"
export FLOWPLANE_SECRET_ENCRYPTION_KEY="<openssl-rand-base64-32>"

export FLOWPLANE_OIDC_ISSUER="https://<tenant>.us.auth0.com/"
export FLOWPLANE_OIDC_AUDIENCE="<auth0-native-app-client-id>"

# CLI-side settings. Note the client var is OIDC_CLIENT_ID, not OIDC_AUDIENCE.
export FLOWPLANE_SERVER="http://127.0.0.1:8080"
export FLOWPLANE_OIDC_CLIENT_ID="<auth0-native-app-client-id>"
export FLOWPLANE_OIDC_SCOPE="openid email profile"
```

Use `FLOWPLANE_OIDC_JWKS_URI` only if your IdP discovery document is not enough:

```bash
export FLOWPLANE_OIDC_JWKS_URI="https://issuer.example/.well-known/jwks.json"
```

Generate the local secret key with:

```bash
openssl rand -base64 32
```

For non-Auth0 OIDC providers, use the issuer exactly as the token's `iss` claim presents it, and set `FLOWPLANE_OIDC_AUDIENCE` to the `aud` value in the token the CLI sends.

## Step C - Migrate and boot the control plane

```bash
set -a; source internal/.env.prod-local; set +a

./target/release/flowplane db migrate
./target/release/flowplane serve
```

Expected startup signals:

- `OIDC authentication enabled`
- `API listener is serving PLAINTEXT...` if using `FLOWPLANE_API_INSECURE=true`
- `xDS listener disabled...` unless you configured xDS mTLS
- on a fresh DB, one bootstrapping line:

```text
instance is uninitialized - POST /api/v1/bootstrap/initialize with this one-shot token
bootstrap_token=fpboot_xxxxxxxx
```

Copy the `fpboot_...` token. Leave the server running.

## Step D - Bootstrap the first platform admin

In a second terminal:

```bash
set -a; source internal/.env.prod-local; set +a

curl -fsS http://127.0.0.1:8080/healthz && echo
curl -fsS http://127.0.0.1:8080/api/v1/bootstrap/status && echo
```

Initialize the platform org and first admin:

```bash
curl -fsS -X POST http://127.0.0.1:8080/api/v1/bootstrap/initialize \
  -H "Authorization: Bearer fpboot_xxxxxxxx" \
  -H "Content-Type: application/json" \
  -d '{
        "org_name": "platform",
        "org_display_name": "Platform",
        "admin_subject": "auth0|6650f0...",
        "admin_email": "you@example.com"
      }'
```

`admin_subject` must match the Auth0 user's `user_id` / OIDC `sub` claim. If it does not, login can succeed but governance actions will be denied.

## Step E - Login and verify

Device-code flow:

```bash
./target/release/flowplane auth login --device-code \
  --issuer "$FLOWPLANE_OIDC_ISSUER" \
  --client-id "$FLOWPLANE_OIDC_CLIENT_ID" \
  --scope "$FLOWPLANE_OIDC_SCOPE"
```

Then verify:

```bash
./target/release/flowplane auth whoami
./target/release/flowplane org list
```

Pass signals:

- Login completes and stores credentials.
- `auth whoami` returns your user and `platform_admin: true`.
- `org list` succeeds.

## Optional - API TLS instead of local plaintext

If you want to test the HTTPS API listener locally:

```bash
mkdir -p .local/prod/tls
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout .local/prod/tls/api.key \
  -out .local/prod/tls/api.crt \
  -days 7 \
  -subj "/CN=localhost" \
  -addext "subjectAltName=DNS:localhost,IP:127.0.0.1"
```

Change the env:

```bash
unset FLOWPLANE_API_INSECURE
export FLOWPLANE_API_TLS_CERT="$PWD/.local/prod/tls/api.crt"
export FLOWPLANE_API_TLS_KEY="$PWD/.local/prod/tls/api.key"
export FLOWPLANE_SERVER="https://127.0.0.1:8080"
```

For ad hoc `curl`, pass `--cacert .local/prod/tls/api.crt` or `-k`. For CLI testing against a self-signed local API cert, use plaintext unless the CLI HTTP stack is configured to trust the cert.

## Optional - Enable local xDS mTLS

Without these variables, prod mode intentionally disables xDS:

```bash
export FLOWPLANE_XDS_TLS_CERT="$PWD/.local/prod/tls/xds.crt"
export FLOWPLANE_XDS_TLS_KEY="$PWD/.local/prod/tls/xds.key"
export FLOWPLANE_XDS_TLS_CLIENT_CA="$PWD/.local/prod/tls/dp-ca.crt"
```

The xDS server cert/key identify the control plane. The client CA validates dataplane client certs. Dataplane certs must also be registered in Flowplane through the dataplane/proxy-certificate path; mTLS chain validity alone is not enough to authorize a dataplane.

Use this only when you are testing Envoy or `fp-agent`. For API/auth/governance validation, keeping xDS disabled is expected and simpler.

## Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| server refuses to start with API TLS error | no API TLS and no plaintext opt-in | set `FLOWPLANE_API_INSECURE=true` for local, or set `FLOWPLANE_API_TLS_CERT` + `FLOWPLANE_API_TLS_KEY` |
| authenticated endpoints return 503 | no OIDC validator configured | set both `FLOWPLANE_OIDC_ISSUER` and `FLOWPLANE_OIDC_AUDIENCE` |
| `invalid issuer` | exact `iss` mismatch | copy the issuer from the token/discovery document, including trailing slash if present |
| `invalid audience` | token `aud` does not match server audience | set `FLOWPLANE_OIDC_AUDIENCE` to the audience in the token the CLI sends |
| login succeeds but `org list` is denied | bootstrap admin subject does not match token `sub` | reinitialize a fresh DB or add the correct user through an existing platform admin |
| tenant commands fail with `org_selector_required` | user belongs to multiple non-platform orgs | pass `--org <org>` or configure an active CLI context |
| xDS does not listen in prod mode | xDS mTLS vars are absent | set all three `FLOWPLANE_XDS_TLS_CERT`, `FLOWPLANE_XDS_TLS_KEY`, `FLOWPLANE_XDS_TLS_CLIENT_CA` |

## Clean reset

For a clean local prod-mode rerun:

```bash
dropdb flowplane_prod_local
createdb flowplane_prod_local
```

Then repeat migration, `serve`, and bootstrap. The bootstrap token is single-use and only useful while the instance is uninitialized.
