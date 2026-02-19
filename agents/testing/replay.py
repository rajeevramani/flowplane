"""LLM replay mechanism for agent integration tests.

Records and replays OpenAI ChatCompletion responses so agent tests can run
without real LLM API calls.  MCP tool calls still hit the live control plane,
preserving integration test value.

Usage:
    # Record cassettes (requires LLM_API_KEY)
    AGENT_TEST_MODE=record pytest tests/ -v

    # Replay from cassettes (no LLM key needed)
    AGENT_TEST_MODE=replay pytest tests/ -v
"""

from __future__ import annotations

import copy
import json
import logging
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from openai.types.chat import ChatCompletion

logger = logging.getLogger(__name__)

# Tools whose results contain server-generated IDs that need rewriting
_ID_SOURCE_TOOLS = frozenset({"cp_list_dataplanes", "cp_list_aggregated_schemas"})


# ---------------------------------------------------------------------------
# Serialization helpers
# ---------------------------------------------------------------------------

def serialize_response(response: ChatCompletion) -> dict:
    """Convert a ChatCompletion to a JSON-safe dict."""
    return response.model_dump(mode="json")


def deserialize_response(data: dict) -> ChatCompletion:
    """Reconstruct a ChatCompletion from a serialized dict."""
    return ChatCompletion.model_validate(data)


# ---------------------------------------------------------------------------
# ID Rewrite Map
# ---------------------------------------------------------------------------

class IdRewriteMap:
    """Tracks recorded -> live ID mappings for replay mode.

    Only two tool chains produce server-generated IDs:
    1. cp_list_dataplanes  -> dataplaneId in cp_create_listener / cp_update_listener
    2. cp_list_aggregated_schemas -> schema_ids in cp_export_schema_openapi

    Resources are matched by their stable *name* field, not position.
    """

    def __init__(self) -> None:
        self._map: dict[str, str] = {}  # recorded_id -> live_id

    def learn_from_results(
        self, tool_name: str, recorded_result: dict, live_result: dict
    ) -> None:
        """Compare recorded vs live results and learn ID mappings."""
        if tool_name == "cp_list_dataplanes":
            rec_by_name = {
                dp["name"]: dp["id"]
                for dp in recorded_result.get("dataplanes", [])
            }
            for dp in live_result.get("dataplanes", []):
                rec_id = rec_by_name.get(dp["name"])
                if rec_id and rec_id != dp["id"]:
                    self._map[rec_id] = dp["id"]

        elif tool_name == "cp_list_aggregated_schemas":
            rec_by_key = {
                (s["http_method"], s["path"]): s["id"]
                for s in recorded_result.get("schemas", [])
            }
            for s in live_result.get("schemas", []):
                rec_id = rec_by_key.get((s["http_method"], s["path"]))
                if rec_id is not None and rec_id != s["id"]:
                    self._map[str(rec_id)] = str(s["id"])

    def rewrite_args(self, tool_name: str, args: dict) -> dict:
        """Rewrite recorded IDs in tool arguments to live IDs."""
        if not self._map:
            return args

        if tool_name in ("cp_create_listener", "cp_update_listener"):
            dp_id = args.get("dataplaneId")
            if dp_id and dp_id in self._map:
                args = {**args, "dataplaneId": self._map[dp_id]}

        elif tool_name == "cp_export_schema_openapi":
            if "schema_ids" in args:
                args = {
                    **args,
                    "schema_ids": [
                        int(self._map.get(str(sid), str(sid)))
                        for sid in args["schema_ids"]
                    ],
                }

        return args


# ---------------------------------------------------------------------------
# Cassette Client
# ---------------------------------------------------------------------------

class CassetteOpenAIClient:
    """Drop-in replacement for ``openai.OpenAI`` that records or replays
    ``chat.completions.create()`` calls.

    In *record* mode every LLM response is saved alongside tool-result
    metadata.  In *replay* mode the saved responses are returned
    sequentially, with server-generated IDs rewritten to match the
    current live CP state.
    """

    def __init__(
        self,
        mode: str,
        cassette_path: Path,
        real_client: Any | None = None,
        prefix: str = "",
        model: str = "",
    ) -> None:
        if mode not in ("record", "replay"):
            raise ValueError(f"Invalid mode: {mode!r}; expected 'record' or 'replay'")
        if mode == "record" and real_client is None:
            raise ValueError("record mode requires a real_client")

        self.mode = mode
        self.cassette_path = Path(cassette_path)
        self.real_client = real_client
        self.prefix = prefix
        self.model = model

        self._turn: int = 0
        self._turns: list[dict] = []
        self._id_map = IdRewriteMap()

        # Mimic ``openai.OpenAI().chat.completions.create()`` access path
        self.chat = self
        self.completions = self

    # -- OpenAI-compatible create -------------------------------------------

    def create(
        self,
        model: str | None = None,
        messages: list[dict] | None = None,
        tools: list[dict] | None = None,
        **kwargs: Any,
    ) -> ChatCompletion:
        """Record or replay a single ``chat.completions.create`` call."""
        if self.mode == "record":
            return self._record_turn(model=model, messages=messages, tools=tools, **kwargs)
        return self._replay_turn()

    # -- Tool result observation --------------------------------------------

    def observe_tool_result(self, tool_name: str, result: dict) -> None:
        """Called after each MCP tool execution.

        * *record*: stores the result so it can be saved in the cassette.
        * *replay*: compares against the recorded result to learn ID mappings.
        """
        if tool_name not in _ID_SOURCE_TOOLS:
            return
        if not isinstance(result, dict):
            return

        if self.mode == "record":
            # Attach to the most recent turn
            if self._turns:
                self._turns[-1].setdefault("tool_results", {})[tool_name] = result
        else:
            # Learn ID mappings from recorded vs live results
            if self._turn > 0 and self._turn - 1 < len(self._turns):
                recorded = (
                    self._turns[self._turn - 1]
                    .get("tool_results", {})
                    .get(tool_name)
                )
                if recorded:
                    self._id_map.learn_from_results(tool_name, recorded, result)

    # -- Persistence --------------------------------------------------------

    def save(self) -> None:
        """Write the recorded cassette to disk."""
        from .fixtures import get_recorded_ports

        cassette = {
            "meta": {
                "recorded_at": datetime.now(timezone.utc).isoformat(),
                "model": self.model,
                "prefix": self.prefix,
                "ports": get_recorded_ports(),
            },
            "turns": self._turns,
        }

        self.cassette_path.parent.mkdir(parents=True, exist_ok=True)
        self.cassette_path.write_text(json.dumps(cassette, indent=2))
        logger.info("Saved cassette with %d turns to %s", len(self._turns), self.cassette_path)

    def load(self) -> None:
        """Load a cassette for replay."""
        from .fixtures import set_replay_ports

        raw = json.loads(self.cassette_path.read_text())
        meta = raw.get("meta", {})
        self._turns = raw.get("turns", [])
        self._turn = 0

        self.prefix = meta.get("prefix", self.prefix)
        self.model = meta.get("model", self.model)

        ports = meta.get("ports", [])
        if ports:
            set_replay_ports(ports)

        logger.info(
            "Loaded cassette with %d turns from %s (prefix=%s)",
            len(self._turns),
            self.cassette_path,
            self.prefix,
        )

    # -- Internals ----------------------------------------------------------

    def _record_turn(self, **kwargs: Any) -> ChatCompletion:
        """Forward to real client, record response."""
        response = self.real_client.chat.completions.create(**kwargs)

        # Extract tool names from the response for drift detection
        tool_names: list[str] = []
        choice = response.choices[0] if response.choices else None
        if choice and choice.message and choice.message.tool_calls:
            tool_names = [tc.function.name for tc in choice.message.tool_calls]

        self._turns.append({
            "turn": self._turn,
            "response": serialize_response(response),
            "tool_names": tool_names,
        })
        self._turn += 1
        return response

    def _replay_turn(self) -> ChatCompletion:
        """Return the next recorded response with IDs rewritten."""
        if self._turn >= len(self._turns):
            raise IndexError(
                f"Cassette exhausted: requested turn {self._turn} "
                f"but only {len(self._turns)} turns recorded"
            )

        entry = self._turns[self._turn]
        data = copy.deepcopy(entry["response"])

        # Rewrite server-generated IDs in tool_call arguments
        for choice in data.get("choices", []):
            msg = choice.get("message", {})
            for tc in (msg.get("tool_calls") or []):
                fn = tc.get("function", {})
                fn_name = fn.get("name", "")
                raw_args = fn.get("arguments", "{}")
                try:
                    args = json.loads(raw_args)
                    rewritten = self._id_map.rewrite_args(fn_name, args)
                    if rewritten is not args:
                        fn["arguments"] = json.dumps(rewritten)
                except (json.JSONDecodeError, TypeError):
                    pass

        self._turn += 1
        return deserialize_response(data)
