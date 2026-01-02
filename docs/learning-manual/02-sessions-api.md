# Learning Sessions API Reference

This document provides a complete API reference for the Learning Sessions endpoints.

## Base URL

All endpoints are prefixed with `/api/v1/learning-sessions`

## Authentication

All endpoints require authentication via Bearer token. The token must include:
- A team scope (for team-scoped operations)
- Appropriate resource scopes (`learning-sessions:read`, `learning-sessions:write`, `learning-sessions:delete`)

---

## Create Learning Session

Creates a new learning session to capture API traffic.

**Endpoint:** `POST /api/v1/learning-sessions`

**Required Scope:** `learning-sessions:write`

### Request Body

```json
{
  "routePattern": "^/api/v1/users/.*",
  "clusterName": "api-cluster",
  "httpMethods": ["GET", "POST"],
  "targetSampleCount": 100,
  "maxDurationSeconds": 3600,
  "triggeredBy": "deploy-pipeline",
  "deploymentVersion": "v1.2.3",
  "configurationSnapshot": {
    "environment": "production"
  }
}
```

### Request Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `routePattern` | string | Yes | Regex pattern to match request paths |
| `clusterName` | string | No | Only capture traffic to this cluster |
| `httpMethods` | string[] | No | HTTP methods to capture (null = all) |
| `targetSampleCount` | integer | Yes | Session completes after this many samples |
| `maxDurationSeconds` | integer | No | Session times out after this duration |
| `triggeredBy` | string | No | What triggered this session (for tracking) |
| `deploymentVersion` | string | No | API version being learned |
| `configurationSnapshot` | object | No | Arbitrary metadata to store with session |

### Validation Rules

**routePattern:**
- Must be a valid regular expression
- Validated at creation time
- Example: `^/api/v1/users/[0-9]+$`

**httpMethods:**
- Must be uppercase
- Valid values: `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`, `TRACE`, `CONNECT`
- If null or omitted, all methods are captured

**targetSampleCount:**
- Must be >= 1
- Recommended: 50-500 for production use

**maxDurationSeconds:**
- If provided, must be >= 60
- Session auto-completes when timeout is reached

### Response

**Status:** `201 Created`

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "team": "my-team",
  "routePattern": "^/api/v1/users/.*",
  "clusterName": "api-cluster",
  "httpMethods": ["GET", "POST"],
  "status": "active",
  "createdAt": "2025-01-15T10:00:00Z",
  "startedAt": "2025-01-15T10:00:00Z",
  "endsAt": "2025-01-15T11:00:00Z",
  "completedAt": null,
  "targetSampleCount": 100,
  "currentSampleCount": 0,
  "progressPercentage": 0.0,
  "triggeredBy": "deploy-pipeline",
  "deploymentVersion": "v1.2.3",
  "configurationSnapshot": {"environment": "production"},
  "errorMessage": null
}
```

### Response Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | string (UUID) | Unique session identifier |
| `team` | string | Team that owns this session |
| `routePattern` | string | Regex pattern for route matching |
| `clusterName` | string | Cluster filter (null if not set) |
| `httpMethods` | string[] | HTTP methods to capture (null = all) |
| `status` | string | Current session status |
| `createdAt` | string (ISO 8601) | When session was created |
| `startedAt` | string (ISO 8601) | When session became active |
| `endsAt` | string (ISO 8601) | When session will timeout |
| `completedAt` | string (ISO 8601) | When session finished (null if not complete) |
| `targetSampleCount` | integer | Desired sample count |
| `currentSampleCount` | integer | Current samples captured |
| `progressPercentage` | number | Progress as percentage (0-100+) |
| `triggeredBy` | string | Source that triggered session |
| `deploymentVersion` | string | Version being learned |
| `configurationSnapshot` | object | Stored configuration metadata |
| `errorMessage` | string | Error details (if status is `failed`) |

### Error Responses

**400 Bad Request:**
```json
{
  "error": "Invalid route pattern regex: unclosed group"
}
```

```json
{
  "error": "Invalid HTTP method 'INVALID'. Valid methods are: GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS, TRACE, CONNECT"
}
```

**403 Forbidden:**
```json
{
  "error": "Forbidden - insufficient permissions"
}
```

---

## List Learning Sessions

Lists learning sessions for the authenticated user's team.

**Endpoint:** `GET /api/v1/learning-sessions`

**Required Scope:** `learning-sessions:read`

### Query Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `status` | string | null | Filter by status |
| `limit` | integer | 50 | Max results to return |
| `offset` | integer | 0 | Pagination offset |

**Status values:** `pending`, `active`, `completing`, `completed`, `cancelled`, `failed`

### Response

**Status:** `200 OK`

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "team": "my-team",
    "routePattern": "^/api/v1/users/.*",
    "status": "active",
    "currentSampleCount": 45,
    "targetSampleCount": 100,
    "progressPercentage": 45.0,
    "createdAt": "2025-01-15T10:00:00Z",
    ...
  }
]
```

### Notes

- Admin users with `admin:all` scope see sessions from all teams
- Regular users only see their team's sessions
- Results are ordered by `created_at` descending (newest first)

---

## Get Learning Session

Retrieves details for a specific learning session.

**Endpoint:** `GET /api/v1/learning-sessions/{session_id}`

**Required Scope:** `learning-sessions:read`

### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `session_id` | string (UUID) | Session identifier |

### Response

**Status:** `200 OK`

Returns the full session object (same format as create response).

### Error Responses

**404 Not Found:**
```json
{
  "error": "Learning session with ID '550e8400-e29b-41d4-a716-446655440000' not found"
}
```

### Security Note

If the session exists but belongs to a different team, `404 Not Found` is returned instead of `403 Forbidden` to prevent information leakage.

---

## Cancel Learning Session

Cancels an active or pending learning session.

**Endpoint:** `DELETE /api/v1/learning-sessions/{session_id}`

**Required Scope:** `learning-sessions:delete`

### Path Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `session_id` | string (UUID) | Session identifier |

### Response

**Status:** `204 No Content`

### Cancellation Behavior

When cancelled, the session:
1. Transitions to `cancelled` status
2. Unregisters from Access Log Service
3. Unregisters from ExtProc Service
4. Triggers LDS refresh to remove configurations

### Error Responses

**400 Bad Request:**
```json
{
  "error": "Cannot cancel a completed learning session"
}
```

```json
{
  "error": "Learning session is already cancelled"
}
```

```json
{
  "error": "Cannot cancel a failed learning session"
}
```

**404 Not Found:**
```json
{
  "error": "Learning session with ID '...' not found"
}
```

### Cancellable States

| Current Status | Can Cancel? |
|----------------|-------------|
| `pending` | Yes |
| `active` | Yes |
| `completing` | Yes |
| `completed` | No |
| `cancelled` | No |
| `failed` | No |

---

## Session Progress Calculation

Progress is calculated as:

```
progressPercentage = (currentSampleCount / targetSampleCount) * 100
```

**Notes:**
- Progress can exceed 100% if more samples are captured than the target
- Progress is updated in real-time as samples are captured
- The background worker checks progress every 30 seconds

---

## Auto-Completion Triggers

A session automatically transitions from `active` to `completing` when **either** condition is met:

1. **Target reached:** `currentSampleCount >= targetSampleCount`
2. **Timeout:** `now >= endsAt` (if `maxDurationSeconds` was set)

After transitioning to `completing`, schema aggregation runs and the session becomes `completed`.

---

## Example Workflows

### Create and Monitor a Session

```bash
# 1. Create session
SESSION=$(curl -X POST http://localhost:8080/api/v1/learning-sessions \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "routePattern": "^/api/v1/orders/.*",
    "targetSampleCount": 50,
    "maxDurationSeconds": 1800
  }' | jq -r '.id')

# 2. Monitor progress
while true; do
  STATUS=$(curl -s http://localhost:8080/api/v1/learning-sessions/$SESSION \
    -H "Authorization: Bearer $TOKEN" | jq -r '.status')

  if [ "$STATUS" = "completed" ]; then
    echo "Session completed!"
    break
  fi

  sleep 10
done
```

### Filter Sessions by Status

```bash
# Get only active sessions
curl "http://localhost:8080/api/v1/learning-sessions?status=active" \
  -H "Authorization: Bearer $TOKEN"

# Get completed sessions with pagination
curl "http://localhost:8080/api/v1/learning-sessions?status=completed&limit=10&offset=20" \
  -H "Authorization: Bearer $TOKEN"
```

### Cancel a Session

```bash
curl -X DELETE http://localhost:8080/api/v1/learning-sessions/$SESSION_ID \
  -H "Authorization: Bearer $TOKEN"
```
