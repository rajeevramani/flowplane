# How-to: bootstrap the first platform admin

> Audience: operators · Status: stable

A fresh, non-dev Flowplane control plane starts **uninitialized**: it has no platform organization and no admin. You initialize it once, with a one-shot **bootstrap token** that you supply — the control plane never generates or logs it.

This page is self-contained: every value you need is here.

## 1. Choose a bootstrap token

Pick a high-entropy secret, **at least 32 characters**. For example:

```bash
openssl rand -hex 32        # 64 hex chars — fine
```

Keep it where only operators can read it. You will hand it to the control plane and then use it once against the API.

## 2. Supply the token to the control plane

Provide it by **either** of these, before `flowplane serve` starts. The control plane stores only its SHA-256 hash and never writes the token to logs.

- A file (preferred — safer than env, which is visible via process inspection):

  ```bash
  printf '%s' "$BOOTSTRAP_TOKEN" > /run/flowplane/bootstrap-token   # tmpfs, mode 0600
  export FLOWPLANE_BOOTSTRAP_TOKEN_FILE=/run/flowplane/bootstrap-token
  ```

- Or directly in the environment:

  ```bash
  export FLOWPLANE_BOOTSTRAP_TOKEN="<your-32+-char-token>"
  ```

`FLOWPLANE_BOOTSTRAP_TOKEN_FILE` takes precedence if both are set. Supply the **same** token to every replica; a different live token on another replica makes startup fail closed.

Then start the server normally (`flowplane serve`). On an uninitialized instance it seeds the token's hash and logs a confirmation **without** the value. If the instance is already initialized, the token is ignored.

> **Fail-closed:** an uninitialized, non-dev instance started with **no** token refuses to start.
> This is deliberate — it prevents a misconfigured production instance from silently generating and
> logging a token. (For local experimentation only, `FLOWPLANE_ALLOW_LOGGED_BOOTSTRAP_TOKEN=yes-this-is-local-only`
> restores the old generate-and-log behavior; never use it in production.)

## 3. Initialize the platform

Call the public bootstrap endpoint once, passing the token as a bearer credential. `admin_subject` is the OIDC `sub` of your first admin (the identity your IdP will assert):

```bash
curl -fsS -X POST https://<control-plane>/api/v1/bootstrap/initialize \
  -H "Authorization: Bearer $BOOTSTRAP_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
        "org_name": "platform",
        "org_display_name": "Platform",
        "admin_subject": "<oidc-sub-of-first-admin>",
        "admin_email": "admin@example.com"
      }'
```

A success response returns the new `org_id` and `admin_user_id`. The token is now consumed (single-use, 24-hour expiry); a replay returns `401`/`409`.

## 4. Verify

- Re-running the same `POST /api/v1/bootstrap/initialize` returns a conflict — the instance is initialized.
- Your admin can now authenticate through your OIDC issuer and reach authenticated endpoints.

## Next step

Bootstrap creates only the **platform org** (governance only — it cannot host tenant teams or dataplanes). To stand up actual gateway config, create a **tenant org and a team**: [create a tenant org and a team](create-tenant-org-and-team.md).

## Troubleshooting

- **Server won't start, "no bootstrap token was supplied":** the instance is uninitialized and you set neither `FLOWPLANE_BOOTSTRAP_TOKEN` nor `FLOWPLANE_BOOTSTRAP_TOKEN_FILE`. Set one and restart.
- **Server won't start, "a different bootstrap token is already active":** another replica was given a different token. Use one identical token across all replicas.
- **"token is too short":** the token must be ≥ 32 characters after trimming whitespace.
- **`401` on initialize:** the token is wrong, expired (24 h), or already used. Restart an uninitialized instance with a fresh token, or confirm you are sending the exact value you seeded.
