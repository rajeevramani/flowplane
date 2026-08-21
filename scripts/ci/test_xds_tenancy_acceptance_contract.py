#!/usr/bin/env python3
"""Black-box acceptance contract for fpv2-d23.5.

The public ``--self-test`` summary is the only supported observation here. A passing
result proves the aggregate CLI contract for the fourteen scenarios covering four
simultaneous one-team VM streams; certificate-bound ADS/SDS isolation; control-plane
ACK/status; foreign-resource behavioral non-effect; redacted structural projection;
team-scoped telemetry, stats, NACKs, and audit; valid-identity rotation reconnect;
fail-closed invalid identities; and freeze-before-cleanup with zero run-owned residue.

The individual fail-closed mutations and their evidence remain internal to the
black-box command. This test therefore does not claim to independently observe or
identify each mutation; it requires the public aggregate mutation count to be at
least twelve.
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
ACCEPTANCE_COMMAND = REPOSITORY_ROOT / "scripts/ci/check-xds-tenancy-acceptance.py"
SUMMARY_PATTERN = re.compile(
    r"xds tenancy acceptance self-test: PASS \(14 scenarios, (\d+) fail-closed mutations\)\n?"
)


def test_xds_tenancy_acceptance_self_test_contract() -> None:
    """Require the fpv2-d23.5 public aggregate self-test contract."""
    assert ACCEPTANCE_COMMAND.is_file(), (
        "expected black-box acceptance command is missing: "
        "scripts/ci/check-xds-tenancy-acceptance.py"
    )

    completed = subprocess.run(
        ["python3", str(ACCEPTANCE_COMMAND), "--self-test"],
        cwd=REPOSITORY_ROOT,
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
    )

    assert completed.returncode == 0, (
        "xDS tenancy acceptance self-test must exit 0; "
        f"got {completed.returncode}"
    )
    assert completed.stderr == "", (
        "xDS tenancy acceptance self-test must emit empty stderr; "
        f"got {completed.stderr!r}"
    )

    summary_match = SUMMARY_PATTERN.fullmatch(completed.stdout)
    assert summary_match is not None, (
        "xDS tenancy acceptance self-test must emit only the exact public summary "
        "'xds tenancy acceptance self-test: PASS (14 scenarios, N fail-closed "
        f"mutations)'; got {completed.stdout!r}"
    )

    mutation_count = int(summary_match.group(1))
    assert mutation_count >= 12, (
        "xDS tenancy acceptance self-test must report at least 12 internal "
        f"fail-closed mutations; got {mutation_count}"
    )


if __name__ == "__main__":
    test_xds_tenancy_acceptance_self_test_contract()
