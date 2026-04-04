# Troubleshooting Decision Tree

Use this guide to systematically diagnose gateway issues. Each scenario starts with a symptom and walks through the diagnostic steps.

## Quick Decision Tree

```
What's the symptom?
│
├── Request returns 404 ──────────────► Go to: Routing Failure
├── Request returns 503 ──────────────► Go to: Service Unreachable
├── Request returns 403/401 ──────────► Go to: Auth Issues
├── Auth filter can't reach remote ───► Go to: Auth Filter Remote Fetch Failed
├── Config correct but not applied ───► Go to: xDS Delivery Failure
├── Multiple unrelated failures ──────► Go to: Cascading xDS Failure
├── Rate limiting not working ────────► Go to: Filter Issues
├── Something broke recently ─────────► Go to: Recent Changes
├── General health concern ───────────► Go to: Health Check
└── Don't know what's wrong ──────────► Start with: ops_xds_delivery_status, then ops_trace_request
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
Step 1: Check for CDS NACKs
  Tool: ops_xds_delivery_status { "dataplane_name": "<dataplane>" }
  Check: Is CDS in NACK state? If yes, cluster updates aren't reaching Envoy.
  If NACKed → Go to: xDS Delivery Failure

Step 2: Check the cluster
  Tool: cp_get_cluster { "name": "<cluster from route action>" }
  Check: Are endpoints defined?
  Check: Are endpoint addresses correct (host:port)?

Step 3: Check cluster health
  Tool: cp_get_cluster_health { "name": "<cluster>" }
  Check: Are any endpoints healthy?

Step 4: Verify route → cluster mapping
  Tool: cp_get_route { "name": "<route>" }
  Check: Does action.cluster reference the correct cluster name?
  Check: For weighted routes, do all cluster names exist?

Step 5: Check listener binding
  Tool: cp_get_listener { "name": "<listener>" }
  Check: Is the route config bound to this listener?
```

---

## Auth Issues (401/403)

**Symptom:** Requests return 401 Unauthorized or 403 Forbidden.

```
Step 0: Check xDS delivery
  Tool: ops_xds_delivery_status { "dataplane_name": "<dataplane>" }
  Check: Is any resource type NACKed?
  Why: Auth filters depend on working xDS delivery. A CDS NACK can block
       clusters that auth filters depend on (e.g. JWKS providers, ext_authz
       backends). An LDS NACK can prevent filter chain updates from applying.
  If NACKed → Go to: xDS Delivery Failure (fix delivery before investigating filter config)

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
Step 1: xDS delivery status
  Tool: ops_xds_delivery_status
  Check: Any NACKs across dataplanes?
  Check: All resource types (CDS/RDS/LDS/EDS) showing ACK?

Step 2: Deployment status
  Tool: devops_get_deployment_status { "include_details": true }
  Check: Any unhealthy clusters?
  Check: Resource counts look correct?

Step 3: Config validation
  Tool: ops_config_validate
  Check: Orphan clusters (not referenced by routes)
  Check: Unbound route configs (not attached to listeners)
  Check: Empty virtual hosts (no routes)
  Check: Duplicate path matchers

Step 4: Topology overview
  Tool: ops_topology
  Check: All expected resources present
  Check: Relationships look correct
  Check: No orphaned resources

Step 5: Recent changes
  Tool: ops_audit_query { "since": "<24 hours ago>" }
  Check: Any unexpected modifications
```

---

## xDS Delivery Failure

**Symptom:** Config looks correct in the DB but Envoy isn't applying it.

```
Step 1: Check delivery status
  Tool: ops_xds_delivery_status { "dataplane_name": "<dataplane>" }
  Check: Which resource types are NACKed?
  Check: What's the error message?

Step 2: Get NACK history
  Tool: ops_nack_history { "dataplane_name": "<dataplane>", "type_url": "<NACKed type>", "limit": 10 }
  Check: When did the first NACK occur?
  Check: What resource does the error message name?

Step 3: Inspect the bad resource
  Use the appropriate cp_get_* tool to examine the named resource.
  Check: Is the config valid? (correct types, no missing required fields)

Step 4: Fix and verify
  Fix or remove the bad resource, then confirm recovery:
  Tool: ops_xds_delivery_status { "dataplane_name": "<dataplane>" }
  Check: NACKed type now shows ACK status
```

---

## Auth Filter Remote Fetch Failed

**Symptom:** Auth filter returns 401/403 and Envoy logs show it can't reach a remote dependency (JWKS provider, ext_authz service, OAuth2 endpoint, etc.). The remote URI is correct.

```
Step 1: Check for CDS NACKs
  Tool: ops_nack_history { "type_url": "CDS", "limit": 5 }
  Check: Is there a CDS NACK? If yes, a bad cluster is blocking ALL cluster
         updates — including clusters that auth filters depend on.

Step 2: Confirm delivery status
  Tool: ops_xds_delivery_status { "dataplane_name": "<dataplane>" }
  Check: CDS should show NACK state.

Step 3: Fix the bad cluster
  The NACK error names the offending cluster. Fix or remove it.

Step 4: Verify recovery
  Tool: ops_xds_delivery_status { "dataplane_name": "<dataplane>" }
  Check: CDS shows ACK. Auth filter can now reach its dependencies.
```

---

## Cascading xDS Failure

**Symptom:** Multiple unrelated features failing simultaneously. New services return 503, auth filters return 401, new filters don't apply.

**How it works:** Envoy processes xDS updates per resource type. One bad resource NACKs the entire type — not just the bad resource.

```
Step 1: Check all resource types
  Tool: ops_xds_delivery_status { "dataplane_name": "<dataplane>" }
  Check: Which types are NACKed? (CDS, RDS, LDS, EDS)

Step 2: Get NACK details for each affected type
  Tool: ops_nack_history { "dataplane_name": "<dataplane>", "type_url": "<type>", "limit": 5 }
  Check: What resource is named in the error?

Step 3: Fix the root cause
  The first NACK event points to the original bad resource.
  Fix it — all pending updates for that resource type will then be delivered.

Common cascading patterns:
  - CDS NACK → auth filter dependencies unreachable → 401/403
  - CDS NACK → new service clusters not delivered → 503
  - LDS NACK → filter chain updates stuck → new filters don't apply
  - RDS NACK → route changes not applied → stale routing
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
| "Is xDS delivery healthy?" | `ops_xds_delivery_status` → `ops_nack_history` (if NACKs found) |
| "Config correct but not applied" | `ops_xds_delivery_status` → `ops_nack_history` → `cp_get_*` (bad resource) |
| "Multiple unrelated failures" | `ops_xds_delivery_status` → `ops_nack_history` per NACKed type |
