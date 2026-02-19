from __future__ import annotations

import time
from concurrent.futures import ThreadPoolExecutor, TimeoutError
from dataclasses import dataclass, field

# Import will be from sibling — add agents/ to sys.path in conftest
from agent import FlowplaneAgent


@dataclass
class ToolCallRecord:
    name: str
    args: dict
    result: dict | str
    turn: int
    elapsed_ms: float


@dataclass
class ConversationTrace:
    prompt: str
    answer: str
    tool_calls: list[ToolCallRecord]
    turn_count: int
    elapsed_s: float
    timed_out: bool
    error: str | None

    def called_tools(self) -> set[str]:
        """Return set of unique tool names that were called."""
        return {tc.name for tc in self.tool_calls}

    def tool_calls_by_name(self, name: str) -> list[ToolCallRecord]:
        """Return all tool call records for a given tool name."""
        return [tc for tc in self.tool_calls if tc.name == name]

    def last_result_for(self, name: str) -> dict | None:
        """Return the result of the last call to the given tool, or None."""
        calls = self.tool_calls_by_name(name)
        if not calls:
            return None
        result = calls[-1].result
        return result if isinstance(result, dict) else None

    def assert_tool_called(self, name: str, min_times: int = 1) -> None:
        """Assert that a tool was called at least min_times."""
        calls = self.tool_calls_by_name(name)
        assert len(calls) >= min_times, (
            f"Expected {name} to be called >= {min_times} times, "
            f"but was called {len(calls)} times"
        )

    def assert_max_turns(self, max_turns: int) -> None:
        """Assert the agent didn't exceed the expected turn count."""
        assert self.turn_count <= max_turns, (
            f"Agent took {self.turn_count} turns (max {max_turns}). "
            f"This suggests the agent is looping or confused. "
            f"Tools called: {[tc.name for tc in self.tool_calls]}"
        )

    def assert_no_error(self) -> None:
        """Assert the conversation completed without error or timeout."""
        assert not self.timed_out, f"Agent timed out after {self.elapsed_s:.1f}s"
        assert self.error is None, f"Agent error: {self.error}"


def run_agent_scenario(
    agent: FlowplaneAgent,
    prompt: str,
    max_turns: int = 20,
    timeout_s: float = 180.0,
) -> ConversationTrace:
    """
    Run agent.run_stream(prompt), collect all events, return ConversationTrace.
    Uses ThreadPoolExecutor for wall-clock timeout.
    """
    tool_calls: list[ToolCallRecord] = []
    answer = ""
    turn = 0
    timed_out = False
    error_msg: str | None = None

    def _run():
        nonlocal answer, turn, error_msg
        try:
            pending_call: dict | None = None
            call_start: float = 0.0
            for event in agent.run_stream(prompt):
                etype = event.get("type", "")
                if etype == "tool_call":
                    pending_call = event
                    call_start = time.monotonic()
                    turn += 1
                elif etype == "tool_result":
                    elapsed_ms = (time.monotonic() - call_start) * 1000 if pending_call else 0.0
                    tool_calls.append(ToolCallRecord(
                        name=event["name"],
                        args=pending_call["args"] if pending_call else {},
                        result=event["result"],
                        turn=turn,
                        elapsed_ms=elapsed_ms,
                    ))
                    pending_call = None
                    # Feed tool results to cassette for ID rewriting
                    if hasattr(agent, 'llm') and hasattr(agent.llm, 'observe_tool_result'):
                        agent.llm.observe_tool_result(event["name"], event.get("result", {}))
                elif etype == "answer":
                    answer = event.get("content", "")
        except Exception as exc:
            error_msg = str(exc)

    start = time.monotonic()
    with ThreadPoolExecutor(max_workers=1) as pool:
        future = pool.submit(_run)
        try:
            future.result(timeout=timeout_s)
        except TimeoutError:
            timed_out = True
            error_msg = f"Timed out after {timeout_s}s"
    elapsed = time.monotonic() - start

    return ConversationTrace(
        prompt=prompt,
        answer=answer,
        tool_calls=tool_calls,
        turn_count=turn,
        elapsed_s=elapsed,
        timed_out=timed_out,
        error=error_msg,
    )
