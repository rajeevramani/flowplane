#!/usr/bin/env python3
"""Black-box empty-eval/shared-exposure release harness (Python stdlib only).

Contract sources: docs/reference/cli.md, docs/how-to/script-the-cli.md,
docs/tutorials/getting-started.md and the delegated empty-eval acceptance.
The old evaluate-no-clone tutorial describes seeded gateway resources; the
slice acceptance deliberately supersedes that behavior. No implementation,
Compose contents, Envoy admin, logs, or host credential files are inspected.

The parent MUST supply a fresh, pre-started, isolated Compose project and its
host gateway URL. This harness authors two shared exposures, tests both removal
orders with same-name repetition, and finishes with empty gateway inventories.
Failures stop immediately without compensating cleanup; the parent owns cleanup.
It never starts, prunes, or resets infrastructure. --restart-test permits
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
Shared/both-order removal runs by default; --restart-test adds ordered restarts
in both cycles. A successful run intentionally leaves the dashboard/dataplane
running but gateway/API inventories empty. Packaged 3.2.0 and strict-mTLS
qualification remain S7; --self-test is synthetic harness verification only.
"""

import argparse
import contextlib
import copy
import io
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
from unittest import mock
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

    def run_command(self, argv, timeout, label, input_bytes=None):
        try:
            result = subprocess.run(argv, input=input_bytes,
                                    stdin=subprocess.DEVNULL if input_bytes is None else None,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    timeout=timeout, check=False)
        except subprocess.TimeoutExpired:
            raise Pending(label + " timed out (output withheld)") from None
        except OSError:
            raise Failure(label + " could not execute (check Compose executable)") from None
        if result.returncode:
            raise Pending(label + " failed: " + cli_failure(result))
        return result.stdout

    def cli(self, command, deadline=None, file_payload=None):
        timeout = self.args.command_timeout
        if deadline is not None:
            timeout = min(timeout, max(0.1, deadline - time.monotonic()))
        # Token is read and exported exclusively inside the container. Positional
        # shell arguments avoid interpolation of passed URLs or CLI arguments.
        script = ('set +x; FLOWPLANE_TOKEN=$(cat /shared/dev-token) || exit 1; '
                  '[ -n "$FLOWPLANE_TOKEN" ] || exit 1; '
                  'export FLOWPLANE_TOKEN; '
                  'export FLOWPLANE_SERVER=http://127.0.0.1:8080; '
                  'export FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default; ')
        input_bytes = None
        if file_payload is None:
            script += 'exec flowplane --output json --quiet --no-color "$@"'
        else:
            # The supported CLI consumes a file inside its own container, not a
            # host path. Keep the file on failure for parent-owned diagnostics.
            input_bytes = json.dumps(file_payload).encode()
            script += ('file=$(mktemp /shared/exposure-smoke.XXXXXX) || exit 1; '
                       'cat > "$file" || exit 1; '
                       'flowplane --output json --quiet --no-color "$@" --file "$file"; '
                       'status=$?; [ "$status" -ne 0 ] || rm -f "$file"; exit "$status"')
        argv = self.compose + ["exec", "-T", "flowplane-eval", "sh", "-c",
                               script, "empty-eval-test", "--timeout",
                               str(max(1, int(timeout))), *command]
        return self.run_command(argv, timeout, "CLI " + command[0], input_bytes)

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

    def gateway(self, deadline=None, url=None):
        timeout = self.args.request_timeout
        if deadline is not None:
            timeout = min(timeout, max(0.1, deadline - time.monotonic()))
        request = urllib.request.Request(url or self.args.gateway_url,
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

    def snapshot(self, clusters):
        inventory = self.inventory()
        for kind, count in (("cluster", clusters), ("route", 1), ("listener", 1), ("api", 0)):
            require(len(inventory[kind]) == count,
                    "unexpected shared lifecycle " + kind + " count")
            # Lists may be summaries. Read every exact target for full spec/revision.
            names = [row["name"] for row in inventory[kind]]
            inventory[kind] = [envelope(self.cli([kind, "get", name]))["data"]
                               for name in names]
            for name, row in zip(names, inventory[kind]):
                require(isinstance(row, dict) and row.get("name") == name
                        and isinstance(row.get("spec"), dict)
                        and isinstance(row.get("id"), str) and bool(row["id"])
                        and isinstance(row.get("name"), str) and bool(row["name"])
                        and type(row.get("revision")) is int and row["revision"] > 0,
                        "resource get lacks identity/full spec/positive revision")
            inventory[kind].sort(key=lambda row: row["name"])
        return inventory

    def routed_urls(self, deadline, urls):
        for url in urls:
            status, body = self.gateway(deadline, url)
            if not body_matches(status, body):
                raise Pending("exposure traffic status=" + str(status)
                              + "; exact demo body not observed (URL/body withheld)")

    def empty_and_refused(self, urls):
        require(all(not rows for rows in self.inventory().values()),
                "final cleanup did not leave empty scoped gateway/API inventories")
        def refused(deadline):
            for url in urls:
                status, _ = self.gateway(deadline, url)
                if status is not None and status < 400:
                    raise Pending("gateway still succeeds/redirects after final cleanup")
        self.wait("final cleanup gateway refusal at both exposure URLs", refused)

    def expose(self, attached=None):
        name, path = ("shared", "/shared") if attached else ("demo", "/")
        command = ["expose", "http://demo-upstream:5678", "--name", name, "--path", path]
        if attached:
            command += ["--listener", attached["listener"]["name"]]
        else:
            command += ["--port", "10000", "--public-base-url", self.args.gateway_url]
        # Mutations are one-shot, outside wait(): an ambiguous failure is terminal.
        data = envelope(self.cli(command))["data"]
        require(isinstance(data, dict) and data.get("name") == name
                and data.get("path") == path and data.get("mode") == (
                    "attached" if attached else "created"), "incorrect expose identity/mode/path")
        for key in ("cluster", "route_config", "listener"):
            require(isinstance(data.get(key), dict), "expose lacks actual " + key)
        current = self.snapshot(2 if attached else 1)
        for key, kind in (("cluster", "cluster"), ("route_config", "route"), ("listener", "listener")):
            require(data[key] in current[kind], "expose response differs from resource readback")
        base = data["listener"]["spec"].get("public_base_url")
        require(isinstance(base, str) and base.rstrip("/") == self.args.gateway_url.rstrip("/"),
                "listener did not preserve explicit public base URL")
        require(data.get("curl_url") == base.rstrip("/") + path,
                "expose curl URL does not derive from stored public base plus prefix")
        require(data["listener"]["spec"].get("route_config") == data["route_config"]["name"]
                and data["listener"]["spec"].get("port") == 10000,
                "expose returned inconsistent actual listener/config/port")
        hosts = data["route_config"]["spec"].get("virtual_hosts")
        require(isinstance(hosts, list) and len(hosts) == 1
                and hosts[0].get("name") == "default" and hosts[0].get("domains") == ["*"],
                "created/shared scaffold must have one wildcard default vhost")
        owned = [r for r in hosts[0].get("routes", []) if r.get("name") == name]
        require(len(owned) == 1 and owned[0].get("match") == {"prefix": {"prefix": path}}
                and owned[0].get("action", {}).get("cluster") == data["cluster"]["name"]
                and owned[0]["action"].get("prefix_rewrite") is None,
                "expose must author the exact route/upstream and forward prefix unchanged")
        if not attached:
            require(len(hosts[0]["routes"]) == 1, "new exposure authored extra routes")
        if attached:
            require(data["listener"] == attached["listener"],
                    "attachment changed listener identity/spec/revision/public endpoint")
            require(data["route_config"]["id"] == attached["route_config"]["id"]
                    and data["route_config"]["name"] == attached["route_config"]["name"],
                    "attachment did not update the actual shared config")
            before = copy.deepcopy(attached["route_config"]["spec"])
            after = copy.deepcopy(data["route_config"]["spec"])
            routes = after["virtual_hosts"][0]["routes"]
            require([r["name"] for r in routes] == ["shared", "demo"],
                    "shared prefix must precede root without reordering original routes")
            rule = routes.pop(0)
            require(rule["match"] == {"prefix": {"prefix": "/shared"}}
                    and rule["action"].get("cluster") == data["cluster"]["name"],
                    "shared route does not select its owned upstream")
            require(after == before, "attachment changed unrelated routes/vhost policies")
            require(data["route_config"]["revision"] > attached["route_config"]["revision"],
                    "attachment did not advance config revision")
            require(attached["cluster"] in current["cluster"], "attachment changed original upstream")
        return data

    def policy(self, exposure):
        before = self.snapshot(2 if exposure["name"] == "shared" else 1)
        config = before["route"][0]
        spec = copy.deepcopy(config["spec"])
        routes = spec["virtual_hosts"][0]["routes"]
        matches = [r for r in routes if r["name"] == exposure["name"]]
        require(len(matches) == 1, "policy target must be the exact exposure route")
        matches[0]["action"]["timeout_secs"] = 23
        if exposure["name"] == "shared":
            # Explicit ordinary authoring makes /shared work with a root-only
            # demo backend too; attachment itself must forward unchanged.
            matches[0]["action"]["prefix_rewrite"] = "/"
        self.cli(["--revision", str(config["revision"]), "route", "update", config["name"]],
                 file_payload={"spec": spec})
        after = self.snapshot(len(before["cluster"]))
        require(after["listener"] == before["listener"] and after["cluster"] == before["cluster"],
                "ordinary route policy edit changed unrelated resources")
        updated = after["route"][0]
        require(updated["id"] == config["id"] and updated["name"] == config["name"]
                and updated["spec"] == spec and updated["revision"] > config["revision"],
                "ordinary route policy edit did not round-trip exact spec/revision")
        # Keep authoritative public representation for subsequent attach checks.
        exposure["route_config"] = copy.deepcopy(updated)
        return after

    def remove(self, exposure, final):
        data = envelope(self.cli(["--yes", "unexpose", exposure["name"]]))["data"]
        expected = {"name": exposure["name"], "cluster_name": exposure["cluster"]["name"],
                    "route_config_name": exposure["route_config"]["name"],
                    "listener_name": exposure["listener"]["name"],
                    "cluster_disposition": "deleted",
                    "route_config_disposition": "deleted" if final else "retained",
                    "listener_disposition": "deleted" if final else "retained"}
        require(isinstance(data, dict) and all(data.get(k) == v for k, v in expected.items()),
                "unexpose returned incorrect exact names/dispositions")

    def removal_order(self, original, attached, before, original_first):
        first, last = (original, attached) if original_first else (attached, original)
        label = "original-first" if original_first else "attached-first"
        print("PHASE shared removal " + label, flush=True)
        self.remove(first, final=False)
        survivor = self.snapshot(1)
        require(survivor["listener"] == before["listener"], "route-only removal changed listener")
        require(survivor["cluster"] == [last["cluster"]], "removal lost/changed surviving upstream")
        expected_spec = copy.deepcopy(before["route"][0]["spec"])
        routes = expected_spec["virtual_hosts"][0]["routes"]
        routes[:] = [r for r in routes if r["name"] != first["name"]]
        config = survivor["route"][0]
        require(config["id"] == before["route"][0]["id"]
                and config["name"] == before["route"][0]["name"]
                and config["spec"] == expected_spec
                and config["revision"] > before["route"][0]["revision"],
                "route-only removal changed surviving policy/identity or failed revision advance")
        self.wait(label + " surviving exposure exact traffic",
                  lambda deadline: self.routed_urls(deadline, [last["curl_url"]]))
        # Successful final attached cleanup after original-first removal is the
        # public behavioral proof of inherited durable managed provenance.
        self.remove(last, final=True)
        self.empty_and_refused([original["curl_url"], attached["curl_url"]])
        print("PASS " + label + " exact retained/deleted dispositions and empty state", flush=True)

    def restart(self, authored, urls):
        self.run_command(self.compose + ["restart", "flowplane-eval"],
                         self.args.phase_timeout, "isolated control-plane restart")
        self.wait("authenticated CP readiness after restart", self.readiness)
        self.run_command(self.compose + ["restart", "envoy"],
                         self.args.phase_timeout, "isolated Envoy restart")
        self.run_command(self.compose + ["restart", "flowplane-agent"],
                         self.args.phase_timeout, "isolated agent restart")
        baseline = self.dataplane(None)["last_heartbeat_at"]
        self.wait("dp-eval heartbeat advances after restart",
                  lambda deadline: self.dataplane(deadline, newer_than=baseline))
        dashboard = self.run_command(self.compose + ["ps", "-q", "flowplane-dashboard"],
                                     self.args.command_timeout, "optional dashboard discovery")
        if dashboard.strip():
            self.run_command(self.compose + ["restart", "flowplane-dashboard"],
                             self.args.phase_timeout, "dashboard restart after CP readiness")
        require(self.snapshot(2) == authored,
                "restart changed exact resource names/IDs/full specs/policies/revisions")
        self.wait("same-volume restart retains both exposure bodies",
                  lambda deadline: self.routed_urls(deadline, urls))

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
        previous_ids = set()
        for original_first in (True, False):
            print("PHASE create/share/policy/restart "
                  + ("original-first" if original_first else "attached-first repeat"), flush=True)
            original = self.expose()
            self.wait("explicit original exposure exact traffic",
                      lambda deadline: self.routed_urls(deadline, [original["curl_url"]]))
            self.policy(original)
            attached = self.expose(original)
            authored = self.policy(attached)
            ids = {row["id"] for rows in authored.values() for row in rows}
            require(len(ids) == 4 and not ids.intersection(previous_ids),
                    "same-name repetition reused resource identities")
            previous_ids.update(ids)
            urls = [original["curl_url"], attached["curl_url"]]
            self.wait("both shared exposures exact traffic",
                      lambda deadline: self.routed_urls(deadline, urls))
            if self.args.restart_test:
                self.restart(authored, urls)
            else:
                print("NOT RUN same-volume restart (requires --restart-test)", flush=True)
            self.removal_order(original, attached, authored, original_first)
        print("PASS same-name lifecycle repeat with fresh IDs; final inventories empty", flush=True)


class SyntheticHarness(Harness):
    """In-memory public-boundary fixture. Never starts processes or opens sockets."""
    def __init__(self, fault=None, restart=True, dashboard=True):
        self.args = argparse.Namespace(gateway_url="http://synthetic.invalid:21000",
            restart_test=restart, phase_timeout=1, command_timeout=1)
        self.compose = ["SYNTHETIC-COMPOSE"]
        self.state = {kind: [] for kind in KINDS}
        self.members = {}
        self.events = []
        self.serial = 0
        self.fault = fault
        self.dashboard = dashboard
        self.heartbeat = 0
        self.creations = 0

    def row(self, name, spec):
        self.serial += 1
        identifier = str(self.serial)
        if self.fault == "reused-id" and self.creations > 1:
            identifier = str(self.serial - 4)
        return {"name": name, "id": identifier, "revision": 1, "spec": spec}

    def wait(self, label, observe):
        self.events.append(("wait", label))
        return observe(None)

    def readiness(self, deadline):
        self.events.append(("ready",))

    def dataplane(self, deadline, newer_than=None):
        self.heartbeat += 1
        value = "2026-10-07T07:00:" + str(self.heartbeat).zfill(2) + "Z"
        self.events.append(("heartbeat", newer_than))
        if newer_than is not None:
            if self.fault == "stale-heartbeat":
                raise Pending("synthetic stale heartbeat")
            require(heartbeat_newer(value, newer_than), "synthetic heartbeat failed")
        return {"last_heartbeat_at": value}

    def run_command(self, argv, timeout, label, input_bytes=None):
        action = tuple(argv[1:])
        self.events.append(action)
        if action == ("ps", "-q", "flowplane-dashboard"):
            return b"synthetic-dashboard" if self.dashboard else b""
        require(action[0] == "restart", "unexpected synthetic command")
        if action[1] == "flowplane-eval":
            config = self.state["route"][0]
            if self.fault == "restart-spec":
                config["spec"]["virtual_hosts"][0]["routes"][0]["action"]["timeout_secs"] = 24
            if self.fault == "restart-revision":
                config["revision"] += 1
            if self.fault == "restart-id":
                config["id"] = "changed"
            if self.fault == "restart-name":
                config["name"] = "changed"
        return b""

    def cli(self, command, deadline=None, file_payload=None):
        self.events.append(("cli", tuple(command)))
        def wrap(data, kind="synthetic"):
            return json.dumps({"schemaVersion": 1, "kind": kind, "data": data})
        if command[0] in KINDS:
            kind, action = command[:2]
            if action == "list":
                rows = copy.deepcopy(self.state[kind])
                if self.fault == "seeded" and kind == "api":
                    rows.append({"name": "seeded-api"})
                return wrap(rows, kind + "List")
            return wrap(next(row for row in self.state[kind] if row["name"] == command[2]))
        if command[0] == "--revision":
            require(file_payload is not None and command[2:4] == ["route", "update"],
                    "synthetic update must use public file boundary")
            config = self.state["route"][0]
            require(command[1] == str(config["revision"]) and command[4] == config["name"],
                    "synthetic explicit revision/actual config selector missing")
            if self.fault == "update-error":
                raise Pending("synthetic one-shot update failure")
            assert file_payload is not None
            config["spec"] = copy.deepcopy(file_payload["spec"])
            if self.fault != "update-stale-revision":
                config["revision"] += 1
            return wrap(config)
        if command[0] == "expose":
            name = command[command.index("--name") + 1]
            attached = "--listener" in command
            if self.fault == "expose-error":
                raise Pending("synthetic one-shot expose failure")
            if not attached:
                self.creations += 1
            cluster = self.row(name + "-actual-upstream", {"fixture": "upstream"})
            route = {"name": name, "match": {"prefix": {"prefix": "/shared" if attached else "/"}},
                     "action": {"cluster": cluster["name"], "timeout_secs": 15}}
            if attached:
                config, listener = self.state["route"][0], self.state["listener"][0]
                require(command[command.index("--listener") + 1] == listener["name"],
                        "attachment guessed listener name")
                require("--port" not in command and "--public-base-url" not in command,
                        "attachment passed conflicting flags")
                config["spec"]["virtual_hosts"][0]["routes"].insert(0, route)
                config["revision"] += 1
                if self.fault == "attach-listener":
                    listener["revision"] += 1
                if self.fault == "attach-policy":
                    config["spec"]["virtual_hosts"][0]["routes"][1]["action"]["timeout_secs"] = 24
                if self.fault == "attach-order":
                    config["spec"]["virtual_hosts"][0]["routes"].reverse()
            else:
                config = self.row("actual-config", {"virtual_hosts": [
                    {"name": "default", "domains": ["*"], "routes": [route], "filter_overrides": []}]})
                listener = self.row("actual-listener", {"public_base_url": self.args.gateway_url,
                    "route_config": config["name"], "port": 10000, "http_filters": []})
                self.state["route"] = [config]
                self.state["listener"] = [listener]
            self.state["cluster"].append(cluster)
            data = {"name": name, "path": "/shared" if attached else "/",
                    "mode": "attached" if attached else "created", "cluster": cluster,
                    "route_config": config, "listener": listener,
                    "curl_url": self.args.gateway_url + ("/shared" if attached else "/")}
            self.members[name] = copy.deepcopy(data)
            if self.fault == "attach-mode" and attached:
                data["mode"] = "created"
            if self.fault == "attach-url" and attached:
                data["curl_url"] += "wrong"
            return wrap(data)
        require(command[:2] == ["--yes", "unexpose"], "unexpected synthetic CLI")
        name = command[2]
        if self.fault == "remove-error":
            raise Pending("synthetic one-shot removal failure")
        exposure = self.members.pop(name)
        final = not self.members
        config = self.state["route"][0]
        self.state["cluster"] = [r for r in self.state["cluster"]
                                 if r["id"] != exposure["cluster"]["id"]]
        if final:
            if self.fault == "lost-provenance" and name == "shared":
                raise Failure("synthetic missing inherited provenance")
            self.state["route"] = []
            self.state["listener"] = []
            if self.fault == "leftover":
                self.state["cluster"] = [exposure["cluster"]]
        else:
            routes = config["spec"]["virtual_hosts"][0]["routes"]
            routes[:] = [r for r in routes if r["name"] != name]
            config["revision"] += 1
            if self.fault == "survivor-policy":
                routes[0]["action"]["timeout_secs"] = 24
        disposition = "deleted" if final else "retained"
        data = {"name": name, "cluster_name": exposure["cluster"]["name"],
                "route_config_name": exposure["route_config"]["name"],
                "listener_name": exposure["listener"]["name"],
                "cluster_disposition": "deleted", "route_config_disposition": disposition,
                "listener_disposition": disposition}
        if self.fault == "disposition":
            data["listener_disposition"] = "deleted" if not final else "retained"
        if self.fault == "remove-name":
            data["cluster_name"] = "wrong"
        return wrap(data)

    def gateway(self, deadline=None, url=None):
        url = url or self.args.gateway_url
        self.events.append(("traffic", urllib.parse.urlsplit(url).path or "/", len(self.members)))
        if not self.members:
            return (200, EXPECTED_BODY) if self.fault == "cleanup-traffic" and self.creations else (None, b"")
        if self.fault == "survivor-traffic" and len(self.members) == 1 and "shared" in self.members:
            return 503, b""
        if self.fault == "body":
            return 200, EXPECTED_BODY + b"\n"
        return 200, EXPECTED_BODY


class SyntheticTests(unittest.TestCase):
    """Synthetic parsers and lifecycle state/sequence faults; not runtime evidence."""
    def execute_synthetic(self, harness):
        with contextlib.redirect_stdout(io.StringIO()), \
                mock.patch.object(subprocess, "run", side_effect=AssertionError("real process forbidden")), \
                mock.patch.object(socket, "socket", side_effect=AssertionError("real socket forbidden")):
            harness.execute()

    def test_full_shared_lifecycle_sequence(self):
        harness = SyntheticHarness()
        self.execute_synthetic(harness)
        self.assertTrue(all(not rows for rows in harness.state.values()))
        self.assertFalse(harness.members)
        mutations = [event[1] for event in harness.events if event[0] == "cli"
                     and (event[1][0] in ("expose", "--revision", "--yes"))]
        self.assertEqual([cmd[2] for cmd in mutations if cmd[0] == "--yes"],
                         ["demo", "shared", "shared", "demo"])
        self.assertEqual(sum(cmd[0] == "expose" for cmd in mutations), 4)
        self.assertEqual(sum(cmd[0] == "--revision" for cmd in mutations), 4)
        self.assertEqual([event for event in harness.events if event[0] == "restart"],
                         [("restart", unit) for _ in range(2) for unit in (
                             "flowplane-eval", "envoy", "flowplane-agent", "flowplane-dashboard")])
        for index, event in enumerate(harness.events):
            if event == ("restart", "flowplane-eval"):
                self.assertEqual(harness.events[index + 2], ("ready",))
            if event == ("restart", "flowplane-agent"):
                self.assertEqual(harness.events[index + 1][0], "heartbeat")
                self.assertIsNone(harness.events[index + 1][1])
                self.assertEqual(harness.events[index + 3][0], "heartbeat")
                self.assertIsNotNone(harness.events[index + 3][1])
        for path in ("/", "/shared"):
            self.assertIn(("traffic", path, 2), harness.events)
            self.assertIn(("traffic", path, 1), harness.events)
            self.assertIn(("traffic", path, 0), harness.events)

    def test_no_restart_and_optional_dashboard(self):
        for restart, dashboard in ((False, False), (True, False)):
            with self.subTest(restart=restart):
                harness = SyntheticHarness(restart=restart, dashboard=dashboard)
                self.execute_synthetic(harness)
                restarts = [e for e in harness.events if e[0] == "restart"]
                self.assertEqual(len(restarts), 6 if restart else 0)
                self.assertNotIn(("restart", "flowplane-dashboard"), restarts)

    def test_seeded_checkpoint_stops_before_mutations(self):
        harness = SyntheticHarness("seeded")
        with self.assertRaisesRegex(Failure, "seeded api"):
            self.execute_synthetic(harness)
        self.assertFalse(any(e[0] == "restart" or (e[0] == "cli" and e[1][0] == "expose")
                             for e in harness.events))

    def test_mutation_failures_never_retry_or_cleanup(self):
        for fault, selector in (("expose-error", "expose"), ("update-error", "--revision"),
                                ("remove-error", "--yes")):
            with self.subTest(fault=fault):
                harness = SyntheticHarness(fault)
                with self.assertRaises(Pending):
                    self.execute_synthetic(harness)
                attempts = [e for e in harness.events if e[0] == "cli" and e[1][0] == selector]
                self.assertEqual(len(attempts), 1)
                self.assertEqual(harness.events[-1], attempts[0])
                if selector != "expose":
                    self.assertTrue(harness.state["listener"])

    def test_shared_state_assertion_faults(self):
        faults = {
            "attach-mode": "identity/mode/path", "attach-url": "curl URL",
            "attach-listener": "changed listener", "attach-policy": "unrelated routes",
            "attach-order": "precede root", "update-stale-revision": "exact spec/revision",
            "restart-spec": "restart changed", "restart-revision": "restart changed",
            "restart-id": "restart changed", "restart-name": "restart changed",
            "stale-heartbeat": "stale heartbeat", "disposition": "names/dispositions",
            "remove-name": "names/dispositions", "survivor-policy": "surviving policy",
            "survivor-traffic": "exact demo body", "lost-provenance": "inherited provenance",
            "leftover": "empty scoped", "cleanup-traffic": "still succeeds",
            "reused-id": "reused resource identities", "body": "exact demo body",
        }
        for fault, message in faults.items():
            with self.subTest(fault=fault):
                harness = SyntheticHarness(fault)
                with self.assertRaisesRegex(Failure, message):
                    self.execute_synthetic(harness)
                # No compensation after a failure; the failed lifecycle is not
                # followed by a fresh create that could hide residual state.
                expected_removals = (2 if fault in ("lost-provenance", "leftover", "cleanup-traffic",
                                                    "reused-id") else
                                     1 if fault in ("disposition", "remove-name", "survivor-policy",
                                                    "survivor-traffic") else 0)
                removals = [e for e in harness.events if e[0] == "cli" and e[1][0] == "--yes"]
                self.assertEqual(len(removals), expected_removals)
                self.assertEqual(harness.creations, 2 if fault == "reused-id" else 1)

    def test_every_removal_name_and_disposition_in_both_branches(self):
        fields = ("name", "cluster_name", "route_config_name", "listener_name",
                  "cluster_disposition", "route_config_disposition", "listener_disposition")
        for final in (False, True):
            for field in fields:
                with self.subTest(final=final, field=field), contextlib.redirect_stdout(io.StringIO()):
                    harness = SyntheticHarness()
                    original = harness.expose()
                    if not final:
                        harness.expose(original)
                    cli = harness.cli
                    def wrong(command, deadline=None, file_payload=None):
                        response = json.loads(cli(command, deadline, file_payload))
                        response["data"][field] = "incorrect"
                        return json.dumps(response)
                    with mock.patch.object(harness, "cli", side_effect=wrong):
                        with self.assertRaisesRegex(Failure, "names/dispositions"):
                            harness.remove(original, final=final)

    def test_cli_container_file_boundary_is_one_shot(self):
        args = argparse.Namespace(compose_command="synthetic compose", project="owned",
            compose_file="/synthetic.yml", command_timeout=2)
        harness = Harness(args)
        payload = {"spec": {"virtual_hosts": []}}
        with mock.patch.object(harness, "run_command", return_value=b"{}") as run:
            harness.cli(["--revision", "7", "route", "update", "actual-config"],
                        file_payload=payload)
        self.assertEqual(run.call_count, 1)
        argv, _, _, raw = run.call_args.args
        self.assertEqual(json.loads(raw), payload)
        self.assertIn('cat > "$file"', argv[argv.index("-c") + 1])
        self.assertIn('--file "$file"', argv[argv.index("-c") + 1])
        self.assertEqual(argv[-5:], ["--revision", "7", "route", "update", "actual-config"])

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
        print("No failure cleanup attempted; remaining resources left intact for parent cleanup."
              " Raw output withheld.", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("Interrupted; resources left intact for parent cleanup.", file=sys.stderr)
        return 130
    print("PASS empty-eval/shared-exposure integration; both removal orders, fresh-ID repeat, empty final state"
          + (" including restart" if args.restart_test else " (restart not exercised)"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
