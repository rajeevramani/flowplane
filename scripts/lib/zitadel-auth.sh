#!/usr/bin/env bash
# Reusable OIDC token acquisition for human Zitadel users.
#
# Uses the Session API + OIDC finalize flow (the canonical programmatic
# login pattern for Zitadel human users acting as a custom login client).
#
# Requires:
#   - ZITADEL_HOST       (e.g. http://localhost:8081)
#   - ZITADEL_PAT        (admin service-account PAT with IAM_LOGIN_CLIENT role)
#   - FLOWPLANE_ZITADEL_SPA_CLIENT_ID
#   - FLOWPLANE_ZITADEL_PROJECT_ID
#
# The admin PAT owner must have IAM_LOGIN_CLIENT role granted via:
#   PUT /admin/v1/members/{userId} { "roles": ["IAM_OWNER", "IAM_LOGIN_CLIENT"] }
#
# Usage:
#   source scripts/lib/zitadel-auth.sh
#   token=$(get_oidc_token "user@example.com" "Password1!")

get_oidc_token() {
  local email="$1" password="$2"
  local client_id="${FLOWPLANE_ZITADEL_SPA_CLIENT_ID:?FLOWPLANE_ZITADEL_SPA_CLIENT_ID not set}"
  local project_id="${FLOWPLANE_ZITADEL_PROJECT_ID:?FLOWPLANE_ZITADEL_PROJECT_ID not set}"
  local host="${ZITADEL_HOST:?ZITADEL_HOST not set}"
  local redirect_uri="http://localhost:8080/auth/callback"

  # 1. PKCE code_verifier + code_challenge (S256)
  local code_verifier code_challenge
  code_verifier=$(openssl rand -base64 96 | tr -d '=+/\n' | cut -c1-128)
  code_challenge=$(printf '%s' "$code_verifier" | openssl dgst -sha256 -binary | openssl base64 -A | tr '+/' '-_' | tr -d '=')

  # 2. Start OIDC authorize with x-zitadel-login-client header
  #    This header tells Zitadel to treat the call as coming from a custom login
  #    client, creating a v2 auth request accessible via the Session/OIDC v2 API.
  #    Without it, the auth request is created in Zitadel's built-in login context
  #    and cannot be finalized via the API.
  local scope="openid profile email urn:zitadel:iam:org:project:id:${project_id}:aud"
  local state="seed-$$"
  local encoded_redirect encoded_scope
  encoded_redirect=$(python3 -c "import urllib.parse; print(urllib.parse.quote('${redirect_uri}', safe=''))")
  encoded_scope=$(python3 -c "import urllib.parse; print(urllib.parse.quote('${scope}', safe=''))")

  local auth_url="${host}/oauth/v2/authorize"
  auth_url+="?response_type=code"
  auth_url+="&client_id=${client_id}"
  auth_url+="&redirect_uri=${encoded_redirect}"
  auth_url+="&scope=${encoded_scope}"
  auth_url+="&code_challenge=${code_challenge}"
  auth_url+="&code_challenge_method=S256"
  auth_url+="&state=${state}"

  local auth_headers
  auth_headers=$(curl -s -D - -o /dev/null \
    -H "x-zitadel-login-client: ${ZITADEL_PAT}" \
    "$auth_url" 2>&1 || true)

  # Extract auth request ID from the Location header redirect.
  # With x-zitadel-login-client, Zitadel redirects to /ui/v2/login/login?authRequest=V2_xxx
  # Without it, it redirects to /ui/login/login?authRequestID=xxx (not accessible via v2 API)
  local auth_request_id
  auth_request_id=$(echo "$auth_headers" | grep -i '^location:' | head -1 | sed -E 's/.*authRequest=([^& ]+).*/\1/' | tr -d '\r\n')

  if [ -z "$auth_request_id" ] || [ "$auth_request_id" = "$(echo "$auth_headers" | grep -i '^location:' | head -1 | tr -d '\r\n')" ]; then
    # Fallback: try authRequestID= (older Zitadel format)
    auth_request_id=$(echo "$auth_headers" | grep -i '^location:' | head -1 | sed -E 's/.*authRequestID=([^& ]+).*/\1/' | tr -d '\r\n')
  fi

  if [ -z "$auth_request_id" ]; then
    echo "ERROR: Could not extract authRequestId from authorize redirect" >&2
    echo "Headers: $auth_headers" >&2
    return 1
  fi

  # 3. Create authenticated session via Session API v2
  #    Use jq to build JSON safely (avoids shell escaping issues with special chars)
  local session_body
  session_body=$(jq -n --arg e "$email" --arg p "$password" \
    '{checks: {user: {loginName: $e}, password: {password: $p}}}')

  local session_raw session_code session_response
  session_raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Authorization: Bearer ${ZITADEL_PAT}" \
    -H "Content-Type: application/json" \
    -d "$session_body" \
    "${host}/v2/sessions")
  session_code=$(echo "$session_raw" | tail -1)
  session_response=$(echo "$session_raw" | sed '$d')

  if [ "$session_code" != "200" ] && [ "$session_code" != "201" ]; then
    echo "ERROR: Session creation failed (HTTP ${session_code}): ${session_response}" >&2
    return 1
  fi

  local session_id session_token
  session_id=$(echo "$session_response" | jq -r '.sessionId')
  session_token=$(echo "$session_response" | jq -r '.sessionToken')

  if [ -z "$session_id" ] || [ "$session_id" = "null" ]; then
    echo "ERROR: No sessionId in response: ${session_response}" >&2
    return 1
  fi

  # 4. Finalize OIDC auth request with session token
  local finalize_body
  finalize_body=$(jq -n --arg sid "$session_id" --arg stok "$session_token" \
    '{session: {sessionId: $sid, sessionToken: $stok}}')

  local finalize_raw finalize_code finalize_response
  finalize_raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Authorization: Bearer ${ZITADEL_PAT}" \
    -H "Content-Type: application/json" \
    -d "$finalize_body" \
    "${host}/v2/oidc/auth_requests/${auth_request_id}")
  finalize_code=$(echo "$finalize_raw" | tail -1)
  finalize_response=$(echo "$finalize_raw" | sed '$d')

  if [ "$finalize_code" != "200" ] && [ "$finalize_code" != "201" ]; then
    echo "ERROR: Auth request finalize failed (HTTP ${finalize_code}): ${finalize_response}" >&2
    return 1
  fi

  local callback_url auth_code
  callback_url=$(echo "$finalize_response" | jq -r '.callbackUrl')
  if [ -z "$callback_url" ] || [ "$callback_url" = "null" ]; then
    echo "ERROR: No callbackUrl in finalize response: ${finalize_response}" >&2
    return 1
  fi

  # Extract code from callback URL
  auth_code=$(echo "$callback_url" | sed -E 's/.*[?&]code=([^&]+).*/\1/')
  if [ -z "$auth_code" ] || [ "$auth_code" = "$callback_url" ]; then
    echo "ERROR: Could not extract code from callbackUrl: ${callback_url}" >&2
    return 1
  fi

  # 5. Exchange authorization code for tokens
  local token_raw token_code token_response
  token_raw=$(curl -s -w '\n%{http_code}' \
    -X POST \
    -H "Content-Type: application/x-www-form-urlencoded" \
    -d "grant_type=authorization_code" \
    -d "code=${auth_code}" \
    -d "redirect_uri=${redirect_uri}" \
    -d "client_id=${client_id}" \
    -d "code_verifier=${code_verifier}" \
    "${host}/oauth/v2/token")
  token_code=$(echo "$token_raw" | tail -1)
  token_response=$(echo "$token_raw" | sed '$d')

  if [ "$token_code" != "200" ]; then
    echo "ERROR: Token exchange failed (HTTP ${token_code}): ${token_response}" >&2
    return 1
  fi

  local access_token
  access_token=$(echo "$token_response" | jq -r '.access_token')
  if [ -z "$access_token" ] || [ "$access_token" = "null" ]; then
    echo "ERROR: No access_token in token response: ${token_response}" >&2
    return 1
  fi

  echo "$access_token"
}
