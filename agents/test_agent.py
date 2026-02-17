"""
Comprehensive pytest tests for agent.py.

Covers: mcp_to_openai_tools, Guardrails, GuardrailReject, ConversationMemory.
All MCP client interactions are mocked — no live server or LLM required.
"""

import sys
import os

sys.path.insert(0, os.path.dirname(__file__))

import pytest
from unittest.mock import MagicMock, call

from agent import (
    mcp_to_openai_tools,
    Guardrails,
    GuardrailReject,
    ConversationMemory,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture
def mock_mcp():
    """Return a MagicMock that mimics FlowplaneMCPClient."""
    mcp = MagicMock()
    mcp.call_tool = MagicMock(return_value={})
    mcp.list_tools = MagicMock(return_value=[])
    return mcp


@pytest.fixture
def guardrails(mock_mcp):
    """Return a Guardrails instance backed by a mock MCP client."""
    return Guardrails(mock_mcp)


@pytest.fixture
def memory():
    """Return a ConversationMemory with a small window for easier testing."""
    return ConversationMemory(max_history=5)


# ---------------------------------------------------------------------------
# Helper MCP tool dicts
# ---------------------------------------------------------------------------

def _tool(name, description=None, schema=None):
    """Build a minimal MCP tool dict."""
    t = {"name": name}
    if description is not None:
        t["description"] = description
    if schema is not None:
        t["inputSchema"] = schema
    return t


# ===========================================================================
# mcp_to_openai_tools
# ===========================================================================

class TestMcpToOpenaiTools:
    """Tests for the mcp_to_openai_tools converter."""

    def test_basic_conversion(self):
        """Each MCP tool should produce an OpenAI function-calling dict."""
        tools = [
            _tool("cp_list_clusters", "List clusters", {"type": "object", "properties": {"scope": {"type": "string"}}}),
        ]
        result = mcp_to_openai_tools(tools)
        assert len(result) == 1
        fn = result[0]
        assert fn["type"] == "function"
        assert fn["function"]["name"] == "cp_list_clusters"
        assert fn["function"]["description"] == "List clusters"
        assert fn["function"]["parameters"]["type"] == "object"
        assert "scope" in fn["function"]["parameters"]["properties"]

    def test_multiple_tools(self):
        """All tools should be converted when no filter is applied."""
        tools = [
            _tool("tool_a", "A"),
            _tool("tool_b", "B"),
            _tool("tool_c", "C"),
        ]
        result = mcp_to_openai_tools(tools)
        assert len(result) == 3
        names = {t["function"]["name"] for t in result}
        assert names == {"tool_a", "tool_b", "tool_c"}

    def test_filtering_with_allowed_set(self):
        """Only tools whose names are in the allowed set should appear."""
        tools = [
            _tool("keep_me", "yes"),
            _tool("drop_me", "no"),
            _tool("also_keep", "yes"),
        ]
        result = mcp_to_openai_tools(tools, allowed={"keep_me", "also_keep"})
        names = {t["function"]["name"] for t in result}
        assert names == {"keep_me", "also_keep"}

    def test_allowed_empty_set_treated_as_no_filter(self):
        """An empty allowed set is falsy in Python, so it acts like None (no filter)."""
        tools = [_tool("anything", "desc")]
        result = mcp_to_openai_tools(tools, allowed=set())
        # Implementation uses `if allowed and ...` — empty set is falsy, so no filtering
        assert len(result) == 1

    def test_allowed_none_keeps_all(self):
        """allowed=None (default) should keep every tool."""
        tools = [_tool("a"), _tool("b")]
        result = mcp_to_openai_tools(tools, allowed=None)
        assert len(result) == 2

    def test_empty_input(self):
        """An empty tool list should produce an empty result."""
        assert mcp_to_openai_tools([]) == []

    def test_missing_description(self):
        """A tool without 'description' should default to empty string."""
        tools = [_tool("nodesc")]
        result = mcp_to_openai_tools(tools)
        assert result[0]["function"]["description"] == ""

    def test_missing_input_schema(self):
        """A tool without 'inputSchema' should get a default empty object schema."""
        tools = [_tool("noschema")]
        result = mcp_to_openai_tools(tools)
        params = result[0]["function"]["parameters"]
        assert params == {"type": "object", "properties": {}}

    def test_preserves_complex_schema(self):
        """A full JSON Schema should pass through unchanged."""
        schema = {
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "port": {"type": "integer", "minimum": 1, "maximum": 65535},
            },
            "required": ["name"],
        }
        tools = [_tool("complex", "Complex tool", schema)]
        result = mcp_to_openai_tools(tools)
        assert result[0]["function"]["parameters"] == schema


# ===========================================================================
# GuardrailReject
# ===========================================================================

class TestGuardrailReject:
    """Tests for the GuardrailReject exception."""

    def test_is_exception_subclass(self):
        """GuardrailReject should be an Exception."""
        assert issubclass(GuardrailReject, Exception)

    def test_can_be_raised_and_caught(self):
        with pytest.raises(GuardrailReject, match="blocked"):
            raise GuardrailReject("blocked")

    def test_message_preserved(self):
        exc = GuardrailReject("some reason")
        assert str(exc) == "some reason"


# ===========================================================================
# Guardrails
# ===========================================================================

class TestGuardrailsInit:
    """Tests for Guardrails initialisation."""

    def test_manifest_keys(self, guardrails):
        """Manifest should contain all expected resource type keys."""
        expected = {"clusters", "route_configs", "listeners", "virtual_hosts", "routes", "filters", "dataplanes"}
        assert set(guardrails.manifest.keys()) == expected
        for v in guardrails.manifest.values():
            assert v == []

    def test_chaining(self, guardrails):
        """enable_* methods return self for builder-pattern chaining."""
        result = guardrails.enable_auto_preflight().enable_dataplane_injection()
        assert result is guardrails


class TestGuardrailsPreHooks:
    """Tests for custom pre-call hooks."""

    def test_pre_hook_modifies_args(self, guardrails):
        """A pre-hook that returns modified args should be used downstream."""
        def inject_team(tool_name, args):
            return {**args, "team": "injected"}

        guardrails.add_pre_hook(inject_team)
        modified = guardrails.before_call("any_tool", {"name": "foo"})
        assert modified["team"] == "injected"
        assert modified["name"] == "foo"

    def test_pre_hook_reject_raises(self, guardrails):
        """A pre-hook that returns None should raise GuardrailReject."""
        def reject_all(tool_name, args):
            return None

        guardrails.add_pre_hook(reject_all)
        with pytest.raises(GuardrailReject, match="Pre-hook rejected"):
            guardrails.before_call("cp_delete_cluster", {"id": "123"})

    def test_multiple_pre_hooks_chain(self, guardrails):
        """Multiple pre-hooks run in order, each receiving the previous result."""
        def add_a(tool_name, args):
            return {**args, "a": True}

        def add_b(tool_name, args):
            assert args.get("a") is True, "Should see result from first hook"
            return {**args, "b": True}

        guardrails.add_pre_hook(add_a)
        guardrails.add_pre_hook(add_b)
        result = guardrails.before_call("tool", {})
        assert result == {"a": True, "b": True}

    def test_pre_hook_receives_correct_arguments(self, guardrails):
        """The hook should receive the exact tool_name and args passed in."""
        received = {}

        def capture(tool_name, args):
            received["tool_name"] = tool_name
            received["args"] = args
            return args

        guardrails.add_pre_hook(capture)
        guardrails.before_call("my_tool", {"key": "value"})
        assert received["tool_name"] == "my_tool"
        assert received["args"] == {"key": "value"}


class TestGuardrailsPostHooks:
    """Tests for custom post-call hooks."""

    def test_post_hook_receives_correct_arguments(self, guardrails):
        """Post-hook should receive tool_name, args, and result."""
        received = {}

        def capture(tool_name, args, result):
            received["tool_name"] = tool_name
            received["args"] = args
            received["result"] = result

        guardrails.add_post_hook(capture)
        guardrails.after_call("my_tool", {"a": 1}, {"status": "ok"})
        assert received["tool_name"] == "my_tool"
        assert received["args"] == {"a": 1}
        assert received["result"] == {"status": "ok"}

    def test_post_hook_exception_swallowed(self, guardrails):
        """Post-hooks that raise should not propagate — errors are swallowed."""
        def exploding_hook(tool_name, args, result):
            raise RuntimeError("boom")

        guardrails.add_post_hook(exploding_hook)
        # Should not raise
        guardrails.after_call("tool", {}, {})

    def test_multiple_post_hooks(self, guardrails):
        """All registered post-hooks should be called."""
        calls = []

        def hook_a(tn, a, r):
            calls.append("a")

        def hook_b(tn, a, r):
            calls.append("b")

        guardrails.add_post_hook(hook_a)
        guardrails.add_post_hook(hook_b)
        guardrails.after_call("tool", {}, {})
        assert calls == ["a", "b"]


class TestGuardrailsAutoPreflight:
    """Tests for the auto-preflight feature."""

    def test_preflight_fires_on_first_create(self, guardrails, mock_mcp):
        """Auto-preflight should call dev_preflight_check on the first cp_create_* call."""
        guardrails.enable_auto_preflight()

        guardrails.before_call("cp_create_cluster", {"name": "my-cluster"})

        mock_mcp.call_tool.assert_called_once_with(
            "dev_preflight_check", {"name": "my-cluster"}
        )

    def test_preflight_does_not_fire_twice(self, guardrails, mock_mcp):
        """After the first cp_create_* call, preflight should not fire again."""
        guardrails.enable_auto_preflight()

        guardrails.before_call("cp_create_cluster", {"name": "c1"})
        guardrails.before_call("cp_create_listener", {"name": "l1"})

        # Only one call to dev_preflight_check
        preflight_calls = [
            c for c in mock_mcp.call_tool.call_args_list
            if c[0][0] == "dev_preflight_check"
        ]
        assert len(preflight_calls) == 1

    def test_preflight_does_not_fire_for_non_create(self, guardrails, mock_mcp):
        """Non cp_create_* tools should not trigger preflight."""
        guardrails.enable_auto_preflight()

        guardrails.before_call("cp_list_clusters", {})
        guardrails.before_call("cp_delete_cluster", {"id": "1"})

        mock_mcp.call_tool.assert_not_called()

    def test_preflight_disabled_by_default(self, guardrails, mock_mcp):
        """Without enable_auto_preflight(), no preflight call should happen."""
        guardrails.before_call("cp_create_cluster", {"name": "c"})
        mock_mcp.call_tool.assert_not_called()

    def test_preflight_error_is_swallowed(self, guardrails, mock_mcp):
        """If dev_preflight_check raises, the call should still proceed."""
        guardrails.enable_auto_preflight()
        mock_mcp.call_tool.side_effect = RuntimeError("preflight failed")

        # Should not raise
        result = guardrails.before_call("cp_create_cluster", {"name": "c"})
        assert result == {"name": "c"}

    def test_reset_turn_resets_preflight(self, guardrails, mock_mcp):
        """After reset_turn(), the next create call should trigger preflight again."""
        guardrails.enable_auto_preflight()

        guardrails.before_call("cp_create_cluster", {"name": "c1"})
        assert mock_mcp.call_tool.call_count == 1

        guardrails.reset_turn()

        guardrails.before_call("cp_create_listener", {"name": "l1"})
        preflight_calls = [
            c for c in mock_mcp.call_tool.call_args_list
            if c[0][0] == "dev_preflight_check"
        ]
        assert len(preflight_calls) == 2


class TestGuardrailsDataplaneInjection:
    """Tests for the dataplane injection feature."""

    def test_injects_dataplane_id_when_missing(self, guardrails, mock_mcp):
        """For cp_create_listener without dataplaneId, it should be injected."""
        guardrails.enable_dataplane_injection()
        mock_mcp.call_tool.return_value = {"dataplanes": [{"id": "dp-42"}]}

        result = guardrails.before_call("cp_create_listener", {"name": "my-listener"})

        assert result["dataplaneId"] == "dp-42"
        assert result["name"] == "my-listener"

    def test_does_not_overwrite_existing_dataplane_id(self, guardrails, mock_mcp):
        """If dataplaneId is already present, it should not be overwritten."""
        guardrails.enable_dataplane_injection()

        result = guardrails.before_call(
            "cp_create_listener",
            {"name": "l", "dataplaneId": "dp-existing"},
        )
        assert result["dataplaneId"] == "dp-existing"
        # Should NOT have called list_dataplanes
        mock_mcp.call_tool.assert_not_called()

    def test_does_not_overwrite_falsy_but_present_dataplane_id(self, guardrails, mock_mcp):
        """If dataplaneId is empty string, injection should kick in."""
        guardrails.enable_dataplane_injection()
        mock_mcp.call_tool.return_value = {"dataplanes": [{"id": "dp-new"}]}

        result = guardrails.before_call(
            "cp_create_listener",
            {"name": "l", "dataplaneId": ""},
        )
        assert result["dataplaneId"] == "dp-new"

    def test_injection_only_for_listener(self, guardrails, mock_mcp):
        """Dataplane injection should only apply to cp_create_listener."""
        guardrails.enable_dataplane_injection()

        result = guardrails.before_call("cp_create_cluster", {"name": "c"})
        assert "dataplaneId" not in result
        mock_mcp.call_tool.assert_not_called()

    def test_injection_disabled_by_default(self, guardrails, mock_mcp):
        """Without enable_dataplane_injection(), no injection should happen."""
        result = guardrails.before_call("cp_create_listener", {"name": "l"})
        assert "dataplaneId" not in result
        mock_mcp.call_tool.assert_not_called()

    def test_does_not_mutate_original_args(self, guardrails, mock_mcp):
        """Injection should not mutate the original args dict passed in."""
        guardrails.enable_dataplane_injection()
        mock_mcp.call_tool.return_value = {"dataplanes": [{"id": "dp-1"}]}

        original = {"name": "l"}
        guardrails.before_call("cp_create_listener", original)
        assert "dataplaneId" not in original

    def test_creates_default_dataplane_when_none_exist(self, guardrails, mock_mcp):
        """If no dataplanes exist, a default should be created."""
        guardrails.enable_dataplane_injection()
        mock_mcp.call_tool.side_effect = [
            {"dataplanes": []},  # list returns empty
            {"id": "dp-created"},  # create returns new id
        ]

        result = guardrails.before_call("cp_create_listener", {"name": "l"})
        assert result["dataplaneId"] == "dp-created"

        # Should have called create
        create_call = mock_mcp.call_tool.call_args_list[1]
        assert create_call[0] == ("cp_create_dataplane", {"name": "default-dp"})

        # Created dataplane should appear in manifest
        assert len(guardrails.manifest["dataplanes"]) == 1
        assert guardrails.manifest["dataplanes"][0]["id"] == "dp-created"

    def test_injection_with_items_key_response(self, guardrails, mock_mcp):
        """If call_tool returns dataplanes under 'items' key, it should still resolve."""
        guardrails.enable_dataplane_injection()
        mock_mcp.call_tool.return_value = {"items": [{"id": "dp-items"}]}

        result = guardrails.before_call("cp_create_listener", {"name": "l"})
        assert result["dataplaneId"] == "dp-items"

    def test_injection_graceful_on_resolve_error(self, guardrails, mock_mcp):
        """If _resolve_dataplane_id raises, injection is skipped gracefully."""
        guardrails.enable_dataplane_injection()
        mock_mcp.call_tool.side_effect = RuntimeError("connection refused")

        result = guardrails.before_call("cp_create_listener", {"name": "l"})
        # dataplaneId not injected because resolution failed, but no crash
        assert "dataplaneId" not in result


class TestGuardrailsManifest:
    """Tests for resource manifest tracking in after_call."""

    def test_tracks_created_cluster(self, guardrails):
        """after_call for cp_create_cluster should add to manifest['clusters']."""
        guardrails.after_call(
            "cp_create_cluster",
            {"name": "my-cluster", "address": "10.0.0.1"},
            {"id": "cluster-1"},
        )
        assert len(guardrails.manifest["clusters"]) == 1
        entry = guardrails.manifest["clusters"][0]
        assert entry["name"] == "my-cluster"
        assert entry["id"] == "cluster-1"
        assert entry["args"]["address"] == "10.0.0.1"

    def test_tracks_created_listener(self, guardrails):
        """after_call for cp_create_listener should add to manifest['listeners']."""
        guardrails.after_call(
            "cp_create_listener",
            {"name": "l1", "port": 8080},
            {"Id": "listener-1"},
        )
        assert len(guardrails.manifest["listeners"]) == 1
        assert guardrails.manifest["listeners"][0]["id"] == "listener-1"

    def test_captures_id_with_various_cases(self, guardrails):
        """The id field should be captured regardless of casing (id, Id, ID)."""
        guardrails.after_call("cp_create_route", {"name": "r1"}, {"ID": "route-99"})
        assert guardrails.manifest["routes"][0]["id"] == "route-99"

    def test_no_id_in_result(self, guardrails):
        """If the result has no id field, the entry should still be tracked without 'id'."""
        guardrails.after_call("cp_create_filter", {"name": "f1"}, {"status": "ok"})
        assert len(guardrails.manifest["filters"]) == 1
        assert "id" not in guardrails.manifest["filters"][0]

    def test_none_result(self, guardrails):
        """after_call should handle None result without crashing."""
        guardrails.after_call("cp_create_cluster", {"name": "c"}, None)
        assert len(guardrails.manifest["clusters"]) == 1

    def test_non_create_tools_not_tracked(self, guardrails):
        """Non cp_create_* tools should not add to the manifest."""
        guardrails.after_call("cp_list_clusters", {}, {"clusters": []})
        guardrails.after_call("cp_delete_cluster", {"id": "1"}, {})
        for items in guardrails.manifest.values():
            assert items == []

    def test_unknown_resource_type_ignored(self, guardrails):
        """A cp_create_* for a type not in manifest should be silently ignored."""
        guardrails.after_call("cp_create_unknown_thing", {"name": "x"}, {"id": "1"})
        # No crash, no new keys created
        assert "unknown_things" not in guardrails.manifest

    def test_multiple_resources_accumulate(self, guardrails):
        """Multiple creates for the same type should accumulate."""
        for i in range(3):
            guardrails.after_call(
                "cp_create_cluster",
                {"name": f"c{i}"},
                {"id": f"id-{i}"},
            )
        assert len(guardrails.manifest["clusters"]) == 3
        names = [e["name"] for e in guardrails.manifest["clusters"]]
        assert names == ["c0", "c1", "c2"]


# ===========================================================================
# ConversationMemory
# ===========================================================================

class TestConversationMemoryBasic:
    """Tests for ConversationMemory core operations."""

    def test_add_and_retrieve(self, memory):
        msg = {"role": "user", "content": "hello"}
        memory.add(msg)
        assert memory.messages == [msg]

    def test_messages_returns_copy(self, memory):
        """Modifying the returned list should not affect internal state."""
        memory.add({"role": "user", "content": "a"})
        msgs = memory.messages
        msgs.clear()
        assert len(memory.messages) == 1

    def test_add_many(self, memory):
        messages = [
            {"role": "user", "content": "q1"},
            {"role": "assistant", "content": "a1"},
        ]
        memory.add_many(messages)
        assert len(memory.messages) == 2

    def test_reset_clears_messages(self, memory):
        memory.add({"role": "user", "content": "msg"})
        memory.add({"role": "assistant", "content": "reply"})
        memory.reset()
        assert memory.messages == []


class TestConversationMemorySlidingWindow:
    """Tests for the sliding-window eviction policy."""

    def test_evicts_oldest_when_over_limit(self, memory):
        """With max_history=5, adding 7 messages should keep only the last 5."""
        for i in range(7):
            memory.add({"role": "user", "content": f"msg-{i}"})
        assert len(memory.messages) == 5
        # Oldest messages (0, 1) should be gone
        contents = [m["content"] for m in memory.messages]
        assert contents == ["msg-2", "msg-3", "msg-4", "msg-5", "msg-6"]

    def test_eviction_via_add_many(self, memory):
        """add_many should also respect the max_history limit."""
        messages = [{"role": "user", "content": f"m-{i}"} for i in range(10)]
        memory.add_many(messages)
        assert len(memory.messages) == 5

    def test_exactly_at_limit(self, memory):
        """Adding exactly max_history messages should not evict any."""
        for i in range(5):
            memory.add({"role": "user", "content": f"msg-{i}"})
        assert len(memory.messages) == 5
        assert memory.messages[0]["content"] == "msg-0"

    def test_default_max_history(self):
        """Default max_history should be 20."""
        mem = ConversationMemory()
        assert mem.max_history == 20


class TestConversationMemoryDeploymentContext:
    """Tests for the deployment_context property and manifest linking."""

    def test_empty_when_no_manifest(self, memory):
        """deployment_context should be empty when no manifest is linked."""
        assert memory.deployment_context == ""

    def test_empty_when_manifest_has_no_items(self, memory):
        """deployment_context should be empty when manifest is all empty lists."""
        manifest = {
            "clusters": [],
            "listeners": [],
            "routes": [],
        }
        memory.link_manifest(manifest)
        assert memory.deployment_context == ""

    def test_returns_summary_with_populated_manifest(self, memory):
        """deployment_context should summarise resource names."""
        manifest = {
            "clusters": [{"name": "web-cluster"}, {"name": "api-cluster"}],
            "listeners": [{"name": "http-listener"}],
            "routes": [],
        }
        memory.link_manifest(manifest)
        ctx = memory.deployment_context
        assert ctx.startswith("Deployed this session:")
        assert "clusters:" in ctx
        assert "web-cluster" in ctx
        assert "api-cluster" in ctx
        assert "listeners:" in ctx
        assert "http-listener" in ctx
        # Empty routes should not appear
        assert "routes:" not in ctx

    def test_uses_id_when_name_missing(self, memory):
        """If a resource has no name, it should fall back to id."""
        manifest = {
            "clusters": [{"id": "c-123"}],
        }
        memory.link_manifest(manifest)
        assert "c-123" in memory.deployment_context

    def test_uses_question_mark_when_no_name_or_id(self, memory):
        """If a resource has neither name nor id, it should show '?'."""
        manifest = {
            "filters": [{"type": "cors"}],
        }
        memory.link_manifest(manifest)
        assert "?" in memory.deployment_context

    def test_link_manifest_replaces_previous(self, memory):
        """Linking a new manifest should replace the previous one."""
        memory.link_manifest({"clusters": [{"name": "old"}]})
        memory.link_manifest({"clusters": [{"name": "new"}]})
        assert "new" in memory.deployment_context
        assert "old" not in memory.deployment_context


class TestConversationMemoryToMessages:
    """Tests for to_messages() output format."""

    def test_prepends_system_prompt(self, memory):
        """First message should be the system prompt."""
        memory.add({"role": "user", "content": "hi"})
        msgs = memory.to_messages("You are helpful.")
        assert msgs[0] == {"role": "system", "content": "You are helpful."}
        assert msgs[1] == {"role": "user", "content": "hi"}

    def test_system_prompt_with_empty_history(self, memory):
        """Even with no history, system prompt should be present."""
        msgs = memory.to_messages("System prompt here")
        assert len(msgs) == 1
        assert msgs[0]["role"] == "system"

    def test_injects_deployment_context(self, memory):
        """When manifest has items, deployment context should be appended to system prompt."""
        manifest = {
            "clusters": [{"name": "prod-cluster"}],
        }
        memory.link_manifest(manifest)
        memory.add({"role": "user", "content": "status?"})

        msgs = memory.to_messages("You are an ops agent.")
        system_content = msgs[0]["content"]

        assert "You are an ops agent." in system_content
        assert "## Session Context" in system_content
        assert "prod-cluster" in system_content

    def test_no_context_injection_without_manifest(self, memory):
        """Without a manifest, system prompt should be unmodified."""
        memory.add({"role": "user", "content": "hello"})
        msgs = memory.to_messages("Base prompt")
        assert msgs[0]["content"] == "Base prompt"

    def test_no_context_injection_with_empty_manifest(self, memory):
        """With an empty manifest, system prompt should be unmodified."""
        memory.link_manifest({"clusters": [], "listeners": []})
        memory.add({"role": "user", "content": "hello"})
        msgs = memory.to_messages("Base prompt")
        assert msgs[0]["content"] == "Base prompt"

    def test_message_order_preserved(self, memory):
        """Messages should appear in insertion order after system prompt."""
        memory.add({"role": "user", "content": "q1"})
        memory.add({"role": "assistant", "content": "a1"})
        memory.add({"role": "user", "content": "q2"})

        msgs = memory.to_messages("sys")
        roles = [m["role"] for m in msgs]
        assert roles == ["system", "user", "assistant", "user"]

    def test_to_messages_does_not_mutate_history(self, memory):
        """Calling to_messages should not alter the internal message list."""
        memory.add({"role": "user", "content": "msg"})
        before = memory.messages
        memory.to_messages("sys")
        after = memory.messages
        assert before == after


# ===========================================================================
# Integration-style: Guardrails + ConversationMemory
# ===========================================================================

class TestGuardrailsMemoryIntegration:
    """Test that Guardrails manifest links correctly to ConversationMemory."""

    def test_manifest_updates_reflected_in_memory_context(self, guardrails, memory):
        """After linking manifest and adding resources, deployment_context should update."""
        memory.link_manifest(guardrails.manifest)
        assert memory.deployment_context == ""

        guardrails.after_call("cp_create_cluster", {"name": "web"}, {"id": "c1"})
        ctx = memory.deployment_context
        assert "web" in ctx

    def test_to_messages_reflects_live_manifest(self, guardrails, memory):
        """to_messages should incorporate resources added after linking."""
        memory.link_manifest(guardrails.manifest)
        memory.add({"role": "user", "content": "deploy"})

        # Add resource after linking
        guardrails.after_call("cp_create_listener", {"name": "http"}, {"id": "l1"})

        msgs = memory.to_messages("Deploy agent")
        assert "http" in msgs[0]["content"]
