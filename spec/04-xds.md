# 04 — Envoy xDS Subsystem

Behavioral specification extracted from Flowplane v1 (`/tmp/flowplane-v1`). This document is
self-contained: a competent engineer should be able to reimplement the xDS subsystem from it
without opening v1 source. v1 file paths are cited for provenance only.

---

## 1. xDS server: services, ports, mTLS, identity, scoping

*Sources: `src/xds/mod.rs`, `src/xds/services/{database.rs,mtls.rs,minimal.rs}`, `src/config/mod.rs`.*

### 1.1 gRPC services on one port

A single tonic gRPC server (the "xDS server") hosts four services on one bind address:

| Service | Proto | Purpose |
|---|---|---|
| ADS | `envoy.service.discovery.v3.AggregatedDiscoveryService` (SOTW `StreamAggregatedResources` + `DeltaAggregatedResources`) | LDS/RDS/CDS/EDS/SDS delivery |
| ALS | `envoy.service.accesslog.v3.AccessLogService` (`StreamAccessLogs`) | Learning-session access logs from Envoy |
| ExtProc | `envoy.service.ext_proc.v3.ExternalProcessor` (`Process`) | Learning-session request/response body capture |
| Diagnostics | `flowplane.diagnostics.v1.EnvoyDiagnosticsService` (`ReportDiagnostics`, bidi stream) — custom proto | Warming-failure / heartbeat reports from the flowplane-agent sidecar |

- Bind: `FLOWPLANE_XDS_BIND_ADDRESS` (default `0.0.0.0`), port `FLOWPLANE_XDS_PORT` (default `18000`; demo/docker deployments commonly use `50051`).
- `FLOWPLANE_XDS_ADVERTISE_ADDRESS` (`host[:port]`) — the address dataplanes use to reach the CP from inside containers; only the host part is used when building dynamic clusters (port comes from the xDS port). Bracketed IPv6 and bare IPv6 must be handled (`[::1]:8080` → `::1`; `::1` passes through unchanged). When bind is `0.0.0.0` and no advertise address is set, dynamic clusters target `127.0.0.1`.
- The diagnostics service is registered only when a DB pool + NACK-event repository exist; it is strictly advisory — its failures must never affect the xDS path.
- A gRPC tracing layer (W3C `traceparent`/`tracestate` extraction from gRPC metadata) wraps all services.
- Two server modes exist in v1: a "minimal" config-file-backed ADS (`services/minimal.rs`, test/demo only, no auth) and the production database-backed ADS. Only the latter matters for v2.

### 1.2 mTLS is mandatory

- The server is always built with `ServerTlsConfig::new().identity(cert,key).client_ca_root(ca)` from `FLOWPLANE_XDS_TLS_CERT_PATH`, `FLOWPLANE_XDS_TLS_KEY_PATH`, `FLOWPLANE_XDS_TLS_CLIENT_CA_PATH`. Config loading hard-fails at boot if any path is missing/unreadable (ADR "cp-xds-mtls-non-negotiable"). There is no plaintext mode.
- `FLOWPLANE_DATAPLANE_TLS_{CERT,KEY,CA}_PATH` (optional, `DataplaneTlsConfig`) are the cert paths *as seen inside the Envoy container*. They are embedded as filename `DataSource`s in the `UpstreamTlsContext` of the dynamic ALS/ExtProc/RLS clusters the CP pushes via CDS (§3.3). If unset, the CP logs a loud boot warning: those clusters will be pushed without TLS, Envoy cannot complete the mTLS handshake back to the CP port, and learning sessions silently collect 0 samples (v1 finding fp-8kzc).

### 1.3 Client identity: SPIFFE URI + DB certificate binding

On every new ADS stream (SOTW and Delta) the handler:

1. Requires peer certs (`request.peer_certs()`); otherwise `UNAUTHENTICATED "Client certificate required for xDS mTLS"`.
2. Parses the first (leaf) cert; extracts the first SAN URI starting with `spiffe://`. No SPIFFE URI → `UNAUTHENTICATED`.
   Supported URI shapes (`src/xds/services/mtls.rs`):
   - org-scoped: `spiffe://{trust_domain}/org/{org}/team/{team}/proxy/{proxy_id}`
   - legacy: `spiffe://{trust_domain}/team/{team}/proxy/{proxy_id}`
   Extracted: `org` (optional), `team`, `proxy_id`, full `spiffe_uri`, cert serial (hex, audit only).
3. **Certificate binding (the authoritative step)** — `validate_mtls_identity_against_db` (`src/xds/services/database.rs`):
   - Look up the **full SPIFFE URI** (globally unique by construction) in the `proxy_certificates` registry.
   - Reject (`UNAUTHENTICATED`) if: registry unavailable, DB error, row missing ("not registered"), row revoked, or row expired. Fail closed in every case.
   - The team is resolved from the matched row's `team_id` via the teams table (`get_team_by_id`); reject if the team no longer exists. **The team string parsed from the SAN is discarded** — never trusted (v1 findings B4/B9, fp-nw8b.16).
4. The resolved team name becomes the stream's `mtls_team` and is passed into every response builder for the lifetime of the stream.

`node.metadata.team` (self-reported) is logged for connection metrics and mismatch warnings but is **never** an authorization source. A request with metadata team but no mTLS-verified team is rejected `PERMISSION_DENIED "mTLS-verified team identity required for xDS connection"`.

### 1.4 Node ID format

Envoy bootstrap node id: `team={team}/dp-{dataplane_uuid}` (e.g. `team=engineering/dp-86107bfe-0819-4c63-8859-197835cc56cc`). The CP parses the UUID by splitting on `/` and stripping the `dp-` prefix; it resolves the human-readable dataplane name via `SELECT name FROM dataplanes WHERE id = $1`, falling back to the raw node id. This is used only for NACK attribution, not authorization.

### 1.5 Per-dataplane resource scoping

Scope is computed per discovery request (`scope_from_discovery`):

```
enum Scope { All /* deprecated, never constructed */, Team { team }, Allowlist { names } }
```

- If `node.metadata.listener_allowlist` is a non-empty list of strings → `Allowlist` (shared-infrastructure escape hatch; takes precedence over team).
- Else if an mTLS-verified team exists → `Team`.
- Else → reject `PERMISSION_DENIED`.
- `Scope::All` is kept only as a tombstone; any code path that encounters it logs a SECURITY error and fails closed (returns error / empty set).

Per-type scoping rules (`teams_from_scope` + builders):

| Type | Team scope | Allowlist scope |
|---|---|---|
| CDS | team's clusters + shared/default (`team IS NULL`) rows; plus built-in CP clusters (§3.3) and cached `*jwks*` clusters | default-only rows + built-ins |
| RDS | team's route configs + defaults | default-only |
| LDS | team's listeners + defaults; **empty result ⇒ empty list** (teams must define listeners explicitly; never fall back to config defaults — prevents port collisions) | default-only (name filtering noted as post-retrieval but v1 effectively serves default-only) |
| EDS | static config only (Phase A: not DB-driven) | same |
| SDS | team's secrets, **then intersected with the request's `resource_names` subscription** (finding B7, least privilege): empty subscription ⇒ zero secrets; unknown names ignored. SDS is treated as non-wildcard. | default-only, then subscription filter |

Team names are resolved to team UUIDs before DB queries (resource tables store team UUID FKs). All DB errors in builders fail closed for team/allowlist scopes (empty list + warn), except CDS/RDS which fall back to config-based resources on DB error (a v1 inconsistency — see §8).

---

## 2. Snapshot / state model

*Sources: `src/xds/state.rs`, `src/xds/services/{database.rs,stream.rs}`.*

### 2.1 In-memory cache

`XdsState` holds:
- `version: AtomicU64`, starts at **1**.
- `resource_caches: RwLock<HashMap<type_url, HashMap<name, CachedResource>>>` where `CachedResource = { name, type_url, version: u64, body: protobuf Any }`.
- `update_tx: tokio broadcast channel (capacity 128)` of `Arc<ResourceUpdate>` where `ResourceUpdate = { version, deltas: Vec<ResourceDelta> }`, `ResourceDelta = { type_url, added_or_updated: Vec<CachedResource>, removed: Vec<String> }`.
- All resource repositories, filter-schema registry, secret backend registry, ALS/ExtProc/learning-session service handles, proxy-cert + NACK repositories, RLS translator handle, raw pool.
- A two-level listener-port-allocation lock (in-process `tokio::Mutex` + PG advisory lock key `0x666c_7770_6f72_7421` / ASCII `flwport!`, taken on a `close_on_drop()` connection so cancellation/panic always releases it). Not strictly xDS but lives on `XdsState`.

### 2.2 Apply/diff ("dirty-marking")

`apply_built_resources(type_url, Vec<BuiltResource>)` is the single mutation entry point. It is **full-snapshot-per-type** semantics:

1. Compute `removed` = cached names not present in the incoming set.
2. Compute `pending_updates` = incoming resources whose `Any` body differs byte-wise from the cached body (or are new).
3. If both empty → return `None` (no version bump, no broadcast).
4. Else `version.fetch_add(1)`, update cache entries (each updated entry stamped with the new version), broadcast one `ResourceUpdate` containing a single delta for that type_url.

Version is therefore a **global monotonically increasing counter shared across all resource types**, presented as a decimal string in `DiscoveryResponse.version_info` / `system_version_info`.

### 2.3 Rebuild triggers

Four independent watcher tasks are spawned when the DB-backed ADS service is constructed (`spawn_{cluster,route_config,listener,secret}_watcher`):

- Each polls every **500 ms**: `SELECT COUNT(*), MAX(updated_at)::TEXT FROM {table}` and compares against the previous `(count, max_updated_at)` tuple. First poll records baseline without refreshing (the initial refresh runs once at watcher start).
- On change → `refresh_{type}_from_repository()`:
  - load up to 1000 rows (`list(Some(1000), None)` — **no team filter**; the cache holds all teams' resources, filtering happens per-stream at response time),
  - run the type-specific build pipeline (§3) including filter injection,
  - call `apply_built_resources`.
- Refresh failures are logged and the loop continues.

Note: the broadcast cache is used (a) to detect change & wake streams, (b) for delta `removed` names, and (c) as the source of auto-created JWKS clusters in CDS responses. **Per-stream responses are otherwise rebuilt from the DB on every request** (§2.4), not served from the cache.

### 2.4 SOTW stream loop (`run_stream_loop_with_mtls`)

Per-stream state: `last_sent: Mutex<HashMap<type_url, LastDiscoverySnapshot{version, nonce, resource_names}>>`, `subscribed_types: Mutex<HashSet<type_url>>`, `team_for_stream` (pre-seeded with mTLS team), `node_for_stream`. Outbound mpsc channel capacity 100. Each incoming request is processed in a spawned task (concurrent).

Per request:
1. **Node stickiness**: Envoy sends node only on the first message (`set_node_on_first_message_only`); the loop stores the first node and re-injects it into subsequent node-less requests so scoping keeps working.
2. **Subscription change detection**: if `request.resource_names != last_snapshot.resource_names`, this is a subscription update, never an ACK — respond.
3. **ACK detection**: it is an ACK iff subscription unchanged AND `response_nonce` non-empty AND nonce == last nonce AND `version_info` == last version AND no `error_detail` AND last version == current global version. ACKs are skipped (no response). (The "last version == current version" clause means a request that ACKs a stale version while a newer one exists triggers a fresh push.)
4. **NACK** (`error_detail` present): log error event; persist a NACK event asynchronously (§2.7). Then, if the global version is unchanged since the NACKed send, **do not resend** (avoids a NACK retry storm with identical bad config); if the version has moved, respond with the new config.
5. Otherwise call the responder: build a fresh `DiscoveryResponse` (version = current global version string, nonce = new UUIDv4, resources = full team-scoped set for the type). Record `{version, nonce, resource_names}` in `last_sent`, send.

Push path: on each broadcast `ResourceUpdate`, for every delta whose type_url is in `subscribed_types`, spawn a task that synthesizes a `DiscoveryRequest { type_url, node: stored node, resource_names: last subscription }` and runs the same responder — so pushes are team-filtered and SDS pushes honor the subscription (B7). The tracker is updated with the new version/nonce, keeping the previous `resource_names`.

Broadcast lag (`RecvError::Lagged`) only logs a warning — skipped updates are *not* re-driven (see §8). Channel-closed ends the loop. Stream close decrements a per-team connection gauge.

### 2.5 Delta stream loop (`run_delta_loop_with_mtls`)

Simplified delta support (not true incremental):
- Initial request (empty `response_nonce`): record `type_url` in `pending_types` and `resource_names_subscribe` in `delta_subscriptions`; respond with **all** team-scoped resources of the type as `Resource{name, version, resource}` entries; `removed_resources` = request's `resource_names_unsubscribe`; nonce = UUIDv4; `system_version_info` = global version.
- Any request with non-empty `response_nonce` is treated as ACK (continue) or NACK (log + persist, **no** resend logic).
- Push: same as SOTW — re-run the responder with stored node + subscription per changed type. This means delta pushes resend the full set too; Envoy tolerates this (idempotent upserts) but it forfeits delta's bandwidth advantage (§8).

### 2.6 CP restart & Envoy reconnect

- **CP restart**: state is rebuilt from the database (watchers run an initial refresh at startup). Version restarts at 1. Envoy reconnects with its last-known `version_info`, which will not match any tracked snapshot (per-stream trackers are empty), so the request is treated as an initial request and a full snapshot is sent. Because the version counter is not persisted, version strings are not comparable across CP restarts — correctness relies on Envoy accepting any response with a fresh nonce.
- **Envoy reconnect**: a new stream gets fresh per-stream tracking; the first request per type always produces a full response. mTLS validation runs again at stream establishment (so cert revocation takes effect on reconnect, not mid-stream).

### 2.7 NACK persistence

`persist_nack_event` (async, spawned, never blocks the stream):
- Requires a stream team (warn + drop otherwise) and a NACK repository.
- Resolves team **name → team id** (the `xds_nack_events.team` column is an FK to `teams.id`; inserting names was bug fp-4g4).
- Resolves dataplane display name from node id (`team=…/dp-{uuid}` → DB name lookup).
- Row fields: `team` (id), `dataplane_name`, `type_url`, `version_rejected` (last sent version or "unknown"), `nonce`, `error_code` (gRPC code from `error_detail`), `error_message`, `node_id`, `resource_names` (JSON array — from request resource_names, else regex-ish extraction of `name: "…"` patterns from the error message), `source = 'stream'`, `dedup_hash = NULL`.
- The same table also stores agent warming reports with `source = 'warming_report'` (§6).

---

## 3. Resource generation, per type

*Sources: `src/xds/resources.rs`, `cluster_spec.rs`, `listener.rs`, `route.rs`, `secret.rs`.*

All stored resource configurations are JSON in the DB (`{clusters,route_configs,listeners,secrets}.configuration`). Before deserialization, `strip_gateway_tags` recursively removes every `flowplaneGateway` key (internal metadata must not reach Envoy). Each built resource is encoded to a `google.protobuf.Any` with type URLs:

```
CLUSTER_TYPE_URL  = type.googleapis.com/envoy.config.cluster.v3.Cluster
ROUTE_TYPE_URL    = type.googleapis.com/envoy.config.route.v3.RouteConfiguration
LISTENER_TYPE_URL = type.googleapis.com/envoy.config.listener.v3.Listener
ENDPOINT          = type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment
SECRET_TYPE_URL   = type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.Secret
```

### 3.1 CDS — `ClusterSpec` → `envoy.config.cluster.v3.Cluster`

Domain model (`cluster_spec.rs`, serde camelCase with snake_case aliases):

```
ClusterSpec {
  connectTimeoutSeconds: Option<u64>,            // default 5
  endpoints: Vec<EndpointSpec>,                  // required, non-empty (validated)
  useTls: Option<bool>, tlsServerName: Option<String>,
  dnsLookupFamily: Option<String>,               // AUTO | V4_ONLY | V6_ONLY | V4_PREFERRED | ALL (unknown → AUTO + warn)
  lbPolicy: Option<String>,                      // ROUND_ROBIN(default) | LEAST_REQUEST | RING_HASH | MAGLEV | RANDOM | CLUSTER_PROVIDED
  leastRequest { choiceCount? }, ringHash { minimumRingSize?, maximumRingSize?, hashFunction? /*XX_HASH|MURMUR_HASH_2*/ }, maglev { tableSize? },
  circuitBreakers { default?: Thresholds, high?: Thresholds },   // Thresholds: maxConnections?, maxPendingRequests?, maxRequests?, maxRetries? (all u32)
  healthChecks: Vec<HealthCheckSpec>,            // tagged "type": http {path, host?, method?, intervalSeconds?, timeoutSeconds?, healthyThreshold?, unhealthyThreshold?, expectedStatuses?} | tcp {…same timing fields}
  outlierDetection { consecutive5xx?, intervalSeconds?, baseEjectionTimeSeconds?, maxEjectionPercent?, minHosts? },
  protocolType: Option<String>,                  // "HTTP2"|"GRPC" → HTTP/2 upstream
}
EndpointSpec = "host:port" string | { host, port }
```

Mapping (`cluster_from_spec`):
- `name` = DB row name; `connect_timeout` = spec value or 5 s.
- `load_assignment` = one `LocalityLbEndpoints` containing all endpoints as `LbEndpoint{Endpoint{SocketAddress{address, port_value, protocol: TCP}}}`. No endpoints ⇒ config error (resource skipped is **not** the behavior — the whole build errors; validation normally catches this at write time).
- **Discovery type**: any hostname endpoint ⇒ `LOGICAL_DNS` if ≤1 endpoint else `STRICT_DNS`; all-IP ⇒ `STATIC`. `dns_lookup_family` only set for hostname clusters.
- **LB**: mapped enum + optional `LeastRequestLbConfig{choice_count}`, `RingHashLbConfig{minimum/maximum_ring_size, hash_function}`, `MaglevLbConfig{table_size}`. Unknown policy ⇒ ROUND_ROBIN + warn.
- **Upstream TLS**: enabled when `useTls == true` **or any endpoint port == 443** (implicit). `transport_socket = envoy.transport_sockets.tls` with `UpstreamTlsContext{ common_tls_context: default, sni }`. SNI = `tlsServerName` (trimmed) else first hostname endpoint else first hostname-with-port-443; warn if TLS on hostname cluster with no SNI. No client cert / validation context is attached (system trust).
- **Circuit breakers**: thresholds at `DEFAULT` and `HIGH` routing priorities; only emitted if at least one threshold set.
- **Health checks**: per check — `timeout` default 5 s, `interval` default 10 s, healthy/unhealthy thresholds as given. HTTP: `HttpHealthCheck{host?, path, method?, expected_statuses: [Int64Range{start: code, end: code+1}]}`. TCP: empty `TcpHealthCheck`.
- **Outlier detection**: `consecutive_5xx`, `interval`, `base_ejection_time`, `max_ejection_percent`, `success_rate_minimum_hosts` ← `minHosts`.
- **HTTP/2**: when `protocolType ∈ {HTTP2, GRPC}` (case-insensitive), set `typed_extension_protocol_options["envoy.extensions.upstreams.http.v3.HttpProtocolOptions"]` = `HttpProtocolOptions{ explicit_http_config { http2_protocol_options {} } }` (the modern replacement for deprecated `http2_protocol_options`).

### 3.2 Cluster endpoint sync (DB derived table)

`src/services/cluster_endpoint_sync.rs` keeps a normalized `cluster_endpoints` table in sync with each cluster's configuration JSON (for UI/EDS-future use). On every cluster write, `sync(cluster_id, config_json)` parses any of three formats — Flowplane `{"endpoints":[...]}`, raw Envoy `{"load_assignment":{...}}` (reads per-locality `priority`, per-endpoint `load_balancing_weight.value`, `health_status` HEALTHY/UNHEALTHY/DEGRADED, `metadata`), legacy `{"hosts":[{socket_address}]}` — keys rows by `(address, port)`, and creates/updates/deletes diffs. Defaults: weight 1, priority 0, health Unknown. Invalid port/empty address entries are silently skipped.

### 3.3 Built-in CP clusters (always appended to CDS)

Appended to every CDS response and every cache refresh, regardless of team:

| Name | Target | Notes |
|---|---|---|
| `flowplane_ext_proc_service` | advertise-host (or 127.0.0.1) : xDS port | STATIC for IPs / LOGICAL_DNS+V4_ONLY for hostnames; HTTP/2 typed protocol options; connect_timeout 5 s |
| `flowplane_access_log_service` | same | same shape |
| `rate_limit_cluster` | `FLOWPLANE_RLS_GRPC_URL` (`host:port`) — only emitted when set | STRICT_DNS for hostnames (DNS-RR rebalancing), STATIC for IPs; HTTP/2 |

If `DataplaneTlsConfig` is set, each gets `transport_socket = UpstreamTlsContext{ tls_certificates:[{certificate_chain: filename(cert), private_key: filename(key)}], validation_context{trusted_ca: filename(ca)}, sni: hostname-if-DNS }` — paths are in-container paths read by Envoy itself.

Additionally, CDS responses include any cached cluster whose name contains `"jwks"` (auto-created JWKS clusters, §4.4) that isn't already present. JWKS clusters (`create_jwks_cluster`): parse the JWKS URI; LOGICAL_DNS, `V4_PREFERRED`, connect 5 s, port = URI port or 443/80 by scheme; HTTPS adds `UpstreamTlsContext{sni: host}`.

### 3.4 RDS — `RouteConfig` → `envoy.config.route.v3.RouteConfiguration`

Domain model (`src/xds/route.rs`):

```
RouteConfig { name, virtual_hosts: [VirtualHostConfig] }
VirtualHostConfig { name, domains: [String], routes: [RouteRule],
                    typed_per_filter_config: Map<String, HttpScopedConfig> (default {}),
                    rate_limits: [RateLimitConfig] }
RouteRule { name?, match: RouteMatchConfig, action: RouteActionConfig,
            typed_per_filter_config: Map<String, HttpScopedConfig> }
RouteMatchConfig { path: PathMatch, headers?: [HeaderMatchConfig], query_parameters?: [QueryParameterMatchConfig] }
PathMatch = Exact(s) | Prefix(s) | Regex(s) | Template(s)
HeaderMatchConfig { name, value?, regex?, present? }
RouteActionConfig =
  Cluster { name, timeout?(s), prefix_rewrite?, path_template_rewrite?, retry_policy?, rate_limits: [] }
| WeightedClusters { clusters: [{name, weight, typed_per_filter_config}], total_weight?, rate_limits: [] }
| Redirect { host_redirect?, path_redirect?, response_code? }
RetryPolicyConfig { num_retries?, retry_on: [String], per_try_timeout_seconds?, base_interval_ms?, max_interval_ms? }
RateLimitConfig { stage?(0-10, default omitted), disable_key?, actions: [RateLimitActionConfig] }
RateLimitActionConfig (tag "type", snake_case) =
  request_headers { header_name, descriptor_key, skip_if_absent(bool, default false) }
| generic_key { descriptor_value, descriptor_key? (Envoy default "generic_key") }
```

Envoy mapping:
- Path: Exact→`path`, Prefix→`prefix`, Regex→`safe_regex` (engine default), Template→`path_match_policy` = `TypedExtensionConfig{name:"envoy.path.match.uri_template", UriTemplateMatchConfig{path_template}}`.
- Headers: value→StringMatch Exact; regex→StringMatch SafeRegex; present:true→`present_match: true`; otherwise matcher with no specifier. **`query_parameters` are parsed but never converted** (v1 gap — see §8).
- Cluster action: `cluster`, `timeout` (Duration), `prefix_rewrite`, `path_rewrite_policy` = `TypedExtensionConfig{name:"envoy.path.rewrite.uri_template", UriTemplateRewriteConfig}`, `rate_limits`.
- Retry policy: `num_retries`, `retry_on` joined with `,`, `per_try_timeout`, `retry_back_off{ base_interval: default 100 ms, max_interval: default 1000 ms }` (ms→Duration conversions; base interval written into `nanos` only — breaks for values ≥ 2 s, see §8).
- WeightedClusters: `ClusterWeight{name, weight, typed_per_filter_config}` + optional `total_weight`.
- Redirect: `host_redirect`, `path_redirect`, `response_code` (default `MOVED_PERMANENTLY` / invalid codes coerced to it).
- `typed_per_filter_config` maps are converted entry-by-entry via `HttpScopedConfig::to_any()` (§4.3) onto VirtualHost, Route, and ClusterWeight.
- VirtualHost/RouteAction `rate_limits` → `envoy.config.route.v3.RateLimit{stage?, disable_key, actions}` with `RequestHeaders`/`GenericKey` action specifiers.

Before building, attached filters are injected into the route-config JSON (§4.5). Listener↔route binding rows (`listener_route_configs`) are maintained by `src/services/listener_route_config_sync.rs`: on each listener write, delete all rows for the listener, then re-insert one row per RDS `route_config_name` found in the listener config (Flowplane format path `filter_chains[].filters[].filter_type.HttpConnectionManager.route_config_name`, raw-Envoy format `…http_connection_manager.typed_config.rds.route_config_name`, or top-level `route_config_name`), deduplicated, ordered by appearance; names that don't resolve to a route-config row are skipped with a debug log (eventual consistency).

### 3.5 LDS — `ListenerConfig` → `envoy.config.listener.v3.Listener`

Domain model (`src/xds/listener.rs`):

```
ListenerConfig { name, address, port, filter_chains: [FilterChainConfig] }
FilterChainConfig { name?, filters: [FilterConfig], tls_context?: TlsContextConfig }
FilterConfig { name, filter_type: FilterType }
FilterType = HttpConnectionManager { route_config_name?, inline_route_config?, access_log?, tracing?, http_filters: [HttpFilterConfigEntry] }
           | TcpProxy { cluster, access_log? }
TlsContextConfig { cert_chain_file?, private_key_file?, ca_cert_file?, require_client_certificate?,
                   tls_certificate_sds_secret_name?, validation_context_sds_secret_name? }
AccessLogConfig { path?, format? }
TracingConfig { provider: OpenTelemetry{service_name, grpc_cluster?, http_cluster?, http_path? (default "/v1/traces"), max_cache_size?}
                        | Zipkin{collector_cluster, collector_endpoint, trace_id_128bit, shared_span_context?, collector_endpoint_version (http_json default | http_proto), collector_hostname?}
                        | Generic{name, config: map<string,string>},
                random_sampling_percentage? (0..=100, validated), spawn_upstream_span?, custom_tags: map }
```

Mapping:
- `address` → `SocketAddress{address, port_value}`.
- HCM filter (`canonical name envoy.filters.network.http_connection_manager`, names normalized regardless of stored name):
  - `route_specifier`: RDS with `config_source = ADS` + `route_config_name`, or inline `RouteConfiguration`; one of the two is required (config error otherwise).
  - `codec_type: AUTO`, `stat_prefix: "ingress_http"`.
  - `http_filters` = `build_http_filters(entries)` (§4.2) — router always appended last.
  - `access_log`: optional `envoy.access_loggers.file` `FileAccessLog{path, optional text-format SubstitutionFormatString}` (path required).
  - `tracing`: provider per above; OTel requires grpc_cluster or http_cluster; Generic wraps a `google.protobuf.Struct` of string fields; custom tags become Literal `CustomTag`s.
  - `generate_request_id: true` and `always_set_request_id_in_response: true` — **always set**, required so ALS entries and ExtProc body captures correlate via `x-request-id`.
- TcpProxy filter (`envoy.filters.network.tcp_proxy`): `cluster`, `stat_prefix: "ingress_tcp"`, optional file access log.
- **Downstream TLS** (`build_transport_socket`): `transport_socket = envoy.transport_sockets.tls` / `DownstreamTlsContext`:
  - cert source: either inline file paths (`cert_chain_file` + `private_key_file`, both required together) **or** `tls_certificate_sds_secret_configs = [SdsSecretConfig{name, sds_config: ADS}]` — SDS name wins if present;
  - validation: `ca_cert_file` (inline) XOR `validation_context_sds_secret_name` (SDS via ADS); both set ⇒ config error; neither ⇒ none;
  - `require_client_certificate` passthrough.

After building, four injection passes mutate the encoded listener protobufs (order matters; §4.5–4.7): listener-attached filters, learning-session access logs, learning-session ExtProc, agent RBAC. These run both on cache refresh and on every per-stream LDS response.

### 3.6 EDS

`endpoints_from_config` only: a single `ClusterLoadAssignment` for the static config's cluster/backend (Phase A). DB-driven EDS does not exist in v1; clusters carry inline `load_assignment` instead. (v2 likely wants real EDS — note for spec/08.)

### 3.7 SDS — `SecretSpec` → `envoy…tls.v3.Secret`

Secrets are stored encrypted (AES via `FLOWPLANE_SECRET_ENCRYPTION_KEY`; unset ⇒ SDS disabled, secret create returns 503). Reference-based secrets carry `backend` (`vault`/`aws`/`gcp`) + `reference`; at build time the registry fetches the live value and overwrites `configuration` with the spec JSON (fetch failures: warn + skip that secret).

`SecretSpec` variants and mapping (`src/xds/secret.rs`); a DB-type vs config-type mismatch is a config error:

| Variant | Envoy `secret.Type` | Mapping |
|---|---|---|
| GenericSecret `{secret: base64}` | `GenericSecret` | base64-decode → `inline_bytes` |
| TlsCertificate `{certificate_chain, private_key, password?, ocsp_staple?(base64)}` | `TlsCertificate` | PEM strings → `inline_string`; staple decoded → `inline_bytes` |
| CertificateValidationContext `{trusted_ca, match_subject_alt_names: [StringMatcher], crl?, only_verify_leaf_cert_crl}` | `ValidationContext` | StringMatcher = Exact/Prefix/Suffix/SafeRegex/Contains (ignore_case=false) |
| SessionTicketKeys `{keys: [{name, key: base64}]}` | `TlsSessionTicketKeys` | each key must decode to exactly **80 bytes** |

Per-secret build failures skip that secret with a warning (other secrets still served). Delivery: only via subscription-filtered SDS (§1.5); referenced from listeners (§3.5) and filters (ext_authz `auth_secret`/`tls_secret`, oauth2 `token_secret`/`hmac_secret`, credential_injector `secret_ref`) through `SdsSecretConfig{name, sds_config: ADS}`.

---

## 4. HTTP filter subsystem

*Sources: `src/xds/filters/**`, `filter-schemas/built-in/*.yaml`, `docs/reference/filters.md`.*

### 4.1 Catalog

16 filter types (15 user-facing + router). Identifier = Flowplane `filter_type` string; each has a YAML schema in `filter-schemas/built-in/` driving validation, attachment points, and dynamic conversion.

| Identifier | Envoy filter name | Listener type URL (`type.googleapis.com/…`) | Per-route |
|---|---|---|---|
| `cors` | `envoy.filters.http.cors` | `…cors.v3.Cors` (empty marker in chain) | full — `…cors.v3.CorsPolicy` |
| `jwt_auth` | `envoy.filters.http.jwt_authn` | `…jwt_authn.v3.JwtAuthentication` | reference-only — `…jwt_authn.v3.PerRouteConfig` (`requirement_name` or `disabled:true`) |
| `local_rate_limit` | `envoy.filters.http.local_ratelimit` | `…local_ratelimit.v3.LocalRateLimit` | full (same type URL) |
| `rate_limit` | `envoy.filters.http.ratelimit` | `…ratelimit.v3.RateLimit` | full — `…ratelimit.v3.RateLimitPerRoute` (`domain?`, `include_vh_rate_limits` default true) |
| `rate_limit_quota` | `envoy.filters.http.rate_limit_quota` | `…rate_limit_quota.v3.RateLimitQuotaFilterConfig` | full — `…RateLimitQuotaOverride` (domain) |
| `header_mutation` | `envoy.filters.http.header_mutation` | `…header_mutation.v3.HeaderMutation` | full — `…HeaderMutationPerRoute` |
| `health_check` | `envoy.filters.http.health_check` | `…health_check.v3.HealthCheck` | not supported (listener only) |
| `compressor` | `envoy.filters.http.compressor` | `…compressor.v3.Compressor` | disable-only — `…CompressorPerRoute` |
| `ext_authz` | `envoy.filters.http.ext_authz` | `…ext_authz.v3.ExtAuthz` | full — `…ExtAuthzPerRoute` |
| `ext_proc` | `envoy.filters.http.ext_proc` | `…ext_proc.v3.ExternalProcessor` | full — `…ExtProcPerRoute` (`disabled` XOR `overrides`) |
| `rbac` | `envoy.filters.http.rbac` | `…rbac.v3.RBAC` | full — `…RBACPerRoute` |
| `oauth2` | `envoy.filters.http.oauth2` | `…oauth2.v3.OAuth2` | **none** — Envoy NACKs per-route oauth2; use `pass_through_matcher` |
| `credential_injector` | `envoy.filters.http.credential_injector` | `…credential_injector.v3.CredentialInjector` | not supported |
| `custom_response` | `envoy.filters.http.custom_response` | `…custom_response.v3.CustomResponse` | full (same type URL) |
| `mcp` | `envoy.filters.http.mcp` | `…mcp.v3.Mcp` | disable-only — per-route proto with PassThrough mode |
| `wasm` | (custom name) | `…wasm.v3.Wasm` | full (same type) — but v1 per-route wasm returns None (unimplemented, fp-vzc7.1) |
| router | `envoy.filters.http.router` | `…router.v3.Router` | n/a |

Key config schemas (fields beyond Envoy defaults; full serde structs mirror Envoy semantics):

- **cors**: `allow_origin: [Exact|Prefix|Suffix|Contains|Regex]` (required non-empty), `allow_methods`/`allow_headers`/`expose_headers`, `max_age` (≤ 315,576,000,000 s), `allow_credentials` (forbidden with wildcard origin), `filter_enabled`/`shadow_enabled` (runtime fractional percent: `{runtime_key?, numerator, denominator: Hundred|TenThousand|Million}`), `allow_private_network_access`, `forward_not_matching_preflights`. Listener chain gets the empty `Cors{}` marker; the policy lives in per-route/vhost `typed_per_filter_config`.
- **jwt_auth**: `providers: map<name, {issuer?, audiences, jwks: Remote{http_uri{uri, cluster, timeout_ms (default 5000)}, cache_duration?…} | Local{inline_string|filename…}, clock_skew_seconds (default 60), claim_to_headers: [{claim_name, header_name}], payload_in_metadata?, require_expiration?, max_lifetime_seconds?}>` (non-empty required); `rules`, `requirement_map: map<name, Requirement>`, requirement kinds: `ProviderName | ProviderWithAudiences | RequiresAny | RequiresAll | AllowMissing | AllowMissingOrFailed`; `bypass_cors_preflight`, `strip_failure_response`, `stat_prefix`. If no rules and no filter_state_rules → auto rule matching all paths requiring any provider.
- **local_rate_limit**: `stat_prefix` (required), `token_bucket {max_tokens, tokens_per_fill (default max_tokens), fill_interval_ms > 0}` (required at to_any time), `status_code` (clamped 400–599), `filter_enabled`/`filter_enforced` (default 100%/100% when omitted), `per_downstream_connection`, `rate_limited_as_resource_exhausted`, `max_dynamic_descriptors`, `always_consume_default_token_bucket`.
- **rate_limit** (global RLS): `domain` (required), `rate_limit_service {cluster_name (required — normally `rate_limit_cluster`), authority?}`, `timeout_ms` default **20**, `failure_mode_deny` default false (fail-open), `enable_x_ratelimit_headers: Off|DraftVersion03`, `disable_x_envoy_ratelimited_header`, `rate_limited_status` (400–599), `stat_prefix`. The CP→RLS admin sync (policies/domains/identities pushed over HTTP with 60 s reconcile, deterministic SHA-256-derived org/team UUIDs, namespace `{org_id}|{team_id}|{domain}`) lives in `src/services/rls_translator.rs` and belongs to the rate-limit spec; xDS's part is the `rate_limit_cluster` (§3.3) and these filter configs.
- **rate_limit_quota**: `domain` (required), `rlqs_server {cluster, authority?}`. Flowplane synthesizes a default catch-all `bucket_matchers` (on_no_match bucket, `reporting_interval: 60s`) because Envoy requires the field.
- **header_mutation**: `request/response_headers_to_add: [{key (non-empty), value, append(bool, default false)}]`, `request/response_headers_to_remove`.
- **health_check**: `pass_through_mode` (default false), `cache_time_ms?`, `endpoint_path` (required, must start with `/`; converted to a `:path` exact header matcher).
- **compressor**: `compressor_library: Gzip {memory_level 1–9, window_bits 9–15, compression_level: BestSpeed|BestCompression|DefaultCompression, compression_strategy: DefaultStrategy|Filtered|HuffmanOnly|Rle|Fixed, chunk_size?}`, response-direction config.
- **ext_authz**: oneof `service`: `Grpc{target_uri (cluster), timeout_ms default 200, initial_metadata}` or `Http{server_uri{uri, cluster, timeout_ms default 200}, path_prefix, headers_to_add, authorization_request?, authorization_response?}`; `failure_mode_allow` default false, `with_request_body?`, `clear_route_cache`, `status_on_error?`, `include_peer_certificate`, `auth_secret?`/`tls_secret?` → SDS refs.
- **ext_proc**: `grpc_service {target_uri (cluster, non-empty, no whitespace), timeout_seconds default 20 (>0)}`, `failure_mode_allow` default false, `processing_mode {request/response_header_mode: SEND|SKIP, request/response_body_mode: STREAMED|BUFFERED|BUFFERED_PARTIAL|FULL_DUPLEX_STREAMED, request/response_trailer_mode: SEND|SKIP}`, `message_timeout_ms?`, `request/response_attributes`. Per-route: `disabled` XOR `overrides{…all fields}`.
- **rbac**: `action: Allow|Deny|Log`, `policies: map<name, {permissions, principals}>`. Permissions: `Any|Header{name, exact/prefix/suffix/present}|UrlPath{path, ignore_case}|DestinationPort|Metadata|AndRules|OrRules|NotRule`. Principals: `Any|Authenticated{principal_name?}|SourceIp|DirectRemoteIp|Header|AndIds|OrIds|NotId`.
- **oauth2**: `token_endpoint {uri, cluster, timeout_ms default 5000}`, `authorization_endpoint`, `credentials {client_id, token_secret? (SDS), hmac_secret (SDS, required, 32-byte key), cookie_domain?, cookie_names?}`, `redirect_uri`, `redirect_path` default `/oauth2/callback`, `signout_path?`, `auth_scopes` default `["openid"]`, `auth_type: UrlEncodedBody|BasicAuth`, `forward_bearer_token`, `preserve_authorization_header`, `use_refresh_token`, `default_expires_in_seconds?`, `pass_through_matcher: [{path_exact|path_prefix|path_regex|header_name+header_value}]`, `stat_prefix`.
- **credential_injector**: `overwrite`, `allow_request_without_credential` (default false → 401), and either `secret_ref {name (SDS secret), header default "Authorization", header_prefix?}` (preferred; builds `…injected_credentials.generic.v3.Generic` with an SDS ref) or inline `credential {name, config: TypedConfig}`.
- **custom_response**: `matchers: [{status_code: Exact{code}|Range{min,max}|List{codes} (100–599), response: {status_code?, body? (non-empty), headers}}]` (builds an Envoy matcher tree) XOR legacy `custom_response_matcher` (base64 protobuf).
- **mcp** (MCP traffic-validation filter): `traffic_mode: PassThrough (default) | RejectNoMcp`. In `RejectNoMcp`, Envoy's MCP filter rejects any request that is not MCP-shaped: only `POST` carrying JSON-RPC 2.0 bodies and `GET` with `Accept: text/event-stream` (SSE) pass; everything else is rejected. Per-route override is disable-only (per-route proto always encodes PassThrough; `disabled:true` turns the filter off for the route). This is the enforcement point for "MCP-only" listeners fronting AI tool traffic.
- **wasm**: `name`, `root_id`, `vm_config {vm_id, runtime default "envoy.wasm.runtime.wamr" (allowed: v8|wasmtime|wamr|null — validated pre-persist so it's a 4xx not a NACK), code: local{filename|inline_bytes(base64)|inline_string} XOR remote{http_uri{uri, cluster, timeout default "30s"}, sha256?}, configuration (JSON), allow_precompiled, nack_on_code_cache_miss}`, plugin `configuration` (JSON), `failure_policy: FAIL_OPEN|FAIL_CLOSED|FAIL_RELOAD`. User-uploaded WASM is exposed as filter type `custom_wasm_{id}`; at injection time the binary is fetched from `custom_wasm_filters` and rewritten to a standard `wasm` config with `inline_bytes`.

### 4.2 Filter chain assembly & ordering

`HttpFilterKind` (serde tag `type`, snake_case) wraps each config; `HttpFilterConfigEntry { name?, is_optional, disabled, filter }` is the chain entry. `build_http_filters(entries)`:
1. Collect non-router filters in declared order.
2. At most one explicit Router entry (error otherwise); router is appended **last** — explicit or a default `Router{}` (`type.googleapis.com/envoy.extensions.filters.http.router.v3.Router`, `is_optional:false, disabled:false`).

### 4.3 Per-route scoped config (`HttpScopedConfig`)

Internally-tagged enum (`filter_type` discriminator) stored in route JSON `typed_per_filter_config` maps:
`Compressor | LocalRateLimit | Cors | HeaderMutation | RateLimit | RateLimitQuota | CustomResponse | Mcp | JwtAuthn | Rbac | ExtProc | Typed(TypedConfig) | Custom(Any)`.
`TypedConfig = { type_url, value: base64 }` (the JSON-safe `Any`). `to_any()` produces the per-route proto; `from_any()` decodes by type URL. OAuth2 deliberately absent.

### 4.4 Listener-level injection (protobuf surgery)

`inject_listener_filters` (`filters/injection/listener.rs`) runs on every LDS build:
1. For each built listener, load filters attached to the listener (`filters` × `listener_filters` junction) **plus** filters attached to any route config the listener references (route config names read out of the decoded HCM); dedupe by filter id.
2. Resolve `custom_wasm_*` types to inline-bytes wasm configs.
3. Convert each filter: try the strongly-typed `FilterConfig` conversion for known types; unknown types fall back to `DynamicFilterConverter`, which wraps the JSON config as a `google.protobuf.Struct` encoded under the schema's `envoy.type_url` (works only for filters that accept Struct — a known sharp edge). Config JSON may be wrapped (`{"type":…, "config":{…}}`) or bare. Filters whose schema lacks the `listener` attachment point are skipped here.
4. **JWT special-case**: all `jwt_auth` filters are merged into one `JwtAuthentication` via `JwtConfigMerger` — providers merged by name (later wins + warn), rules appended, requirement_map merged (later wins), boolean flags OR'd, last non-empty stat_prefix; if the merged requirement_map is empty it is auto-populated with one `ProviderName` entry per provider. Remote-JWKS providers trigger `create_jwks_cluster` registrations (cached into CDS).
5. Inject via `ListenerModifier` (decode Listener → for each HCM in each filter chain → mutate → re-encode): JWT uses replace-or-add; everything else `add_filter_before_router` (insert just before `envoy.filters.http.router`, or append if no router).

Conversion/injection failures are warn-and-skip per filter; listener decode failures skip the listener.

### 4.5 Route-level injection (JSON surgery, 3-level hierarchy)

`inject_route_filters_hierarchical` (`filters/injection/route.rs`), run before RDS build:
- Levels: (1) route-config-attached filters (base, no per-scope settings) → all routes; (2) virtual-host-attached → routes of that vhost; (3) route-attached → that route. Effective filters keyed by `filter_type`; more specific replaces less specific.
- VH/route attachments carry `PerScopeSettings { behavior: "use_base" | "disable" | "override", config?, requirementName? }`:
  - `use_base`/none → convert the filter's base config to its per-route form;
  - `override` → convert `{type, config: settings.config}` instead;
  - `disable` → emit a disable scoped config (only jwt_auth → `{disabled:true}`, compressor, mcp, rbac; other types are skipped).
- Per-route conversion: typed `FilterConfig::to_per_route_config()` first (per-type behavior in §4.1 — e.g. jwt_auth emits `RequirementName{first provider}`; mcp/compressor/oauth2/ext_authz/rbac return None at this layer; ext_proc wraps as overrides; rate_limit emits `{domain, include_vh_rate_limits:true}`), falling back to the dynamic converter (`schema.envoy.per_route_type_url` + Struct payload wrapped as `HttpScopedConfig::Typed`).
- Result entries are inserted into each route's `typed_per_filter_config` JSON object (preserving existing keys), then the route config JSON is re-serialized. A simple non-hierarchical fallback (route-config level only) exists when hierarchy repos are absent.

### 4.6 Learning-session injection

When learning sessions are active (`LearningSessionService.list_active_sessions()`), every LDS build also:
- **Access log**: appends to each HCM an `envoy.access_loggers.http_grpc` `HttpGrpcAccessLogConfig` with `log_name = flowplane_learning_session_{session_id}`, `grpc_service = EnvoyGrpc{cluster: "flowplane_access_log_service"}`, `transport_api_version: V3`, `buffer_size_bytes: 16384`, `additional_request_headers_to_log = [content-type, content-length, accept, user-agent, authorization, proxy-authorization, x-api-key, x-auth-token, x-request-id, x-envoy-original-path]`, `additional_response_headers_to_log = [content-type, content-length, www-authenticate]`. Dedupe: skip if an existing access log name contains the session id.
- **ExtProc**: inserts before router an HTTP filter named `envoy.filters.http.ext_proc.session_{session_id}` (`is_optional: true` → fail-open) with `ExternalProcessor{ grpc_service: EnvoyGrpc("flowplane_ext_proc_service"), timeout 5 s, failure_mode_allow: true, processing_mode { request/response headers SEND, request/response body BUFFERED, trailers SKIP }, message_timeout 5000 ms }`. Dedupe by name-contains-session-id.

### 4.7 Agent RBAC injection

For listeners whose routes have active external route grants (`grants` rows of `grant_type='route'` joined through users/routes/vhosts/route_configs/listener_route_configs, `exposure='external'`, unexpired), inject an `envoy.filters.http.rbac` filter before the router: one ALLOW policy per grant named `agent-{agent_id}-route-{route_id}` with principal = exact `x-flowplane-sub` header (the JWT `sub` claim forwarded by a JWT provider configured with `claim_to_headers: sub → x-flowplane-sub`) and permission = `UrlPath(route path)` AND (OR over `:method` exact matches) when methods are restricted. A companion Lua sanitizer filter (`envoy.filters.http.lua.sub-sanitizer`, inline `handle:headers():remove("x-flowplane-sub")`) is defined to run before JWT so clients can't spoof the header.

---

## 5. Access Log Service & ExtProc (learning data path)

*Sources: `src/xds/services/{access_log_service.rs,ext_proc_service.rs}`. Downstream processing (schema inference, OpenAPI/MCP generation) is spec/06; the interface points are below.*

### 5.1 ALS (`FlowplaneAccessLogService`)

- Envoy sends `StreamAccessLogsMessage` over the injected HTTP-gRPC access logger (§4.6). The service holds `Arc<RwLock<Vec<LearningSession{id, team, route_patterns: [Regex], methods?}>>>` (add/remove driven by the learning-session service) and an **unbounded** mpsc of `ProcessedLogEntry` consumed by the learning pipeline (`access_log_processor`, spec/06).
- Per HTTP log entry: map Envoy `RequestMethod` enum (1=GET … 9=PATCH) to a string; path = `x-envoy-original-path` header if present else `request.path` (rewritten); match path+method against sessions (first match wins; id+team captured atomically).
- On match, build `ProcessedLogEntry { session_id, request_id (x-request-id), team, method, path, request_headers, request_body: None, request_body_size, response_status, response_headers, response_body: None, response_body_size, start_time_seconds, duration_ms (time_to_last_downstream_tx_byte), trace_context }` and queue it; then increment the session's persisted `sample_count`.
- Header hygiene (applies to stored headers): drop infrastructure headers (prefixes `x-envoy-`, `x-forwarded-`, `x-b3-`, `x-trace-`, `x-amzn-`, `x-request-id`; exact `server,date,connection,transfer-encoding,via,keep-alive,traceparent,tracestate,content-length`), cap at 20 headers, redact sensitive values (`authorization`/`proxy-authorization` keep scheme → `"Bearer ***"`; `cookie,set-cookie,x-api-key,x-auth-token,x-csrf-token,x-session-id` → `"***"`).
- W3C `traceparent` is parsed (`00-{32hex}-{16hex}-{2hex}`) into a `TraceContext` for correlation; `tracestate` carried along.
- **Bodies never arrive via ALS** (Envoy HTTP log protos carry sizes only) — bodies come from ExtProc and are merged by `(session_id, x-request-id)` downstream.
- TCP logs are ignored. Response is an empty `StreamAccessLogsResponse` at stream end. No authentication is performed on this service beyond transport mTLS (see §8).

### 5.2 ExtProc (`FlowplaneExtProcService`)

- Holds `HashMap<session_id, route_pattern: Regex>` and an unbounded mpsc of `CapturedBody { session_id, request_id, request_body?, response_body?, request_truncated, response_truncated }`.
- Stream state machine per request: on RequestHeaders extract `:path` and `x-request-id`; match path against sessions; always reply CONTINUE (fail-open). If matched, accumulate request body chunks; on ResponseBody end_of_stream, truncate both bodies to **10 KB** (`MAX_BODY_SIZE`), emit `CapturedBody` if both session_id and request_id are known, reset state.
- Design split: ALS = metadata + sample counting; ExtProc = bodies only (avoids double counting).

---

## 6. Diagnostics service & flowplane-agent

*Sources: `proto/flowplane/diagnostics/v1/diagnostics.proto`, `src/xds/services/diagnostics_service.rs`, `crates/flowplane-agent/`.*

### 6.1 Protocol (`flowplane.diagnostics.v1`)

One bidi RPC: `EnvoyDiagnosticsService.ReportDiagnostics(stream DiagnosticsReport) returns (stream Ack)`.

```
DiagnosticsReport { schema_version: u32 (=1), report_id: string (opaque unique),
                    dataplane_id: string, observed_at: Timestamp,
                    oneof payload { ListenerStateReport listener_state = 10;
                                    HeartbeatReport heartbeat = 20; } }   // wide ranges reserved
ListenerStateReport { resource_type: ResourceType, resource_name: string,
                      error_details: string,            // verbatim Envoy UpdateFailureState.details; empty = informational
                      last_update_attempt: Timestamp,   // unused by MVP agent
                      failed_config_hash: string }      // optional agent-side SHA256 for dedup
ResourceType { UNSPECIFIED=0, LISTENER=1, CLUSTER=2, ROUTE_CONFIG=3, SECRET=4 }
HeartbeatReport {}                                       // intentionally empty
Ack { report_id: repeated string, status: AckStatus, message: string }
AckStatus { UNSPECIFIED=0, OK=1, UNKNOWN_PAYLOAD=2, INVALID=3, RETRY=4, UNAUTHORIZED=5 }
```

### 6.2 Server behavior

- mTLS SPIFFE identity required (`UNAUTHENTICATED` otherwise); envelope `dataplane_id` **must equal** the cert's `proxy_id` → else per-report `UNAUTHORIZED`. Envelope validation: non-empty `dataplane_id`/`report_id`, `schema_version > 0` → else `INVALID`. Empty payload → `INVALID`.
- Every recognized payload (listener state or heartbeat) touches liveness: `UPDATE dataplanes SET last_config_verify = NOW() … WHERE dataplanes.name = $dataplane AND team matches cert team` (best-effort; 0 rows = warn). Malformed envelopes never touch it.
- `ListenerStateReport` with empty `error_details` is informational — acked OK, not persisted. With error: map resource_type → type URL; dedup hash = agent-provided `failed_config_hash` or SHA256 over `(dataplane_id, resource_type_i32, resource_name, error_details)` NUL-separated; resolve team name → id (failure ⇒ `RETRY`); insert into `xds_nack_events` with `source='warming_report'`, `error_code=0`, `nonce/version_rejected=NULL`, `resource_names=["name"]`, `dedup_hash`. Unique-violation on the partial unique index over `dedup_hash` ⇒ idempotent OK; other DB errors ⇒ `RETRY`.
- Reports surface to users through the same NACK-events query surface as stream NACKs (API/UI lists per team/dataplane/type, distinguishing `stream` vs `warming_report`). This is how warming failures — which never appear as ACK/NACK on the xDS stream — become visible.

### 6.3 flowplane-agent (dataplane sidecar)

Config (clap + env): `FLOWPLANE_AGENT_ENVOY_ADMIN_URL` (default `http://127.0.0.1:9901`, warn if non-loopback), `FLOWPLANE_AGENT_CP_ENDPOINT` (required), `FLOWPLANE_AGENT_POLL_INTERVAL_SECS` (default 10, min 1), `FLOWPLANE_AGENT_DATAPLANE_ID` (required, must match cert proxy_id), `FLOWPLANE_AGENT_TLS_{CERT,KEY,CA}_PATH` (cert+key both-or-neither; missing pair = loud-warn plaintext fallback), `FLOWPLANE_AGENT_QUEUE_CAP` (default 256), `FLOWPLANE_AGENT_HEALTH_BIND_ADDR` (default `127.0.0.1:19902`).

Behavior:
- **Poll loop**: every interval, GET Envoy admin `/config_dump`; parse only `ListenersConfigDump.dynamic_listeners`, `ClustersConfigDump.dynamic_active_clusters + dynamic_warming_clusters`, `RoutesConfigDump.dynamic_route_configs`; collect entries with `error_state` (name resolution: top-level `name`, else nested `cluster.name`/`route_config.name`, else `"<unnamed>"`). Per-cycle dedup by SHA256(dataplane_id NUL kind NUL name NUL details) — `last_update_attempt` deliberately excluded so Envoy's own retries don't bust dedup. New errors → `DiagnosticsReport` (schema 1, UUIDv4 report_id, now) → bounded FIFO queue (overflow evicts oldest + warn).
- **First contact**: one-shot informational `ListenerStateReport` (empty fields) when the first error-free cycle completes. **Heartbeat**: `HeartbeatReport` every `max(3 × poll_interval, 30 s)` while error-free.
- **Stream loop**: connect to CP endpoint (HTTP/2 keepalive ping 2 s / timeout 4 s / while-idle), exponential backoff 500 ms→30 s (reset on connect); inner loop drains the queue every 100 ms into the request stream (buffer 32), logs inbound Acks, treats tx-closed/recv-error as disconnect.
- **Health**: `GET /healthz` → 200 if last successful admin poll within `max(2 × interval, 2 s)`, else 503 (`never polled` / `stale {age}`).
- Process exits non-zero if any task dies (supervisor restarts); SIGINT exits 0.

---

## 7. Envoy bootstrap contract

*Source: `envoy-config-default.yaml` (default template; `flowplane init` provisions the TLS-complete variant).*

A dataplane joins by booting Envoy with:

1. `node.id = team={team}/dp-{dataplane_uuid}`; `node.cluster` (e.g. `{team}-cluster`); `node.metadata` containing `team`, `dataplane_id`, `dataplane_name` (informational — never trusted for authz), optionally `listener_allowlist` (list of listener names → Allowlist scope), `gateway_host`.
2. `dynamic_resources`: `ads_config { api_type: GRPC, grpc_services: [{envoy_grpc: {cluster_name: xds_cluster}}], transport_api_version: V3 }`; `cds_config: {ads: {}}`; `lds_config: {ads: {}}`. RDS/EDS/SDS also flow over ADS (RDS config sources and `SdsSecretConfig.sds_config` generated by the CP all say ADS).
3. `static_resources.clusters`: `xds_cluster` (and in v1's template a same-target `sds_cluster`) → CP host : xDS port, `type: LOGICAL_DNS`, `dns_lookup_family: V4_ONLY`, HTTP/2 enabled, `connect_timeout: 1s`. **Production bootstraps must attach an `UpstreamTlsContext` transport socket with the dataplane's SPIFFE client cert/key and the CP CA** (mirroring §3.3's dynamic-cluster TLS block) — mTLS is mandatory; the checked-in default template omits it and only works against a dev PKI setup where `flowplane init` rewrites it.
4. Admin interface on a local port (default template 9902; agent default expects 9901) — loopback-only in production, consumed by flowplane-agent.
5. The client certificate must be issued by Flowplane (SPIFFE URI SAN embedding org/team/proxy_id) and registered in `proxy_certificates`; the bootstrap's team/dp-uuid should match the cert, since resource scoping follows the cert and node-id mismatches only produce attribution warnings.
6. The flowplane-agent sidecar runs alongside Envoy with `FLOWPLANE_AGENT_DATAPLANE_ID = {dataplane_uuid}` and the same cert material.

No listeners/clusters/routes beyond the xDS bootstrap cluster(s) are required — everything else is served dynamically. A team with zero listeners receives an empty LDS set by design.

---

## 8. Gaps and smells (input to spec/08)

1. **Polling, not eventing.** 4 × 500 ms `COUNT(*) + MAX(updated_at)` polls per CP replica; deletes that don't change `MAX(updated_at)` are detected only via COUNT (delete+insert in one tick with equal counts and non-monotonic timestamps can be missed); sub-second writes with equal timestamps likewise. CRUD handlers don't trigger refresh directly, so worst-case propagation = poll + rebuild latency. v2 should use transactional outbox/notify.
2. **Global version counter, per-type snapshots.** One AtomicU64 across all type URLs; a CDS change bumps the version that LDS responses also report. ACK detection compares against the *global* current version, so an ACK for type A arriving after an unrelated type-B bump is mis-classified as "not an ACK" and triggers a redundant full resend. Harmless but noisy; true per-type versioning is cleaner.
3. **No cross-type consistency / ordering.** Listeners, routes, clusters, secrets refresh independently; there is no make-before-break ordering (e.g. cluster removed before the route referencing it), no warming coordination. Envoy NACKs / warming failures are the safety net (hence the diagnostics subsystem).
4. **Version not persisted across CP restart** — restarts reset to 1; correctness relies on nonce-based ACK matching and full resends. Multi-replica CPs have *independent* version counters and caches; an Envoy reconnecting to another replica gets a full snapshot with an unrelated version string. Works, but version strings are meaningless globally.
5. **Broadcast-lag loses pushes.** `broadcast::channel(128)`; on `Lagged` the stream only logs — missed deltas for a subscribed type are never re-driven until the next unrelated update or client request. A slow stream can silently run stale.
6. **Dirty-check races with injection nondeterminism.** Equality is byte-wise on encoded protobufs; injections (filters, learning sessions, RBAC) re-encode listeners, and any map-ordering nondeterminism in JSON→Struct conversion or filter merge order produces spurious "changes" (version churn) or, worse, per-stream responses that differ from the cached snapshot. Also, per-stream responses rebuild from the DB while the change-detector uses the cache — a window where the broadcast fires before/after the DB row settles is benign only because responses re-read the DB.
7. **Per-request DB fan-out.** Every discovery request and every push re-queries listeners/routes/clusters/secrets plus N filter-junction queries per resource (hierarchical injection is N+1-heavy). Hot reconnect storms hit the DB hard. v2 should serve from the snapshot cache with per-team indexes.
8. **`list(Some(1000))` truncation.** Cache refresh loads at most 1000 rows per type with no pagination — silent truncation beyond that.
9. **NACK handling is "wait for fix".** On NACK with unchanged config the CP stops resending; nothing marks the resource bad, no rollback to last-ACKed snapshot, no per-resource quarantine. One team's bad listener NACKs the whole LDS response for that team (SOTW all-or-nothing).
10. **Delta protocol is fake.** Delta responses always carry the full team set; `removed_resources` merely echoes the client's unsubscribe list; ACK/NACK per-resource tracking absent. Either implement real delta with per-resource versions or drop it.
11. **Retry backoff Duration bug.** `retry_back_off.base_interval` writes `ms * 1_000_000` into `nanos` only (i32) — base intervals ≥ ~2.1 s overflow; `max_interval` splits seconds/nanos correctly. (`src/xds/route.rs`.)
12. **`query_parameters` matchers parsed but never emitted** into `RouteMatch` (`src/xds/route.rs`) — silent no-op for users.
13. **Implicit TLS on port 443** (`cluster_from_spec`): any endpoint on :443 force-enables upstream TLS even if `useTls:false`. Surprising; should be explicit.
14. **Dynamic Struct fallback for unknown filters** encodes JSON as `google.protobuf.Struct` under the filter's typed URL — Envoy rejects this for almost all real filters (only Struct-accepting extensions work). It papers over missing typed conversions and converts config errors into runtime NACKs.
15. **JWT merge semantics are lossy**: name collisions silently last-write-win (warn only); `filter_state_rules` dropped on merge; per-route jwt reference picks the *first* provider key of an unordered map — nondeterministic across rebuilds (HashMap iteration), another source of churn and surprise.
16. **ALS/ExtProc tenancy is weak.** Session matching is by path regex only — concurrent sessions from different teams with overlapping patterns capture each other's traffic (first match wins); ALS/ExtProc don't verify the caller's cert team against the session team. Queues are unbounded mpsc (memory risk under load). ExtProc keys per-stream state assuming one in-flight request per stream.
17. **Learning depends on fragile name conventions**: session correlation via filter/log names containing the session id (`name.contains(session_id)`), `x-request-id` correlation between ALS and ExtProc, `x-envoy-original-path` for pre-rewrite paths.
18. **Allowlist scope is half-baked**: grants default-only resources, listener name filtering "happens after retrieval" but isn't actually applied in the LDS builder; metadata-driven (unauthenticated) — any cert-holding dataplane can opt into allowlist scope by sending metadata.
19. **CDS/RDS fall back to config-based static resources on DB *error*** while LDS fails closed — inconsistent failure semantics; a DB blip could serve a test cluster/route to a real dataplane.
20. **EDS is vestigial** (static config only); endpoints ride inside CDS. Real EDS (with the `cluster_endpoints` table as source) would avoid cluster reconnects on endpoint churn.
21. **Mid-stream revocation gap**: cert binding is validated at stream establishment only; revoking a cert doesn't kill live streams.
22. **NACK resource-name extraction by string-scanning** Envoy error text (`name: "…"` pattern) — brittle attribution.
23. **`ctrl_c` listener inside every stream task** (`tokio::select!` arm) — each stream registering a signal listener is wasteful and surprising; shutdown should come from a watch channel.
