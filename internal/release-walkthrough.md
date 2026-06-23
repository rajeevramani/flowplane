# Release Walkthrough

This is the S12g release gate for `v1.0.0`. It reuses existing evidence and harnesses; it does not define new product behavior.

## Live Envoy Gate

Decision for v1.0: live Envoy E2E is a documented manual release gate, not a required GitHub Actions job.

Evidence:

- Runner: `bash scripts/e2e-envoy.sh`
- Report: `../../flowplane-private-vault/archive/repo-import-2026-06-24/internal/e2e/CERTIFICATION-REPORT.md`
- Coverage: `../../flowplane-private-vault/archive/repo-import-2026-06-24/internal/e2e/COVERAGE.md`
- Recorded pass signal: 5 consecutive runs, 12 phases each, 0 known failures, Envoy 1.37.4.

CI-with-Docker promotion is deferred post-1.0. The live runner is broad, Docker-dependent, and has timing-sensitive streaming coverage; S12 keeps the gate recorded instead of refactoring the harness.

The live runner uses the dev-mode dataplane path. Release dataplane bootstrap is still the mTLS path documented in `internal/release/release-packaging.md` and `docs/how-to/production-readiness.md`.

## Seeded Walkthrough

Run the existing live gate:

```bash
bash scripts/e2e-envoy.sh
```

Run release packaging:

```bash
scripts/release/package-release.sh
FLOWPLANE_PACKAGE_IMAGE=1 scripts/release/package-release.sh
```

Verify release artifacts:

```bash
ls target/release-artifacts/flowplane-v0.1.0
cat target/release-artifacts/flowplane-v0.1.0/SHA256SUMS
file target/x86_64-unknown-linux-musl/release/flowplane
```

Operator mTLS dataplane bootstrap form:

```bash
FLOWPLANE_SERVER=https://cp.example \
FLOWPLANE_TOKEN=<operator-token> \
FLOWPLANE_PACKAGE_DATAPLANE=1 \
FLOWPLANE_PACKAGE_TEAM=default \
FLOWPLANE_PACKAGE_DATAPLANE_NAME=edge-1 \
FLOWPLANE_PACKAGE_DATAPLANE_MODE=mtls \
FLOWPLANE_PACKAGE_XDS_HOST=cp.example \
FLOWPLANE_PACKAGE_XDS_PORT=18000 \
FLOWPLANE_PACKAGE_CERT_PATH=/etc/flowplane/tls/tls.crt \
FLOWPLANE_PACKAGE_KEY_PATH=/etc/flowplane/tls/tls.key \
FLOWPLANE_PACKAGE_CA_PATH=/etc/flowplane/tls/ca.crt \
scripts/release/package-release.sh
```

CLI first-contact flow is documented in `docs/how-to/production-readiness.md`; the release gate references that workflow rather than duplicating every operator command here.

## v1.0.0 Tag Checklist

- [ ] All #86 children closed or explicitly accepted; #95 reviewed and accepted.
- [ ] Required CI green: fmt, clippy twice, workspace tests on real Postgres, boot smoke,
      cargo-deny.
- [ ] `../../flowplane-private-vault/archive/repo-import-2026-06-24/internal/adversarial-surface-map.md` is green or accepted.
- [ ] `../../flowplane-private-vault/archive/repo-import-2026-06-24/internal/failure-mode-matrix.md` is green, with live phases covered by the manual gate evidence.
- [ ] Release artifacts reproducible with `scripts/release/package-release.sh`.
- [ ] OCI image reproducible with `FLOWPLANE_PACKAGE_IMAGE=1 scripts/release/package-release.sh`.
- [ ] Operator docs complete: `docs/how-to/production-readiness.md`.
- [ ] `../../flowplane-private-vault/archive/repo-import-2026-06-24/REWRITE-REPORT.md` committed.
- [ ] Accepted risks below are signed off.

Do not create the `v1.0.0` tag until this checklist is reviewed and accepted.

## Accepted Risks

| Risk | Status |
| --- | --- |
| Q-006 license posture | Resolved: Apache-2.0 (`LICENSE`/`NOTICE`). Public distribution no longer license-gated; tag needs only release approval. |
| Live Envoy E2E gate | Manual recorded gate for v1.0; CI-with-Docker promotion deferred post-1.0. |
| P1d AI streaming timing | Certified on Envoy 1.37.4 after #92; keep Envoy version pinned for release evidence. |
| Native `gen_ai.*` OTel semantic-convention metrics | Deferred post-1.0; Flowplane counters cover v1.0 operations. |
