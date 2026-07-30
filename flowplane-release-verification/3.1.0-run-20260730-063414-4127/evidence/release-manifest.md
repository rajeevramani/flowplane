# Flowplane 3.1.0 Release Manifest

- CP binary: `bin/flowplane`
- DP sidecar binary: `bin/flowplane-agent`
- DP sidecar compatibility alias: `bin/fp-agent` (deprecated; use `flowplane-agent`)
- Rate Limit Service binary: `bin/flowplane-rls`
- Binary target: `linux-amd64`
- Linkage: default release artifacts are native GNU/Linux builds for the release runner.
  They may dynamically link glibc/OpenSSL from that environment. For a custom static/musl
  build, set `FLOWPLANE_RELEASE_TARGET=x86_64-unknown-linux-musl` and verify with
  `ldd` or `file`.
- License: Apache-2.0 (Q-006 resolved); see `LICENSE`/`NOTICE`. Public distribution is not license-gated.
- OCI image tag: `flowplane:3.1.0`
- SBOM source artifact: `flowplane-3.1.0.cargo-metadata.sbom.json`
- Checksums: `SHA256SUMS`

Dataplane bootstrap:

```bash
FLOWPLANE_SERVER=... FLOWPLANE_TOKEN=... \
  bin/flowplane --team <team> dataplane bootstrap <dataplane> --mode mtls \
  --xds-host <cp-xds-host> --xds-port 18000 \
  --cert-path /etc/flowplane/tls/tls.crt \
  --key-path /etc/flowplane/tls/tls.key \
  --ca-path /etc/flowplane/tls/ca.crt
```
