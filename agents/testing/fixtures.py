from __future__ import annotations

import random
import uuid
from dataclasses import dataclass

# Port tracking for record/replay determinism
_replay_port_queue: list[int] = []
_recorded_ports: list[int] = []


def unique_prefix() -> str:
    """Generate a short unique prefix for test resource names."""
    return "t" + uuid.uuid4().hex[:7]


def unique_port() -> int:
    """Generate a unique port for test listeners.

    Uses 15000-15099 to match the port range mapped in the test Envoy
    container (docker-compose.test.yml).  In replay mode, ports are
    returned from the recorded queue to ensure deterministic creation.
    """
    if _replay_port_queue:
        return _replay_port_queue.pop(0)
    port = random.randint(15000, 15099)
    _recorded_ports.append(port)
    return port


def set_replay_ports(ports: list[int]) -> None:
    """Load recorded ports into the replay queue."""
    _replay_port_queue.clear()
    _replay_port_queue.extend(ports)


def get_recorded_ports() -> list[int]:
    """Return the list of ports recorded during this test."""
    return list(_recorded_ports)


def reset_port_tracking() -> None:
    """Clear both port tracking lists between tests."""
    _replay_port_queue.clear()
    _recorded_ports.clear()


@dataclass
class APIScenario:
    prefix: str
    cluster_name: str
    route_config_name: str
    listener_name: str
    virtual_host_name: str
    port: int
    backend_host: str
    backend_port: int
    path: str


def make_scenario(
    prefix: str,
    port: int = 10001,
    path: str = "/",
    backend_host: str = "httpbin-test",
    backend_port: int = 80,
) -> APIScenario:
    """Create an APIScenario with deterministic names derived from the prefix."""
    return APIScenario(
        prefix=prefix,
        cluster_name=f"{prefix}-cluster",
        route_config_name=f"{prefix}-rc",
        listener_name=f"{prefix}-listener",
        virtual_host_name=f"{prefix}-vhost",
        port=port,
        backend_host=backend_host,
        backend_port=backend_port,
        path=path,
    )


def scenario_to_prompt(scenario: APIScenario) -> str:
    """Build a deterministic deployment prompt with explicit resource names.

    Uses exact names so the agent doesn't improvise, making assertions reliable.
    """
    return (
        f"Deploy an API with the following exact configuration:\n"
        f"- Backend: {scenario.backend_host}:{scenario.backend_port}\n"
        f"- Cluster name: {scenario.cluster_name}\n"
        f"- Route config name: {scenario.route_config_name}\n"
        f"- Listener name: {scenario.listener_name} on port {scenario.port}\n"
        f"- Virtual host name: {scenario.virtual_host_name} with domains [\"*\"]\n"
        f"- Route path prefix: {scenario.path}\n"
        f"\n"
        f"Use EXACTLY these names. Do not modify or rename them."
    )


def multi_path_prompt(scenario: APIScenario, paths: list[str]) -> str:
    """Build a prompt for deploying multiple paths under the same listener."""
    path_list = "\n".join(f"  - {p}" for p in paths)
    return (
        f"Deploy an API with the following exact configuration:\n"
        f"- Backend: {scenario.backend_host}:{scenario.backend_port}\n"
        f"- Cluster name: {scenario.cluster_name}\n"
        f"- Route config name: {scenario.route_config_name}\n"
        f"- Listener name: {scenario.listener_name} on port {scenario.port}\n"
        f"- Virtual host name: {scenario.virtual_host_name} with domains [\"*\"]\n"
        f"- Route path prefixes:\n{path_list}\n"
        f"\n"
        f"Use EXACTLY these names. Create one route per path, all under the same virtual host."
    )
