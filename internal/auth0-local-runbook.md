# Auth0 Local Test Runbook

Validate the Flowplane control-plane OIDC integration against **Auth0** as the IdP, entirely on
your machine. This is decoupled from the AWS deploy on purpose — it isolates the auth variable from
the infra/xDS variable. No code changes are needed; OIDC is already implemented
(`crates/fp-core/src/oidc.rs`, `crates/fp-api/src/auth.rs`).

## How it works (read this first)

- The **CLI sends the OIDC ID token**, not an access token (`crates/flowplane/src/cli/mod.rs`
  returns `id_token.or(access_token)`). So the token's `aud` is the **Auth0 Application Client ID**.
  → set `FLOWPLANE_OIDC_AUDIENCE` to the **Client ID**. **Do not** create an Auth0 API/audience
  object; that path issues tokens the CLI never sends.
- The CP validates `iss` + `aud` strictly and fetches JWKS via OIDC discovery
  (`{issuer}/.well-known/openid-configuration`).
- **Authorization (org/team/role) is flowplane-internal**, not from Auth0 claims. Auth0 only
  supplies identity (`sub`, `email`, `name`); the user is JIT-provisioned on first login with **no
  grants**.
- **The first platform admin is bootstrapped, not from Auth0.** On first boot of an uninitialized
  DB the server logs a one-time `fpboot_…` token; you `POST /api/v1/bootstrap/initialize` with it,
  passing `admin_subject` = the Auth0 user's `sub`. That ties the bootstrap admin to the Auth0
  identity, so when that user logs in via Auth0 they are recognized as the platform admin.

## Three gotchas (each will fail the login if wrong)

1. **Trailing slash on the issuer.** Auth0's `iss` claim is `https://<tenant>.auth0.com/`. The CP
   does an exact-match, so `FLOWPLANE_OIDC_ISSUER` must end with `/`.
2. **`FLOWPLANE_OIDC_AUDIENCE` = Client ID** (ID-token audience), not an API identifier.
3. **scope must include `openid`** (the CLI default `"openid email profile"` is fine) or Auth0
   issues no ID token.

## Prerequisites

- A built `flowplane` binary: `cargo build --release --bin flowplane` (binary at
  `target/release/flowplane`). The examples below use that path; `cargo run --release --bin
  flowplane -- …` works too.
- An Auth0 tenant (free is fine).

## Step 0 — local Postgres (macOS)

**Postgres 15+ is required** — migration `0019_spec_version_lifecycle.sql` uses the per-column
`ON DELETE SET NULL (col)` referential action, which is a PostgreSQL 15 feature. PG14 fails with
`syntax error at or near "("` on migration 19.

Use Homebrew Postgres directly — simplest, and avoids the local Podman/Docker machine (which has
been flaky here).

```bash
brew install postgresql@16
brew services stop postgresql@14 2>/dev/null   # if an older one is running on 5432
brew services start postgresql@16
createdb flowplane
```

Homebrew Postgres uses **your macOS username** as superuser with **no password** (trust auth on
localhost), so the connection URL is:

```
postgres://<your-mac-user>@127.0.0.1:5432/flowplane
```

Put that in `FLOWPLANE_DATABASE_URL` in `internal/.env.auth0`.

(The repo's `scripts/ensure-postgres.sh` is Linux-container only — not for macOS. A
`docker/podman run postgres:16` container also works if your machine is healthy, but it's not
needed when Homebrew Postgres is already installed.)

## Step A — Auth0 console (one-time)

1. **Applications → Create Application → Native.** (Device Code grant requires a Native app type.)
2. In the app **Settings**, note the **Domain** (`<tenant>.us.auth0.com`) and **Client ID**.
3. **Settings → Advanced → Grant Types**: enable **Device Code** (and **Authorization Code** if you
   want to try `--pkce`).
4. For `--pkce` only: add **Allowed Callback URL** `http://127.0.0.1:8976/callback`.
5. **Authentication → Database → Users → Create User** (a test user). Open the user and copy its
   **`user_id`** — this is the OIDC `sub` (e.g. `auth0|6650f0…`). You need it in Step C.

## Step B — boot the control plane

### B1. Fill the env file

```bash
cp internal/.env.auth0.example internal/.env.auth0
```

Edit `internal/.env.auth0` and set:
- `FLOWPLANE_DATABASE_URL` — from Step 0
- `FLOWPLANE_SECRET_ENCRYPTION_KEY` — `openssl rand -base64 32`
- `FLOWPLANE_OIDC_ISSUER` — `https://<tenant>.us.auth0.com/` (trailing slash!)
- `FLOWPLANE_OIDC_AUDIENCE` — your Auth0 **Client ID**

`FLOWPLANE_API_ADDR=127.0.0.1:8080` and `FLOWPLANE_API_INSECURE=true` are already set for local
plaintext (without the insecure opt-in, `serve` refuses to boot — D-008).

### B2. Load it and run migrations

```bash
set -a; source internal/.env.auth0; set +a

./target/release/flowplane db migrate
```

Expected: migrations apply cleanly and the command exits 0.

### B3. Start the server

```bash
./target/release/flowplane serve
```

What you should see in the log:
- a plaintext-API warning (expected — you opted into `FLOWPLANE_API_INSECURE`),
- the API listening on `127.0.0.1:8080` and xDS on `0.0.0.0:18000`,
- **the one-time bootstrap line** (only on a fresh, uninitialized DB):

```
instance is uninitialized — POST /api/v1/bootstrap/initialize with this token
bootstrap_token=fpboot_xxxxxxxx
```

Copy that `fpboot_…` value. Leave `serve` running here; do the next steps in a **second terminal**
(re-`source internal/.env.auth0` there so `FLOWPLANE_SERVER` etc. are set).

### B4. Health check (second terminal)

```bash
curl -fsS http://127.0.0.1:8080/healthz && echo
curl -fsS http://127.0.0.1:8080/api/v1/bootstrap/status   # should report uninitialized
```

## Step C — initialize the platform admin (ties it to your Auth0 user)

`admin_subject` **must** equal the Auth0 user's `sub` from Step A.5.

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
# -> {"org_id":"...","admin_user_id":"..."}
```

The bootstrap token is single-use and expires; if you mistype it, restart `flowplane serve` to get
a fresh one (only re-issued while the instance is still uninitialized).

## Step D — log in through Auth0 and verify

```bash
flowplane auth login --device \
  --issuer "$FLOWPLANE_OIDC_ISSUER" \
  --client-id "$FLOWPLANE_OIDC_AUDIENCE" \
  --scope "openid email profile"
# open the printed verification URL, approve as the test user

flowplane auth whoami
```

**Pass signals:**

- `auth login` completes (device approved) and stores the token.
- `whoami` returns your identity — the Auth0 ID token validated against the CP, and because
  `admin_subject` matched your `sub`, you are the platform admin.
- A governance call works, e.g. `flowplane org list` / `flowplane team create <name> --org platform`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `invalid issuer` | issuer mismatch | `FLOWPLANE_OIDC_ISSUER` must be exactly `https://<tenant>.auth0.com/` **with** trailing slash |
| `invalid audience` | aud mismatch | `FLOWPLANE_OIDC_AUDIENCE` must be the **Client ID** |
| token not a JWT / can't validate | you created an Auth0 API and got an opaque/access token | don't pass an `audience`; rely on the ID token (default) |
| `whoami` 401 but login worked | scope missing `openid` | re-login with `--scope "openid email profile"` |
| login OK but every action denied | JIT user has no grants, and `admin_subject` ≠ your `sub` | re-run bootstrap (fresh server) with the correct `admin_subject`, or grant the user via an existing admin |
| `device_authorization_endpoint` not advertised | Device Code grant not enabled | enable it on the Auth0 Native app (Step A.3) |

## Next

Once this passes locally, the AWS Fargate CP flips to the same OIDC config via one Terraform var —
the first AWS loop test stays dev-mode to isolate the xDS/mTLS path.
