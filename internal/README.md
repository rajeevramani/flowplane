# Internal engineering docs

> Internal — engineering; may be stale; **not** product documentation.

This directory holds material written for *building and operating* Flowplane, not for using it. End-user documentation lives in [`../docs/`](../docs/README.md).

## What lives here

Dev and operator runbooks (kept current while in use):

- [`auth0-local-runbook.md`](auth0-local-runbook.md) — local Auth0 setup for development.
- [`prod-local-runbook.md`](prod-local-runbook.md) — local production-like environment.
- [`dev-dataplane.md`](dev-dataplane.md) — running a dev-mode dataplane.
- [`release-walkthrough.md`](release-walkthrough.md) — release gate and v1.0 checklist.
- [`release/release-packaging.md`](release/release-packaging.md) — release packaging procedure.

## Where moved material went

- **Architecture-integrity constitution** (formerly `spec/14`) is now canonical in the vault at `../../flowplane-private-vault/constitution.md`.
- **Historical evidence and process docs** — progress ledgers, the failure-mode matrix, the adversarial surface map, e2e certification/coverage, onboarding and workflow notes — were lifted verbatim into `../../flowplane-private-vault/archive/repo-import-2026-06-24/`. They are historical reference, not maintained.

## Rules

- **No product truth originates here.** If a user needs it to operate Flowplane, it belongs in `../docs/` as a stand-alone page; internal docs link to it.
- **Internal docs may link into `../docs/`.** The reverse is constrained: `docs/` content pages must not cite `internal/` as required reading (CI carves out the `docs/README.md` index and bucket-index links so navigation still works).
