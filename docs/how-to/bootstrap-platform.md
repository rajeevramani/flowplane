# How-to: bootstrap the first platform admin

> Audience: operators · Status: stable

A fresh, non-dev Flowplane control plane starts **uninitialized**: it has no platform organization and no admin. You initialize it once, with a one-shot **bootstrap token** that you supply — the control plane never generates or logs it.

Before starting, configure your OIDC provider and find the first admin's immutable subject. See [configure an OIDC provider](configure-oidc-provider.md).

## 1. Choose a bootstrap token

Pick a high-entropy secret, **at least 32 characters**. For example:

```bash
openssl rand -hex 32        # 64 hex chars — fine
```

Keep it where only operators can read it. You will hand it to the control plane and then use it once against the API.

## 2. Supply the token to the control plane

Provide it by **either** of these, before `flowplane serve` starts. The control plane stores only its SHA-256 hash and never writes the token to logs.

- A file (preferred — safer than env, which is visible via process inspection):

  On Linux, `/run` is normally volatile, but it is not necessarily mounted as `tmpfs`. Set
  `FLOWPLANE_OS_USER` and `FLOWPLANE_OS_GROUP` for the configured control-plane service account
  before running this block. Creating the directory and assigning its ownership normally requires
  root; use `sudo` as shown, or have `systemd-tmpfiles` or equivalent deployment setup pre-create
  the same paths with these owners and modes. The Flowplane service reads the token file as its
  configured OS user.

  ```bash
  (
    set -eu
    : "${FLOWPLANE_OS_USER:?set this to the service OS user}"
    : "${FLOWPLANE_OS_GROUP:?set this to the service OS group}"
    : "${BOOTSTRAP_TOKEN:?set this to the chosen bootstrap token}"
    sudo install -d -m 0700 -o "$FLOWPLANE_OS_USER" -g "$FLOWPLANE_OS_GROUP" /run/flowplane
    sudo install -m 0600 -o "$FLOWPLANE_OS_USER" -g "$FLOWPLANE_OS_GROUP" \
      /dev/null /run/flowplane/bootstrap-token
    printf '%s' "$BOOTSTRAP_TOKEN" \
      | sudo -u "$FLOWPLANE_OS_USER" tee /run/flowplane/bootstrap-token >/dev/null
  )
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

Call the public bootstrap endpoint once, passing the token as a bearer credential. `admin_subject` is the OIDC `sub` of your first admin (the identity your IdP will assert). Use the exact `sub` string from your IdP; do not use email, username, or display name:

If you do not yet know the subject, copy the stable user identifier from your IdP's user profile, decode the intended admin's own ID token locally and read `sub`, or call your IdP's userinfo endpoint with that user's own access token. Do not paste production tokens into third-party websites.

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
- `flowplane auth whoami` shows the bootstrapped user as a platform admin after OIDC login.

## Recover an unavailable sole platform administrator

Use this offline path only when the initialized database has exactly one platform owner and that identity is unavailable. It is not a routine promotion command. PostgreSQL access, control-plane deployment access, and a verified backup are the break-glass authority.

1. Have the replacement identity complete one normal OIDC login while the control plane is running. This provisions an active Flowplane user through the normal identity path; it does not grant authority.
2. Obtain the replacement immutable OIDC subject from trusted issuer records. Write it to an owner-only regular file without placing it in argv, environment variables, shell history, logs, or tickets:

   ```bash
   umask 077
   # Populate this file from a trusted IdP record without printing the subject.
   $EDITOR replacement-subject
   chmod 0600 replacement-subject
   ```

3. Run a read-only plan while the control plane is live. Name a tenant only when its source-owned `owner` membership must move with platform ownership:

   ```bash
   umask 077
   flowplane db recover-platform-admin plan \
     --subject-file replacement-subject \
     --transfer-owned-org tenant-a > recovery-plan.json
   chmod 0600 recovery-plan.json
   ```

   Review every UUID, transfer count, status, and the `sha256:` digest. A selected tenant with source user grants is refused because recovery never moves or deletes grants. Recover platform governance first and use normal authenticated membership/grant administration for any separate tenant remediation.

4. Stop **every** control-plane replica and verify none can access the database. The command cannot prove this deployment-owned precondition. Take a PostgreSQL backup or snapshot and verify it can be restored into an isolated database.
5. Apply exactly the reviewed plan:

   ```bash
   PLAN_DIGEST="$(jq -er '.digest' recovery-plan.json)"
   flowplane db recover-platform-admin apply \
     --subject-file replacement-subject \
     --transfer-owned-org tenant-a \
     --expected-plan "$PLAN_DIGEST" \
     --yes
   ```

   Missing confirmation, malformed or stale digests, changed identity/membership state, audit failure, and concurrent bootstrap/recovery all fail without committing membership or audit changes. Output and audit evidence contain IDs/counts/digests, never subjects, emails, credentials, database URLs, or private paths.

6. Restart one control-plane replica. Authenticate as the replacement, run `flowplane auth whoami`, and verify platform governance. Verify tenant access separately: platform administration alone grants no tenant authority, and only explicitly selected tenant-owner memberships move. Then restore the normal replica count.

### Roll back during the bounded rollback window

Rollback is a fresh reverse recovery, not an inverse digest. It is available only while the prior Flowplane user remains active.

1. Obtain the prior immutable subject from trusted bootstrap/IdP records or an owner-only read-only database query. The recovery tool never emits it. Store it with the same `0600` rules.
2. Stop every control-plane replica and retain the verified backup.
3. Run a fresh `plan` with the prior subject and the same explicit tenant list, review the new digest, then run `apply --yes` with that digest.
4. Restart and verify the prior identity and tenant boundaries through normal authentication.

After the rollback window, review the old user's residual memberships, team memberships, and grants. Permanently retire the old identity at the IdP and enforce non-reassignment of that exact immutable subject. Flowplane cannot verify this issuer-owned control and version `3.1.3` has no supported user suspension/reactivation lifecycle; that separate work is tracked independently.

## Next step

Bootstrap creates only the **platform org** (governance only — it cannot host tenant teams or dataplanes). To stand up actual gateway config, create a **tenant org and a team**: [create a tenant org and a team](create-tenant-org-and-team.md).

## Troubleshooting

- **Server won't start, "no bootstrap token was supplied":** the instance is uninitialized and you set neither `FLOWPLANE_BOOTSTRAP_TOKEN` nor `FLOWPLANE_BOOTSTRAP_TOKEN_FILE`. Set one and restart.
- **Server won't start, "a different bootstrap token is already active":** another replica was given a different token. Use one identical token across all replicas.
- **"token is too short":** the token must be ≥ 32 characters after trimming whitespace.
- **`401` on initialize:** the token is wrong, expired (24 h), or already used. Restart an uninitialized instance with a fresh token, or confirm you are sending the exact value you seeded.
- **OIDC login works but the user is not platform admin:** if the instance was just initialized, the `admin_subject` used during bootstrap may not match the OIDC `sub` claim. Check the subject discovery steps in [configure an OIDC provider](configure-oidc-provider.md). If the initialized instance's sole owner is genuinely unavailable, use the guarded offline recovery procedure above; do not rerun bootstrap or mutate membership rows manually.
