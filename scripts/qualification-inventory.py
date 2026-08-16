#!/usr/bin/env python3
"""Capture exact Flowplane artifact surfaces for `flowplane qualification inventory`."""

import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import sys

METHODS = {"get", "put", "post", "delete", "options", "head", "patch", "trace"}
DASHBOARD_SCREENS = {
    "Overview": "/",
    "Resources": "/resources",
    "APIs": "/apis",
    "Learning": "/learning",
    "AI": "/ai",
    "MCP": "/mcp",
    "Operations": "/operations",
}


def run_json(binary, *args):
    result = subprocess.run(
        [str(binary), *args], check=True, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    if result.stderr:
        raise ValueError(f"{binary} {' '.join(args)} wrote unexpected stderr")
    return json.loads(result.stdout)


def digest(path):
    value = hashlib.sha256()
    with pathlib.Path(path).open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def cli_commands(command, prefix=""):
    rows = []
    for child in command.get("subcommands", []):
        name = f"{prefix} {child['name']}".strip()
        rows.append(name)
        rows.extend(cli_commands(child, name))
    return rows


def markdown_cli_commands(text, live_commands):
    candidates = set(re.findall(r"^### `([^`]+)`\s*$", text, re.MULTILINE))
    candidates.update(re.findall(r"^\| `([^`]+)` \|", text, re.MULTILINE))
    documented = set()
    for candidate in candidates:
        matches = [
            command
            for command in live_commands
            if candidate == command or candidate.startswith(command + " ")
        ]
        if matches:
            documented.add(max(matches, key=len))
    return documented


def markdown_operations(text):
    return {
        f"{method} {path}"
        for method, path in re.findall(
            r"^\|\s*(GET|POST|PUT|PATCH|DELETE|OPTIONS|HEAD|TRACE)\s*\|\s*`([^`]+)`\s*\|",
            text,
            re.MULTILINE,
        )
    }


def dashboard_routes(source):
    match = re.search(r"ROUTE_PATHS:.*?= &\[(.*?)\];", source, re.DOTALL)
    if not match:
        raise ValueError("dashboard ROUTE_PATHS declaration not found")
    return set(re.findall(r'"([^\"]+)"', match.group(1)))


def configuration_variables(text):
    return set(re.findall(r"`(FLOWPLANE_[A-Z0-9_]+)`", text))


def row(identifier):
    return {"id": identifier}


def classify(identifier, seen):
    if identifier.startswith("binary:"):
        return (
            "supported-core",
            "Exact candidate release binary with a recorded SHA-256 digest",
        )
    if (
        seen.intersection({"openapi", "cli_schema", "dashboard_routes"})
        and seen.intersection({"stable_docs", "cli_help", "config"})
    ):
        return (
            "supported-core",
            "Executable candidate surface and independent declaration agree",
        )
    return (
        "incomplete",
        "Observed in only one surface or lacks stable documentation corroboration",
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--flowplane", required=True, type=pathlib.Path)
    parser.add_argument("--agent", required=True, type=pathlib.Path)
    parser.add_argument("--rls", required=True, type=pathlib.Path)
    parser.add_argument("--release", required=True)
    parser.add_argument("--repo", default=pathlib.Path(__file__).resolve().parents[1], type=pathlib.Path)
    parser.add_argument("--output-path", required=True, type=pathlib.Path)
    args = parser.parse_args(argv)

    for binary in (args.flowplane, args.agent, args.rls):
        if not binary.is_file():
            parser.error(f"candidate binary does not exist: {binary}")

    version = run_json(args.flowplane, "version", "-o", "json")["data"]["version"]
    if version != args.release:
        parser.error(f"candidate version {version} does not match --release {args.release}")
    openapi = run_json(args.flowplane, "openapi")
    schema = run_json(args.flowplane, "schema", "-o", "json")

    cli_doc = (args.repo / "docs/reference/cli.md").read_text()
    rest_doc = (args.repo / "docs/reference/rest-api.md").read_text()
    dashboard_doc = (args.repo / "docs/how-to/view-team-dashboard.md").read_text()
    config_doc = (args.repo / "docs/reference/configuration.md").read_text()
    dashboard_source = (args.repo / "crates/flowplane/src/cli/dashboard/mod.rs").read_text()

    openapi_ids = {
        f"api:{method.upper()} {path}"
        for path, item in openapi.get("paths", {}).items()
        for method in item
        if method.lower() in METHODS
    }
    live_cli_commands = set(cli_commands(schema["data"]["command"]))
    cli_schema_ids = {f"cli:{name}" for name in live_cli_commands}
    cli_help_ids = {
        f"cli:{name}" for name in markdown_cli_commands(cli_doc, live_cli_commands)
    }
    documented_api_ids = {f"api:{operation}" for operation in markdown_operations(rest_doc)}
    dashboard_route_ids = {f"dashboard:{route}" for route in dashboard_routes(dashboard_source)}
    documented_dashboard_ids = {
        f"dashboard:{route}"
        for screen, route in DASHBOARD_SCREENS.items()
        if f"**{screen}**" in dashboard_doc
    }
    config_ids = {f"config:{name}" for name in configuration_variables(config_doc)}
    binary_ids = {f"binary:{path.name}" for path in (args.flowplane, args.agent, args.rls)}

    stable_docs = documented_api_ids | documented_dashboard_ids
    surfaces = {
        "openapi": [row(value) for value in sorted(openapi_ids)],
        "cli_schema": [row(value) for value in sorted(cli_schema_ids)],
        "cli_help": [row(value) for value in sorted(cli_help_ids)],
        "stable_docs": [row(value) for value in sorted(stable_docs)],
        "dashboard_routes": [row(value) for value in sorted(dashboard_route_ids)],
        "config": [row(value) for value in sorted(config_ids)],
        "binaries": [row(value) for value in sorted(binary_ids)],
    }
    observed = {}
    for surface, rows in surfaces.items():
        for item in rows:
            observed.setdefault(item["id"], set()).add(surface)

    classifications = {}
    for identifier, seen in sorted(observed.items()):
        classification, rationale = classify(identifier, seen)
        classifications[identifier] = {
            "classification": classification,
            "rationale": rationale,
        }

    binaries = [
        {"name": path.name, "sha256": digest(path)}
        for path in sorted((args.flowplane, args.agent, args.rls), key=lambda path: path.name)
    ]
    artifact_seed = json.dumps(
        {"release": args.release, "binaries": binaries},
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    document = {
        "artifact": {
            "release": args.release,
            "digest": "sha256:" + hashlib.sha256(artifact_seed).hexdigest(),
            "binaries": binaries,
        },
        "surfaces": surfaces,
        "classifications": classifications,
    }
    args.output_path.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (KeyError, ValueError, OSError, subprocess.SubprocessError, json.JSONDecodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(2)
