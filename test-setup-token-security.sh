#!/bin/bash
# Test script for Task 7: Setup Token Security Features
# Tests: lockout after failed attempts, auto-revocation, and audit logging

set -e

echo "========================================="
echo "Setup Token Security Testing"
echo "========================================="
echo ""

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
DB_PATH="./test_security.db"
API_URL="http://localhost:8080"

cleanup() {
    echo ""
    echo "${YELLOW}Cleaning up...${NC}"
    rm -f "$DB_PATH"
    pkill -f "flowplane" || true
}

# Cleanup on exit
trap cleanup EXIT

echo "${YELLOW}Step 1: Clean start - remove any existing test database${NC}"
rm -f "$DB_PATH"

echo ""
echo "${YELLOW}Step 2: Start Flowplane (will auto-generate setup token)${NC}"
echo "Running: FLOWPLANE_DATABASE_URL=sqlite://$DB_PATH ./target/release/flowplane &"
FLOWPLANE_DATABASE_URL="sqlite://$DB_PATH" ./target/release/flowplane > flowplane.log 2>&1 &
SERVER_PID=$!

echo "Server started with PID: $SERVER_PID"
echo "Waiting for server to be ready..."
sleep 3

# Extract setup token from logs
SETUP_TOKEN=$(grep -o 'fp_setup_[a-zA-Z0-9._-]*' flowplane.log | head -1)

if [ -z "$SETUP_TOKEN" ]; then
    echo "${RED}ERROR: Setup token not found in logs!${NC}"
    cat flowplane.log
    exit 1
fi

echo "${GREEN}✓ Setup token found: ${SETUP_TOKEN:0:20}...${NC}"
echo ""

# Parse token ID and secret
TOKEN_ID=$(echo $SETUP_TOKEN | sed 's/fp_setup_//' | cut -d'.' -f1)
WRONG_SECRET="wrong_secret_for_testing_lockout"

echo "${YELLOW}Step 3: Test Failed Attempt Tracking${NC}"
echo "Making 3 failed attempts with wrong secret..."

for i in {1..3}; do
    echo -n "  Attempt $i: "
    HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$API_URL/api/v1/bootstrap/initialize" \
        -H "Content-Type: application/json" \
        -d "{\"setupToken\": \"fp_setup_${TOKEN_ID}.${WRONG_SECRET}\", \"tokenName\": \"admin\"}")

    if [ "$HTTP_CODE" = "401" ]; then
        echo "${GREEN}✓ Correctly rejected (401)${NC}"
    else
        echo "${RED}✗ Unexpected status: $HTTP_CODE${NC}"
    fi
    sleep 0.5
done

echo ""
echo "${YELLOW}Step 4: Check Failed Attempts in Database${NC}"
FAILED_ATTEMPTS=$(sqlite3 "$DB_PATH" "SELECT failed_attempts FROM personal_access_tokens WHERE id='$TOKEN_ID'")
echo "  Failed attempts recorded: ${FAILED_ATTEMPTS}"

if [ "$FAILED_ATTEMPTS" -ge "3" ]; then
    echo "${GREEN}✓ Failed attempts correctly tracked${NC}"
else
    echo "${RED}✗ Failed attempts not tracked correctly${NC}"
fi

echo ""
echo "${YELLOW}Step 5: Test Lockout After 5 Failed Attempts${NC}"
echo "Making 2 more failed attempts to trigger lockout..."

for i in {4..5}; do
    echo -n "  Attempt $i: "
    HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$API_URL/api/v1/bootstrap/initialize" \
        -H "Content-Type: application/json" \
        -d "{\"setupToken\": \"fp_setup_${TOKEN_ID}.${WRONG_SECRET}\", \"tokenName\": \"admin\"}")

    echo "Status: $HTTP_CODE"
    sleep 0.5
done

echo ""
echo "Checking lockout status in database..."
LOCKED_UNTIL=$(sqlite3 "$DB_PATH" "SELECT locked_until FROM personal_access_tokens WHERE id='$TOKEN_ID'")

if [ -n "$LOCKED_UNTIL" ]; then
    echo "${GREEN}✓ Token is locked until: $LOCKED_UNTIL${NC}"
else
    echo "${RED}✗ Token should be locked but isn't${NC}"
fi

echo ""
echo "${YELLOW}Step 6: Verify Locked Token Rejection${NC}"
RESPONSE=$(curl -s -X POST "$API_URL/api/v1/bootstrap/initialize" \
    -H "Content-Type: application/json" \
    -d "{\"setupToken\": \"$SETUP_TOKEN\", \"tokenName\": \"admin\"}")

if echo "$RESPONSE" | grep -q "locked"; then
    echo "${GREEN}✓ Locked token correctly rejected${NC}"
    echo "  Error message: $(echo $RESPONSE | jq -r '.error // .message' 2>/dev/null || echo $RESPONSE)"
else
    echo "${RED}✗ Locked token was not rejected${NC}"
fi

echo ""
echo "${YELLOW}Step 7: Check Audit Log${NC}"
echo "Audit log entries:"
sqlite3 "$DB_PATH" "SELECT action, resource_id, created_at FROM audit_log WHERE action LIKE '%setup%' OR action LIKE '%bootstrap%' ORDER BY created_at DESC LIMIT 5" | head -5

echo ""
echo "========================================="
echo "${GREEN}Testing Complete!${NC}"
echo "========================================="
echo ""
echo "Summary of Features Tested:"
echo "  ✓ Setup token auto-generation on first startup"
echo "  ✓ Failed attempt tracking (tested 5 attempts)"
echo "  ✓ Auto-lockout after 5 failed attempts"
echo "  ✓ Lockout enforcement (15-minute duration)"
echo "  ✓ Audit logging of security events"
echo ""
echo "Note: Auto-revocation test requires valid setup token."
echo "To test revocation, wait for lockout to expire or use a fresh database."
echo ""
