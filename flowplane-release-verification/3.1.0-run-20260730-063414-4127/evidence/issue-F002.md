## Summary

The `release-manifest.md` shipped inside the published Linux binary tarball names the SBOM as `flowplane-3.1.0.cargo-metadata.sbom.json`, but no release asset by that exact name exists. The published SBOM assets are architecture-suffixed: `flowplane-3.1.0-linux-amd64.cargo-metadata.sbom.json` and `flowplane-3.1.0-linux-arm64.cargo-metadata.sbom.json`. A reader who looks up the SBOM by the manifest's name will not find a matching asset.

## Release identity

- Requested version: 3.1.0
- Git tag / commit: `v3.1.0` → `a9ea214fae7dda12aa79554942129b800d9a6fe3`
- Artifact under test: `flowplane-3.1.0-linux-amd64.tar.gz` (SHA-256 `4a12db0e6c0a8beb72b43b82df27e2223767e41c2472f1cb7cf2ed5c5597ff95`, verified against the release `SHA256SUMS`), file `release-manifest.md` inside it.
- Executed platform: linux/amd64

## Finding / Test IDs

- F-002 (release-verification asset/manifest reconciliation, Phase 0/Phase 7)

## Affected documentation

- `release-manifest.md` (inside `flowplane-3.1.0-linux-amd64.tar.gz`), the line:

```
- SBOM source artifact: `flowplane-3.1.0.cargo-metadata.sbom.json`
```

## Prerequisites

None beyond downloading the release assets.

## Steps to reproduce

1. Download and checksum-verify the tarball:
   `curl -fsSLO https://github.com/rajeevramani/flowplane/releases/download/v3.1.0/flowplane-3.1.0-linux-amd64.tar.gz`
2. Extract and read `flowplane-3.1.0-linux-amd64/release-manifest.md`.
3. Note the SBOM artifact name it lists.
4. Compare with the actual release assets (GitHub Release `v3.1.0` / `SHA256SUMS`): the SBOMs are `flowplane-3.1.0-linux-amd64.cargo-metadata.sbom.json` and `flowplane-3.1.0-linux-arm64.cargo-metadata.sbom.json`. No `flowplane-3.1.0.cargo-metadata.sbom.json` is published.

## Expected behavior

The manifest's SBOM reference should match a real published asset name (e.g. `flowplane-3.1.0-linux-amd64.cargo-metadata.sbom.json`, matching the tarball's own `Binary target: linux-amd64`), or the release should publish an asset by the exact name the manifest states.

## Actual behavior

The manifest references `flowplane-3.1.0.cargo-metadata.sbom.json`, which is not a published asset; the published SBOMs carry the `-linux-<arch>` suffix.

## Error evidence

```
# Published v3.1.0 SBOM assets (from SHA256SUMS):
flowplane-3.1.0-linux-amd64.cargo-metadata.sbom.json
flowplane-3.1.0-linux-arm64.cargo-metadata.sbom.json

# release-manifest.md (inside the amd64 tarball) states:
- SBOM source artifact: `flowplane-3.1.0.cargo-metadata.sbom.json`
```

## Corrected diagnostic or workaround

Not applicable. The SBOM does exist and verifies against `SHA256SUMS` under its arch-suffixed name; only the manifest's stated name is wrong.

## Impact

Low. Provenance/discoverability only — an operator cross-referencing the SBOM by the manifest's name finds no such file. The SBOM content itself is published and checksum-verifiable.

## Acceptance criteria

- `release-manifest.md`'s SBOM reference matches an actually-published asset name for the tarball's architecture (or a generic-named SBOM asset is published to match).

## Scope not tested

SBOM contents were not audited; only the asset-name consistency between the in-tarball manifest and the published release assets was checked.
