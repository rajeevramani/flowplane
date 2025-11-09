# Testing Guide: Task 7 Security Features

This guide shows how to test the setup token security features implemented in Task 7.

## Quick Automated Test

```bash
./test-setup-token-security.sh
```

This script automatically tests all security features.

## Manual Testing Steps

### Prerequisites

Build the project:
```bash
cargo build --release
```

### Test 1: Setup Token Auto-Generation

1. **Start fresh server** (will auto-generate setup token):
```bash
rm -f test.db
FLOWPLANE_DATABASE_URL=sqlite://test.db ./target/release/flowplane
```

2. **Verify setup token in output**:
You should see a banner with the setup token:
```
╔══════════════════════════════════════════════════════════════════════════════╗
║                  🚀 FLOWPLANE CONTROL PLANE - FIRST TIME SETUP               ║
╠══════════════════════════════════════════════════════════════════════════════╣
║                                                                              ║
║  A setup token has been automatically generated for initial bootstrap.      ║
║                                                                              ║
║  Setup Token:                                                                ║
║  fp_setup_XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX.XXXXXXXXXXXXXXXXXXX          ║
╚══════════════════════════════════════════════════════════════════════════════╝
```

Copy the setup token for next steps.

### Test 2: Failed Attempt Tracking

**Make 3 failed attempts with wrong secret:**

```bash
TOKEN="fp_setup_XXXXXXXX.wrong_secret"

# Attempt 1
curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{"setupToken": "'$TOKEN'", "tokenName": "admin"}'

# Attempt 2
curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{"setupToken": "'$TOKEN'", "tokenName": "admin"}'

# Attempt 3
curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{"setupToken": "'$TOKEN'", "tokenName": "admin"}'
```

**Verify failed attempts in database:**
```bash
TOKEN_ID="XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX"  # Extract from token
sqlite3 test.db "SELECT failed_attempts, locked_until FROM personal_access_tokens WHERE id='$TOKEN_ID'"
```

Expected: `failed_attempts` should be 3

### Test 3: Auto-Lockout After 5 Failed Attempts

**Make 2 more failed attempts (total = 5):**

```bash
# Attempt 4
curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{"setupToken": "'$TOKEN'", "tokenName": "admin"}'

# Attempt 5 - This triggers lockout
curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{"setupToken": "'$TOKEN'", "tokenName": "admin"}'
```

**Verify lockout in database:**
```bash
sqlite3 test.db "SELECT failed_attempts, locked_until FROM personal_access_tokens WHERE id='$TOKEN_ID'"
```

Expected:
- `failed_attempts` = 5
- `locked_until` = timestamp 15 minutes in the future

### Test 4: Lockout Enforcement

**Try to use the correct setup token while locked:**

```bash
CORRECT_TOKEN="fp_setup_XXXXXXXX.CORRECT_SECRET"  # Use the actual token from startup

curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{"setupToken": "'$CORRECT_TOKEN'", "tokenName": "admin"}'
```

Expected response:
```json
{
  "error": "Setup token is temporarily locked due to too many failed attempts. Please try again later."
}
```

### Test 5: Successful Bootstrap and Auto-Revocation

**Wait for lockout to expire (15 minutes) OR start fresh:**

```bash
# Start fresh
rm -f test.db
FLOWPLANE_DATABASE_URL=sqlite://test.db ./target/release/flowplane
```

**Bootstrap with correct token:**

```bash
SETUP_TOKEN="fp_setup_..."  # Copy from startup banner

curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{"setupToken": "'$SETUP_TOKEN'", "tokenName": "my-admin-token"}'
```

Expected response:
```json
{
  "id": "...",
  "token": "fp_pat_...",
  "message": "Bootstrap initialization successful. Admin token created. Setup token has been revoked."
}
```

**Verify auto-revocation:**

```bash
sqlite3 test.db "SELECT status FROM personal_access_tokens WHERE is_setup_token = TRUE"
```

Expected: `revoked`

**Try to reuse the setup token:**

```bash
curl -X POST http://localhost:8080/api/v1/bootstrap/initialize \
  -H "Content-Type: application/json" \
  -d '{"setupToken": "'$SETUP_TOKEN'", "tokenName": "another-admin"}'
```

Expected: Rejection because token is revoked

### Test 6: Audit Logging

**Check audit log entries:**

```bash
sqlite3 test.db "SELECT action, resource_id, metadata, created_at FROM audit_log ORDER BY created_at DESC LIMIT 10"
```

Look for:
- `setup_token.auto_generated` - Token creation
- `bootstrap.initialize` - Successful bootstrap with `setup_token_revoked: true`
- Failed attempt records (if logged)

## Testing with CLI

You can also test using the CLI client:

```bash
# Bootstrap using CLI
./target/release/flowplane-cli auth bootstrap \
  --api-url http://localhost:8080 \
  --setup-token "fp_setup_..." \
  --token-name "my-admin"
```

## Expected Security Behavior

1. **Failed Attempts**: Every failed validation increments counter
2. **Lockout**: At 5 failed attempts, token locks for 15 minutes
3. **Lock Enforcement**: Locked tokens rejected even with correct secret
4. **Auto-Revocation**: Setup token revoked immediately after successful bootstrap
5. **Single Use**: Even if max_usage_count > 1, token is revoked after first successful use
6. **Audit Trail**: All security events logged with metadata

## Troubleshooting

**Token not found in startup banner:**
- Check logs: `cat flowplane.log`
- Ensure database is empty on first start

**Lockout not working:**
- Verify migration applied: `sqlite3 test.db ".schema personal_access_tokens"`
- Should see `failed_attempts` and `locked_until` columns

**Auto-revocation not working:**
- Check token status: `sqlite3 test.db "SELECT status FROM personal_access_tokens WHERE is_setup_token=TRUE"`
- Check audit log for revocation event
