# Error codes & HTTP status mapping

> Audience: api-consumers, operators · Status: stable

Every Flowplane API failure is returned as a single stable JSON envelope carrying a machine-readable `code`. Clients branch on `code`; the HTTP status is derived from it. The code set is a closed, append-only contract — codes may be added but never change meaning.

## Error envelope

All error responses share this body:

```json
{
  "code": "validation_failed",
  "message": "human-readable statement of fact",
  "hint": "what to do next (optional)",
  "details": { "optional": "structured context" },
  "request_id": "01J…"
}
```

| Field | Type | Always present | Meaning |
|-------|------|----------------|---------|
| `code` | string | yes | Machine-actionable error code (see table below). Stable `snake_case`. |
| `message` | string | yes | Human-readable description. Never contains secrets or cross-tenant data. |
| `hint` | string | no | Suggested next action; omitted when absent. |
| `details` | any JSON value | no | Structured context safe for the caller (commonly an object); omitted when absent. |
| `request_id` | string | yes | Correlates the response with server logs and traces. |

`hint` and `details` are omitted from the JSON entirely when not set (not sent as `null`).

## Codes

One row per code. **Retryable** = the identical request may succeed if retried without modification.

| `code` | HTTP status | Retryable | Meaning |
|--------|-------------|-----------|---------|
| `validation_failed` | 400 Bad Request | no | Request was syntactically or semantically invalid. |
| `org_selector_required` | 400 Bad Request | no | No tenant org could be resolved for this tenant-scoped request: the caller belongs to several orgs and named none, has no tenant org yet, or selected the **platform** org — which is never a selectable tenant context. Name a tenant org with `X-Flowplane-Org: <name\|uuid>` (or `--org` on the CLI). |
| `unauthorized` | 401 Unauthorized | no | Authentication missing or invalid. |
| `forbidden` | 403 Forbidden | no | Authenticated but not permitted; `message` names the missing (resource, action). |
| `not_found` | 404 Not Found | no | Resource does not exist within the caller's visibility. Cross-tenant existence is indistinguishable from absence. |
| `conflict` | 409 Conflict | no | Uniqueness or state conflict (duplicate name, illegal lifecycle transition). |
| `revision_mismatch` | 409 Conflict | no | Optimistic-concurrency failure: the resource changed since the revision the caller read. |
| `quota_exceeded` | 422 Unprocessable Entity | no | Per-tenant quota exceeded. |
| `rate_limited` | 429 Too Many Requests | **yes** | Request rate limit exceeded. Response carries a `Retry-After` header (seconds). |
| `payload_too_large` | 413 Payload Too Large | no | Payload exceeds a configured size limit. |
| `invalid_config` | 500 Internal Server Error | no | Server-side configuration problem detected at startup or reload. Message and details are redacted (see below). |
| `unavailable` | 503 Service Unavailable | **yes** | A dependency (database, IdP, provider) is unavailable. |
| `internal` | 500 Internal Server Error | no | Unexpected internal failure. Message and details are redacted (see below). |

## Redaction of internal errors

For `internal` and `invalid_config`, the server does **not** return the underlying message or `details`. The response body is:

- `message`: `"an internal error occurred; report the request_id to your operator"`
- `details`: omitted
- `code` and `request_id`: present as normal

The original message is written to the server log keyed by `request_id`, so an operator can correlate the redacted response with the real cause. The original `details` are not returned and not logged here.

## Retry-After

`rate_limited` (and any error carrying a retry-after value) sets the standard `Retry-After` response header to the number of seconds after which a retry may succeed.

## Source of truth

- Code set, retryable predicate, and per-code meaning: `crates/fp-domain/src/error.rs` (`ErrorCode`, `ErrorCode::is_retryable`).
- HTTP status mapping, envelope, redaction, and `Retry-After`: `crates/fp-api/src/error.rs` (`ApiError::status`, `ErrorBody`, `IntoResponse`).

<!-- ci-skip-verification: docs-only PR, expect quality to skip -->
