#!/usr/bin/env python3
"""Black-box fpv2-fme.1 acceptance harness (Python stdlib only).

Contract sources: docs/reference/cli.md, docs/how-to/script-the-cli.md,
docs/tutorials/getting-started.md and the delegated empty-eval acceptance.
The old evaluate-no-clone tutorial describes seeded gateway resources; the
slice acceptance deliberately supersedes that behavior. No implementation,
Compose contents, Envoy admin, logs, or host credential files are inspected.

The parent MUST supply a fresh, pre-started, isolated Compose project and its
host gateway URL. This harness authors one exposure and leaves it intact.
It never starts, removes, prunes, or resets resources. --restart-test permits
only `compose -p PROJECT -f FILE restart` (same containers/volumes).
A failed empty-state checkpoint stops before expose or restart.

Usage:
  python3 scripts/ci/test-empty-eval-integration.py --self-test
  python3 scripts/ci/test-empty-eval-integration.py \
    --compose-command 'podman compose' --project isolated-fme-s1 \
    --compose-file /absolute/path/to/compose.eval.yml \
    --gateway-url http://127.0.0.1:21000 --restart-test

Self-tests use explicitly synthetic fixtures, not runtime evidence. Gateway
body matching is byte-exact: no whitespace stripping or substring matching.
Resource names are read from actual CLI lists, not guessed from private
expose naming conventions; exact inventories must survive restart.
"""

import argparse
import copy
from datetime import datetime
import http.client
import json
import math
import re
import shlex
import socket
import subprocess
import sys
import time
import unittest
import urllib.error
import urllib.parse
import urllib.request

EXPECTED_BODY = b"hello from the flowplane eval demo upstream\n"
KINDS = ("cluster", "route", "listener", "api")


class Failure(Exception):
    """Sanitized contract or execution failure; never contains CLI output."""


class Pending(Failure):
    """An observation which may converge before the phase deadline."""


def require(condition, message):
    if not condition:
        raise Failure(message)


def heartbeat_newer(current, previous):
    try:
        observed = datetime.fromisoformat(current.replace("Z", "+00:00"))
        baseline = datetime.fromisoformat(previous.replace("Z", "+00:00"))
    except (AttributeError, TypeError, ValueError):
        raise Failure("heartbeat must be a valid timestamp") from None
    require(observed.tzinfo is not None and baseline.tzinfo is not None,
            "heartbeat timestamp must include a timezone")
    return observed > baseline


def envelope(raw):
    try:
        value = json.loads(raw)
    except (ValueError, UnicodeError):
        raise Failure("CLI stdout is not one valid JSON envelope") from None
    require(isinstance(value, dict), "CLI envelope must be an object")
    require(type(value.get("schemaVersion")) is int
            and value["schemaVersion"] == 1, "unsupported CLI schemaVersion")
    require(isinstance(value.get("kind"), str) and value["kind"],
            "CLI envelope lacks a nonempty kind")
    require("data" in value, "CLI envelope lacks data")
    return value


def list_rows(raw):
    value = envelope(raw)
    require(value["kind"].endswith("List"), "CLI collection kind must end in List")
    data = value["data"]
    # Both shapes are documented. Never infer a paginated total from rows.
    if isinstance(data, list):
        rows = data
    else:
        require(isinstance(data, dict) and isinstance(data.get("items"), list),
                "unknown CLI list shape (expected data array or data.items)")
        for key in ("total", "offset", "limit"):
            require(type(data.get(key)) is int and data[key] >= 0,
                    "paginated CLI list lacks valid " + key)
        rows = data["items"]
        require(data["offset"] == 0, "CLI list begins at a nonzero offset")
        require(data["total"] == len(rows),
                "CLI total disagrees with collected rows; truncated inventory rejected")
        require(data["limit"] >= len(rows), "CLI list exceeds declared limit")
    require(all(isinstance(row, dict) for row in rows), "CLI list rows must be objects")
    names = [row.get("name") for row in rows]
    require(all(isinstance(name, str) and name for name in names),
            "CLI list row lacks a nonempty name")
    require(len(set(names)) == len(names), "CLI list has duplicate names")
    return rows


def body_matches(status, body):
    return status == 200 and body == EXPECTED_BODY


def safe_names(rows):
    # Never print arbitrary server-controlled names (they could carry secrets).
    names = sorted(row["name"] for row in rows)
    return [name if re.fullmatch(r"[A-Za-z0-9_.-]{1,80}", name)
            else "<non-displayable-name>" for name in names]


def cli_failure(result):
    details = "exit=" + str(result.returncode)
    try:
        error = json.loads(result.stderr)
        if isinstance(error, dict):
            status = error.get("status")
            if type(status) is int and 100 <= status <= 599:
                details += " HTTP=" + str(status)
            if type(error.get("retryable")) is bool:
                details += " retryable=" + str(error["retryable"]).lower()
    except (ValueError, UnicodeError):
        pass
    # message/hint/stdout/stderr/environment are intentionally never emitted.
    return details


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Harness:
    def __init__(self, args):
        self.args = args
        self.compose = shlex.split(args.compose_command) + [
            "-p", args.project, "-f", args.compose_file]
        self.http = urllib.request.build_opener(
            urllib.request.ProxyHandler({}), NoRedirect())

    def run_command(self, argv, timeout, label):
        try:
            result = subprocess.run(argv, stdin=subprocess.DEVNULL,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    timeout=timeout, check=False)
        except subprocess.TimeoutExpired:
            raise Pending(label + " timed out (output withheld)") from None
        except OSError:
            raise Failure(label + " could not execute (check Compose executable)") from None
        if result.returncode:
            raise Pending(label + " failed: " + cli_failure(result))
        return result.stdout

    def cli(self, command, deadline=None):
        timeout = self.args.command_timeout
        if deadline is not None:
            timeout = min(timeout, max(0.1, deadline - time.monotonic()))
        # Token is read and exported exclusively inside the container. Positional
        # shell arguments avoid interpolation of passed URLs or CLI arguments.
        script = ('set +x; FLOWPLANE_TOKEN=$(cat /shared/dev-token) || exit 1; '
                  '[ -n "$FLOWPLANE_TOKEN" ] || exit 1; '
                  'export FLOWPLANE_TOKEN; '
                  'export FLOWPLANE_SERVER=http://127.0.0.1:8080; '
                  'export FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default; '
                  'exec flowplane --output json --quiet --no-color "$@"')
        argv = self.compose + ["exec", "-T", "flowplane-eval", "sh", "-c",
                               script, "empty-eval-test", "--timeout",
                               str(max(1, int(timeout))), *command]
        return self.run_command(argv, timeout, "CLI " + command[0])

    def wait(self, label, observe):
        deadline = time.monotonic() + self.args.phase_timeout
        last = "no observation"
        while time.monotonic() < deadline:
            try:
                value = observe(deadline)
                print("PASS " + label, flush=True)
                return value
            except Pending as error:
                last = str(error)
            time.sleep(min(self.args.poll_interval,
                           max(0, deadline - time.monotonic())))
        raise Failure(label + " did not converge: " + last)

    def readiness(self, deadline):
        data = envelope(self.cli(["auth", "whoami"], deadline))["data"]
        require(isinstance(data, dict) and data,
                "authenticated readiness returned an empty/non-object principal")
        # A scoped authenticated list is the CP readiness proof, not a public
        # health endpoint or inferred service/process status.
        list_rows(self.cli(["cluster", "list"], deadline))

    def dataplane(self, deadline, newer_than=None):
        rows = list_rows(self.cli(["dataplane", "list"], deadline))
        if not rows:
            raise Pending("dp-eval has not automatically registered")
        require(len(rows) == 1 and rows[0]["name"] == "dp-eval",
                "expected exactly one automatic dataplane named dp-eval")
        data = envelope(self.cli(["dataplane", "get", "dp-eval"], deadline))["data"]
        require(isinstance(data, dict) and data.get("name") == "dp-eval",
                "dataplane get does not identify dp-eval")
        require("last_heartbeat_at" in data, "dataplane lacks last_heartbeat_at")
        if data["last_heartbeat_at"] is None:
            raise Pending("dp-eval exists but last_heartbeat_at is null")
        require(isinstance(data["last_heartbeat_at"], str)
                and bool(data["last_heartbeat_at"]), "heartbeat must be a nonempty timestamp")
        if newer_than is not None and not heartbeat_newer(data["last_heartbeat_at"], newer_than):
            raise Pending("dp-eval heartbeat has not advanced after restart")
        return data

    def inventory(self, deadline=None):
        return {kind: list_rows(self.cli([kind, "list"], deadline)) for kind in KINDS}

    def gateway(self, deadline=None):
        timeout = self.args.request_timeout
        if deadline is not None:
            timeout = min(timeout, max(0.1, deadline - time.monotonic()))
        request = urllib.request.Request(self.args.gateway_url,
                                         headers={"Connection": "close"})
        try:
            with self.http.open(request, timeout=timeout) as response:
                return response.status, response.read(len(EXPECTED_BODY) + 2)
        except urllib.error.HTTPError as error:
            with error:
                return error.code, error.read(len(EXPECTED_BODY) + 2)
        except (urllib.error.URLError, TimeoutError, socket.timeout,
                ConnectionError, http.client.HTTPException):
            return None, b""

    def routed(self, deadline):
        status, body = self.gateway(deadline)
        if not body_matches(status, body):
            raise Pending("gateway status=" + str(status)
                          + "; exact demo body not observed (body withheld)")

    def assert_exposure(self, expected=None):
        inventory = self.inventory()
        for kind in ("cluster", "route", "listener"):
            require(len(inventory[kind]) == 1,
                    "authored exposure must have exactly one " + kind)
        require(not inventory["api"], "S1 exposure must not seed API definitions")
        if expected is not None:
            for kind in KINDS:
                before = sorted((r["name"], r.get("id")) for r in expected[kind])
                after = sorted((r["name"], r.get("id")) for r in inventory[kind])
                require(before == after, "restart changed " + kind + " names/identities")
        for kind, rows in inventory.items():
            print("inventory " + kind + ": count=" + str(len(rows))
                  + " names=" + json.dumps(safe_names(rows)), flush=True)
        return inventory

    def execute(self):
        self.wait("authenticated CP readiness (dev-org/default)", self.readiness)
        empty = self.inventory()
        for kind, rows in empty.items():
            require(not rows, "fresh eval contains seeded " + kind + " resources")
            print("PASS empty " + kind + ": count=0 names=[]", flush=True)
        self.wait("automatic dp-eval registration and non-null heartbeat", self.dataplane)
        # Heartbeat/readiness startup must not have authored gateway resources.
        require(all(not rows for rows in self.inventory().values()),
                "resources appeared during automatic dataplane startup")
        status, _ = self.gateway()
        require(status is None or status >= 400,
                "gateway request succeeded/redirected before explicit exposure")
        print("PASS gateway fails before exposure: status=" + str(status), flush=True)
        # Mutating expose is executed exactly once; retrying can mask partial writes.
        result = envelope(self.cli([
            "expose", "http://demo-upstream:5678", "--name", "demo", "--path", "/",
            "--port", "10000", "--public-base-url", self.args.gateway_url]))
        require(isinstance(result["data"], dict) and result["data"],
                "expose must return an object payload")
        authored = self.assert_exposure()
        self.wait("Envoy serves exact demo body after explicit expose", self.routed)
        if self.args.restart_test:
            # Restart the serving units, not completed PKI/setup jobs or PostgreSQL.
            # Compose restart does not honor healthy dependencies; a simultaneous
            # dashboard launch could retain the CP's old per-boot dev token forever.
            self.run_command(self.compose + ["restart", "flowplane-eval"],
                             self.args.phase_timeout, "isolated control-plane restart")
            self.wait("authenticated CP readiness after restart", self.readiness)
            # Restart in dependency order. Podman's Docker-compatible API rejects
            # concurrent agent restart while its Envoy dependency is stopped.
            self.run_command(self.compose + ["restart", "envoy"],
                             self.args.phase_timeout, "isolated Envoy restart")
            self.run_command(self.compose + ["restart", "flowplane-agent"],
                             self.args.phase_timeout, "isolated agent restart")
            # Sample only after the old agent is stopped; its final report must
            # not satisfy the new-process heartbeat assertion.
            baseline = self.dataplane(None)["last_heartbeat_at"]
            self.wait("dp-eval heartbeat advances after restart",
                      lambda deadline: self.dataplane(deadline, newer_than=baseline))
            dashboard = self.run_command(self.compose + ["ps", "-q", "flowplane-dashboard"],
                                         self.args.command_timeout, "optional dashboard discovery")
            if dashboard.strip():
                # Read the newly minted token only after CP authentication succeeds.
                # The release workflow then checks dashboard health, URL and HTTP.
                self.run_command(self.compose + ["restart", "flowplane-dashboard"],
                                 self.args.phase_timeout, "dashboard restart after CP readiness")
                print("PASS existing dashboard restarted after CP readiness", flush=True)
            self.assert_exposure(authored)
            self.wait("same-volume restart retains exact demo body", self.routed)
        else:
            print("NOT RUN same-volume restart (requires --restart-test)", flush=True)


class SyntheticTests(unittest.TestCase):
    """Pure synthetic parser/body fixtures; no Compose or HTTP requests."""
    def test_positive_lists(self):
        page = {"schemaVersion": 1, "kind": "clusterList", "data": {
            "items": [{"name": "demo"}], "total": 1, "limit": 50, "offset": 0}}
        self.assertEqual(list_rows(json.dumps(page)), [{"name": "demo"}])
        page["data"] = []
        self.assertEqual(list_rows(json.dumps(page)), [])
        page["data"] = {"items": [], "total": 0, "limit": 50, "offset": 0}
        self.assertEqual(list_rows(json.dumps(page)), [])

    def test_negative_envelopes(self):
        good = {"schemaVersion": 1, "kind": "clusterList", "data": {
            "items": [], "total": 0, "limit": 50, "offset": 0}}
        bad = ["not json", "[]", json.dumps({"data": []})]
        for key, value in (("schemaVersion", True), ("schemaVersion", 2),
                           ("kind", "cluster"), ("data", None),
                           ("data", {"unexpected": []})):
            item = copy.deepcopy(good)
            item[key] = value
            bad.append(json.dumps(item))
        for key, value in (("total", 1), ("total", True), ("offset", 1),
                           ("limit", -1), ("items", [None]),
                           ("items", [{"name": "demo"}, {"name": "demo"}])):
            item = copy.deepcopy(good)
            item["data"][key] = value
            bad.append(json.dumps(item))
        item = copy.deepcopy(good)
        del item["data"]["total"]
        bad.append(json.dumps(item))
        for raw in bad:
            with self.subTest(fixture=raw):
                with self.assertRaises(Failure):
                    list_rows(raw)
        for rows in ([{}], [{"name": ""}], [{"name": 12}],
                     [{"name": "demo"}, {"name": "demo"}]):
            with self.assertRaises(Failure):
                list_rows(json.dumps({"schemaVersion": 1,
                                      "kind": "clusterList", "data": rows}))

    def test_heartbeat_freshness(self):
        before = "2026-10-07T07:00:00Z"
        self.assertFalse(heartbeat_newer(before, before))
        self.assertFalse(heartbeat_newer("2026-10-07T06:59:59Z", before))
        self.assertFalse(heartbeat_newer("2026-10-07T18:00:00+11:00", before))
        self.assertTrue(heartbeat_newer("2026-10-07T07:00:00.001Z", before))
        for invalid in (None, "", "not-time", "2026-10-07T07:00:00"):
            with self.assertRaises(Failure):
                heartbeat_newer(invalid, before)

    def test_exact_body(self):
        self.assertTrue(body_matches(200, EXPECTED_BODY))
        for body in (b"", EXPECTED_BODY + b"\n", b" " + EXPECTED_BODY,
                     EXPECTED_BODY + b" extra", b"other upstream"):
            self.assertFalse(body_matches(200, body))
        for status in (None, 201, 301, 404, 503):
            self.assertFalse(body_matches(status, EXPECTED_BODY))

    def test_failures_do_not_echo_secret_output(self):
        marker = "SYNTHETIC_SECRET_CANARY"
        result = subprocess.CompletedProcess([], 3, stdout=marker.encode(),
            stderr=json.dumps({"message": marker, "hint": marker,
                               "status": 401, "retryable": False}).encode())
        self.assertNotIn(marker, cli_failure(result))
        self.assertIn("HTTP=401", cli_failure(result))


def main():
    parser = argparse.ArgumentParser(description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--compose-command", default="docker compose")
    parser.add_argument("--project", help="explicit isolated project owned by the parent")
    parser.add_argument("--compose-file")
    parser.add_argument("--gateway-url", help="host gateway origin, root path only")
    parser.add_argument("--restart-test", action="store_true")
    parser.add_argument("--phase-timeout", type=float, default=120)
    parser.add_argument("--command-timeout", type=float, default=20)
    parser.add_argument("--request-timeout", type=float, default=5)
    parser.add_argument("--poll-interval", type=float, default=2)
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(SyntheticTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    for key in ("project", "compose_file", "gateway_url"):
        if not getattr(args, key):
            parser.error("--" + key.replace("_", "-") + " is required")
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]*", args.project):
        parser.error("--project must be an explicit valid Compose project name")
    for key in ("phase_timeout", "command_timeout", "request_timeout", "poll_interval"):
        if not math.isfinite(getattr(args, key)) or getattr(args, key) <= 0:
            parser.error("timeouts and poll interval must be finite and positive")
    try:
        prefix = shlex.split(args.compose_command)
        url = urllib.parse.urlsplit(args.gateway_url)
        valid = (bool(prefix) and url.scheme in ("http", "https") and url.hostname
                 and not url.username and not url.password and not url.query
                 and not url.fragment and url.path in ("", "/"))
        _ = url.port
    except ValueError:
        valid = False
    if not valid:
        parser.error("invalid Compose command or gateway URL (origin only; no credentials)")
    try:
        Harness(args).execute()
    except Failure as error:
        print("FAIL " + str(error), file=sys.stderr)
        print("Resources left intact; parent owns cleanup. Raw output withheld.", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("Interrupted; resources left intact for parent cleanup.", file=sys.stderr)
        return 130
    print("PASS fpv2-fme.1 empty-eval integration"
          + (" including restart" if args.restart_test else " (restart not exercised)"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
