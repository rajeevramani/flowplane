# API Gateway Recipes

This guide groups together common gateway scenarios and shows which cluster, route, and listener configurations to combine. Every recipe references features supported by the control plane today—no extra Envoy wiring required. Use the Swagger UI at `http://127.0.0.1:8080/swagger-ui` or the Bruno workspace under `bruno/` to run the requests.

## 1. Protect a Public API (JWT + Rate Limiting)
Goal: authenticate clients with JWTs and throttle traffic globally while giving critical endpoints their own caps.

1. **Cluster** – create an upstream (HTTPS optional) using the [TLS cluster example](cluster-cookbook.md#2-tls-enabled-upstream) if your service speaks TLS.
2. **Route** – define a prefix route for `/api` and add a per-route Local Rate Limit (see [routing cookbook §5](routing-cookbook.md#5-scoped-filters-on-routes)).
3. **Listener** – insert the JWT auth filter followed by a global Local Rate Limit (see [listener cookbook §3](listener-cookbook.md#3-jwt-authentication-filter) and [§2](listener-cookbook.md#2-listener-wide-local-rate-limit)).
4. **Verify** – curl the listener; valid tokens pass, exceeding the route bucket returns HTTP 429.

## 2. Route Multiple Services by Path Prefix
Goal: expose `/api/orders`, `/api/catalog`, and `/status` from different upstreams through a single listener.

1. **Clusters** – create one cluster per upstream (see [cluster cookbook §1](cluster-cookbook.md#1-basic-http-cluster)).
2. **Routes** – add multiple prefix matches in one virtual host, each forwarding to the appropriate cluster (see [routing cookbook §1](routing-cookbook.md#1-basic-forward-route)).
3. **Listener** – point the HTTP connection manager at the new route config (see [listener cookbook §1](listener-cookbook.md#1-minimal-http-listener)).
4. Optionally attach per-route filters (e.g., stricter limits on `/api/orders`).

## 3. Blue/Green Release Behind One Hostname
Goal: shift a percentage of traffic to a new cluster while keeping the majority on the stable version.

1. **Clusters** – register `service-blue` and `service-green` (cluster cookbook §1).
2. **Route** – use a weighted action with `totalWeight` and per-cluster `typedPerFilterConfig` as needed (routing cookbook §2).
3. **Listener** – reference the route config; add tracing/logging if you need extra observability (listener cookbook §4).
4. Adjust weights over time via `PUT /api/v1/route-configs/{name}`.

## 4. Terminate TLS at the Edge and Forward with mTLS
Goal: accept HTTPS from clients, terminate TLS at Envoy, and forward to an upstream that expects TLS.

1. **Cluster** – create a TLS-enabled upstream with `useTls: true` and `tlsServerName` (cluster cookbook §2).
2. **Listener** – wrap the filter chain in a TLS context (`certChainFile`, `privateKeyFile`, optional `caCertFile` / `requireClientCertificate`) using listener cookbook §5.
3. **Routes** – standard prefix/regex/template routing applies; no extra steps required.
4. Ensure certificates referenced in the listener exist on the Envoy host.

## 5. Expose a TCP Service
Goal: front non-HTTP protocols (e.g., Redis, MySQL) through Envoy.

1. **Cluster** – define the upstream endpoints (cluster cookbook §1).
2. **Listener** – create a TCP proxy listener using listener cookbook §6.
3. No route object is required; the TCP proxy attaches directly to the cluster.

## 6. Observability on Every Request
Goal: emit access logs and record basic traces for troubleshooting.

1. **Listener** – add the access log and tracing blocks from listener cookbook §4 to your HTTP connection manager.
2. **Routes** – optionally enable per-route metadata (e.g., JWT payload in metadata) to enrich logs.
3. **Clusters** – ensure health checks are configured (cluster cookbook §3) so metrics reflect only healthy upstreams.

## Checklist & Operations
- List resources with `GET /api/v1/{clusters|routes|listeners}` before and after each change.
- Update resources via `PUT` using the same payload shapes shown in the cookbooks.
- Delete unused resources with `DELETE /api/v1/{resource}/{name}` to keep the control plane tidy.
- Run `scripts/smoke-listener.sh` whenever you need a quick end-to-end sanity check.

Combine these recipes with the detailed cookbooks to assemble gateways that match your needs today, and extend them incrementally as new filters are added to the control plane.
