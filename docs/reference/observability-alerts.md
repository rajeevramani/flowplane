# Observability Alert Pack

> Audience: operators, platform-engineers · Status: stable

This reference is the operator baseline for production alerting. It uses the metrics that exist in the current control-plane and data-plane-adjacent services. Dashboards and alerts should keep labels bounded: do not add team, org, dataplane, route, budget, tool, or agent labels unless a later issue explicitly accepts the cardinality cost.

Native OpenTelemetry `gen_ai.*` semantic-convention meters are deferred for v1.0. Flowplane emits pragmatic counters for shipped AI budget behavior instead.

## Metric Inventory

| Family | Metric | Type | Labels | Source |
| --- | --- | --- | --- | --- |
| API traffic | `fp_api_requests_total` | counter | `status` | API middleware |
| API latency | `fp_api_request_duration_ms` | histogram | none | API middleware |
| Authn failures | `fp_authn_failures_total` | counter | none | API auth middleware |
| Authz denials | `fp_authz_denied_total` | counter | `resource`, `action` | shared denial recording hook |
| Audit write failures | `fp_audit_write_failures_total` | counter | none | shared audit writer |
| Tenant throttling | `fp_tenant_write_throttled_total` | counter | none | write throttle |
| xDS NACKs | `fp_xds_nacks_total` | counter | none | ADS NACK handling |
| xDS quarantine | `fp_xds_quarantined_resources_total` | counter | none | snapshot quarantine |
| xDS rebuilds | `fp_xds_snapshot_rebuilds_total` | counter | none | snapshot cache |
| xDS translation failures | `fp_xds_resource_translation_failures_total` | counter | `resource_kind` | snapshot translation |
| xDS secret translation failures | `fp_xds_secret_translation_failures_total` | counter | none | secret translation |
| xDS prime failures | `fp_xds_prime_team_failures_total` | counter | none | startup priming |
| ADS stream opens | `fp_xds_ads_streams_opened_total` | counter | none | authenticated ADS stream lifecycle |
| ADS stream closes | `fp_xds_ads_streams_closed_total` | counter | none | authenticated ADS stream lifecycle |
| DB pool size | `fp_db_pool_size` | gauge | none | serve-owned sampler |
| DB pool idle | `fp_db_pool_idle` | gauge | none | serve-owned sampler |
| DB pool in use | `fp_db_pool_in_use` | gauge | none | serve-owned sampler |
| DB pool max | `fp_db_pool_max` | gauge | none | serve-owned sampler |
| Outbox pending | `fp_outbox_pending_events` | gauge | `consumer` | serve-owned sampler |
| Outbox age | `fp_outbox_oldest_pending_age_seconds` | gauge | `consumer` | serve-owned sampler |
| Outbox handled | `fp_outbox_events_handled_total` | counter | `consumer` | outbox consumer |
| Outbox handler failures | `fp_outbox_handler_failures_total` | counter | `consumer` | outbox consumer |
| Capture drops | `fp_capture_dropped_total` | counter | `source`, `reason` | ALS/ext_proc capture ingest |
| AI budget threshold crossings | `fp_ai_budget_threshold_crossings_total` | counter | `mode`, `result` | capture budget enforcement |
| Sampler failures | `fp_observability_sampler_failures_total` | counter | `source` | serve-owned sampler |

## Alert Baseline

These expressions are a starting point. Tune the thresholds after production traffic establishes normal baselines.

| Alert | Expression | Initial severity | Rationale |
| --- | --- | --- | --- |
| xDS NACKs observed | `increase(fp_xds_nacks_total[5m]) > 0` | page | Envoy rejected a pushed resource; the last-good/quarantine path is active. |
| xDS quarantine active | `increase(fp_xds_quarantined_resources_total[5m]) > 0` | page | A resource has been quarantined and may be serving stale last-good config. |
| xDS rebuild failures | `increase(fp_xds_resource_translation_failures_total[5m]) > 0 or increase(fp_xds_secret_translation_failures_total[5m]) > 0` | page | The control plane cannot translate persisted state into xDS resources. |
| ADS disconnect churn | `increase(fp_xds_ads_streams_closed_total[10m]) > increase(fp_xds_ads_streams_opened_total[10m]) + 5` | ticket | Dataplanes are disconnecting faster than new authenticated streams are opening. |
| DB pool saturation | `fp_db_pool_max > 0 and fp_db_pool_in_use / fp_db_pool_max > 0.8` | ticket | API latency and outbox processing can degrade when the pool is close to exhausted. |
| Outbox lag count | `fp_outbox_pending_events{consumer="xds-snapshot"} > 100` | page | xDS has fallen behind the mutation log. |
| Outbox lag age | `fp_outbox_oldest_pending_age_seconds{consumer="xds-snapshot"} > 30` | page | At least one unprocessed xDS event has aged beyond the expected near-real-time window. |
| Authz denial spike | `sum(increase(fp_authz_denied_total[5m])) > 50` | ticket | Can indicate broken client permissions, grant drift, or abusive probing. |
| Authn failure spike | `sum(increase(fp_authn_failures_total[5m])) > 50` | ticket | Can indicate IdP/JWKS trouble or token abuse. |
| Capture drops | `sum(increase(fp_capture_dropped_total[5m])) > 0` | ticket | Learning observations are being dropped by the capture ingest path. |
| AI budget exhausted | `sum(increase(fp_ai_budget_threshold_crossings_total{mode="enforcing",result="exhausted"}[5m])) > 0` | ticket | Enforcing AI budgets denied traffic; investigate expected spend versus limits. |
| Audit write failures | `increase(fp_audit_write_failures_total[5m]) > 0` | page | Security/audit trail durability is degraded. |
| Sampler failures | `increase(fp_observability_sampler_failures_total[5m]) > 0` | ticket | Alert visibility is degraded; inspect DB connectivity and sampler logs. |

## Dashboard Panels

Use one production dashboard with these panels:

1. API request rate, error rate, and p95 latency from `fp_api_requests_total` and `fp_api_request_duration_ms`.
2. xDS health: NACKs, quarantines, translation failures, ADS stream opens/closes.
3. Outbox health: pending events, oldest pending age, handled events, handler failures.
4. DB pool health: in-use, idle, max, and in-use/max ratio.
5. Security signals: authn failures, authz denials, audit write failures, tenant throttling.
6. Capture and AI policy: capture drops and enforcing budget exhaustion.

## Operational Notes

- The serve-owned sampler is read-only. It does not advance outbox cursors and does not contact dataplanes.
- `fp_outbox_pending_events` and `fp_outbox_oldest_pending_age_seconds` are currently emitted for the `xds-snapshot` consumer.
- `fp_xds_snapshot_rebuilds_total` emits no labels. Do not add per-team labels to this metric or future rebuild-family metrics without an explicit cardinality decision.
- The ADS stream counters count authenticated ADS streams only. Failed authentication is covered by xDS/API logs and the existing authn/authz surfaces, not these lifecycle counters.
