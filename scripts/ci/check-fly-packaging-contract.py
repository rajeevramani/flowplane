#!/usr/bin/env python3
"""Black-box acceptance contract for the optional Fly.io deployment package.

This test intentionally treats deploy/fly as declarative provider packaging.  It
must not import or inspect Flowplane production implementation modules.
"""

from __future__ import annotations

import ast
import re
import unittest
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
FLY_ROOT = ROOT / "deploy" / "fly"
FORBIDDEN_PUBLIC_PORTS = {18000, 5432, 50051, 8081, 9901, 19902}
REQUIRED_BINARIES = {"flowplane", "flowplane-agent", "flowplane-rls"}
PRIVATE_FILE_INPUTS = {
    "FLOWPLANE_API_TLS_KEY": "API TLS private key",
    "FLOWPLANE_XDS_TLS_KEY": "xDS TLS private key",
    "FLOWPLANE_CERT_ISSUER_CA_KEY_PATH": "certificate-issuer private key",
    "FLOWPLANE_BOOTSTRAP_TOKEN_FILE": "one-shot bootstrap token",
    "FLOWPLANE_RLS_ADMIN_TOKEN_FILE": "RLS admin bearer token",
}
CP_RUNTIME_INPUTS = {
    "FLOWPLANE_DATABASE_URL": "PostgreSQL",
    "FLOWPLANE_OIDC_ISSUER": "production OIDC issuer",
    "FLOWPLANE_OIDC_AUDIENCE": "production OIDC audience",
    "FLOWPLANE_SECRET_ENCRYPTION_KEY": "secret encryption",
    "FLOWPLANE_API_TLS_CERT": "API TLS server certificate",
    "FLOWPLANE_API_TLS_KEY": "API TLS server private key",
    "FLOWPLANE_CERT_ISSUER_CA_CERT_PATH": "certificate issuer CA certificate",
    "FLOWPLANE_CERT_ISSUER_CA_KEY_PATH": "certificate issuer CA private key",
    "FLOWPLANE_XDS_TLS_CERT": "xDS server certificate",
    "FLOWPLANE_XDS_TLS_KEY": "xDS server private key",
    "FLOWPLANE_XDS_TLS_CLIENT_CA": "xDS client CA",
}


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def recursive_text() -> str:
    files = sorted(
        path
        for path in FLY_ROOT.rglob("*")
        if path.is_file() and path.suffix.lower() in {"", ".md", ".sh", ".toml"}
    )
    return "\n".join(read_text(path) for path in files)


def parse_toml_value(value: str) -> Any:
    value = re.sub(r"\btrue\b", "True", value, flags=re.IGNORECASE)
    value = re.sub(r"\bfalse\b", "False", value, flags=re.IGNORECASE)
    try:
        return ast.literal_eval(value)
    except (SyntaxError, ValueError):
        return value.strip().strip('"').strip("'")


def parse_manifest(text: str, source: Path) -> dict[str, Any]:
    """Parse the small public-service/env/files subset used by Fly manifests."""
    root: dict[str, Any] = {}
    current: dict[str, Any] = root
    for number, raw_line in enumerate(text.splitlines(), 1):
        line = raw_line.split("#", 1)[0].strip()
        if not line:
            continue
        array_header = re.fullmatch(r"\[\[([A-Za-z0-9_.-]+)\]\]", line)
        table_header = re.fullmatch(r"\[([A-Za-z0-9_.-]+)\]", line)
        if array_header:
            parts = array_header.group(1).split(".")
            parent: Any = root
            for part in parts[:-1]:
                if isinstance(parent, list):
                    if not parent or not isinstance(parent[-1], dict):
                        raise AssertionError(f"{source.relative_to(ROOT)}:{number}: invalid TOML array parent")
                    parent = parent[-1]
                if not isinstance(parent, dict):
                    raise AssertionError(f"{source.relative_to(ROOT)}:{number}: conflicting TOML table")
                parent = parent.setdefault(part, {})
            if isinstance(parent, list):
                if not parent or not isinstance(parent[-1], dict):
                    raise AssertionError(f"{source.relative_to(ROOT)}:{number}: invalid TOML array parent")
                parent = parent[-1]
            if not isinstance(parent, dict):
                raise AssertionError(f"{source.relative_to(ROOT)}:{number}: conflicting TOML array")
            values = parent.setdefault(parts[-1], [])
            if not isinstance(values, list):
                raise AssertionError(f"{source.relative_to(ROOT)}:{number}: conflicting TOML array")
            current = {}
            values.append(current)
            continue
        if table_header:
            current_value: Any = root
            for part in table_header.group(1).split("."):
                if isinstance(current_value, list):
                    if not current_value or not isinstance(current_value[-1], dict):
                        raise AssertionError(f"{source.relative_to(ROOT)}:{number}: invalid TOML array table")
                    current_value = current_value[-1]
                if not isinstance(current_value, dict):
                    raise AssertionError(f"{source.relative_to(ROOT)}:{number}: conflicting TOML table")
                current_value = current_value.setdefault(part, {})
            if not isinstance(current_value, dict):
                raise AssertionError(f"{source.relative_to(ROOT)}:{number}: conflicting TOML table")
            current = current_value
            continue
        assignment = re.fullmatch(r"([A-Za-z0-9_.-]+)\s*=\s*(.+)", line)
        if assignment:
            current[assignment.group(1)] = parse_toml_value(assignment.group(2))
            continue
        raise AssertionError(f"{source.relative_to(ROOT)}:{number}: unsupported or invalid TOML: {line}")
    return root


def load_manifest() -> tuple[Path, dict[str, Any]]:
    path = FLY_ROOT / "fly.toml"
    if not path.is_file():
        raise AssertionError(f"missing Fly machine manifest: {path.relative_to(ROOT)}")
    return path, parse_manifest(read_text(path), path)


def public_service_ports(manifest: dict[str, Any]) -> list[tuple[int, set[int]]]:
    """Return (internal port, externally published ports) for public services."""
    services: list[tuple[int, set[int]]] = []
    http_service = manifest.get("http_service")
    if isinstance(http_service, dict):
        services.append((int(http_service.get("internal_port", 80)), {80, 443}))

    raw_services = manifest.get("services", [])
    if isinstance(raw_services, dict):
        raw_services = [raw_services]
    for service in raw_services:
        if not isinstance(service, dict):
            continue
        external: set[int] = set()
        raw_ports = service.get("ports", [])
        if isinstance(raw_ports, dict):
            raw_ports = [raw_ports]
        for port in raw_ports:
            if isinstance(port, dict) and "port" in port:
                external.add(int(port["port"]))
        services.append((int(service.get("internal_port", 0)), external))
    return services


def secret_file_bindings(manifest: dict[str, Any]) -> dict[str, str]:
    raw_files = manifest.get("files", [])
    if isinstance(raw_files, dict):
        raw_files = [raw_files]
    bindings: dict[str, str] = {}
    for item in raw_files:
        if not isinstance(item, dict):
            continue
        secret_name = item.get("secret_name")
        guest_path = item.get("guest_path")
        if isinstance(secret_name, str) and isinstance(guest_path, str):
            bindings[secret_name] = guest_path
    return bindings


class FlyPackagingContract(unittest.TestCase):
    def setUp(self) -> None:
        self.assertTrue(
            FLY_ROOT.is_dir(),
            f"Fly packaging does not exist: expected {FLY_ROOT.relative_to(ROOT)}/",
        )

    def test_release_container_build_is_locked_and_contains_supported_binaries(self) -> None:
        candidates = [FLY_ROOT / "Containerfile", FLY_ROOT / "Dockerfile"]
        build_file = next((path for path in candidates if path.is_file()), None)
        self.assertIsNotNone(build_file, "deploy/fly needs Containerfile or Dockerfile")
        assert build_file is not None
        text = read_text(build_file)

        self.assertRegex(
            text,
            r"(?m)^FROM\s+--platform=linux/amd64\s+\S+@sha256:[0-9a-f]{64}(?:\s+AS\s+\S+)?\s*$",
        )
        from_lines = re.findall(r"(?m)^FROM\s+.*$", text)
        self.assertEqual(len(from_lines), 3, "image must retain the audited three-stage build")
        self.assertTrue(
            all("--platform=linux/amd64" in line for line in from_lines),
            "every stage must target Fly Machines' x86_64 runtime architecture",
        )
        self.assertIn(
            "tailscale/tailscale@sha256:4107a12b1a0466bb3f2c968d5fa35acf509cd7865a958ce1af36724e9f016342",
            text,
            "Tailscale must use the v1.102.2 linux/amd64 child manifest, not the arm64 child",
        )
        self.assertNotIn("fdbdb434c50a6d3a5ed73f2b15ef66228dd2d265c1729e55f9a663ae804c5453", text)
        self.assertIn("Cargo.lock", text, "container build must consume the locked dependency graph")
        self.assertRegex(text, r"cargo\s+build[^\n]*--release[^\n]*--locked")
        self.assertRegex(
            text,
            r"cargo\s+build[^\n]*(?:-p\s+flowplane|--package\s+flowplane)[^\n]*--no-default-features",
            "production control-plane build must exclude dev-oidc defaults",
        )
        missing = sorted(binary for binary in REQUIRED_BINARIES if binary not in text)
        self.assertEqual(missing, [], f"container package omits supported binaries: {missing}")
        self.assertNotRegex(text, r"(?m)^\s*(?:ADD|COPY)\s+target/", "image must build, not copy host target output")

    def test_remote_build_context_is_an_allowlist(self) -> None:
        dockerignore = ROOT / ".dockerignore"
        self.assertTrue(dockerignore.is_file(), "remote builds require a root .dockerignore")
        lines = [
            line.strip()
            for line in read_text(dockerignore).splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        ]
        self.assertEqual(
            lines,
            [
                "*",
                "!Cargo.toml",
                "!Cargo.lock",
                "!crates/",
                "!crates/**",
                "!deploy/",
                "deploy/**",
                "!deploy/fly/",
                "deploy/fly/**",
                "!deploy/fly/entrypoint.sh",
            ],
            "remote build context must be the audited deny-by-default allowlist",
        )
        allowed = {line for line in lines if line.startswith("!")}
        self.assertTrue(
            {"!Cargo.toml", "!Cargo.lock", "!crates/", "!crates/**", "!deploy/fly/entrypoint.sh"}.issubset(allowed),
            "build context must allow only locked Rust sources and the runtime entrypoint",
        )
        forbidden = {"!.beads/", "!.git/", "!.artifacts/", "!graphify-out/", "!.ramu-kaka/"}
        self.assertFalse(allowed & forbidden, "private/local state must never reach the remote builder")

    def test_manifest_passes_public_tls_through_to_flowplane(self) -> None:
        path, manifest = load_manifest()
        services = public_service_ports(manifest)
        self.assertEqual(
            len(services),
            1,
            f"{path.relative_to(ROOT)} must declare exactly one public service",
        )
        internal, external = services[0]
        self.assertEqual(internal, 8080, "the sole public service must target Flowplane API TLS :8080")
        self.assertEqual(external, {443}, "the sole public service must expose only TLS passthrough port 443")
        exposed = {internal, *external}
        self.assertFalse(
            exposed & FORBIDDEN_PUBLIC_PORTS,
            f"private/admin port exposed publicly: {sorted(exposed & FORBIDDEN_PUBLIC_PORTS)}",
        )
        self.assertNotIn("http_service", manifest, "http_service would terminate TLS at Fly Proxy")
        raw_services = manifest.get("services", [])
        if isinstance(raw_services, dict):
            raw_services = [raw_services]
        self.assertEqual(len(raw_services), 1)
        service = raw_services[0]
        ports = service.get("ports", [])
        if isinstance(ports, dict):
            ports = [ports]
        self.assertEqual(len(ports), 1)
        self.assertFalse(ports[0].get("handlers", []), "TLS/HTTP handlers must be absent for passthrough")
        checks = service.get("http_checks", [])
        if isinstance(checks, dict):
            checks = [checks]
        self.assertTrue(checks, "public API service must declare backend HTTPS health/readiness checks")
        check_paths = {
            str(check.get("path", ""))
            for check in checks
            if isinstance(check, dict)
        }
        self.assertTrue(
            check_paths & {"/healthz", "/readyz"},
            f"public API checks must use a documented operational endpoint, got {sorted(check_paths)}",
        )
        for check in checks:
            self.assertEqual(str(check.get("protocol", "")).lower(), "https")
            self.assertTrue(check.get("tls_server_name"), "HTTPS checks must verify the API certificate hostname")
            self.assertNotEqual(check.get("tls_skip_verify"), True)
            self.assertEqual(
                check.get("tls_server_name"),
                "cp.getflowplane.io",
                "the backend check must verify the selected public API certificate SAN",
            )

        for other in sorted(FLY_ROOT.glob("*.toml")):
            if other == path:
                continue
            other_manifest = parse_manifest(read_text(other), other)
            for other_internal, other_external in public_service_ports(other_manifest):
                all_ports = {other_internal, *other_external}
                self.assertFalse(
                    all_ports & FORBIDDEN_PUBLIC_PORTS,
                    f"{other.relative_to(ROOT)} exposes private/admin port(s) {sorted(all_ports & FORBIDDEN_PUBLIC_PORTS)}",
                )

    def test_control_plane_declares_every_production_security_input(self) -> None:
        _, manifest = load_manifest()
        text = recursive_text()
        for variable, purpose in sorted(CP_RUNTIME_INPUTS.items()):
            with self.subTest(variable=variable):
                self.assertIn(variable, text, f"Fly package does not declare {purpose} input {variable}")

        env = manifest.get("env", {})
        self.assertIsInstance(env, dict)
        self.assertNotIn("FLOWPLANE_DEV_MODE", env)
        self.assertNotIn("FLOWPLANE_DEV_MODE_ACK", env)
        self.assertNotEqual(str(env.get("FLOWPLANE_API_INSECURE", "false")).lower(), "true")

    def test_rls_admin_listens_on_fly_ipv6_private_network(self) -> None:
        path = FLY_ROOT / "rls.fly.toml"
        self.assertTrue(path.is_file(), "separate private RLS manifest is required")
        manifest = parse_manifest(read_text(path), path)
        env = manifest.get("env", {})
        self.assertIsInstance(env, dict)
        self.assertEqual(
            env.get("FLOWPLANE_RLS_ADMIN_LISTEN"),
            "[::]:8081",
            "cross-app Fly .internal DNS resolves to IPv6, so an IPv4-only admin bind is unreachable",
        )
        self.assertEqual(
            public_service_ports(manifest),
            [],
            "the RLS admin listener must remain private and unpublished",
        )

    def test_private_pem_and_token_material_is_delivered_as_files(self) -> None:
        _, manifest = load_manifest()
        env = manifest.get("env", {})
        self.assertIsInstance(env, dict)
        bindings = secret_file_bindings(manifest)
        guest_paths = set(bindings.values())
        self.assertTrue(bindings, "Fly [[files]] secret bindings are required for private material")

        for variable, purpose in sorted(PRIVATE_FILE_INPUTS.items()):
            with self.subTest(variable=variable):
                value = env.get(variable)
                self.assertIsInstance(value, str, f"{purpose} must be configured by file path in [env]")
                self.assertIn(value, guest_paths, f"{variable} must point at a Fly secret-backed [[files]] guest_path")

        forbidden_direct = {
            "FLOWPLANE_BOOTSTRAP_TOKEN",
            "FLOWPLANE_RLS_ADMIN_TOKEN",
        }
        self.assertFalse(forbidden_direct & set(env), "private tokens must use their _FILE inputs")
        for key in ("FLOWPLANE_API_TLS_KEY", "FLOWPLANE_XDS_TLS_KEY", "FLOWPLANE_CERT_ISSUER_CA_KEY_PATH"):
            value = env.get(key, "")
            self.assertNotRegex(str(value), r"-----BEGIN|\$\{|\{\{", f"{key} must be a literal guest file path")

    def test_machine_entrypoint_migrates_then_serves_without_secret_logging(self) -> None:
        build_file = next(path for path in (FLY_ROOT / "Containerfile", FLY_ROOT / "Dockerfile") if path.is_file())
        build_text = read_text(build_file)
        match = re.search(r'(?m)^\s*ENTRYPOINT\s+(?:\[\s*)?["\']([^"\']+)', build_text)
        self.assertIsNotNone(match, "container must declare a machine ENTRYPOINT")
        assert match is not None
        entrypoint = FLY_ROOT / Path(match.group(1)).name
        self.assertTrue(entrypoint.is_file(), f"entrypoint script missing: {entrypoint.relative_to(ROOT)}")
        text = read_text(entrypoint)

        migrate = re.search(r"\bflowplane\s+db\s+migrate\b", text)
        serve = re.search(r"\b(?:exec\s+)?flowplane\s+serve\b", text)
        self.assertIsNotNone(migrate, "entrypoint must run database migrations")
        self.assertIsNotNone(serve, "entrypoint must run flowplane serve")
        assert migrate is not None and serve is not None
        self.assertLess(migrate.start(), serve.start(), "migrations must complete before serve")
        self.assertRegex(text, r"(?m)^\s*exec\s+flowplane\s+serve\b", "serve must replace the shell")
        self.assertRegex(
            text,
            r'(?m)^\s*if \[ "\$#" -gt 0 \]; then\s*\n\s*exec "\$@"',
            "Fly release_command arguments must bypass machine startup and replace the shell",
        )
        self.assertNotRegex(text, r"(?m)^\s*set\s+-[^\n]*x", "shell tracing can disclose secrets")
        self.assertNotRegex(text, r"(?m)^\s*(?:env|printenv)\b", "entrypoint must not dump its environment")
        self.assertNotRegex(
            text,
            r"(?i)(?:echo|printf)[^\n]*(?:TOKEN|PASSWORD|SECRET|PRIVATE|TLS_KEY|DATABASE_URL)",
            "entrypoint must not print secret-bearing variables",
        )

    def test_fly_is_documented_as_packaging_not_product_identity(self) -> None:
        readme = FLY_ROOT / "README.md"
        self.assertTrue(readme.is_file(), "deploy/fly/README.md must state the provider boundary")
        text = read_text(readme).lower()
        self.assertRegex(text, r"(?:provider|deployment) packaging")
        self.assertRegex(text, r"(?:not|never).{0,80}(?:product identity|tenant identity|dataplane identity)")
        self.assertRegex(text, r"(?:oidc|client certificate|mTLS)", "README must name the actual identity boundary")


if __name__ == "__main__":
    unittest.main(verbosity=2)
