# 17 — Split-Node Runtime Rehearsal (v2.1.1-rc branch evidence)

> Status: runtime verification report (companion to `spec/17-split-node-release-readiness.md`)
> Scope: branch `feature/fpv2-0ym-adoption-evaluation-spine`, packaged `2.1.1-rc` Linux artifacts
> Verdict rule: fail closed. Any `FAIL` or `UNVERIFIED` means the checked area is NOT READY.
> Canonicality note: per the architecture-integrity constitution, release evidence is canonical
> in `../flowplane-private-vault`. This file is the in-repo delivery copy of that evidence for the
> branch work; the vault remains the canonical home.

## Post-Release Update

`v2.1.1` is now published from tag commit `cbb40e4b3ed75fa6275b8594e89abc01ae956fed`. The GitHub
Release contains `flowplane-2.1.1-linux-amd64.tar.gz`, `flowplane-2.1.1-linux-arm64.tar.gz`, the
per-arch cargo metadata SBOMs, `SHA256SUMS`, and `compose.eval.yml`. The pre-release row below that
marked published assets as `FAIL` is retained as historical evidence for the branch rehearsal; it is
superseded by the published `v2.1.1` release assets.

This report supersedes the *source-inspection-only* `UNVERIFIED` rows in
`spec/17-split-node-release-readiness.md` (checks 3 and 6) with **captured runtime evidence** from a
real, network-isolated split-node rehearsal, and re-checks the prior `FAIL` rows (published binary
artifacts, `fp-agent` naming, missing `flowplane-rls`) against the branch.

## Runtime Environment Under Test

| Field | Value |
| --- | --- |
| Branch | `feature/fpv2-0ym-adoption-evaluation-spine` |
| Branch HEAD SHA | `aaeef638295d4b97d26a9daa542e20cd33cf9c7e` |
| Relevant branch commits | `ef857ac fpv2-snr.1 publish split-node artifacts`, `aaeef63 fpv2-snr.2 document split-node release path` |
| Artifact under test | `flowplane-2.1.1-rc-linux-amd64.tar.gz` (built from this branch) |
| Compiled-in version | `2.1.0` (workspace `Cargo.toml` version; `-rc` affects packaging/naming only — see Observations) |
| OS / arch | Ubuntu 24.04.4 LTS (Noble), `x86_64`, kernel `6.18.5` |
| Container runtime | Docker `29.3.1` (build c2be9cc); Podman: not installed |
| Rust | `rustc 1.94.1 (e408947bf 2026-03-25)`, `cargo 1.94.1` |
| Envoy | `envoyproxy/envoy:v1.37-latest` (the image pinned by `compose.eval.yml`) |
| Build command | `FLOWPLANE_RELEASE_VERSION=2.1.1-rc FLOWPLANE_RELEASE_HOST=linux-amd64 scripts/release/package-release.sh` |

All binaries exercised in the proof path are the **packaged release-style binaries** from the
unpacked archive; `target/debug` and source-tree-only binaries were not used.

## Verdict Table

| Check | Area | Status | Evidence |
| --- | --- | --- | --- |
| 1. Release artifact build | Binary | PASS | `package-release.sh` produced `flowplane-2.1.1-rc-linux-amd64.tar.gz` (16 MB), `*.cargo-metadata.sbom.json`, `SHA256SUMS`. `sha256sum -c SHA256SUMS` → both `OK`. |
| 2. Archive contents | Binary | PASS | Tarball contains `bin/flowplane`, `bin/flowplane-agent`, `bin/flowplane-rls`, `bin/fp-agent` (symlink → `flowplane-agent`), `release-manifest.md`. `SHA256SUMS` is published alongside the tarball (it checksums it), per standard release layout. |
| 3. Binary smoke checks | Binary | PASS | `flowplane --help`, `flowplane-agent --help`, `fp-agent --help` (alias usage prints `fp-agent`) all emit usage. `timeout 5s flowplane-rls` → exit `124` (ran until killed; startup log `flowplane-rls starting grpc=0.0.0.0:50051 admin=0.0.0.0:8081`). |
| 4. CP non-loopback bind | Runtime | PASS | `FLOWPLANE_API_ADDR=0.0.0.0:8080`, `FLOWPLANE_XDS_ADDR=0.0.0.0:18000`. In-namespace `/proc/net/tcp` shows `LISTEN 0.0.0.0:8080` (`00000000:1F90`) and `LISTEN 0.0.0.0:18000` (`00000000:4650`) — both `0.0.0.0`, not `127.0.0.1`. CP log: `xDS ADS server starting (mTLS, certificate-registry binding) addr 0.0.0.0:18000`. |
| 5. Envoy bootstrap → remote CP | Runtime | PASS | CLI-generated bootstrap. `grep` over `envoy.yaml`: only `127.0.0.1` is the Envoy **admin** (`:9901`, loopback by design); `xds_cluster` endpoint is `flowplane-cp:18000`. mTLS `UpstreamTlsContext` references client cert/key + `server-ca.crt`. |
| 6. xDS mTLS cross-node | Runtime | PASS | Envoy admin: `control_plane.connected_state: 1`, `cluster.xds_cluster.ssl.handshake: 1`, `upstream_cx_connect_fail: 0`, `xds_cluster::172.18.0.4:18000::health_flags::healthy`. CP log: `dataplane authenticated via certificate registry … dataplane connected`. No TLS/auth failures in CP or Envoy logs. |
| 7. Agent cross-node | Runtime | PASS | `flowplane-agent` (separate container) → `--cp-endpoint https://flowplane-cp:18000` mTLS, `--envoy-admin-url http://flowplane-envoy:9901`. Agent `/healthz` → `ok`. CP log: `dataplane authenticated via certificate registry … node: diagnostics`. `dataplane get` shows `last_heartbeat_at` advancing. |
| 8. CP health/readiness | Runtime | PASS | `/healthz` → `{"status":"ok","version":"2.1.0"}`; `/readyz` → `{"status":"ready","checks":[{database,ok},{xds_outbox_consumer,ok},{xds_outbox_lag,ok}]}`. |
| 9. RLS health/readiness | Runtime | PASS | `flowplane-rls` (separate container) `/healthz` → HTTP `200`, `/readyz` → HTTP `200`. |
| 10. CLI operational checks | Runtime | PASS | `flowplane stats overview --team payments` → `live_dataplanes: 1`. `flowplane ops xds status --team payments` → `health: healthy`, `recent_nack_count: 0`, dataplane `live: true`. |
| 11. DP identity = cert-registry binding | Runtime | PASS | Issued client cert SAN `URI:spiffe://flowplane.local/org/<org>/team/<team>/proxy/<dp-id>`, issuer `flowplane-dp-issuer-ca` (from `FLOWPLANE_CERT_ISSUER_CA_*`). CP authenticated the stream "via certificate registry", not node metadata. |
| 12. Split-node network identities | Runtime | PASS | 6 containers with distinct bridge IPs on `flowplane-net` (CP `172.18.0.4`, Envoy `172.18.0.5`, agent `172.18.0.6`). DP reaches CP by bridge IP, never `127.0.0.1`. All host port publications bound to `127.0.0.1` only. |
| 13. Published v2.1.1 release assets | Binary/Docs | **FAIL** | `gh`/Releases API: latest published release is **`v2.1.0`** (assets: `compose.eval.yml` + source archives only). **No `v2.1.1` or `v2.1.1-rc` release exists.** The branch wires `release.yml` to attach binary tarballs/SBOM/`SHA256SUMS` and smoke-test all three binaries, but it is **unmerged and unpublished**. |

## Topology Used

```
                     Docker bridge: flowplane-net (172.18.0.0/16)
  flowplane-postgres(.2)  flowplane-idp(.3)  flowplane-cp(.4)
  flowplane-envoy(.5)     flowplane-agent(.6)  flowplane-rls(.7)
```

- CP↔DP traffic crosses the bridge by DNS name / bridge IP — never localhost.
- Throwaway local CA material: a `flowplane-xds-server-ca` (signs the CP xDS **server** cert, SAN
  `flowplane-cp`) and a separate `flowplane-dp-issuer-ca` (`FLOWPLANE_CERT_ISSUER_CA_*`; the CP mints
  DP **client** certs from it and trusts it as `FLOWPLANE_XDS_TLS_CLIENT_CA`).
- Throwaway local IdP: an nginx container (`flowplane-idp`) serving a static JWKS; CP configured with
  `FLOWPLANE_OIDC_ISSUER`/`FLOWPLANE_OIDC_AUDIENCE`/`FLOWPLANE_OIDC_JWKS_URI`. Platform admin
  bootstrapped with test OIDC subject `oidc-rehearsal-admin`; a locally-minted RS256 JWT validated
  against the JWKS (`whoami` → `platform_admin: true`). No real credentials or external IdP.

## Exact Commands (abridged, secrets redacted)

```bash
# Build
FLOWPLANE_RELEASE_VERSION=2.1.1-rc FLOWPLANE_RELEASE_HOST=linux-amd64 scripts/release/package-release.sh
sha256sum -c SHA256SUMS
tar xzf flowplane-2.1.1-rc-linux-amd64.tar.gz -C <staging>

# Smoke
bin/flowplane --help; bin/flowplane-agent --help; bin/fp-agent --help; timeout 5s bin/flowplane-rls

# CP xDS mTLS + cert issuer + OIDC (env-file, key/token redacted)
FLOWPLANE_API_ADDR=0.0.0.0:8080  FLOWPLANE_XDS_ADDR=0.0.0.0:18000  FLOWPLANE_API_INSECURE=true
FLOWPLANE_XDS_TLS_CERT=/etc/flowplane/tls/xds-server.crt  FLOWPLANE_XDS_TLS_KEY=…  FLOWPLANE_XDS_TLS_CLIENT_CA=/etc/flowplane/ca/issuer-ca.crt
FLOWPLANE_CERT_ISSUER_CA_CERT_PATH=/etc/flowplane/ca/issuer-ca.crt  FLOWPLANE_CERT_ISSUER_CA_KEY_PATH=…  FLOWPLANE_CERT_ISSUER_TRUST_DOMAIN=flowplane.local
FLOWPLANE_OIDC_ISSUER=https://idp.flowplane.local  FLOWPLANE_OIDC_AUDIENCE=flowplane  FLOWPLANE_OIDC_JWKS_URI=http://flowplane-idp/jwks.json
flowplane db migrate && flowplane serve

# Bootstrap + provision (admin JWT)
curl -X POST .../api/v1/bootstrap/initialize -H "Authorization: Bearer <bootstrap-token>" -d '{"org_name":"platform",…,"admin_subject":"oidc-rehearsal-admin",…}'
flowplane org create rehearsal-org;  flowplane org member add rehearsal-org --role owner --subject oidc-rehearsal-admin
flowplane --org rehearsal-org team create payments
flowplane --org rehearsal-org dataplane create edge-gateway-1 --team payments
flowplane --org rehearsal-org dataplane cert issue edge-gateway-1 --team payments --ttl-hours 24

# Bootstrap pointing at remote CP DNS
flowplane --org rehearsal-org --out envoy.yaml dataplane bootstrap edge-gateway-1 --team payments --mode mtls \
  --xds-host flowplane-cp --xds-port 18000 --cert-path … --key-path … --ca-path /etc/flowplane/dp/server-ca.crt
grep -n "flowplane-cp\|127.0.0.1\|localhost" envoy.yaml

# Envoy + agent + RLS (separate containers on flowplane-net)
docker run … envoyproxy/envoy:v1.37-latest -c /etc/envoy/envoy.yaml
flowplane-agent --envoy-admin-url http://flowplane-envoy:9901 --cp-endpoint https://flowplane-cp:18000 \
  --dataplane-id <uuid> --tls-cert-path … --tls-key-path … --tls-ca-path … --tls-server-name flowplane-cp --health-bind-addr 0.0.0.0:19902
flowplane-rls   # FLOWPLANE_RLS_GRPC_LISTEN=0.0.0.0:50051 FLOWPLANE_RLS_ADMIN_LISTEN=0.0.0.0:8081

# Checks
curl .../healthz .../readyz   # CP and RLS
curl http://127.0.0.1:<agent>/healthz
flowplane stats overview --team payments;  flowplane ops xds status --team payments
```

## Rehearsal Accommodations (documented, non-product)

1. **API plaintext** (`FLOWPLANE_API_INSECURE=true`): the operator/API hop ran plaintext behind the
   loopback-only host mapping to avoid standing up a second TLS endpoint. The **xDS** path — the hop
   that matters for split-node security — was full mTLS. The CP logged the expected plaintext-API
   warning.
2. **Envoy admin rebind for a two-container split:** the *generated* bootstrap correctly defaults
   admin to `127.0.0.1` (Check 5 evidence — the loopback-only invariant holds by default). For this
   rehearsal the admin address was rebound to `0.0.0.0` **only** so the separate `flowplane-agent`
   container could reach `flowplane-envoy:9901` (the task topology) and so the host could capture
   `/clusters`/`/config_dump`. Admin was published to `127.0.0.1` on the host only, never to a public
   interface. In a real deployment the agent is a dataplane-local sidecar sharing Envoy's namespace
   and reaches admin over loopback.

## Sub-Verdicts

- **BINARY READY (branch artifacts + runtime)? PASS.** The packaged `2.1.1-rc` artifacts contain all
  three operator binaries plus the `fp-agent` compatibility alias, pass smoke checks, and run a full
  cross-node split-node topology: CP binds non-loopback, Envoy bootstrap targets the remote CP DNS,
  xDS connects over mTLS with certificate-registry binding, the agent connects cross-node, and CP/RLS
  health/readiness and CLI ops all succeed with zero TLS/auth failures and zero NACKs.
- **DOCS READY (branch content)? PASS.** `docs/how-to/production-readiness.md` now provides a no-clone
  install path (download tarball + `SHA256SUMS` verify + `install`), uses `flowplane-agent` (with
  `fp-agent` called out as a deprecated alias), documents `flowplane-rls`, ports/firewall matrix,
  upgrade/rollback, version-skew, and split-node TLS/bootstrap troubleshooting.
  `register-dataplane-mtls.md` and `global-rate-limit.md` now point at published artifacts. Caveat:
  these docs reference `2.1.1` download URLs that **do not yet resolve** until v2.1.1 is published.
- **PUBLISHED-ARTIFACT READY (v2.1.1 release)? FAIL.** No `v2.1.1` GitHub Release or GHCR tag exists;
  `v2.1.0` still publishes only `compose.eval.yml`. The fix is present in branch code but unmerged.

## Overall Verdict

- **BRANCH split-node readiness: READY.** Every runtime and artifact-build check passed against the
  packaged `2.1.1-rc` binaries from this branch. The prior report's `UNVERIFIED` runtime rows (xDS
  mTLS, cross-host health) are now `PASS` with captured evidence, and its `FAIL` rows for binary
  packaging (`flowplane-rls` absent, `fp-agent` naming, no standalone tarballs) are resolved in branch
  artifacts and the release workflow.
- **PUBLISHED v2.1.1 release readiness: NOT READY.** Until the branch merges and a `v2.1.1` tag runs
  the updated `release.yml` and attaches the tarballs/SBOM/`SHA256SUMS` (and the docs' `2.1.1` URLs
  resolve), the published surface is unchanged from `v2.1.0`.

## Blocking Items (most severe first)

1. **No published v2.1.1 artifacts (issue #220 blocker).** Latest release is `v2.1.0` with only
   `compose.eval.yml`. **Do not close issue #220** on this evidence — local `2.1.1-rc` artifacts prove
   branch readiness but are not published assets. Closing requires a real `v2.1.1` GitHub Release/GHCR
   build verified to contain `flowplane`, `flowplane-agent`, `flowplane-rls`, checksums, and SBOM.
2. **Docs reference unpublished `2.1.1` download URLs.** `production-readiness.md` install commands
   resolve only after v2.1.1 is published; this is doc/runtime skew until the release ships, not a
   code defect.

## Observations (non-blocking)

- **Compiled version vs artifact label.** `flowplane --version` / `flowplane-agent --version` and CP
  `/healthz` report `2.1.0` (the workspace `Cargo.toml` version on this branch).
  `FLOWPLANE_RELEASE_VERSION=2.1.1-rc` only renames/labels the artifact; it does not bump the
  compiled-in version. The real v2.1.1 release path bumps the workspace version (the `resolve-version`
  job + version-bump commit), so published binaries will self-report `2.1.1`. Worth confirming at tag
  time.
- `flowplane-rls` has no `--version` flag (it is a long-running service binary); `--help`/smoke run is
  the appropriate liveness check.

## Teardown

All rehearsal containers (`flowplane-postgres`, `flowplane-idp`, `flowplane-cp`, `flowplane-envoy`,
`flowplane-agent`, `flowplane-rls`), the `flowplane-net` bridge network, and the ephemeral Postgres
volume were removed; `docker ps -a`/`network ls`/`volume ls` confirm none remain. All tokens,
bootstrap tokens, and private keys were redacted from this report.
