# Token Management CLI

Flowplane ships a built-in CLI for administering personal access tokens. The commands connect
straight to the configured database and use the same hashing, audit logging, and scope rules as
the HTTP API.

## Prerequisites

- A working Flowplane configuration file (`config.yml` by default)
- Valid database credentials (override with `--database-url` if needed)
- Flowplane migrations have been applied (`flowplane database migrate`)

## Creating a Token

```bash
# Issue a bootstrap-style admin token (expires in 90 days)
flowplane auth create-token \
  --name "Bootstrap Admin" \
  --scope tokens:read --scope tokens:write \
  --scope clusters:read --scope clusters:write \
  --scope routes:read --scope routes:write \
  --scope listeners:read --scope listeners:write \
  --expires-in 90d

# Sample output
# Token created successfully!
#   ID: 550e8400-e29b-41d4-a716-446655440000
#   Name: Bootstrap Admin
#   Token: fp_pat_550e8400-e29b-41d4-a716-446655440000.4cTPzK... (save immediately)
#   Expires: 2025-12-25T12:00:00Z
#   Scopes: tokens:read, tokens:write, clusters:read, clusters:write, routes:read, routes:write, listeners:read, listeners:write
```

You can also supply an absolute expiration time:

```bash
flowplane auth create-token \
  --name "CI/CD Pipeline" \
  --scope clusters:read --scope routes:write --scope listeners:read \
  --expires-at 2025-06-30T23:59:59Z \
  --description "Token used by the CI deployment job" \
  --created-by "ci-bot"
```

## Listing Tokens

```bash
# List tokens (defaults to 50 records)
flowplane auth list-tokens

# Control pagination
flowplane auth list-tokens --limit 10 --offset 10
```

The output includes status, timestamps, and granted scopes.

## Rotating a Token Secret

```bash
flowplane auth rotate-token 550e8400-e29b-41d4-a716-446655440001
# Token rotated successfully
#   ID: 550e8400-e29b-41d4-a716-446655440001
#   Name: CI/CD Pipeline
#   New Token: fp_pat_...
#   Scopes: clusters:read, routes:write, listeners:read
```

The previous token value stops working immediately. Update dependent systems with the new value.

## Revoking a Token

```bash
flowplane auth revoke-token 550e8400-e29b-41d4-a716-446655440001
# Token 'CI/CD Pipeline' (550e8400-e29b-41d4-a716-446655440001) revoked
```

Revoked tokens remain visible with `Status = revoked` so you can audit their history.

## Bootstrap Token Reminder

If this is the first token in a new installation, store the generated secret in a secure vault.
The CLI prints the value once and audit events (`auth.token.created`, `auth.token.rotated`, `auth.token.revoked`)
are written immediately so you can verify provisioning.
