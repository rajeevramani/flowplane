#!/bin/bash

# Test script for /api/v1/teams endpoint
# Tests both admin and non-admin user access

API_BASE="http://localhost:8080"

echo "Testing /api/v1/teams endpoint..."
echo "=================================="
echo ""

# Function to login and test teams endpoint
test_teams_for_user() {
    local email=$1
    local password=$2
    local user_type=$3

    echo "Testing as $user_type ($email):"
    echo "-----------------------------------"

    # Login
    echo "1. Logging in..."
    LOGIN_RESPONSE=$(curl -s -c /tmp/cookies-$user_type.txt -X POST "$API_BASE/api/v1/auth/login" \
        -H "Content-Type: application/json" \
        -d "{\"email\": \"$email\", \"password\": \"$password\"}")

    if echo "$LOGIN_RESPONSE" | grep -q "csrfToken"; then
        echo "   ✓ Login successful"
    else
        echo "   ✗ Login failed: $LOGIN_RESPONSE"
        return 1
    fi

    # Get session info
    echo "2. Getting session info..."
    SESSION_RESPONSE=$(curl -s -b /tmp/cookies-$user_type.txt "$API_BASE/api/v1/auth/sessions/me")
    echo "   Session: $SESSION_RESPONSE" | head -c 100
    echo "..."

    # Get teams
    echo "3. Fetching teams..."
    TEAMS_RESPONSE=$(curl -s -b /tmp/cookies-$user_type.txt "$API_BASE/api/v1/teams")
    echo "   Response: $TEAMS_RESPONSE"

    # Parse and display teams
    if echo "$TEAMS_RESPONSE" | grep -q "teams"; then
        TEAMS_COUNT=$(echo "$TEAMS_RESPONSE" | grep -o "\"team-[^\"]*\"" | wc -l)
        echo "   ✓ Teams endpoint accessible. Found $TEAMS_COUNT team(s)"
    else
        echo "   ✗ Unexpected response format"
    fi

    echo ""
}

# Test for admin user
test_teams_for_user "admin@example.com" "admin123" "admin"

# Test for non-admin user
test_teams_for_user "testuser@example.com" "password123" "non-admin"

# Cleanup
rm -f /tmp/cookies-*.txt

echo "=================================="
echo "Test complete!"
echo ""
echo "Expected results:"
echo "  - Admin user: Should see all teams (team-test-1)"
echo "  - Non-admin user: Should see only their teams (team-test-1)"
