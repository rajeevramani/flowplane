#!/usr/bin/env python3
"""Fail closed when the shipped agent recovery contract drifts."""

from __future__ import annotations

from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]


def require(text: str, needle: str, source: str) -> None:
    if needle not in text:
        raise AssertionError(f"{source}: missing required recovery contract text: {needle!r}")


def require_line(text: str, expected: str, source: str) -> None:
    if not any(line.strip() == expected for line in text.splitlines()):
        raise AssertionError(f"{source}: missing required recovery contract line: {expected!r}")


def compose_service(text: str, service: str) -> str:
    lines = text.splitlines()
    marker = f"  {service}:"
    try:
        start = lines.index(marker)
    except ValueError as error:
        raise AssertionError(f"compose.eval.yml: service {service!r} not found") from error

    body = [lines[start]]
    for line in lines[start + 1 :]:
        if line and not line.startswith(" "):
            break
        if line.startswith("  ") and not line.startswith("    ") and line.strip().endswith(":"):
            break
        body.append(line)
    return "\n".join(body)


def main() -> int:
    compose = (ROOT / "compose.eval.yml").read_text(encoding="utf-8")
    readiness = (ROOT / "docs/how-to/production-readiness.md").read_text(encoding="utf-8")
    mtls = (ROOT / "docs/how-to/register-dataplane-mtls.md").read_text(encoding="utf-8")
    configuration = (ROOT / "docs/reference/configuration.md").read_text(encoding="utf-8")

    agent = compose_service(compose, "flowplane-agent")
    require_line(agent, 'network_mode: "service:envoy"', "compose.eval.yml:flowplane-agent")
    require_line(agent, "restart: unless-stopped", "compose.eval.yml:flowplane-agent")

    require(
        readiness,
        "The surviving agent reconnects automatically after the control plane is restored",
        "production-readiness.md",
    )
    require(
        readiness,
        "readiness returns only after the control plane accepts and commits a diagnostics report",
        "production-readiness.md",
    )
    require(
        readiness,
        "Envoy continues serving its last-good configuration",
        "production-readiness.md",
    )

    for source, text in (
        ("register-dataplane-mtls.md", mtls),
        ("configuration.md", configuration),
    ):
        require(text, "readiness signal, not a process-liveness signal", source)
        require(text, "remaining current report-attempt deadline", source)
        require(text, "maximum jittered backoff of 6 seconds", source)
        require(text, "one complete successful report-attempt deadline", source)
        require(text, "at most one poll interval", source)
        require(text, "rereads the CA, client certificate, and private key files", source)

    require(
        mtls,
        "If Envoy is recreated, recreate the namespace-sharing agent container as well",
        "register-dataplane-mtls.md",
    )
    require(mtls, "production-readiness.md", "register-dataplane-mtls.md")
    require(mtls, "../reference/configuration.md", "register-dataplane-mtls.md")

    print("agent recovery Compose/documentation contract: PASS")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as error:
        print(f"agent recovery Compose/documentation contract: FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
