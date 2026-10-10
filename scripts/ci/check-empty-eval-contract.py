#!/usr/bin/env python3
"""Pin empty-install packaging without treating static checks as runtime proof."""
from pathlib import Path
import argparse
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]


def service(text: str, name: str) -> str:
    match = re.search(rf"^  {re.escape(name)}:\n(.*?)(?=^  [a-z][a-z0-9-]*:|^volumes:|\Z)", text, re.M | re.S)
    if not match:
        raise AssertionError(f"missing service {name}")
    return match.group(1)


def check(text: str) -> None:
    init = service(text, "init")
    forbidden = re.search(r"^\s*flowplane\s+(?:expose\b|(?:cluster|route|listener)\s+create\b)", init, re.M)
    if forbidden:
        raise AssertionError("init must not create a gateway exposure/resource")
    for required in (
        "flowplane dataplane create dp-eval",
        "flowplane -o json dataplane cert issue dp-eval",
        "dataplane bootstrap dp-eval --mode mtls",
        "--cert-path /pki-dp/client.crt",
        "--key-path /pki-dp/client.key",
        "--ca-path /pki-dp/ca.crt",
        "/pki-dp/.init-mtls-v1",
    ):
        if required not in init:
            raise AssertionError(f"init must retain {required}")
    if "hashicorp/http-echo:1.0.0" not in service(text, "demo-upstream"):
        raise AssertionError("retain sample backend")
    cp = service(text, "flowplane-eval")
    for required in ("FLOWPLANE_XDS_TLS_CLIENT_CA: /pki-cp/ca.crt", "FLOWPLANE_XDS_TLS_CERT: /pki-cp/server.crt", "FLOWPLANE_XDS_TLS_KEY: /pki-cp/server.key", "flowplane auth whoami"):
        if required not in cp:
            raise AssertionError(f"CP must retain {required}")
    envoy = service(text, "envoy")
    if "10000}:10000" not in envoy or "127.0.0.1:" not in envoy:
        raise AssertionError("retain loopback-published authored gateway port")
    verifier = service(text, "pki-client-verify")
    if "-purpose sslclient" not in verifier or "touch /pki-dp/.init-mtls-v1" not in verifier:
        raise AssertionError("strict PKI verification must own completion sentinel")


class Fixtures(unittest.TestCase):
    def setUp(self):
        self.text = (ROOT / "compose.eval.yml").read_text()
        # The positive fixture is packaging with only the old exposure line removed.
        self.empty = re.sub(r"^\s*flowplane expose[^\n]*\n", "", self.text, flags=re.M)

    def test_empty_preserves_bootstrap(self):
        check(self.empty)

    def test_seeded_exposure_rejected(self):
        seeded = self.empty.replace('        set -e\n', '        set -e\n        flowplane expose http://example.test --name forbidden\n')
        with self.assertRaisesRegex(AssertionError, "gateway exposure"):
            check(seeded)

    def test_missing_backend_rejected(self):
        with self.assertRaisesRegex(AssertionError, "sample backend"):
            check(self.empty.replace("hashicorp/http-echo:1.0.0", "other:1"))

    def test_plaintext_bootstrap_rejected(self):
        with self.assertRaisesRegex(AssertionError, "mode mtls"):
            check(self.empty.replace("dataplane bootstrap dp-eval --mode mtls", "dataplane bootstrap dp-eval --mode plaintext"))

    def test_missing_strict_client_check_rejected(self):
        with self.assertRaisesRegex(AssertionError, "strict PKI"):
            check(self.empty.replace("-purpose sslclient", "-purpose any"))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        unittest.main(argv=[sys.argv[0]])
    else:
        try:
            check((ROOT / "compose.eval.yml").read_text())
        except AssertionError as error:
            print(f"empty eval static contract: FAIL: {error}", file=sys.stderr)
            raise SystemExit(1)
        print("empty eval static contract: PASS (runtime qualification is separate)")
