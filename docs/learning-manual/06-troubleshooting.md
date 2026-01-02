# Troubleshooting and Reference

This document provides troubleshooting guidance and technical reference for the Learning Feature.

## Table of Contents

1. [Common Issues](#common-issues)
2. [Error Messages Reference](#error-messages-reference)
3. [Session State Troubleshooting](#session-state-troubleshooting)
4. [Schema Quality Issues](#schema-quality-issues)
5. [Performance Considerations](#performance-considerations)
6. [Authorization & Team Scopes](#authorization--team-scopes)
7. [Database Schema Reference](#database-schema-reference)

---

## Common Issues

### Session Stuck in "pending" State

**Symptom:** Session remains in `pending` status and never transitions to `active`.

**Cause:** The session activation failed or LDS refresh did not complete.

**How to Diagnose:**
```bash
# Check session status
GET /api/v1/learning-sessions/{session_id}

# Review server logs
grep "activate_learning_session" flowplane.log
grep "Failed to refresh listeners" flowplane.log
```

**Resolution:**
1. Verify XdsState is properly configured
2. Check Access Log Service and ExtProc Service availability
3. Review logs for "Failed to refresh listeners" errors
4. Cancel the session and create a new one

### Session Not Capturing Traffic

**Symptom:** Session is `active` but `current_sample_count` remains at 0.

**Common Causes:**
1. Route pattern doesn't match actual paths
2. HTTP method filter too restrictive
3. No traffic to the route
4. Access Log Service not registered

**Resolution:**

**For Route Pattern Mismatch:**
- Verify regex matches actual request paths
- Pattern `^/api/v1/users/.*` matches `/api/v1/users/123` but NOT `/api/users/123`
- Test patterns at regex101.com

**For HTTP Method Filter:**
- If `httpMethods: ["POST"]`, only POST requests captured
- Set `httpMethods: null` to capture all methods
- Methods must be uppercase

### Low Confidence Scores

**Symptom:** Aggregated schemas have `confidence_score` below 0.5.

**Cause:** Confidence depends on:
1. Sample size (40% weight)
2. Field consistency (40% weight)
3. Type stability (20% weight)

**Resolution:**
1. Increase `targetSampleCount` (50-100 recommended)
2. Ensure API returns consistent field structures
3. Fix type conflicts in your API

**Confidence Score Formula:**
```
Sample Score (40%):
- 10 samples: 0.5
- 50 samples: ~0.85
- 100+ samples: 1.0

Field Consistency (40%):
- required_fields / total_fields

Type Stability (20%):
- stable_fields / total_fields

Total = (sample * 0.4) + (field * 0.4) + (type * 0.2)
```

### Empty Schemas

**Symptom:** `requestSchema` or `responseSchemas` is null.

**Common Reasons:**
1. No request/response body (e.g., GET requests)
2. Body too large for ExtProc buffer
3. ExtProc Service not configured
4. Content-Type not `application/json`

**Resolution:**
1. Verify endpoint sends request/response bodies
2. Check Content-Type headers
3. Ensure ExtProc Service is enabled

### Session Times Out Before Target

**Symptom:** Session completes with `currentSampleCount < targetSampleCount`.

**Cause:** `maxDurationSeconds` timeout reached before collecting enough samples.

**Resolution:**
1. Increase `maxDurationSeconds`
2. Ensure sufficient traffic volume
3. Reduce `targetSampleCount`
4. Use broader `routePattern`

**Recommendations:**
- High-traffic: 1000 samples, 60 min timeout
- Medium-traffic: 100 samples, 2-4 hour timeout
- Low-traffic: 20-50 samples, 8-24 hour timeout

---

## Error Messages Reference

### Validation Errors (HTTP 400)

| Error Message | Cause | Resolution |
|---------------|-------|------------|
| "Team scope required for learning sessions" | Token missing team scope | Add team scope to auth token |
| "Invalid route pattern regex: {error}" | Invalid regex syntax | Fix regex, test at regex101.com |
| "Invalid HTTP method '{method}'" | Invalid HTTP verb | Use uppercase: GET, POST, etc. |
| "Cannot activate session in '{status}' state" | Session not pending | Create a new session |
| "Cannot cancel a completed learning session" | Session already finished | No action needed |
| "Learning session is already cancelled" | Already cancelled | No action needed |
| "Cannot cancel a failed learning session" | Session failed | Review error_message field |

### Authorization Errors (HTTP 403)

| Error Message | Cause | Resolution |
|---------------|-------|------------|
| "Forbidden - insufficient permissions" | Missing required scope | Request appropriate scope |

**Required Scopes:**
- `learning-sessions:read` - View sessions
- `learning-sessions:write` - Create/activate sessions
- `learning-sessions:delete` - Cancel sessions
- `schemas:read` - View aggregated schemas

### Not Found Errors (HTTP 404)

| Error Message | Cause | Resolution |
|---------------|-------|------------|
| "Learning session with ID '{id}' not found" | Session doesn't exist or wrong team | Verify session ID and team access |

**Security Note:** Cross-team access returns 404 (not 403) to prevent information leakage.

### Internal Errors (HTTP 500)

| Error Message | Cause | Resolution |
|---------------|-------|------------|
| "Repository not configured" | Database connection failed | Check database connectivity |
| "Failed to create learning session" | Database constraint error | Check database logs |
| "Failed to aggregate schemas for session" | Aggregation error | Review logs, session still completes |

---

## Session State Troubleshooting

### State Machine

```
pending → active → completing → completed
            ↓
         cancelled
            ↓
          failed
```

### State Descriptions

| State | Meaning | Can Cancel? |
|-------|---------|-------------|
| `pending` | Created, not activated | Yes |
| `active` | Capturing traffic | Yes |
| `completing` | Aggregating schemas | Yes |
| `completed` | Finished | No |
| `cancelled` | User cancelled | No |
| `failed` | Error occurred | No |

### Check Current State

```bash
GET /api/v1/learning-sessions/{session_id}

Response:
{
  "status": "active",
  "currentSampleCount": 45,
  "targetSampleCount": 100,
  "progressPercentage": 45.0
}
```

### Manual Intervention

**Cancel Active Session:**
```bash
DELETE /api/v1/learning-sessions/{session_id}
```

This will:
1. Transition to `cancelled` status
2. Unregister from Access Log Service
3. Trigger LDS refresh

**If Session is Stuck:**
1. Check logs for errors
2. Cancel the session
3. Create new session with adjusted parameters

### Background Worker

Runs every 30 seconds to:
- Check if `targetSampleCount` reached
- Check if `endsAt` exceeded
- Trigger schema aggregation

---

## Schema Quality Issues

### Type Conflicts (oneOf in Schema)

**Symptom:** Schema shows `"type": {"oneOf": ["string", "integer"]}`

**Cause:** Field has different types across observations.

**Example:**
```json
// Observation 1
{"status": "active"}

// Observation 2
{"status": 1}

// Result
{"status": {"type": {"oneOf": ["integer", "string"]}}}
```

**Resolution:**
1. Fix API for consistent types
2. Run new learning session after fix

**Impact:** Type conflicts reduce confidence score.

### Missing Required Fields

**Symptom:** Few fields in `required` array despite many properties.

**Cause:** Fields don't appear in 100% of observations.

**How Required is Determined:**
- Field is `required` if `presence_count == sample_count`
- 100% presence threshold

**Resolution:**
- If fields should be required, ensure API always returns them
- If intentionally optional, behavior is correct

### Low Presence Counts

**Symptom:** Fields have low `presence_count` relative to `sample_count`.

**Cause:** Conditional fields appearing in certain scenarios.

**Normal For:**
- Pagination fields (`next_page`, `prev_page`)
- Error details
- Feature flags

**Resolution:** Separate sessions by response status code if needed.

---

## Performance Considerations

### Target Sample Count Recommendations

| Use Case | Count | Rationale |
|----------|-------|-----------|
| Production (stable) | 100-500 | High confidence |
| Development/Staging | 20-50 | Quick feedback |
| High-traffic endpoint | 500-1000 | Statistical significance |
| Low-traffic endpoint | 10-20 | Avoid long waits |
| Schema validation | 1000+ | Maximum confidence |

### Session Duration Best Practices

**High-traffic (100+ req/min):**
```json
{"targetSampleCount": 1000, "maxDurationSeconds": 3600}
```

**Medium-traffic (10-50 req/min):**
```json
{"targetSampleCount": 100, "maxDurationSeconds": 7200}
```

**Low-traffic (<5 req/min):**
```json
{"targetSampleCount": 20, "maxDurationSeconds": 28800}
```

### Multiple Concurrent Sessions

**Supported:** Yes, multiple sessions can run simultaneously.

**Isolation:** Sessions isolated by team, route pattern, HTTP methods, session ID.

**Performance Impact:**
- Each session adds regex matching overhead
- Recommended: <10 concurrent sessions per Envoy instance

**Best Practice:**
- Use specific route patterns
- Use HTTP method filters
- Complete/cancel sessions when done

---

## Authorization & Team Scopes

### Required Scopes by Operation

| Operation | Required Scope |
|-----------|----------------|
| Create session | `learning-sessions:write` |
| List sessions | `learning-sessions:read` |
| Get session | `learning-sessions:read` |
| Cancel session | `learning-sessions:delete` |
| View schemas | `schemas:read` |
| Export schemas | `schemas:read` |

### Team Isolation

**Team-Scoped Users:**
- Can only create sessions for their team
- Can only view their team's sessions
- Cannot access other teams' data

**Admin Users (`admin:all` scope):**
- Can list sessions across all teams
- Can view any team's schemas
- Still need team scope to create/cancel

### Security Pattern

Cross-team access returns 404 (not 403) to prevent information leakage about resource existence.

---

## Database Schema Reference

### learning_sessions Table

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT (UUID) | Primary key |
| `team` | TEXT | Team identifier |
| `route_pattern` | TEXT | Regex pattern |
| `cluster_name` | TEXT | Optional cluster filter |
| `http_methods` | TEXT | JSON array or NULL |
| `status` | TEXT | Session status |
| `created_at` | DATETIME | Creation time |
| `started_at` | DATETIME | Activation time |
| `ends_at` | DATETIME | Timeout deadline |
| `completed_at` | DATETIME | Completion time |
| `target_sample_count` | INTEGER | Target samples |
| `current_sample_count` | INTEGER | Current samples |
| `triggered_by` | TEXT | Trigger source |
| `deployment_version` | TEXT | API version |
| `configuration_snapshot` | TEXT | JSON metadata |
| `error_message` | TEXT | Error details |
| `updated_at` | DATETIME | Last modified |

### Indexes

- `idx_learning_sessions_team` - Team queries
- `idx_learning_sessions_status` - Status filtering
- `idx_learning_sessions_team_status` - Combined queries
- `idx_learning_sessions_team_status_created` - List queries

---

## Quick Reference

### Common API Commands

```bash
# Create session
POST /api/v1/learning-sessions
{"routePattern": "^/api/.*", "targetSampleCount": 100}

# Check status
GET /api/v1/learning-sessions/{session_id}

# List active sessions
GET /api/v1/learning-sessions?status=active

# Cancel session
DELETE /api/v1/learning-sessions/{session_id}

# View schemas
GET /api/v1/aggregated-schemas?path=users

# Export schema
GET /api/v1/aggregated-schemas/{id}/export
```

### Log Patterns to Monitor

```bash
# Successful activation
grep "Activated learning session: pending → active" flowplane.log

# Traffic capture
grep "Registered learning session with Access Log Service" flowplane.log

# Completion
grep "Session completed: completing → completed" flowplane.log

# Errors
grep "Failed to activate learning session" flowplane.log
grep "Failed to refresh listeners" flowplane.log
grep "Failed to aggregate schemas" flowplane.log
```

### When to Contact Support

1. Sessions consistently fail to activate
2. LDS refresh errors appear repeatedly
3. Access Log Service or ExtProc Service unavailable
4. Database errors during session operations
5. Authorization scopes cannot be resolved

**Provide:**
- Session ID
- Timestamps
- Error messages from logs
- Session configuration
