# S12 Failure-Mode Matrix

This is the S12c release-readiness matrix for Flowplane v2. It records the invariant, command,
pass signal, failure signal, and run context for each failure-mode row. It is verification evidence,
not a new product surface.

## D-021 Reconciliation

D-021 defines four failure-mode rows. This matrix expands them into six verification rows so the
evidence is specific without changing the S12 exit scope:

| D-021 row | Verification row(s) |
| --- | --- |
| Postgres mid-write | Postgres killed or unavailable mid-write |
| Envoy restart | Envoy restarted while CP has live config state; xDS resync/convergence after restart or reconnect |
| CP restart under load | CP restarted under active mutation/load; outbox consumer crash/replay/no event loss |
| Agent version-skew | Agent-to-CP diagnostics proto/gRPC additive compatibility |

## Matrix

| Row | Invariant | Command / Harness | Pass Signal | Failure Signal | Run Context | Coverage |
| --- | --- | --- | --- | --- | --- | --- |
| Postgres killed or unavailable mid-write | Mutations and their outbox events commit or roll back together; no partial domain row without its event, and no rolled-back event is delivered. | `FLOWPLANE_TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/flowplane_dev cargo test -p fp-storage outbox::tests::events_commit_with_their_transaction_and_roll_back_with_it` | The committed `real` event is delivered; the rolled-back `ghost` event is never delivered. | The rolled-back event is observed, the committed event is missing, or the test cannot reach the DB. | Automated DB-backed integration test. | Existing transactional-outbox atomicity coverage; intentionally no heavyweight chaos harness. |
| Envoy restarted while CP has live config state | A dataplane restart/reconnect receives the persisted CP state through ADS and resumes serving without orphaned or quarantined resources. | `bash scripts/e2e-envoy.sh`; Phase 2a. | Phase 2a prints `PHASE 2a OK: Envoy restarted, reconnected...`; traffic flows after restart; `ops xds nacks` reports no rows. | Envoy fails to reconnect, traffic never flows, config dump lacks the expected resources, or unexpected NACKs appear. | Live Envoy release gate. Reproducible in CI only if S12d/S12g choose Docker; otherwise manual release gate using Docker or the script's local Envoy fallback. | Targeted S12c live-runner phase; no separate chaos harness. |
| Control plane restarted under active mutation/load | Restarting CP does not panic, does not wipe dataplane config, primes snapshots from DB, and post-restart mutations still reach the connected Envoy. | `bash scripts/e2e-envoy.sh`; Phase 2. | Phase 2 prints `PHASE 2 OK: CP restarted, Envoy survived and converged ...`; CP logs include `snapshot cache primed`; post-restart mutation changes served traffic. | Restarted CP does not become healthy, snapshot priming is absent, original traffic breaks, or post-restart mutation never reaches Envoy. | Live Envoy release gate. Reproducible in CI only if S12d/S12g choose Docker; otherwise manual release gate using Docker or the script's local Envoy fallback. | Existing phase P2 evidence from `scripts/e2e-envoy.sh` and `internal/e2e/COVERAGE.md`. |
| Outbox consumer crash/replay/no event loss | If a consumer fails before cursor advancement commits, the same batch is redelivered and cursor advancement happens exactly once after success. | `FLOWPLANE_TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/flowplane_dev cargo test -p fp-storage outbox::tests::failed_handler_redelivers_the_same_batch` | The simulated failure returns an error; retry receives events again; after a successful retry the marker event is not delivered a third time. | The failed attempt advances the cursor, retry misses the event, or the marker event repeats after successful cursor advancement. | Automated DB-backed integration test. | Existing S3 outbox crash-redelivery coverage; decomposes D-021 CP restart under load into the no-event-loss invariant. |
| xDS resync/convergence after restart or reconnect | ADS resubscription and CP snapshot rebuild converge to the latest persisted config without orphaned or quarantined resources. | `bash scripts/e2e-envoy.sh`; Phase 2 and Phase 2a plus `ops xds nacks` checks; `cargo test -p fp-xds snapshot::tests::events_drive_rebuilds_with_per_type_versions_and_team_isolation` for the cache rebuild invariant. | Live P2 converges after CP restart; live P2a converges after Envoy restart; snapshot tests prove event-driven rebuilds are team-isolated and versioned; no unexpected NACKs are reported in the live run. | Config does not converge, stale config persists after mutation, wrong-team resources appear, or NACKs/quarantine appear unexpectedly. | Mixed: automated unit/integration for snapshot rebuild; live Envoy release gate for live resync. Live command is CI-with-Docker only if S12d/S12g choose that gate; otherwise manual with Docker or local Envoy fallback. | Existing P2 evidence, new P2a evidence, plus snapshot tests; decomposes D-021 Envoy restart. |
| Agent-to-CP diagnostics proto/gRPC version-skew | The agent diagnostics protobuf surface tolerates additive unknown fields while preserving known fields. REST DTOs intentionally reject unknown fields via `deny_unknown_fields`; that strict JSON contract is separate and not contradictory. | `cargo test -p fp-xds diagnostics::tests::diagnostics_report_decode_ignores_additive_unknown_fields` | A report encoded with an extra unknown field decodes successfully and preserves `schema_version`, `report_id`, `dataplane_id`, and heartbeat payload fields. | Decoding fails or known fields are lost/corrupted when an unknown field is present. | Automated unit test at the prost message boundary. | New S12c coverage for D-014 agent proto/gRPC additive compatibility. |

## Release-Gate Notes

- The Postgres row deliberately uses the existing transactional-outbox DB-backed test plus a thin
  integration command. It does not introduce a database kill harness.
- The Envoy restart and live xDS resync rows depend on Docker or a local Envoy binary through
  `scripts/e2e-envoy.sh`. Their reproducibility status must be reconciled again in S12d/S12g when
  the project decides whether live Envoy E2E is a GitHub Actions Docker gate or a documented manual
  release gate.
- The agent version-skew row covers only the agent-to-CP diagnostics proto/gRPC surface. REST
  request DTOs remain strict by design.
