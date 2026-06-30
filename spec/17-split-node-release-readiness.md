# 17 - Split-Node Release Readiness Verification

> Status: verification report
> Scope: current published release `v2.1.0`
> Verdict rule: fail closed. Any `FAIL` or `UNVERIFIED` means the checked area is not ready.

## v2.1.1-rc Branch Runtime Addendum

The original report below verifies the **published `v2.1.0` surface** and correctly marks it
`NOT READY` for no-clone split-node deployment. A follow-up runtime rehearsal on branch
`feature/fpv2-0ym-adoption-evaluation-spine` at commit
`aaeef638295d4b97d26a9daa542e20cd33cf9c7e` verifies the branch's packaged
`2.1.1-rc` Linux amd64 artifacts:

- Companion report: `spec/17-split-node-runtime-rehearsal-v2.1.1-rc.md`.
- Packaged archive contained `flowplane`, `flowplane-agent`, `flowplane-rls`, and the deprecated
  `fp-agent` compatibility symlink.
- Runtime proof used separate Docker bridge identities for Postgres, local JWKS IdP, CP, Envoy,
  `flowplane-agent`, and `flowplane-rls`.
- CP listened on `0.0.0.0:8080` and `0.0.0.0:18000`.
- The generated Envoy bootstrap used `flowplane-cp:18000` for `xds_cluster`, not localhost.
- Envoy and `flowplane-agent` connected to CP xDS/diagnostics over mTLS with certificate-registry
  binding.
- CP `/healthz` and `/readyz`, agent `/healthz`, RLS `/healthz` and `/readyz`,
  `flowplane stats overview`, and `flowplane ops xds status` all passed.

Updated readiness interpretation:

| Surface | Verdict | Reason |
| --- | --- | --- |
| Published `v2.1.0` | NOT READY | Release assets lack standalone operator/agent/RLS binaries and docs cannot be completed no-clone. |
| Branch `2.1.1-rc` artifacts | READY for branch runtime evidence | Split-node artifact, mTLS, agent, RLS, health, and ops checks passed in a network-isolated rehearsal. |
| Published `v2.1.1` | NOT READY until release | No `v2.1.1` GitHub Release/GHCR assets exist yet. Do not close issue #220 until real published assets are verified. |

## Release Under Review

Current published release:

```text
$ git rev-parse v2.1.0
b35db069ef59df274b3710d5d6b3d36a3143656e

$ git show -s --format='%H %D %s' v2.1.0
73cc5f84f36f5b6e7adbf24faebbf95b526436cf tag: v2.1.0, origin/main, main chore(release): bump workspace version to 2.1.0 (#198)
```

`v2.1.0` is an annotated tag object (`b35db069...`) pointing at commit `73cc5f84...`.

Published GitHub Release assets:

```text
$ gh release view v2.1.0 --json tagName,name,assets,url,targetCommitish,publishedAt,isDraft,isPrerelease
{"assets":[{"name":"compose.eval.yml","size":6236,"url":"https://github.com/rajeevramani/flowplane/releases/download/v2.1.0/compose.eval.yml"}],"isDraft":false,"isPrerelease":false,"name":"v2.1.0","publishedAt":"2026-06-28T00:30:39Z","tagName":"v2.1.0","targetCommitish":"main","url":"https://github.com/rajeevramani/flowplane/releases/tag/v2.1.0"}
```

Binary-producing workspace members at `v2.1.0`:

- `Cargo.toml:3-11` includes `crates/fp-agent`, `crates/flowplane`, and `crates/flowplane-rls`.
- `Cargo.toml:15` sets workspace version `2.1.0`.
- `Containerfile.release:4-12` builds and copies only `flowplane` and `fp-agent` into the hardened image.
- `Containerfile.eval:11-19` builds and copies only `flowplane` and `fp-agent` into the evaluation image.
- `crates/flowplane-rls/Cargo.toml:1-12` defines a separate `flowplane-rls` binary, but it is not built into either release image and is not attached as a GitHub Release asset.
- `.github/workflows/release.yml:1-7` says the release workflow publishes GHCR images.
- `.github/workflows/release.yml:249-254` attaches only `compose.eval.yml` as a release asset.

Therefore, the published `v2.1.0` install surface is:

- GHCR hardened image `ghcr.io/rajeevramani/flowplane:2.1.0`, containing `flowplane` and `fp-agent`.
- GHCR eval image `ghcr.io/rajeevramani/flowplane:2.1.0-eval`, containing `flowplane` and `fp-agent`.
- GitHub Release asset `compose.eval.yml`.
- No standalone downloadable `flowplane`, `fp-agent`, or `flowplane-rls` binary tarball.

## Verification Inputs And Method

Artifacts under test are the published `v2.1.0` release artifacts above. Documentation evidence is read from the `v2.1.0` tag unless a row explicitly names a different ref. If a future check compares feature-branch docs against `v2.1.0` artifacts, that doc/runtime skew must be called out in the evidence instead of treated as a product failure.

The binaries relevant to split-node readiness are explicit:

- `flowplane`: control-plane server and operator/API-team CLI.
- `fp-agent`: v2.1.0 dataplane diagnostics sidecar binary. The intended public name for future releases is `flowplane-agent`.
- `flowplane-rls`: global rate-limit service binary.
- Envoy: external dataplane process/image, not built by Flowplane.

Cross-node runtime proof requires more than source inspection. A passing runtime proof must run CP and DP in separate network identities, for example separate VMs or containers on a user-defined bridge where the DP reaches CP through a non-loopback address. The proof must capture:

- CP listening on non-loopback API and xDS addresses.
- A generated Envoy bootstrap whose `xds_cluster` target is the remote CP host/port, not `127.0.0.1` or `localhost`.
- Envoy and the dataplane agent connecting to CP over the documented mTLS path.
- CP `/healthz` and `/readyz`, agent `/healthz`, xDS status, and RLS health/readiness where RLS is part of the deployment.

The architecture-integrity constitution is a precondition for the final cross-check, not a product runtime dependency. If `flowplane-private-vault/constitution.md` is unavailable, the constitution cross-check is `UNVERIFIED` and must be reported separately; it should not be confused with a binary split-node failure. This run had the vault available.

## Verdict Table

| Check | A/B | Status | Evidence |
| --- | --- | --- | --- |
| 1. Published artifact availability | A | FAIL | Split-node requires an operator/CP CLI (`flowplane`), a DP agent (`fp-agent` in v2.1.0, intended future public name `flowplane-agent`), Envoy, and RLS when global rate limiting is enabled. `gh release view v2.1.0` shows only `compose.eval.yml` as a GitHub Release asset. `.github/workflows/release.yml:249-254` attaches only `compose.eval.yml`. The release images contain `flowplane` and `fp-agent` (`Containerfile.release:4-12`; `Containerfile.eval:11-19`), so the agent can be obtained by running the combined image, but no standalone binary tarballs/checksums are published for operator installation and `flowplane-rls` is not included in the images or release assets. |
| 2. Network binding | A | PASS | CP API and xDS bind addresses are configurable and default non-loopback: `FLOWPLANE_API_ADDR` resolves from env/file/default `0.0.0.0:8080` in `crates/fp-core/src/config.rs:153,182-190`; `FLOWPLANE_XDS_ADDR` resolves from env/file/default `0.0.0.0:18000` in `crates/fp-core/src/config.rs:193-201`. API server binds the resolved address in `crates/flowplane/src/serve.rs:236-266`; xDS server passes resolved `xds_addr` to `serve_mtls` or dev plaintext in `crates/flowplane/src/serve.rs:140-181`. RLS listen defaults are non-loopback in `docs/reference/configuration.md:59-60`. Hardcoded loopback remains intentional for Envoy admin inside the DP unit: `crates/fp-api/src/dataplanes_api.rs:644-648` and `crates/fp-agent/src/main.rs:32-37`. |
| 3. DP bootstrap remote CP address | A | UNVERIFIED | Source inspection shows the intended mechanism: the CLI accepts `--xds-host` and `--xds-port`, defaulting to `127.0.0.1` / `18000`, in `crates/flowplane/src/cli/commands.rs:966-974`; CLI forwards those query params in `crates/flowplane/src/cli/mod.rs:1861-1865`; REST bootstrap query names `xds_host` as the host/DNS Envoy uses to reach CP in `crates/fp-api/src/dataplanes_api.rs:188-198`; rendering places `{xds_host}` / `{xds_port}` into the `xds_cluster` socket address in `crates/fp-api/src/dataplanes_api.rs:673-681`. However, no captured command output in this verification generated bootstrap from a running CP with a remote, non-loopback `--xds-host`, so the release behavior remains unverified. Localhost default is still a documented hazard for split-node use if the operator omits `--xds-host`. |
| 4. Transport security | A | PASS | Production xDS is mTLS-or-disabled: when `config.xds_tls` exists, xDS starts with `serve_mtls` and cert/key/client-CA paths in `crates/flowplane/src/serve.rs:140-163`; without xDS TLS and outside dev mode the listener is disabled with a warning in `crates/flowplane/src/serve.rs:186-190`. Config validates the xDS TLS triad all-or-none in `crates/fp-core/src/config.rs:248-270`, with tests for partial rejection in `crates/fp-core/src/config.rs:617-642`. Envoy bootstrap requires cert/key/CA in `mtls` mode in `crates/flowplane/src/cli/mod.rs:1853-1859` and `crates/fp-api/src/dataplanes_api.rs:568-587`. Agent diagnostics supports mTLS materials and TLS server verification in `crates/fp-agent/src/main.rs:51-65,106-121,241-258`. |
| 5. DP AuthN/AuthZ and secret handling | A | PASS | `spec/04-xds.md:33-48` specifies DP identity as SPIFFE URI plus database certificate binding, and explicitly says `node.metadata.team` is never authorization. The API issues certificates at `POST /api/v1/teams/{team}/proxy-certificates/issue` and the returned view includes `certificate_pem`, `private_key_pem`, and `ca_certificate_pem` once in `crates/fp-api/src/dataplanes_api.rs:116-130`; v2.1.0 docs state Flowplane never stores the private key in `docs/how-to/register-dataplane-mtls.md:39-70`. Agent TLS client identity is loaded from cert/key files in `crates/fp-agent/src/main.rs:245-256`. |
| 6. Health/readiness with CP and DP on different hosts | A | UNVERIFIED | CP `/healthz` and `/readyz` exist (`crates/fp-api/src/routes.rs` found via grep at routes `238-239` and handlers `344-366`); agent `/healthz` exists and requires recent Envoy admin poll plus diagnostics ack in `crates/fp-agent/src/main.rs:340-382`; RLS `/healthz` and `/readyz` exist in `crates/flowplane-rls/src/admin.rs` per grep output. However, no command output in this verification proves these endpoints working with CP and DP on separate hosts. |
| 7. Split-node mode documented | B | PASS | `docs/how-to/production-readiness.md:10-13` says to deploy control plane and dataplane bundle separately. `docs/how-to/register-dataplane-mtls.md:91-103` documents an agent dialing `https://cp.example.com:18000`. |
| 8. Documentation completeness | B | FAIL | Docs cover some pieces but not all. They cover CP xDS/API config in `docs/how-to/production-readiness.md:16-28,51-68`, DP cert issuance in `docs/how-to/register-dataplane-mtls.md:39-89`, and agent startup in `docs/how-to/register-dataplane-mtls.md:91-116`. Missing or incomplete: no no-clone install path for `fp-agent`/future `flowplane-agent`; no no-clone install path for `flowplane-rls`; no DP-only image or binary artifact path; no complete two-host example with ports/firewall; no release asset download commands for operator binaries; no cert rotation runbook for the DP agent; no fully self-contained RLS production deployment path. |
| 9. Documentation accuracy | B | FAIL | Drift/gaps: v2.1.0 docs use `fp-agent` (`docs/how-to/register-dataplane-mtls.md:5,91-103`), while the preferred public name is `flowplane-agent`; `docs/how-to/global-rate-limit.md:19` says `flowplane-rls` can be built with `cargo build` or a release artifact, but no release artifact exists; `docs/how-to/production-readiness.md:32-44` tells operators to run repo-local `scripts/release/package-release.sh`, which violates no-clone operator documentation. The GitHub Release asset output above proves only `compose.eval.yml` is published. |
| 10. Runnable by reader on two hosts using only docs | B | FAIL | Not runnable. A reader cannot obtain standalone `fp-agent`/`flowplane-agent` or `flowplane-rls` from `v2.1.0` GitHub Release assets; release workflow only attaches `compose.eval.yml` (`.github/workflows/release.yml:249-254`). The combined GHCR image contains `fp-agent`, but the docs do not present a no-clone split-node path that uses that image as the DP agent artifact. The docs also require a repo-local packaging script (`docs/how-to/production-readiness.md:32-44`) and source build fallback for RLS (`docs/how-to/global-rate-limit.md:19`). |
| 11. Operations: upgrade/rollback/version-skew/troubleshooting | B | FAIL | `docs/how-to/production-readiness.md:47-49` briefly says CP/DP upgrade order is independent and existing DPs keep serving last-applied config during CP restart. Troubleshooting table covers generic connectivity/TLS signals in `docs/how-to/production-readiness.md:97-109`. Missing: explicit version-skew policy between CP, Envoy bootstrap, agent, and RLS; rollback steps; agent/RLS binary compatibility matrix; cross-node TLS/SNI/firewall troubleshooting examples; no-clone artifact validation for all required binaries. |
| 12. Location/discoverability | B | PASS | `docs/README.md:54-61` says implementation truth is code/tests, user/operator truth is canonical docs pages, deployment examples link to canonical pages, and design rationale is `spec/` plus issues. `docs/README.md:20-30` says public docs must be completable without `spec/` or `internal/`. This report belongs in `spec/` as behavioral/release-readiness analysis; public operator fixes belong in `docs/how-to` / `docs/reference`; release evidence is canonical in the vault per constitution boundary `flowplane-private-vault/constitution.md:81-82` and docs policy `docs/README.md:7`. |

## Binary / Runtime Sub-Verdict

**BINARY READY? NO.**

Runtime code has strong split-node primitives:

- API and xDS bind addresses are configurable and default to non-loopback.
- Envoy bootstrap can point to a remote CP host/port.
- xDS production path is mTLS-or-disabled.
- DP identity is certificate/SPIFFE plus registry binding.
- Agent diagnostics can connect outbound to a remote CP over TLS/mTLS.

Blocking binary/runtime items:

1. `v2.1.0` publishes no standalone operator binary artifacts. The release workflow attaches only `compose.eval.yml`, and the release page contains no tarballs for `flowplane`, `fp-agent`/`flowplane-agent`, or `flowplane-rls`.
2. The release images include `fp-agent`, not the intended public binary name `flowplane-agent`.
3. `flowplane-rls` is a separate binary crate, but it is neither in the release images nor attached as a release asset.
4. No captured split-node runtime command output proves CP health, DP agent health, xDS status, and RLS health with CP and DP on separate hosts.

## Documentation Sub-Verdict

**DOCS READY? NO.**

The docs describe the architecture direction, but they do not make the split-node deployment completable by an outside operator using only published artifacts.

Blocking documentation items:

1. The docs require or imply binaries that are not downloadable from the current release.
2. The docs still rely on repo-local/source-oriented paths (`scripts/release/package-release.sh`, `cargo build --bin flowplane-rls`) for operator flows.
3. The dataplane agent is documented as `fp-agent`; the desired public/operator name is `flowplane-agent`.
4. The RLS path is under-documented for no-clone production-shaped evaluation.
5. Operations coverage lacks explicit version-skew, rollback, and split-node troubleshooting details.

## Overall Verdict

**OVERALL: NOT READY.**

Both sub-verdicts must pass for split-node deployment readiness. Binary/runtime is not ready because the current release does not publish all required operator binaries and lacks split-node runtime proof. Documentation is not ready because a reader cannot stand up CP and DP on separate hosts using only public docs and published artifacts.

## Constitution Cross-Check

The fail-closed verdict is required by the architecture-integrity constitution:

- The constitution requires CP and dataplane to be separate deployment units and says localhost/loopback is dev-only, with dataplane units connecting outbound to CP and generated bootstrap carrying resolved addresses/ports/cert paths explicitly (`flowplane-private-vault/constitution.md:43`).
- It requires tenant isolation by mTLS certificate-registry binding, not self-reported node metadata (`flowplane-private-vault/constitution.md:45`).
- It requires fail-closed security boundaries: production xDS is mTLS-or-off, partial TLS config fails boot, dev plaintext is explicit/gated (`flowplane-private-vault/constitution.md:49`).
- It requires deployment packaging to preserve security boundaries, with runtime secrets/certs from deployment secret mechanisms and dataplane control traffic over CP-terminated mTLS (`flowplane-private-vault/constitution.md:50`).
- It forbids Envoy admin as an operator/product API and requires diagnostics through the agent/CP path (`flowplane-private-vault/constitution.md:51`).

The code mostly aligns with these invariants. The release packaging and public docs do not yet provide a no-clone, split-node-completable operator path, so readiness cannot be declared.

## Required Blocking Fixes Before READY

1. Publish standalone release tarballs/assets for supported platforms containing:
   - `flowplane`
   - `flowplane-agent`
   - `flowplane-rls`
   - checksums and install/verify instructions
2. Rename the public dataplane agent binary and docs from `fp-agent` to `flowplane-agent`; keep any internal crate name as implementation detail only.
3. Include `flowplane-rls` in release packaging or publish a documented container/artifact for it.
4. Replace public docs that require `scripts/release/package-release.sh` or `cargo build` for operator evaluation.
5. Add a complete split-node runbook covering:
   - CP bind/listen config
   - remote DP bootstrap with `--xds-host`
   - xDS mTLS server/client CA roles
   - dataplane cert issue/register/rotation/revocation
   - agent and RLS install/run commands from release artifacts
   - ports and firewall matrix
   - health/readiness/xDS/RLS checks
   - upgrade/rollback/version-skew policy
   - troubleshooting for DNS, TCP, TLS, SPIFFE/cert binding, NACKs, and diagnostics acks
6. Capture and attach runtime evidence from a real two-node or network-isolated rehearsal.
