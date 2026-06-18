# S1–S10 Certification Coverage Matrix

Traceability for the build-quality certification (#66) and the exhaustive E2E (#65).
Every shipped capability and every abuse-matrix row maps to exactly one **authoritative**
evidence source. This is the certification document: "does it cover everything?" is answered
by this table.

## Legend

- `live:<phase>` — proven by the live Envoy E2E (`scripts/e2e-envoy.sh`; phases re-homed under
  `scripts/e2e/` after the split). A live phase is the authoritative evidence.
- `cargo:<test>` — unit/integration is the right boundary. The named test **must** run with
  `FLOWPLANE_TEST_DATABASE_URL` + `FLOWPLANE_SECRET_ENCRYPTION_KEY` set, or it silently skips and
  counts as **not tested**.
- `gap` — certification must add coverage.
- `out-of-scope:<reason>` — deliberately not covered now (needs an external service or belongs to
  S12 hardening), with the reason and owning slice named.

A capability may list a secondary source in parentheses, but the first token is authoritative.

Live phases (current `scripts/e2e-envoy.sh`): `P1` basic ADS traffic · `P1a` AI
(injection/failover/budget/usage/cleanup) · `P1b` config-first learning · `P1c` traffic-first
discovery+route-gen · `P2` CP-restart convergence · `P2a` Envoy-restart convergence · `P3` cross-team xDS isolation · `P4`
local-rate-limit/header-mutation · `P5` JWT/RBAC/ext_authz · `P6` SDS rotation · `P7` advanced
route/listener/filter parity (+ global-RLS filter ACK).

---

## S1 — Skeleton, Runtime, Observability

| Capability | Evidence | Status |
|---|---|---|
| Fresh DB migrates | `live:setup` (`flowplane db migrate` in run); `cargo:fp-storage migrations_apply_and_ping` | covered |
| Boots on validated config; fails closed on invalid | `cargo:fp-core config (env>file>defaults, validation)` | covered |
| `/healthz` `/readyz` `/metrics` `/openapi` | `cargo:router::{healthz_reports_ok_and_version, readyz_passes_with_live_database, readyz_fails_when_xds_consumer_failed}` (live: `P1` readyz gate) | covered |
| Request ID in error envelope + logs | `cargo:router::{unknown_path_returns_standard_envelope_with_request_id, valid_inbound_request_id_is_honored, malformed_inbound_request_id_is_replaced}` | covered |
| W3C `traceparent` honored | `cargo:` S1.5 trace-context test | covered |
| TLS API listener; insecure opt-in | `cargo:` S1.6 TLS smoke (live runs insecure dev mode by design) | covered |
| Graceful shutdown drains API/xDS without corruption | `out-of-scope:S12 failure-mode suite (kill/restart under load)` | deferred |

## S2 — Identity, Authn, Authz, Tenancy

| Capability | Evidence | Status |
|---|---|---|
| Bootstrap one-shot + concurrency-safe | `live:setup` (bootstrap); `cargo:` S2.5 single-use/advisory-lock test | covered |
| Dev/OIDC token via production validation path | `cargo:fp-core oidc` (live uses dev issuer through same path) | covered |
| `whoami` returns active org/team | `live:setup` (`whoami`) | covered |
| Multi-org requires `X-Flowplane-Org` when ambiguous | `cargo:api_crud::multi_org_user_selects_active_org_with_header` | covered |
| Cross-org/team anti-enumerating 404/403 | `cargo:tenancy::cross_org_isolation_holds_from_database_to_decision`, `gateway::create_emits_event_and_cross_org_caller_sees_not_found` | covered |
| Grants permit only intended resource/action/team | `cargo:tenancy::grants_load_from_real_rows_and_cross_org_grants_are_unrepresentable`, `gateway::grantless_member_denied_with_actionable_forbidden` | covered |
| Tenant write throttle org-keyed, no cross-tenant bleed | `cargo:` S2.6 throttle tenant-isolation test | covered |
| Authn failures + important mutations emit audit rows | `cargo:` S2 audit tests | covered |
| Service-layer **authz-denial** audit rows | `cargo:` denial primitive (storage-tested) — but **not wired into service-layer denials** (Open Risk R2). Known residual (see defect log) | residual (R2) |
| **Governance CRUD** (org/team/user/member/grant create/list/get/delete) | `cargo:api_crud::full_crud_journey_over_http_with_bearer_auth`, `multi_org_user_selects_active_org_with_header`, `tenancy::*`; live bootstrap→team exercised in `live:setup`. Thin live org/member/grant walk optional (cargo is the authoritative boundary) | covered (cargo) |

## S3 / S4 — Gateway Resources, REST API, OpenAPI

| Capability | Evidence | Status |
|---|---|---|
| Clusters/route-configs/listeners/bindings CRUD (REST + CLI) | `live:P1` (expose materializes); `cargo:api_crud::full_crud_journey_over_http_with_bearer_auth` | covered |
| Optimistic concurrency / `If-Match` rejects stale | `cargo:gateway::concurrent_updates_one_wins_one_gets_revision_mismatch`, `delete_requires_current_revision_and_emits_deletion_event` | covered |
| Per-team uniqueness, port conflicts, quotas, dependency guards | `cargo:gateway::{port_collisions_within_a_team_conflict, references_resolve_and_deletion_is_guarded_end_to_end}`, `quotas::quota_caps_cluster_count_per_team` | covered |
| Cross-org resource isolation | `cargo:gateway::create_emits_event_and_cross_org_caller_sees_not_found` | covered |
| OpenAPI contains every op; CLI path coverage pinned | `cargo:api_crud::openapi_document_covers_every_registered_operation` | covered |
| Stable error envelopes (validation/conflict/404/403/500) | `cargo:api_crud`, `router` | covered |

## S5 — xDS, Envoy, Filters, Quarantine

| Capability | Evidence | Status |
|---|---|---|
| Envoy joins ADS; CDS/EDS/RDS/LDS make-before-break | `live:P1`; `cargo:ads_stream::subscribe_receive_ack_and_live_push` | covered |
| Basic HTTP traffic routes to mock upstream | `live:P1` | covered |
| CP restart primes snapshots from DB | `live:P2` | covered |
| Cross-team xDS isolation in config dump | `live:P3`; `cargo:tenancy` | covered |
| EDS endpoint change bumps only endpoint version; traffic follows | `cargo:` S5.7 EDS determinism test | covered |
| mTLS dataplane auth binds registry identity (not node/SAN) | `cargo:ads_mtls::registry_binds_team_and_revocation_kills_live_stream` (live xDS runs dev plaintext) | covered |
| Cert revocation kills streams + rejects reconnect | `cargo:ads_mtls::{registry_binds_team_and_revocation_kills_live_stream, unregistered_and_expired_certificates_are_rejected, connection_without_client_certificate_fails}` | covered |
| NACK quarantine serves last-good; clears after fix | `cargo:ads_stream::nack_quarantines_offender_and_pushes_corrected_set` (live: P1 nacks check) | covered |
| local rate limit + header mutation | `live:P4` | covered |
| JWT auth + ext_authz + RBAC (DENY enforced) | `live:P5` | covered |
| CORS (where shipped) | `cargo:fp-domain filters cors` (live: not exercised) | covered |
| Route/vhost overrides + disabled per-route filters | `live:P4` (per-route exempt) ; `cargo:fp-domain filters` | covered |
| **Global rate limit (RLS) filter** | `live:P7` (filter ACKs in config dump). Enforcement → `out-of-scope:needs external RLS+Redis service` | partial |
| Advanced route/listener/cluster fields ACK + affect traffic | `live:P7` | covered |

## S6 — Secrets, SDS, Dataplane Surface, Agent Telemetry

| Capability | Evidence | Status |
|---|---|---|
| Secret create/list/get metadata-only; values never returned | `cargo:api_crud::secret_values_are_write_only_over_http`; `live:P1a/P6` | covered |
| Secret rotation changes SDS material without Envoy restart | `live:P6` | covered |
| SDS subscriptions name-filtered; no unrelated leak | `cargo:` S6.4a SDS name-filter test (live: P6) | covered |
| Dataplane CRUD, bootstrap gen, cert issue/register/revoke | `live:setup` (dataplane create + bootstrap); `cargo:api_crud::proxy_certificate_registry_flow_over_http` | covered |
| Generated mTLS bootstrap connects to xDS | `cargo:ads_mtls` (live uses dev plaintext bootstrap) | covered |
| `fp-agent` diagnostics: heartbeats/config-verify/stats | `cargo:` S6.5b diagnostics test | covered |
| Stats overview aggregates per-team dataplane counters | `live:P1` (`stats overview`); `cargo:` S6.5 | covered |

## S7 — CLI and Operator Workflows

| Capability | Evidence | Status |
|---|---|---|
| Config precedence (flags > env > file > context) | `cargo:` S7.1 CLI precedence test | covered |
| `auth login` token-stdin / device-code / PKCE forms | `cargo:` S7.2/7.4/7.6 CLI parse + flow tests (mocked OIDC) | covered |
| Table/JSON/YAML output stable | `cargo:` S7.2 transcript tests | covered |
| `apply --diff` plans without mutation | `cargo:` S7.3 apply/diff tests | covered |
| Non-diff `apply` creates/updates, preserves advanced specs | `cargo:` S7.3 ; `live:P1` (expose path). **Note:** `apply` intentionally covers gateway/secrets/dataplane only — **not** AI resources (`ai *`-managed) | covered |
| `expose` / `unexpose` round-trip | `live:P1` (expose + cleanup) | covered |
| Dev runbook: empty DB → curlable Envoy traffic + stats | `live:` the whole runner is this path | covered |

## S8 — Config-first Learning

| Capability | Evidence | Status |
|---|---|---|
| API import from OpenAPI JSON/YAML → append-only spec versions | `cargo:api_crud::api_definition_import_status_and_delete_over_http`, `api_lifecycle::spec_versions_are_append_only`; `live:P1b` | covered |
| Route/API binding to gateway resources | `cargo:api_lifecycle::{route_bindings_reject_cross_team_gateway_references, route_bindings_allow_only_one_unscoped_binding, route_bindings_allow_only_one_vhost_binding}` | covered |
| Learning session start/list/get/cancel (REST + CLI) | `cargo:api_crud::learning_session_lifecycle_over_http`; `live:P1b` | covered |
| ALS/ExtProc injected only for selected team/session/route | `live:P1b`; `cargo:api_lifecycle::capture_ingest_binding_validates_active_team_and_scope` | covered |
| Real traffic persists redacted, bounded observations + merge | `live:P1b`; `cargo:api_lifecycle::{raw_observation_ingest_redacts_and_counts_accepted_rows, raw_observation_body_merge_does_not_increment_sample_count, raw_observation_duplicate_merges...}` | covered |
| Abuse: header flood / oversized / truncated / path explosion / dup / poisoning fail safely | `cargo:api_lifecycle::{raw_observation_quota_drop_does_not_insert, raw_observation_truncated_body_quota_uses_reported_original_bytes}`, `capture_sessions_reject_cross_team_route_scope` | covered |
| Aggregation → deterministic learned candidate | `cargo:quotas::learned_spec_generation_persists_deterministic_candidate`; `live:P1b` | covered |
| Review/reject/publish gates tools + route generation | `cargo:route_generation::{route_plan_create_rejects_unreviewed_spec, route_plan_create_rejects_rejected_spec}`; `live:P1b` | covered |
| Stop/cancel leaves no learning-owned orphans | `live:P1b` (zero S8 orphans) | covered |

## S9 — Traffic-first Discovery and Route Generation

| Capability | Evidence | Status |
|---|---|---|
| Discovery requires explicit upstream; rejects unsafe dests (SSRF) | `cargo:discovery::discovery_rejects_loopback_upstream_before_creating_session`; `live:P1c` | covered |
| Discovery-owned listeners xDS-visible but hidden/protected from user paths | `cargo:discovery::discovery_lifecycle_hides_resources_from_user_paths_and_tears_down`; `live:P1c` | covered |
| Stop tears down discovery-owned resources + rebuilds xDS | `live:P1c`; `cargo:discovery::discovery_lifecycle_hides_resources...` | covered |
| Observations persist host/SNI/upstream/session/listener provenance | `cargo:storage discovery::discovery_observations_persist_payload_and_provenance` | covered |
| Multi-host/upstream → separate candidate APIs | `cargo:discovery::discovery_learning_creates_one_spec_per_host_cluster` | covered |
| Unreviewed/rejected specs cannot generate/apply | `cargo:route_generation::{route_plan_create_rejects_unreviewed_spec, route_plan_create_rejects_rejected_spec}` | covered |
| Reviewed/published → persisted dry-run plan | `cargo:route_generation::route_plan_apply_replays_persisted_preview` | covered |
| Apply replays plan; fails on intervening invalidation | `cargo:route_generation::{route_plan_apply_fails_on_intervening_conflict, route_plan_apply_rechecks_review_state}` | covered |
| Generated resources route real traffic + cleanup | `live:P1c` | covered |

## S10 — AI Gateway, Usage, Budgets, Failover

| Capability | Evidence | Status |
|---|---|---|
| AI provider CRUD; secret-backed; no credential leak | `live:P1a`; `cargo:api_crud` (cross-team secret reject) | covered |
| AI route CRUD materializes gateway-owned resources via existing services | `live:P1a`; `cargo:` services::ai materialization | covered |
| Provider update marks dependent routes stale / blocks delete | `live:P1a` (stale propagation); `cargo:` services::ai | covered |
| AI-owned resources count against quota + clean up partial failures | `cargo:quotas::ai_route_materialization_cleans_clusters_after_partial_quota_failure` | covered |
| Request path: model extract → routing header → credential inject → prefix/base/model override → reject malformed/oversized/no-backend | `live:P1a`; `cargo:fp-domain ai::*`, fp-xds ai_ext_proc tests | covered |
| Non-streaming + streaming usage capture for selected backend | `live:P1a`; `cargo:fp-xds ai_ext_proc` (unary + split-SSE) | covered |
| Synthetic `include_usage` chunk stripped from client | `cargo:fp-domain ai::strips_synthetic_stream_usage_chunk`, fp-xds ai_ext_proc | covered |
| Usage events + `ai usage` team-scoped, append-only | `live:P1a`; `cargo:` | covered |
| Shadow budgets never block but settle | `live:P1a`; `cargo:fp-xds` shadow check | covered |
| Enforcing budgets block at request start; fixed-window; bounded overdraft | `live:P1a`; `cargo:fp-xds ai_upstream_auth_injection...` | covered |
| Concurrent same-team settlement atomic; cross-team isolated | `cargo:ai_budgets::concurrent_budget_settlement_is_atomic_and_team_scoped` | covered |
| Priority failover: higher-priority first, fall-through pre-byte, re-inject per-backend credential, attribute usage to backend used | `live:P1a` (primary unavailable → fallback with fallback credential + attribution) | covered |
| **Never fails over after streaming starts (stream-start boundary)** | `live:P1d` — boundary logic verified on the non-stream path (partial delivered, no failover after first byte, fallback untouched). **Blocked on `stream:true` by #67 (D9)** — tracked known-fail; phase auto-validates once #67 lands | partial (D9/#67) |
| Malformed provider response handling | `live:P1e` — non-OpenAI 200 body passed through, no 500, no usage settled | covered |
| Mock-provider E2E: credential→route→traffic→usage→budget trip→failover→cleanup | `live:P1a` | covered |

## Abuse / Regression Matrix

| Case | Evidence | Status |
|---|---|---|
| Cross-org / cross-team reads & writes | `cargo:tenancy::cross_org_isolation_holds`, `gateway::create_emits_event_and_cross_org_caller_sees_not_found` | covered |
| Stale revision writes | `cargo:gateway::concurrent_updates_one_wins_one_gets_revision_mismatch` | covered |
| Quota exhaustion + partial cleanup | `cargo:quotas::ai_route_materialization_cleans_clusters_after_partial_quota_failure`, `quota_caps_cluster_count_per_team` | covered |
| Missing/malformed/expired/wrong-team creds & certs | `cargo:ads_mtls::unregistered_and_expired_certificates_are_rejected`; `cargo:api_crud` cross-team secret reject | covered |
| Envoy NACK + corrected-config recovery | `cargo:ads_stream::nack_quarantines_offender_and_pushes_corrected_set` | covered |
| CP restart while dataplane connected | `live:P2` | covered |
| Envoy restart while CP has live config | `live:P2a` | covered |
| Secret rotation while traffic active | `live:P6` | covered |
| Learning/discovery poisoned or excessive input | `cargo:api_lifecycle::raw_observation_quota_drop...`, `discovery::discovery_rejects_loopback_upstream` | covered |
| AI budget exhaustion | `live:P1a` | covered |
| AI missing usage (fail-open, documented) | `cargo:` (persist no-ops on missing usage); D-018 documents fail-open | covered |
| AI malformed provider response | `live:P1e` | covered |
| AI unavailable primary backend (failover) | `live:P1a` (priority-0 connection refused → priority-1) | covered |
| AI stream-start failure boundary | `live:P1d` (boundary verified non-stream; `stream:true` blocked by #67) | partial (D9/#67) |

## Certification-only checks (not feature bullets, but Tier-0 exit gates)

| Check | Evidence | Status |
|---|---|---|
| Teardown **redaction sweep** over all artifacts (CP/Envoy logs, config dumps, access logs, DB usage rows; mock auth logs + one-time bootstrap PKI excluded) | `live:redaction_sweep` (also greps the API bearer token) | covered |
| **Cross-team isolation under concurrency** (no bleed in usage/budget counters under concurrent settlement) | `cargo:ai_budgets::concurrent_budget_settlement_is_atomic_and_team_scoped` — 32 concurrent settlements assert team B's counters/enforcement untouched; live re-impl is redundant | covered (cargo) |
| Live E2E green **5 consecutive** runs | blocked on #67 (D9); the non-#67 suite is otherwise green every run | pending #67 |

## Out-of-scope (named, with owning slice)

- **Global RLS enforcement** — needs an external RLS+Redis service; only filter-ACK is verified live (P7). Owner: S12 / when RLS server ships.
- **OTLP wire export** — needs an OTLP collector to assert spans on the wire; layer-init + `traceparent` are covered. Owner: S12 GenAI-semconv verification.
- **Graceful-shutdown-no-corruption, kill-Postgres-mid-write, Envoy crash/restart, load/soak** — S12 failure-mode + load suite. Tier-1 *smoke* may be done under #66; the heavy pass is S12 (recorded residual risk).

---

## Defect log (found during certification)

| ID | Severity | Finding | Status |
|---|---|---|---|
| D8 | Minor | Phase 7 global-RLS `config_dump` check flaked ~1/3 even after #64 (single-shot, unbounded curl) | Fixed: consistent-snapshot + `--max-time` poll; 5× green |
| D9 | **Major** | AI gateway returns 500 on `stream:true` requests before backend dispatch (#67) | **Open (#67)** — tracked known-fail; blocks streaming + the 5× close gate |
| R2 | Residual | Service-layer authz-**denial** audit rows: primitive exists + storage-tested, but not wired into service denials (pre-existing Open Risk in DECISIONS/PROGRESS) | Known residual — wire or formally accept before final sign-off |

## Remaining for Tier-0 sign-off
1. **#67 / D9** — fix streaming `stream:true` 500 (product, S10d). Phase 1d auto-validates the boundary once fixed.
2. **R2** — wire service-layer authz-denial audit, or formally accept as residual risk in the certification report.
3. **5× consecutive green** close gate — currently blocked only by #67 (the rest of the suite is green every run).
4. **Script split** (`run.sh`/`lib.sh`/`NN-*.sh` + `--only`/`--from`) — structural; fold `wait_converged`/`assert_status`/`redaction_sweep`/`known_fail` into `lib.sh`. Tracking-only; not a coverage gap.

All other S1–S10 capabilities and abuse-matrix rows are covered by a passing `live:` phase or a named `cargo:` test (see tables above).
