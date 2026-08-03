#!/usr/bin/env python3
"""Fail closed when binary-manifest and published SBOM names drift."""

from __future__ import annotations

from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]


def require(text: str, needle: str, source: str) -> None:
    if needle not in text:
        raise AssertionError(f"{source}: missing release artifact contract text: {needle!r}")


def check(package: str, workflow: str) -> None:
    require(package, 'ARTIFACT="flowplane-$VERSION-$HOST"', "package-release.sh")
    require(
        package,
        "- SBOM source artifact: \\`$ARTIFACT.cargo-metadata.sbom.json\\`",
        "package-release.sh:release-manifest.md",
    )
    require(
        workflow,
        '-e FLOWPLANE_RELEASE_HOST="linux-${{ matrix.arch }}"',
        "release.yml:build-candidate",
    )
    require(
        workflow,
        '"${{ runner.temp }}/release-assets/flowplane-${VERSION}-linux-${{ matrix.arch }}.cargo-metadata.sbom.json"',
        "release.yml:published SBOM destination",
    )


def main() -> int:
    package = (ROOT / "scripts/release/package-release.sh").read_text(encoding="utf-8")
    workflow = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
    check(package, workflow)
    print("release manifest/published SBOM contract: PASS")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as error:
        print(f"release manifest/published SBOM contract: FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
