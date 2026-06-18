# S12 Adversarial Surface Map

This is the S12d verification map for `spec/08a-security-and-tenancy.md` section 4. It records
the current code-backed evidence for each adversarial case and names the remaining accepted
boundaries. It is not a new product surface.

## Run Context

- `cargo:` means an automated Rust test. DB-backed tests require `FLOWPLANE_TEST_DATABASE_URL`
  and `FLOWPLANE_SECRET_ENCRYPTION_KEY` when the test touches encrypted secrets.
- `live:` means `bash scripts/e2e-envoy.sh` against a real Envoy dataplane. This is a release-gate
  check. It is reproducible in CI only if S12g promotes the Docker live gate; until then it remains
  a manual release gate with recorded evidence.
- `doc:` means an explicit accepted boundary rather than executable product coverage.

## 08a Section 4 Matrix

| 08a case | External surface(s) | Current evidence | Pass signal | Status |
| --- | --- | --- | --- | --- |
| 4.1 Capture poaching | xDS capture, learning REST/CLI, learned-spec export | `cargo:api_lifecycle::capture_ingest_binding_validates_active_team_and_scope`; `cargo:api_lifecycle::capture_sessions_reject_cross_team_route_scope`; `live:P1b` | Capture sessions bind to the owning team/listener/route and cross-team route scope is rejected before capture/export. | covered |
| 4.1 Name/port oracles | REST/CLI listener and gateway CRUD | `cargo:tenancy::cross_org_isolation_holds`; `cargo:gateway::create_emits_event_and_cross_org_caller_sees_not_found`; `cargo:gateway::port_collisions_within_a_team_conflict` | Cross-org callers see not-found style denial, while same-team conflicts remain explicit. | covered |
| 4.1 Resource exhaustion | REST/CLI mutations, capture ingest, AI usage/budget paths | `cargo:quotas::quota_caps_cluster_count_per_team`; `cargo:quotas::ai_route_materialization_cleans_clusters_after_partial_quota_failure`; `cargo:api_lifecycle::raw_observation_quota_drop_does_not_insert_or_move_sample_count`; `cargo:ai_budgets::concurrent_budget_settlement_is_atomic_and_team_scoped`; `live:P1a` | Quotas fail closed, partial materialization is cleaned up, capture drops are counted without inserts, and concurrent same-team settlement does not bleed into another team. | covered |
| 4.1 Grant escalation probes | REST identity APIs, MCP list/call, CLI wrappers | `cargo:tenancy::cross_org_isolation_holds`; `cargo:fp-api mcp_static_tools::tools_list_filters_by_principal_kind_grant_and_team`; `cargo:fp-api mcp_static_tools::static_tool_denial_records_shared_authz_denial_signal`; `cargo:fp-core authz::gateway_agents_only_get_granted_mcp_tools_access`; current service helpers call `record_authz_denial` on denials | Each team-affecting request is checked through `check_resource_access`; denied service and MCP calls emit the shared denial signal. | covered |
| 4.2 Compromised operator token: token/grant revocation and MCP sessions | REST auth middleware, agents, MCP Streamable HTTP | `cargo:fp-api agent_auth::mcp_session_reauth_rejects_disabled_agent_on_next_request`; `cargo:fp-api agent_auth::mcp_session_reauth_removes_grants_on_next_request`; `cargo:fp-api mcp_transport::mcp_session_is_bound_to_reauthenticated_principal` | Existing MCP sessions do not cache authorization decisions; disable/revoke is enforced on the next request. | covered |
| 4.2 Compromised operator token: audit and anomaly counters | REST/MCP services, observability | `cargo:fp-core tenancy::cross_org_isolation_holds`; `cargo:fp-core gateway::create_emits_event_and_cross_org_caller_sees_not_found`; `cargo:fp-api mcp_static_tools::static_tool_denial_records_shared_authz_denial_signal`; `docs/observability-alerts.md` | Authz denials produce `authz.denied` audit rows and `fp_authz_denied_total`/alert-family coverage. | covered |
| 4.2 Secrets and high-risk dataplane actions | REST/CLI secrets, SDS, proxy certificates | `cargo:api_crud` cross-team secret rejection; `cargo:api_crud::proxy_certificate_registry_flow_over_http`; `cargo:ads_mtls::{registry_binds_team_and_revocation_kills_live_stream,unregistered_and_expired_certificates_are_rejected,connection_without_client_certificate_fails}`; `live:P6`; `live:redaction_sweep` | Secrets are redacted/write-only at the API boundary, delivered through SDS, redacted from artifacts, and certificate revocation kills live streams/reconnects. | covered |
| 4.3 Observed traffic is data, never config | Learning/discovery REST/CLI, generated routes/tools | `cargo:route_generation::{route_plan_create_rejects_unreviewed_spec,route_plan_create_rejects_rejected_spec,route_plan_apply_replays_persisted_preview,route_plan_apply_rechecks_review_state}`; `live:P1c` | Learned/generated output remains a proposal/dry-run until explicit review/apply and rechecks review state on apply. | covered |
| 4.3 Spec poisoning resistance | Capture ingest, discovery, schema inference | `cargo:api_lifecycle::{raw_observation_quota_drop_does_not_insert_or_move_sample_count,raw_observation_truncated_body_quota_uses_reported_original_bytes}`; `cargo:discovery::discovery_rejects_loopback_upstream_before_creating_session`; `live:P1b`; `live:P1c` | Oversize/truncated/unsafe inputs fail safely or are bounded, discovery rejects unsafe upstreams, and capture/drop metrics record the offender path. | covered |
| 4.3 Captured-data hygiene | Capture storage, spec export, logs/artifacts | `cargo:storage discovery::discovery_observations_persist_payload_and_provenance`; `live:redaction_sweep`; `docs/observability-alerts.md` | Captured rows retain provenance, live artifact sweep finds no secrets/API tokens, and capture drops are observable. | covered |
| 4.3 Traffic-first generated routes cannot cross teams | Discovery, route generation, xDS | `cargo:discovery::discovery_learning_creates_one_spec_per_host_cluster`; `cargo:discovery::discovery_lifecycle_hides_resources_from_user_paths_and_tears_down`; `cargo:route_generation::*`; `live:P1c`; `live:P3` | Generated resources are reviewed, quota-validated, cleaned up on stop, and live xDS proves another team does not receive them. | covered |
| 4.4 Rogue dataplane: per-message team binding | ADS/SDS/ALS/ExtProc diagnostics | `cargo:api_lifecycle::capture_ingest_binding_validates_active_team_and_scope`; `cargo:ads_stream::nack_quarantines_offender_and_pushes_corrected_set`; `cargo:fp-xds diagnostics::tests::diagnostics_report_decode_ignores_additive_unknown_fields` | Dataplane-submitted messages are bound to the registered team/cert path; bad xDS is quarantined without cross-team fallout; diagnostics tolerate additive proto fields. | covered |
| 4.4 Rogue dataplane: SDS ownership and revocation | SDS and mTLS cert registry | `cargo:ads_mtls::{registry_binds_team_and_revocation_kills_live_stream,unregistered_and_expired_certificates_are_rejected,connection_without_client_certificate_fails}`; `live:P6` | Wrong/expired/revoked dataplane certs cannot receive config/secrets, and revocation terminates active streams. | covered |

## Load Sanity

S12d intentionally does not add a heavyweight soak/performance suite. The bounded load sanity pass
is covered by existing concurrency and live-runner evidence:

| Check | Evidence | Status |
| --- | --- | --- |
| Tenant-scoped concurrent AI budget settlement | `cargo:ai_budgets::concurrent_budget_settlement_is_atomic_and_team_scoped` | covered |
| Quota cleanup under partial failure | `cargo:quotas::ai_route_materialization_cleans_clusters_after_partial_quota_failure` | covered |
| Event replay/cursor failure under retry | `cargo test -p fp-storage outbox::tests::failed_handler_redelivers_the_same_batch` | covered |
| CP restart while dataplane has live config | `live:P2` through `bash scripts/e2e-envoy.sh` | release-gate covered |
| Envoy restart/resubscribe convergence | `live:P2a` through `bash scripts/e2e-envoy.sh` | release-gate covered |

## Accepted Boundaries

- Global RLS enforcement still needs an external RLS+Redis service. Current live coverage verifies
  filter ACK/config parity only.
- OTLP wire export still needs an external collector. Layer initialization and `traceparent`
  propagation are covered; native GenAI semantic-convention spans are deferred for v1.0.
- Live Envoy E2E remains a manual release gate unless S12g promotes the Docker-backed run into
  required GitHub Actions.
- No long-duration soak or benchmark target is part of S12d; the release criterion is bounded
  concurrency/load sanity plus the failure-mode matrix.
