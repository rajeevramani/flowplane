#!/usr/bin/env python3
"""Executable, provider-neutral eval PKI and static recipe contract."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
COMPOSE = ROOT / "compose.eval.yml"
DOC = ROOT / "docs/how-to/global-rate-limit.md"
MIGRATION_DOCS = (
    ROOT / "docs/how-to/production-readiness.md",
    ROOT / "docs/how-to/register-dataplane-mtls.md",
    ROOT / "deploy/aws/README.md",
    ROOT / "docs/how-to/aws-secure-deployment.md",
    ROOT / "docs/reference/configuration.md",
    ROOT / "CHANGELOG.md",
)

CA_EXTENSIONS = (
    "basicConstraints=critical,CA:TRUE",
    "keyUsage=critical,keyCertSign,cRLSign",
    "subjectKeyIdentifier=hash",
    "authorityKeyIdentifier=keyid:always",
)
SERVER_EXTENSIONS = (
    "basicConstraints=critical,CA:FALSE",
    "keyUsage=critical,digitalSignature",
    "extendedKeyUsage=serverAuth",
    "subjectKeyIdentifier=hash",
    "authorityKeyIdentifier=keyid:always",
    "subjectAltName=DNS:server.test",
)
CLIENT_EXTENSIONS = (
    "basicConstraints=critical,CA:FALSE",
    "keyUsage=critical,digitalSignature",
    "extendedKeyUsage=clientAuth",
    "subjectKeyIdentifier=hash",
    "authorityKeyIdentifier=keyid:always",
    "subjectAltName=URI:spiffe://flowplane.test/client",
)
SERVER_VERIFY = (
    "openssl verify -x509_strict -partial_chain -purpose sslserver "
    "-verify_hostname server.test -CAfile ca.crt server.crt"
)
CLIENT_VERIFY = (
    "openssl verify -x509_strict -partial_chain -purpose sslclient "
    "-CAfile ca.crt client.crt"
)


class ContractError(AssertionError):
    """A deterministic contract violation."""


def require(text: str, needle: str, label: str) -> None:
    if needle not in text:
        raise ContractError(f"{label}: missing {needle!r}")


def require_regex(text: str, pattern: str, label: str, expected: str) -> None:
    if re.search(pattern, text, flags=re.MULTILINE) is None:
        raise ContractError(f"{label}: missing {expected}")


def openssl_path() -> Path:
    configured = os.environ.get("OPENSSL_BIN")
    candidate = configured if configured else shutil.which("openssl")
    if candidate is None:
        raise ContractError("OpenSSL executable not found; set OPENSSL_BIN to an executable path")
    path = Path(candidate).expanduser()
    if not path.is_file() or not os.access(path, os.X_OK):
        raise ContractError(f"OPENSSL_BIN is not an executable file: {str(path)!r}")
    return path.resolve()


def run(command: list[str], *, expect_success: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(command, text=True, capture_output=True, check=False)
    succeeded = result.returncode == 0
    if succeeded != expect_success:
        outcome = "failed" if expect_success else "unexpectedly succeeded"
        detail = (result.stderr or result.stdout).strip()
        if len(detail) > 800:
            detail = detail[:800] + "..."
        raise ContractError(f"command {outcome}: {' '.join(command)}: {detail}")
    return result


def checked_openssl_version(openssl: Path) -> str:
    output = run([str(openssl), "version"]).stdout.strip()
    if output.startswith("LibreSSL"):
        raise ContractError(f"LibreSSL is unsupported; OpenSSL >=3.0 is required (found {output})")
    match = re.match(r"^OpenSSL\s+(\d+)\.(\d+)(?:\.(\d+))?", output)
    if match is None:
        raise ContractError(f"cannot parse OpenSSL version output: {output!r}")
    version = tuple(int(part or 0) for part in match.groups())
    if version < (3, 0, 0):
        raise ContractError(f"OpenSSL >=3.0 is required (found {output})")
    return output


def write_profile(path: Path, section: str, extensions: tuple[str, ...]) -> None:
    path.write_text(
        "[req]\n"
        "distinguished_name=dn\n"
        "prompt=no\n"
        f"x509_extensions={section}\n"
        "req_extensions=req_ext\n"
        "[dn]\n"
        "CN=unused\n"
        "[req_ext]\n"
        "subjectAltName=DNS:unused.invalid\n"
        f"[{section}]\n"
        + "\n".join(extensions)
        + "\n",
        encoding="utf-8",
    )


def extension(openssl: Path, certificate: Path, name: str) -> str:
    return run(
        [str(openssl), "x509", "-in", str(certificate), "-noout", "-ext", name]
    ).stdout


def key_identifier(text: str, label: str) -> str:
    values = re.findall(r"(?:[0-9A-Fa-f]{2}:){3,}[0-9A-Fa-f]{2}", text)
    if not values:
        raise ContractError(f"{label}: no key identifier value found")
    return values[-1].upper()


def assert_ec_p256(openssl: Path, certificate: Path, label: str) -> None:
    text = run([str(openssl), "x509", "-in", str(certificate), "-noout", "-text"]).stdout
    if "ASN1 OID: prime256v1" not in text and "NIST CURVE: P-256" not in text:
        raise ContractError(f"{label}: public key is not explicit EC P-256")


def assert_basic_constraints(openssl: Path, certificate: Path, is_ca: bool, label: str) -> None:
    text = extension(openssl, certificate, "basicConstraints")
    require(text, "critical", f"{label} Basic Constraints")
    require(text, f"CA:{str(is_ca).upper()}", f"{label} Basic Constraints")


def assert_key_usage(
    openssl: Path, certificate: Path, required: tuple[str, ...], forbidden: tuple[str, ...], label: str
) -> None:
    text = extension(openssl, certificate, "keyUsage")
    require(text, "critical", f"{label} Key Usage")
    for value in required:
        require(text, value, f"{label} Key Usage")
    for value in forbidden:
        if value in text:
            raise ContractError(f"{label} Key Usage: forbidden value {value!r}")


def generate_ca(openssl: Path, directory: Path, stem: str, common_name: str) -> tuple[Path, Path]:
    key = directory / f"{stem}.key"
    certificate = directory / f"{stem}.crt"
    profile = directory / f"{stem}.cnf"
    write_profile(profile, "v3_ca", CA_EXTENSIONS)
    run(
        [
            str(openssl),
            "genpkey",
            "-algorithm",
            "EC",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-out",
            str(key),
        ]
    )
    run(
        [
            str(openssl),
            "req",
            "-new",
            "-x509",
            "-sha256",
            "-days",
            "1",
            "-key",
            str(key),
            "-subj",
            f"/CN={common_name}",
            "-config",
            str(profile),
            "-extensions",
            "v3_ca",
            "-out",
            str(certificate),
        ]
    )
    return key, certificate


def generate_leaf(
    openssl: Path,
    directory: Path,
    stem: str,
    common_name: str,
    section: str,
    extensions: tuple[str, ...],
    ca_key: Path,
    ca_certificate: Path,
    serial: int,
) -> tuple[Path, Path]:
    key = directory / f"{stem}.key"
    request = directory / f"{stem}.csr"
    certificate = directory / f"{stem}.crt"
    profile = directory / f"{stem}.cnf"
    write_profile(profile, section, extensions)
    run(
        [
            str(openssl),
            "genpkey",
            "-algorithm",
            "EC",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-out",
            str(key),
        ]
    )
    run(
        [
            str(openssl),
            "req",
            "-new",
            "-sha256",
            "-key",
            str(key),
            "-subj",
            f"/CN={common_name}",
            "-out",
            str(request),
        ]
    )
    run(
        [
            str(openssl),
            "x509",
            "-req",
            "-sha256",
            "-days",
            "1",
            "-in",
            str(request),
            "-CA",
            str(ca_certificate),
            "-CAkey",
            str(ca_key),
            "-set_serial",
            str(serial),
            "-extfile",
            str(profile),
            "-extensions",
            section,
            "-out",
            str(certificate),
        ]
    )
    return key, certificate


def check_executable_pki(openssl: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="flowplane-pki-contract-") as temporary:
        directory = Path(temporary)
        ca_key, ca_certificate = generate_ca(openssl, directory, "ca", "Flowplane contract CA")
        _, server_certificate = generate_leaf(
            openssl,
            directory,
            "server",
            "server.test",
            "server_cert",
            SERVER_EXTENSIONS,
            ca_key,
            ca_certificate,
            2,
        )
        _, client_certificate = generate_leaf(
            openssl,
            directory,
            "client",
            "contract client",
            "client_cert",
            CLIENT_EXTENSIONS,
            ca_key,
            ca_certificate,
            3,
        )
        _, wrong_ca = generate_ca(openssl, directory, "wrong-ca", "Flowplane contract CA")

        for certificate, label in (
            (ca_certificate, "CA"),
            (server_certificate, "server leaf"),
            (client_certificate, "client leaf"),
        ):
            assert_ec_p256(openssl, certificate, label)

        assert_basic_constraints(openssl, ca_certificate, True, "CA")
        assert_key_usage(
            openssl,
            ca_certificate,
            ("Certificate Sign", "CRL Sign"),
            ("Digital Signature", "Key Encipherment", "Key Agreement"),
            "CA",
        )
        ca_ski = key_identifier(extension(openssl, ca_certificate, "subjectKeyIdentifier"), "CA SKI")
        ca_aki = key_identifier(extension(openssl, ca_certificate, "authorityKeyIdentifier"), "CA AKI")
        if ca_ski != ca_aki:
            raise ContractError("CA AKI does not identify its SKI")

        for certificate, label, eku, san in (
            (server_certificate, "server leaf", "TLS Web Server Authentication", "DNS:server.test"),
            (
                client_certificate,
                "client leaf",
                "TLS Web Client Authentication",
                "URI:spiffe://flowplane.test/client",
            ),
        ):
            assert_basic_constraints(openssl, certificate, False, label)
            assert_key_usage(
                openssl,
                certificate,
                ("Digital Signature",),
                ("Key Encipherment", "Key Agreement", "Certificate Sign", "CRL Sign"),
                label,
            )
            eku_text = extension(openssl, certificate, "extendedKeyUsage")
            require(eku_text, eku, f"{label} Extended Key Usage")
            leaf_ski = key_identifier(extension(openssl, certificate, "subjectKeyIdentifier"), f"{label} SKI")
            if not leaf_ski:
                raise ContractError(f"{label}: empty SKI")
            leaf_aki = key_identifier(extension(openssl, certificate, "authorityKeyIdentifier"), f"{label} AKI")
            if leaf_aki != ca_ski:
                raise ContractError(f"{label}: AKI does not identify the issuer SKI")
            require(extension(openssl, certificate, "subjectAltName"), san, f"{label} SAN")

        strict = [str(openssl), "verify", "-x509_strict", "-partial_chain"]
        run(
            strict
            + [
                "-purpose",
                "sslserver",
                "-verify_hostname",
                "server.test",
                "-CAfile",
                str(ca_certificate),
                str(server_certificate),
            ]
        )
        run(
            strict
            + [
                "-purpose",
                "sslclient",
                "-CAfile",
                str(ca_certificate),
                str(client_certificate),
            ]
        )
        run(
            strict
            + [
                "-purpose",
                "sslclient",
                "-CAfile",
                str(wrong_ca),
                str(client_certificate),
            ],
            expect_success=False,
        )


def compose_services(text: str) -> dict[str, str]:
    lines = text.splitlines()
    try:
        services_start = next(index for index, line in enumerate(lines) if line.strip() == "services:")
    except StopIteration as error:
        raise ContractError("compose.eval.yml: services section not found") from error

    services: dict[str, list[str]] = {}
    current: str | None = None
    for line in lines[services_start + 1 :]:
        if line and not line.startswith(" "):
            break
        match = re.match(r"^  ([A-Za-z0-9_.-]+):\s*(?:#.*)?$", line)
        if match:
            service_name = match.group(1)
            current = service_name
            services[service_name] = [line]
        elif current is not None:
            services[current].append(line)
    return {name: "\n".join(body) for name, body in services.items()}


def check_s1(compose: str) -> None:
    for extension_value in CA_EXTENSIONS:
        require(compose, extension_value, "compose.eval.yml eval issuer CA profile")
    require(
        compose,
        'openssl x509 -in /pki-cp/ca.crt -noout -ext subjectKeyIdentifier',
        "compose.eval.yml persisted issuer CA guard",
    )
    require(
        compose,
        'openssl x509 -in /pki-cp/ca.crt -noout -ext basicConstraints | grep -q "CA:TRUE"',
        "compose.eval.yml persisted CA constraints guard",
    )
    require(
        compose,
        'openssl x509 -in /pki-cp/ca.crt -noout -ext keyUsage | grep -q "Certificate Sign"',
        "compose.eval.yml persisted keyCertSign guard",
    )
    require(
        compose,
        "openssl verify -x509_strict -CAfile /pki-cp/ca.crt /pki-cp/ca.crt",
        "compose.eval.yml persisted strict issuer guard",
    )
    require(
        compose,
        "openssl verify -x509_strict -CAfile ca.crt ca.crt",
        "compose.eval.yml generated strict issuer guard",
    )


def check_documentation(document: str) -> None:
    if "the same `openssl` recipe" in document:
        raise ContractError(
            "global-rate-limit.md must not claim register-dataplane-mtls contains the same OpenSSL recipe"
        )
    for profile, extensions in (
        ("CA", CA_EXTENSIONS),
        ("server leaf", SERVER_EXTENSIONS[:-1]),
        ("client leaf", CLIENT_EXTENSIONS[:-1]),
    ):
        for extension_value in extensions:
            require(document, extension_value, f"global-rate-limit.md explicit {profile} profile")
    require(document, "subjectAltName=DNS:rls.example", "global-rate-limit.md server SAN")
    require(document, "subjectAltName=DNS:envoy-fleet.example", "global-rate-limit.md client SAN")
    require(
        document,
        "openssl verify -x509_strict -partial_chain -purpose sslserver -verify_hostname rls.example -CAfile rls-ca.pem rls-grpc-server.pem",
        "global-rate-limit.md exact strict server verification",
    )
    require(
        document,
        "openssl verify -x509_strict -partial_chain -purpose sslclient -CAfile rls-ca.pem envoy-client.pem",
        "global-rate-limit.md exact strict client verification",
    )


def check_migration_documentation() -> None:
    for path in MIGRATION_DOCS:
        text = path.read_text(encoding="utf-8")
        label = str(path.relative_to(ROOT))
        for requirement in ("CA:TRUE", "keyCertSign", "Subject Key Identifier", "intermediate"):
            require(text, requirement, f"{label} issuer compatibility disclosure")
        has_canonical_link = path == MIGRATION_DOCS[0] or "issuer-ca-compatibility-and-upgrade" in text
        has_inline_upgrade = all(term in text.lower() for term in ("reissue", "rotation", "running"))
        if not has_canonical_link and not has_inline_upgrade:
            raise ContractError(f"{label}: missing issuer upgrade sequence or canonical procedure link")

    readiness = MIGRATION_DOCS[0].read_text(encoding="utf-8")
    normalized_readiness = " ".join(readiness.split())
    for requirement in (
        "openssl x509 -in issuer-ca.pem -noout -dates",
        "openssl pkey -in issuer-ca.key -pubout | sha256sum",
        "first redistribute the corrected **public CA certificate**",
        "only then issue replacement leaves",
        "does not rewrite trust stores",
    ):
        require(
            normalized_readiness,
            " ".join(requirement.split()),
            "production-readiness.md canonical issuer migration",
        )


def command_with_options_pattern(purpose: str, hostname: bool) -> str:
    hostname_part = r"\s+-verify_hostname\s+[^\s\\]+" if hostname else ""
    return (
        r"openssl\s+verify\s+-x509_strict\s+-partial_chain\s+-purpose\s+"
        + purpose
        + hostname_part
        + r"\s+-CAfile\s+[^\s\\]+\s+[^\s\\]+"
    )


def check_compose_pki(compose: str) -> None:
    services = compose_services(compose)
    helper = services.get("pki")
    if helper is None or re.search(r"image:\s*[^\n#]*postgres:16(?:\s|$)", helper) is None:
        raise ContractError("compose.eval.yml: postgres:16 PKI helper service not found")
    label = "compose.eval.yml:pki postgres:16 PKI helper"

    require(helper.lower(), "openssl version", f"{label} OpenSSL floor probe")
    require(helper, "LibreSSL*)", f"{label} LibreSSL rejection")
    require_regex(
        helper,
        r"OpenSSL[^\n]*(?:>=\s*3(?:\.0)?|3\.x|major[^\n]*3)",
        f"{label} OpenSSL floor",
        "an explicit OpenSSL >=3.0 floor",
    )

    for extension_value in SERVER_EXTENSIONS[:-1]:
        require(helper, extension_value, f"{label} explicit xDS server profile")
    require_regex(
        helper,
        r"subjectAltName=DNS:[^\s'\"]+",
        f"{label} explicit xDS server profile",
        "a DNS subjectAltName",
    )

    server_verify_pattern = command_with_options_pattern("sslserver", hostname=True)
    server_verify_count = len(re.findall(server_verify_pattern, helper, flags=re.MULTILINE))
    if server_verify_count < 2:
        raise ContractError(
            f"{label} strict xDS server reuse/generation guards: "
            f"expected at least 2 strict server-purpose, hostname, partial-chain verifications; "
            f"found {server_verify_count}"
        )
    verifier = services.get("pki-client-verify")
    if verifier is None:
        raise ContractError("compose.eval.yml:pki-client-verify service not found")
    require_regex(
        verifier,
        command_with_options_pattern("sslclient", hostname=False),
        "compose.eval.yml:pki-client-verify strict product-issued client guard",
        "strict client-purpose, partial-chain verification",
    )
    verify_position = verifier.find("openssl verify -x509_strict")
    sentinel_position = verifier.find("touch /pki-dp/.init-mtls-v1")
    if verify_position < 0 or sentinel_position < 0 or verify_position >= sentinel_position:
        raise ContractError("pki-client-verify must strict-verify before writing the completion sentinel")
    init = services.get("init", "")
    if "touch /pki-dp/.init-mtls-v1" in init:
        raise ContractError("init must not write the completion sentinel before strict verification")
    for consumer in ("envoy", "flowplane-agent"):
        body = services.get(consumer, "")
        require(body, "pki-client-verify:", f"compose.eval.yml:{consumer} verifier dependency")
        require(
            body,
            "condition: service_completed_successfully",
            f"compose.eval.yml:{consumer} verifier completion condition",
        )


def check_static_repo() -> None:
    compose = COMPOSE.read_text(encoding="utf-8")
    check_s1(compose)
    document = DOC.read_text(encoding="utf-8")
    check_documentation(document)
    check_migration_documentation()
    check_compose_pki(compose)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--version-check-only",
        action="store_true",
        help="validate OPENSSL_BIN and its version without invoking cryptographic commands",
    )
    mode.add_argument(
        "--crypto-only",
        action="store_true",
        help="run executable OpenSSL PKI assertions without static repository assertions",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    openssl = openssl_path()
    version = checked_openssl_version(openssl)
    if args.version_check_only:
        print(f"OpenSSL version contract: PASS: {version}")
        return 0

    check_executable_pki(openssl)
    if args.crypto_only:
        print(f"executable PKI contract: PASS: {version}")
        return 0

    check_static_repo()
    print(f"eval PKI executable/static contract: PASS: {version}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ContractError, OSError) as error:
        print(f"eval PKI executable/static contract: FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
