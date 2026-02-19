from __future__ import annotations

import os
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path

import httpx


@dataclass
class BootstrapResult:
    token: str
    dataplane_id: str
    team: str
    org: str
    base_url: str


class CPBootstrapper:
    """Bootstrap a fresh CP: admin, org, team, dataplane, token.

    Mirrors the flow in scripts/seed-data.sh using httpx.
    Idempotent — skips steps that are already done (409/duplicate).
    """

    ADMIN_EMAIL = "agent-test-admin@flowplane.local"
    ADMIN_PASSWORD = "AgentTest123!"
    ADMIN_NAME = "Agent Test Admin"

    ORG_NAME = "agent-test-org"
    ORG_DISPLAY = "Agent Test Org"

    ORG_ADMIN_EMAIL = "agent-test-orgadmin@flowplane.local"
    ORG_ADMIN_PASSWORD = "AgentTestOrg123!"
    ORG_ADMIN_NAME = "Agent Test Org Admin"

    TEAM_NAME = "agent-test-team"
    DATAPLANE_NAME = "agent-test-dataplane"

    def __init__(self, base_url: str):
        self.base_url = base_url.rstrip("/")
        self._admin_client = httpx.Client(timeout=30.0)
        self._org_admin_client = httpx.Client(timeout=30.0)
        self._admin_csrf = ""
        self._org_admin_csrf = ""
        self._org_id = ""
        self._user_id = ""

    def wait_for_cp(self, timeout_s: float = 60.0) -> None:
        """Poll bootstrap/status until the CP is reachable."""
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            try:
                resp = httpx.get(f"{self.base_url}/api/v1/bootstrap/status", timeout=5.0)
                if resp.status_code == 200:
                    return
            except httpx.HTTPError:
                pass
            time.sleep(1.0)
        raise TimeoutError(f"CP not reachable at {self.base_url} after {timeout_s}s")

    def bootstrap(self) -> BootstrapResult:
        """Run the full bootstrap sequence. Idempotent."""
        self._bootstrap_admin()
        self._login_admin()
        self._create_org()
        self._create_org_admin_user()
        self._assign_org_admin_role()
        self._login_org_admin()
        self._create_team()
        dp_id = self._create_dataplane()
        token = self._generate_token()

        self._admin_client.close()
        self._org_admin_client.close()

        return BootstrapResult(
            token=token,
            dataplane_id=dp_id,
            team=self.TEAM_NAME,
            org=self.ORG_NAME,
            base_url=self.base_url,
        )

    def generate_envoy_bootstrap(self, dataplane_id: str) -> Path:
        """Write envoy-bootstrap.yaml from the template with real IDs.

        Returns the path to the generated config file.
        """
        testing_dir = Path(__file__).parent
        template = testing_dir / "envoy-bootstrap.yml.tpl"
        output = testing_dir / "envoy-bootstrap.yaml"

        content = template.read_text()
        content = content.replace("__DATAPLANE_ID__", dataplane_id)
        content = content.replace("__DATAPLANE_NAME__", self.DATAPLANE_NAME)
        content = content.replace("__XDS_ADDRESS__", "control-plane-test")
        output.write_text(content)
        return output

    @staticmethod
    def start_envoy(timeout_s: float = 60.0) -> None:
        """Start the Envoy container and wait for it to be healthy."""
        compose_file = Path(__file__).parent / "docker-compose.test.yml"
        subprocess.run(
            ["docker-compose", "-f", str(compose_file), "--profile", "envoy",
             "up", "-d", "envoy-test"],
            check=True, capture_output=True,
        )
        # Wait for Envoy admin to respond
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            try:
                resp = httpx.get("http://localhost:9902/ready", timeout=3.0)
                if resp.status_code == 200:
                    return
            except httpx.HTTPError:
                pass
            time.sleep(2.0)
        raise TimeoutError(f"Envoy not ready after {timeout_s}s")

    def _bootstrap_admin(self) -> None:
        resp = httpx.get(f"{self.base_url}/api/v1/bootstrap/status", timeout=10.0)
        resp.raise_for_status()
        if resp.json().get("needsInitialization"):
            resp = httpx.post(
                f"{self.base_url}/api/v1/bootstrap/initialize",
                json={"email": self.ADMIN_EMAIL, "password": self.ADMIN_PASSWORD, "name": self.ADMIN_NAME},
                timeout=10.0,
            )
            if resp.status_code not in (200, 201):
                raise RuntimeError(f"Bootstrap failed: {resp.status_code} {resp.text}")

    def _login_admin(self) -> None:
        resp = self._admin_client.post(
            f"{self.base_url}/api/v1/auth/login",
            json={"email": self.ADMIN_EMAIL, "password": self.ADMIN_PASSWORD},
        )
        resp.raise_for_status()
        self._admin_csrf = resp.json().get("csrfToken", "")

    def _admin_headers(self) -> dict:
        return {"X-CSRF-Token": self._admin_csrf}

    def _org_admin_headers(self) -> dict:
        return {"X-CSRF-Token": self._org_admin_csrf}

    def _create_org(self) -> None:
        resp = self._admin_client.post(
            f"{self.base_url}/api/v1/admin/organizations",
            json={"name": self.ORG_NAME, "displayName": self.ORG_DISPLAY, "description": "Agent test org"},
            headers=self._admin_headers(),
        )
        if resp.status_code in (200, 201):
            self._org_id = resp.json()["id"]
        elif resp.status_code == 409 or "already exists" in resp.text or "duplicate key" in resp.text:
            # Already exists — look it up
            self._org_id = self._find_org_id()
        else:
            raise RuntimeError(f"Create org failed: {resp.status_code} {resp.text}")

    def _find_org_id(self) -> str:
        resp = self._admin_client.get(
            f"{self.base_url}/api/v1/admin/organizations",
            headers=self._admin_headers(),
        )
        resp.raise_for_status()
        data = resp.json()
        items = data if isinstance(data, list) else data.get("items", [])
        for org in items:
            if org["name"] == self.ORG_NAME:
                return org["id"]
        raise RuntimeError(f"Org '{self.ORG_NAME}' not found after create conflict")

    def _create_org_admin_user(self) -> None:
        resp = self._admin_client.post(
            f"{self.base_url}/api/v1/users",
            json={
                "email": self.ORG_ADMIN_EMAIL,
                "password": self.ORG_ADMIN_PASSWORD,
                "name": self.ORG_ADMIN_NAME,
                "isAdmin": False,
                "orgId": self._org_id,
            },
            headers=self._admin_headers(),
        )
        if resp.status_code in (200, 201):
            self._user_id = resp.json()["id"]
        elif resp.status_code == 409 or "already exists" in resp.text or "duplicate key" in resp.text:
            self._user_id = self._find_user_id()
        else:
            raise RuntimeError(f"Create user failed: {resp.status_code} {resp.text}")

    def _find_user_id(self) -> str:
        resp = self._admin_client.get(
            f"{self.base_url}/api/v1/users",
            headers=self._admin_headers(),
        )
        resp.raise_for_status()
        data = resp.json()
        items = data if isinstance(data, list) else data.get("items", [])
        for user in items:
            if user["email"] == self.ORG_ADMIN_EMAIL:
                return user["id"]
        raise RuntimeError(f"User '{self.ORG_ADMIN_EMAIL}' not found after create conflict")

    def _assign_org_admin_role(self) -> None:
        resp = self._admin_client.post(
            f"{self.base_url}/api/v1/admin/organizations/{self._org_id}/members",
            json={"userId": self._user_id, "role": "admin"},
            headers=self._admin_headers(),
        )
        # Accept success or already-exists
        if resp.status_code not in (200, 201, 409) and "duplicate" not in resp.text.lower():
            raise RuntimeError(f"Assign org-admin failed: {resp.status_code} {resp.text}")

    def _login_org_admin(self) -> None:
        resp = self._org_admin_client.post(
            f"{self.base_url}/api/v1/auth/login",
            json={"email": self.ORG_ADMIN_EMAIL, "password": self.ORG_ADMIN_PASSWORD},
        )
        resp.raise_for_status()
        self._org_admin_csrf = resp.json().get("csrfToken", "")

    def _create_team(self) -> None:
        resp = self._org_admin_client.post(
            f"{self.base_url}/api/v1/orgs/{self.ORG_NAME}/teams",
            json={"name": self.TEAM_NAME, "displayName": "Agent Test Team", "description": "Agent test team"},
            headers=self._org_admin_headers(),
        )
        if resp.status_code not in (200, 201, 409) and "already exists" not in resp.text and "duplicate key" not in resp.text:
            raise RuntimeError(f"Create team failed: {resp.status_code} {resp.text}")

    def _create_dataplane(self) -> str:
        resp = self._org_admin_client.post(
            f"{self.base_url}/api/v1/teams/{self.TEAM_NAME}/dataplanes",
            json={
                "team": self.TEAM_NAME,
                "name": self.DATAPLANE_NAME,
                "gatewayHost": "127.0.0.1",
                "description": "Agent test dataplane",
            },
            headers=self._org_admin_headers(),
        )
        if resp.status_code in (200, 201):
            return resp.json()["id"]
        elif resp.status_code == 409 or "already exists" in resp.text or "duplicate key" in resp.text:
            return self._find_dataplane_id()
        else:
            raise RuntimeError(f"Create dataplane failed: {resp.status_code} {resp.text}")

    def _find_dataplane_id(self) -> str:
        resp = self._org_admin_client.get(
            f"{self.base_url}/api/v1/teams/{self.TEAM_NAME}/dataplanes",
            headers=self._org_admin_headers(),
        )
        resp.raise_for_status()
        data = resp.json()
        items = data if isinstance(data, list) else data.get("items", [])
        for dp in items:
            if dp["name"] == self.DATAPLANE_NAME:
                return dp["id"]
        raise RuntimeError(f"Dataplane '{self.DATAPLANE_NAME}' not found after create conflict")

    def _generate_token(self) -> str:
        resp = self._org_admin_client.post(
            f"{self.base_url}/api/v1/tokens",
            json={
                "name": f"agent-test-token-{int(time.time())}",
                "description": "Agent integration test token",
                "scopes": [f"org:{self.ORG_NAME}:admin"],
            },
            headers=self._org_admin_headers(),
        )
        if resp.status_code in (200, 201):
            return resp.json()["token"]
        raise RuntimeError(f"Generate token failed: {resp.status_code} {resp.text}")


@dataclass
class CPSnapshot:
    clusters: list[dict]
    listeners: list[dict]
    route_configs: list[dict]
    virtual_hosts: list[dict]
    routes: list[dict]
    filters: list[dict]


class CPStateHelper:
    """MCP-based state assertions and cleanup."""

    def __init__(self, mcp):
        # mcp is a FlowplaneMCPClient instance
        self.mcp = mcp

    def snapshot(self) -> CPSnapshot:
        """Take a snapshot of all CP resources."""
        return CPSnapshot(
            clusters=self._list("cp_list_clusters", "clusters"),
            listeners=self._list("cp_list_listeners", "listeners"),
            route_configs=self._list("cp_list_route_configs", "route_configs"),
            virtual_hosts=self._list("cp_list_virtual_hosts", "virtual_hosts"),
            routes=self._list("cp_list_routes", "routes"),
            filters=self._list("cp_list_filters", "filters"),
        )

    def _list(self, tool: str, key: str) -> list[dict]:
        """Call a cp_list_* tool and extract the items list."""
        try:
            result = self.mcp.call_tool(tool, {})
            items = result.get(key) or result.get("items") or []
            if isinstance(result, list):
                items = result
            return items
        except Exception:
            return []

    # -- Positive assertions ------------------------------------------------

    def assert_cluster_exists(self, name: str) -> dict:
        items = self._list("cp_list_clusters", "clusters")
        for item in items:
            if item.get("name") == name:
                return item
        raise AssertionError(f"Cluster '{name}' not found. Existing: {[i.get('name') for i in items]}")

    def assert_listener_exists(self, name: str, port: int | None = None) -> dict:
        items = self._list("cp_list_listeners", "listeners")
        for item in items:
            if item.get("name") == name:
                if port is not None:
                    actual_port = item.get("port")
                    assert actual_port == port, f"Listener '{name}' port {actual_port} != expected {port}. Item: {item}"
                return item
        raise AssertionError(f"Listener '{name}' not found. Existing: {[i.get('name') for i in items]}")

    def assert_route_config_exists(self, name: str) -> dict:
        items = self._list("cp_list_route_configs", "route_configs")
        for item in items:
            if item.get("name") == name:
                return item
        raise AssertionError(f"RouteConfig '{name}' not found. Existing: {[i.get('name') for i in items]}")

    def assert_virtual_host_exists(self, name: str) -> dict:
        items = self._list("cp_list_virtual_hosts", "virtual_hosts")
        for item in items:
            if item.get("name") == name:
                return item
        raise AssertionError(f"VirtualHost '{name}' not found. Existing: {[i.get('name') for i in items]}")

    def assert_route_exists(self, vhost_name: str, path: str) -> dict:
        items = self._list("cp_list_routes", "routes")
        for item in items:
            item_vhost = item.get("virtual_host", "")
            item_path = item.get("path_pattern", "")
            if item_vhost == vhost_name and item_path == path:
                return item
        raise AssertionError(
            f"Route with vhost='{vhost_name}' path='{path}' not found. "
            f"Existing routes: {[{'virtual_host': i.get('virtual_host'), 'path_pattern': i.get('path_pattern'), 'name': i.get('name')} for i in items]}"
        )

    def assert_filter_exists(self, name: str, filter_type: str | None = None) -> dict:
        items = self._list("cp_list_filters", "filters")
        for item in items:
            if item.get("name") == name:
                if filter_type is not None:
                    actual = item.get("filter_type")
                    assert actual == filter_type, f"Filter '{name}' filter_type '{actual}' != expected '{filter_type}'. Item: {item}"
                return item
        raise AssertionError(f"Filter '{name}' not found. Existing: {[i.get('name') for i in items]}")

    def assert_listener_port(self, port: int) -> dict:
        items = self._list("cp_list_listeners", "listeners")
        for item in items:
            if item.get("port") == port:
                return item
        raise AssertionError(f"No listener on port {port}. Existing: {[(i.get('name'), i.get('port')) for i in items]}")

    # -- Negative assertions ------------------------------------------------

    def assert_not_exists(self, resource_type: str, name: str) -> None:
        tool_map = {
            "cluster": ("cp_list_clusters", "clusters"),
            "listener": ("cp_list_listeners", "listeners"),
            "route_config": ("cp_list_route_configs", "route_configs"),
            "virtual_host": ("cp_list_virtual_hosts", "virtual_hosts"),
            "route": ("cp_list_routes", "routes"),
            "filter": ("cp_list_filters", "filters"),
        }
        tool, key = tool_map[resource_type]
        items = self._list(tool, key)
        names = [i.get("name", "") for i in items]
        assert name not in names, f"{resource_type} '{name}' exists but should not"

    # -- Ops assertions -----------------------------------------------------

    def assert_trace_reaches(self, path: str, port: int, cluster_name: str) -> dict:
        result = self.mcp.call_tool("ops_trace_request", {"path": path, "port": port})
        matches = result.get("matches", [])
        matched_cluster = matches[0].get("cluster_name", "") if matches else ""
        assert matched_cluster == cluster_name, (
            f"Trace for {path}:{port} reached cluster '{matched_cluster}', expected '{cluster_name}'. "
            f"matches={matches}"
        )
        return result

    def assert_trace_no_match(self, path: str, port: int) -> dict:
        result = self.mcp.call_tool("ops_trace_request", {"path": path, "port": port})
        match_count = result.get("match_count", 0)
        assert match_count == 0, (
            f"Expected no match for {path}:{port} but got match_count={match_count}. Result: {result}"
        )
        return result

    # -- Cleanup ------------------------------------------------------------

    def delete_resources_with_prefix(self, prefix: str) -> int:
        """Delete all resources whose name starts with the given prefix.

        Deletes in reverse dependency order:
        routes -> virtual_hosts -> route_configs -> filters -> listeners -> clusters

        Each delete tool uses `name` (not id). Compound resources (routes,
        virtual_hosts) require parent context (route_config, virtual_host).
        """
        count = 0

        # 1. Delete routes (require route_config + virtual_host + name)
        routes = self._list("cp_list_routes", "routes")
        for item in routes:
            name = item.get("name", "")
            if name.startswith(prefix):
                rc = item.get("route_config", "")
                vh = item.get("virtual_host", "")
                if rc and vh:
                    try:
                        self.mcp.call_tool("cp_delete_route", {
                            "route_config": rc, "virtual_host": vh, "name": name,
                        })
                        count += 1
                    except Exception as e:
                        print(f"[cleanup] Failed to delete route '{name}': {e}")

        # 2. Delete virtual hosts (require route_config + name)
        #    VH list returns route_config_id (UUID), not route_config name.
        #    Easier to just delete the route_config which cascades to VHs.
        #    Skip explicit VH deletion — route_config delete cascades.

        # 3. Delete route configs (cascade deletes VHs and routes)
        route_configs = self._list("cp_list_route_configs", "route_configs")
        for item in route_configs:
            name = item.get("name", "")
            if name.startswith(prefix):
                try:
                    self.mcp.call_tool("cp_delete_route_config", {"name": name})
                    count += 1
                except Exception as e:
                    print(f"[cleanup] Failed to delete route_config '{name}': {e}")

        # 4. Delete filters
        filters = self._list("cp_list_filters", "filters")
        for item in filters:
            name = item.get("name", "")
            if name.startswith(prefix):
                try:
                    self.mcp.call_tool("cp_delete_filter", {"name": name})
                    count += 1
                except Exception as e:
                    print(f"[cleanup] Failed to delete filter '{name}': {e}")

        # 5. Delete listeners
        listeners = self._list("cp_list_listeners", "listeners")
        for item in listeners:
            name = item.get("name", "")
            if name.startswith(prefix):
                try:
                    self.mcp.call_tool("cp_delete_listener", {"name": name})
                    count += 1
                except Exception as e:
                    print(f"[cleanup] Failed to delete listener '{name}': {e}")

        # 6. Delete clusters
        clusters = self._list("cp_list_clusters", "clusters")
        for item in clusters:
            name = item.get("name", "")
            if name.startswith(prefix):
                try:
                    self.mcp.call_tool("cp_delete_cluster", {"name": name})
                    count += 1
                except Exception as e:
                    print(f"[cleanup] Failed to delete cluster '{name}': {e}")

        return count
