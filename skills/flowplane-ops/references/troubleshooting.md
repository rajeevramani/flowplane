# Troubleshooting Decision Tree

Use this guide to systematically diagnose gateway issues. Each scenario starts with a symptom and walks through the diagnostic steps.

## Quick Decision Tree

```
What's the symptom?
│
├── Request returns 404 ──────────────► Go to: Routing Failure
├── Request returns 503 ──────────────► Go to: Service Unreachable
├── Request returns 403/401 ──────────► Go to: Auth Issues
├── Rate limiting not working ────────► Go to: Filter Issues
├── Something broke recently ─────────► Go to: Recent Changes
├── General health concern ───────────► Go to: Health Check
└── Don't know what's wrong ──────────► Start with: ops_trace_request
```

---

## Routing Failure (404)

**Symptom:** Requests return 404 Not Found.

```
Step 1: Trace the request
  Tool: ops_trace_request
  Args: { "method": "GET", "path": "<the failing path>", "host": "<the host header>" }
  
  Look at: Which step fails? (listener / route_config / virtual_host / route)

Step 2a: No listener matched
  Tool: cp_list_listeners
  Check: Is there a listener on the expected port?
  Check: Is the listener bound to a route config?
  
Step 2b: No virtual host matched
  Tool: cp_list_virtual_hosts { "route_config_name": "<from trace>" }
  Check: Do any virtual host domains match the Host header?
  Check: Is there a wildcard domain ("*")?
  
Step 2c: No route matched
  Tool: cp_list_routes
  Check: Does any path matcher match the request path?
  Common issues:
    - Exact match when prefix was needed
    - Missing leading "/"
    - Regex doesn't account for query params (they're stripped before matching)
    - Template variable syntax wrong

Step 3: Verify the full chain
  Tool: cp_query_service { "cluster_name": "<expected cluster>" }
  Check: Is the cluster referenced by a route → virtual host → route config → listener?
```

---

## Service Unreachable (503)

**Symptom:** Requests match a route but return 503 Service Unavailable.

```
Step 1: Check the cluster
  Tool: cp_get_cluster { "name": "<cluster from route action>" }
  Check: Are endpoints defined?
  Check: Are endpoint addresses correct (host:port)?

Step 2: Check cluster health
  Tool: cp_get_cluster_health { "name": "<cluster>" }
  Check: Are any endpoints healthy?

Step 3: Verify route → cluster mapping
  Tool: cp_get_route { "name": "<route>" }
  Check: Does action.cluster reference the correct cluster name?
  Check: For weighted routes, do all cluster names exist?

Step 4: Check listener binding
  Tool: cp_get_listener { "name": "<listener>" }
  Check: Is the route config bound to this listener?
```

---

## Auth Issues (401/403)

**Symptom:** Requests return 401 Unauthorized or 403 Forbidden.

```
Step 1: Check filter chain
  Tool: cp_list_filters
  Look for: JWT auth, ext_authz, RBAC, or OAuth2 filters

Step 2: Check filter config
  Tool: cp_get_filter { "name": "<auth filter>" }
  Check: Are JWKS URIs correct?
  Check: Are required issuers/audiences correct?

Step 3: Check per-route overrides
  Tool: cp_get_route { "name": "<route>" }
  Check: Does typedPerFilterConfig override the auth filter?
  Check: Is the override setting "allow_missing" or "disabled"?

Step 4: Check filter attachment
  Tool: cp_list_filter_attachments { "filter_name": "<auth filter>" }
  Check: Is the filter attached to the correct listener/route config?
```

---

## Filter Issues

**Symptom:** A filter (rate limit, CORS, etc.) isn't applying as expected.

```
Step 1: Verify filter exists
  Tool: cp_get_filter { "name": "<filter>" }
  Check: Is the filter enabled (not disabled)?
  Check: Is the config correct?

Step 2: Check attachment
  Tool: cp_list_filter_attachments { "filter_name": "<filter>" }
  Check: Is it attached to the correct resource?
  Check: Is it attached to a listener (global) or route config (scoped)?

Step 3: Check for per-route overrides
  Tool: cp_get_route { "name": "<route>" }
  Check: Does typedPerFilterConfig override or disable this filter?

Step 4: Check filter order
  Tool: cp_get_listener { "name": "<listener>" }
  Check: Filter chain order — filters execute top to bottom
  Issue: Auth filter after rate limiter = unauthenticated requests get rate-limited
```

---

## Recent Changes

**Symptom:** Something worked before and now doesn't.

```
Step 1: Query audit log
  Tool: ops_audit_query { "since": "<time before issue started>" }
  Look at: What was created, updated, or deleted?

Step 2: Narrow by operation
  Tool: ops_audit_query { "since": "<time>", "operation": "delete" }
  Tool: ops_audit_query { "since": "<time>", "operation": "update" }

Step 3: Inspect the changed resource
  Use the appropriate cp_get_* tool to examine current state.

Step 4: Validate config
  Tool: ops_config_validate
  Check: Did the change create orphans or broken references?
```

---

## Health Check

**Symptom:** Proactive check or general concern about gateway health.

```
Step 1: Deployment status
  Tool: devops_get_deployment_status { "include_details": true }
  Check: Any unhealthy clusters?
  Check: Resource counts look correct?

Step 2: Config validation
  Tool: ops_config_validate
  Check: Orphan clusters (not referenced by routes)
  Check: Unbound route configs (not attached to listeners)
  Check: Empty virtual hosts (no routes)
  Check: Duplicate path matchers

Step 3: Topology overview
  Tool: ops_topology
  Check: All expected resources present
  Check: Relationships look correct
  Check: No orphaned resources

Step 4: Recent changes
  Tool: ops_audit_query { "since": "<24 hours ago>" }
  Check: Any unexpected modifications
```

---

## Useful Tool Combinations

| Scenario | Tool Sequence |
|----------|--------------|
| "Is this service exposed?" | `cp_query_service` → `ops_trace_request` |
| "Why is this route not matching?" | `ops_trace_request` → `cp_get_route` → `cp_get_virtual_host` |
| "What's on port 8080?" | `cp_query_port { "port": 8080 }` |
| "Show me everything" | `ops_topology` → `devops_get_deployment_status` |
| "Is the config valid?" | `ops_config_validate` |
| "What happened in the last hour?" | `ops_audit_query { "since": "<1 hour ago>" }` |
