#!/usr/bin/env python3
"""Independent S5 documentation contract lint, derived only from supplied acceptance.

No shell execution, network, dependencies, or repository writes. --self-test uses
in-memory synthetic Markdown; --repo reads public Markdown only. This is a
supported-pattern structural lint, NOT a shell interpreter, fail-closed shell
grammar, or qualification of runtime, platform, safety, or freshness. Unknown
forms may pass: quoted substitutions, command wrappers, variable commands and
Docker global-option/volume-flag variants are not exhaustively analyzed. Review
EVERY executable block manually; zero findings means only no matched violation
of the tested patterns. Do not use this lint as execution/security approval.
"""
from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
import re
import shlex
import sys
from urllib.parse import unquote, urlsplit

GUIDE = "docs/how-to/evaluation-readiness-and-recovery.md"
PREFIX = "[SYNTHETIC CONTRACT CHECK]"


def has(text: str, pattern: str) -> bool:
    return re.search(pattern, text, re.I | re.S) is not None


def blocks(text: str) -> list[str]:
    return [m.group(2).replace("\\\n", " ") for m in
            re.finditer(r"(?m)^\s*(`{3,}|~{3,})[^\n]*\n(.*?)^\s*\1\s*$", text, re.S)]


def prose(text: str) -> str:
    return re.sub(r"(?ms)^\s*(`{3,}|~{3,})[^\n]*\n.*?^\s*\1\s*$", "", text)


def section(text: str, pattern: str) -> str:
    """Include child headings, stop at the next peer/ancestor; ignore fenced headings."""
    masked = text
    for m in reversed(list(re.finditer(
            r"(?ms)^\s*(`{3,}|~{3,})[^\n]*\n.*?^\s*\1\s*$", text))):
        masked = masked[:m.start()] + " " * (m.end() - m.start()) + masked[m.end():]
    headings = list(re.finditer(r"(?m)^(#{1,6})\s+([^\n]+)", masked))
    for i, heading in enumerate(headings):
        # A document title is not a task section when it contains child sections.
        if len(heading.group(1)) == 1 and any(len(h.group(1)) > 1 for h in headings[i + 1:]):
            continue
        if has(heading.group(2), pattern):
            end = len(text)
            for other in headings[i + 1:]:
                if len(other.group(1)) <= len(heading.group(1)):
                    end = other.start()
                    break
            return text[heading.start():end]
    return ""


def compose_tail(tokens: list[str], begin: int) -> tuple[str, int] | None:
    i = begin + (1 if tokens[begin].endswith("-compose") else 2)
    files = []
    while i < len(tokens):
        token = tokens[i]
        if token in ("-f", "--file", "-p", "--project-name"):
            if i + 1 >= len(tokens):
                raise ValueError("missing Compose option value")
            if token in ("-f", "--file"):
                files.append(tokens[i + 1])
            i += 2
        elif token.startswith("-"):
            raise ValueError("unsupported Compose global option: " + token)
        else:
            break
    if not files or files[0] != "compose.eval.yml" or any(not re.fullmatch(r"[\w.-]+\.ya?ml", f) for f in files):
        raise ValueError("unsupported Compose file/interface")
    end = i
    while end < len(tokens) and not all(c in ";|&()\n" for c in tokens[end]):
        end += 1
    return (shlex.join(tokens[i:end]), end) if i < end else None


def compose_commands(text: str, context: str | None = None) -> list[str]:
    """Recognize Docker/Podman Compose and simple documented function aliases."""
    aliases = set()
    for b in blocks(context if context is not None else text):
        for m in re.finditer(r"(?m)^\s*(?:function\s+)?([\w-]+)\s*\(\)\s*\{([^}]+)\}", b):
            if has(m.group(2), r"(?:docker|podman)(?:\s+compose|-compose)\s+(?:-p\s+\S+\s+)?(?:-f|--file)\s+['\"]?compose\.eval\.yml"):
                aliases.add(m.group(1))
        for m in re.finditer(r"(?m)^\s*(\w+)=['\"]((?:docker|podman)(?:\s+compose|-compose)\s+(?:-p\s+\S+\s+)?(?:-f|--file)\s+compose\.eval\.yml)['\"]", b):
            aliases.add("\\$\\{?" + m.group(1) + "\\}?")
    result = []
    for b in blocks(text):
        try:
            tokens = shell_tokens(b)
        except ValueError:
            continue  # check_guide emits an explicit unsupported-shell finding.
        for j, token in enumerate(tokens):
            if token in ("docker-compose", "podman-compose") or (token in ("docker", "podman") and j + 1 < len(tokens) and tokens[j + 1] == "compose"):
                tail = compose_tail(tokens, j)
                if tail:
                    result.append(tail[0])
    if aliases:
        starters = [re.escape(a) if not a.startswith("\\$") else a for a in aliases]
        pattern = r"(?:" + "|".join(starters) + r")\s+([^\n;]+)"
        result.extend(m.group(1).strip() for b in blocks(text) for m in re.finditer(pattern, b))
    return result


def bounded_failure(block: str) -> bool:
    bound = has(block, r"(?:\bseq\s+\d+(?:\s+\d+)?|\{\d+\.\.\d+\}|\btimeout\s+\d+|range\(\s*\d+\s*\)|\bfor\s*\(\([^;]+;[^;]+(?:<|<=)\s*\d+\s*;|(?:attempt|tries|count|deadline|elapsed|SECONDS)[^\n]{0,60}(?:-lt|-le|<|<=)\s*\d+)")
    failure = has(block, r"\b(?:echo|printf|print\s*\()[^\n]*(?:fail|timed?\s*out|not ready|did not become ready|unavailable)")
    nonzero = has(block, r"\b(?:exit|return)\s+[1-9]\d*\b|raise\s+(?:SystemExit|RuntimeError)|sys\.exit\([1-9]")
    guard = has(block, r"\b(?:if|unless|test)\b|\|\||\[\[?|\belse\b")
    return bound and failure and nonzero and guard


@dataclass(frozen=True)
class Finding:
    rule: str
    message: str


def json_field(text: str, field: str) -> bool:
    return has(text, r"data\." + field + r"\b|\[['\"]data['\"]\]\s*\[['\"]" + field + r"['\"]\]")


def empty_inventory(text: str) -> bool:
    return ("data.items" in text and has(text, r"length\s*==\s*0")) or has(
        text, r"\[['\"]data['\"]\]\s*\[['\"]items['\"]\]\s*==\s*\[\s*\]")


def manual_inventory(text: str) -> bool:
    code = "\n".join(blocks(text))
    return (all(has(code, r"(?m)^\s*fp\s+(?:-o\s+json\s+)?" + resource + r"\s+list\b")
                for resource in ("listener", "route", "cluster"))
            and has(prose(text), r"Expected:[^\n]*all three inventories are empty[^\n]*fresh")
            and has(prose(text), r"If they are not, stop and inspect"))


def manual_initial_auth(text: str) -> bool:
    code = "\n".join(blocks(text))
    description = prose(text)
    return (has(code, r"(?m)^\s*fp\s+auth\s+whoami\s*$")
            and has(code, r"(?m)^\s*fp\s+-o\s+json\s+dataplane\s+get\s+dp-eval\s*$")
            and has(description, r"Expected:[^\n]*authenticated identity")
            and has(description, r"data\.last_heartbeat_at[^\n]*non-null and recent")
            and has(description, r"run the same read again to see it advance")
            and has(description, r"If authentication fails[^\n]*timestamp remains missing/stale[^\n]*stop before exposure")
            and has(code, r"(?m)^\s*docker compose -f compose\.eval\.yml logs[^\n]*flowplane-agent[^\n]*envoy"))


def shell_tokens(block: str, depth: int = 0) -> list[str]:
    """Block-level quoting; literal heredocs are opaque, never shell code.

    Only standalone literal heredocs to cat/python are supported. Dynamic shell
    construction and other heredoc consumers are rejected in the recognized
    positions only. This tokenizer does not reject every opaque/dynamic form;
    manual review of every executable block remains mandatory.
    """
    if depth > 8:
        raise ValueError("unsupported deeply nested shell")
    lines = block.splitlines(keepends=True)
    output: list[str] = []
    i = 0
    while i < len(lines):
        line = lines[i]
        match = re.search(r"<<(-?)\s*(['\"]?)([A-Za-z_][\w]*)\2\s*$", line.rstrip())
        if match:
            if not has(line, r"\b(?:cat|python3?)\b"):
                raise ValueError("unsupported heredoc consumer")
            output.append(line[:match.start()] + "\n")
            i += 1
            while i < len(lines) and lines[i].strip() != match.group(3):
                i += 1
            if i == len(lines):
                raise ValueError("unterminated heredoc")
        elif "<<" in line:
            raise ValueError("unsupported heredoc syntax")
        else:
            output.append(line)
        i += 1
    lexer = shlex.shlex("".join(output), posix=True, punctuation_chars=";|&()<>\n")
    lexer.whitespace = " \t\r"
    lexer.whitespace_split = True
    tokens = list(lexer)
    if any(t in ("eval", "source", ".") and (j == 0 or tokens[j - 1] in (";", "\n", "&&", "||", "then", "do"))
           for j, t in enumerate(tokens)):
        raise ValueError("unsupported dynamic shell construction")
    for j, token in enumerate(tokens):
        if token.startswith(("$", "`")) and (j == 0 or (tokens[j - 1] and all(c in ";|&()\n" for c in tokens[j - 1])) or tokens[j - 1] in ("then", "do", "!")):
            raise ValueError("unsupported variable/backtick command construction")
    for j, token in enumerate(tokens[:-1]):
        if token in ("sh", "bash", "zsh") and tokens[j + 1].startswith("-") and "c" in tokens[j + 1]:
            if j + 2 >= len(tokens) or tokens[j + 2].startswith("$") or "`" in tokens[j + 2]:
                raise ValueError("unsupported dynamic shell command")
            shell_tokens(tokens[j + 2], depth + 1)
    for j, token in enumerate(tokens):
        if token in ("docker-compose", "podman-compose") or (token in ("docker", "podman") and j + 1 < len(tokens) and tokens[j + 1] == "compose"):
            compose_tail(tokens, j)
    return [(t.replace("\n", "") or ";") if "\n" in t and all(c in ";|&()<>\n" for c in t) else t for t in tokens]


def render_tokens(tokens: list[str]) -> str:
    return " ".join(t if (t and all(c in ";|&()<>" for c in t))
                    or re.fullmatch(r"\w+(?:<|<=)\d+", t) else shlex.quote(t) for t in tokens)


def tokens_mutate(tokens: list[str], depth: int = 0) -> bool:
    if depth > 8:
        return True
    segments: list[list[str]] = [[]]
    for token in tokens:
        if token and all(c in ";|&()" for c in token):
            segments.append([])
        else:
            segments[-1].append(token)
    for segment in segments:
        while segment and (segment[0] in ("if", "then", "do", "while", "until", "sudo", "command", "env", "!", "{")
                           or re.match(r"^\w+=", segment[0])):
            segment = segment[1:]
        if not segment:
            continue
        if segment[0] in ("fp", "flowplane") and "expose" in segment[1:]:
            return True
        if segment[0] in ("docker", "podman", "docker-compose", "podman-compose"):
            if len(segment) > 2 and segment[1] == "volume" and segment[2] in ("rm", "prune"):
                return True
            if "exec" in segment:
                tail = segment[segment.index("exec") + 1:]
                for j, token in enumerate(tail):
                    if token in ("fp", "flowplane", "sh", "bash", "zsh") and tokens_mutate(tail[j:], depth + 1):
                        return True
        if segment[0] in ("sh", "bash", "zsh"):
            for j, token in enumerate(segment[1:], 1):
                if token.startswith("-") and "c" in token and j + 1 < len(segment):
                    try:
                        if tokens_mutate(shell_tokens(segment[j + 1]), depth + 1):
                            return True
                    except ValueError:
                        return True
    return False


def exposure_or_volume_mutation(text: str, depth: int = 0) -> bool:
    for block in blocks(text):
        try:
            if tokens_mutate(shell_tokens(block), depth):
                return True
        except ValueError:
            return True
    return False


def loop_regions(block: str) -> list[str]:
    """Include while/until conditions, not merely their bodies."""
    tokens = shell_tokens(block)
    stack: list[int] = []
    regions = []
    for j, token in enumerate(tokens):
        if token in ("for", "while", "until"):
            stack.append(j)
        elif token == "done" and stack:
            begin = stack.pop()
            regions.append(render_tokens(tokens[begin:j + 1]))
    if stack:
        raise ValueError("unterminated shell loop")
    return regions


def mutation_in_loop(text: str) -> bool:
    for block in blocks(text):
        try:
            for region in loop_regions(block):
                if tokens_mutate(shell_tokens(region)):
                    return True
        except ValueError:
            return True
    return False


def bounded_auth(block: str) -> bool:
    try:
        regions = loop_regions(block)
    except ValueError:
        return False
    auth = [r for r in regions if has(r, r"\bfp\s+auth\s+whoami\b")]
    return bool(auth) and all(bounded_failure(r + "\n" + block[block.rfind("done") + 4:]) for r in auth)


def newer_heartbeat(block: str) -> bool:
    """Recognize an auditable post-disruption capture-then-advance pattern.

    This is a structural contract, not execution or a general dataflow proof.
    Raw string/shell comparisons are intentionally unsupported (ISO fractions).
    """
    try:
        regions = loop_regions(block)
    except ValueError:
        return False
    for m in re.finditer(r"(\w+)\s*=\s*json\.load\(open\(['\"]([^'\"]+)['\"]\)\)\s*\[['\"]data['\"]\]", block):
        old, baseline = m.groups()
        for n in re.finditer(r"(\w+)\s*=\s*json\.load\(open\(['\"]([^'\"]*(?:now|current)[^'\"]*)['\"]\)\)\s*\[['\"]data['\"]\]", block):
            current, now = n.groups()
            if current == old:
                continue
            o, c = re.escape(old), re.escape(current)
            comparison = r"assert\s+parse\(" + c + r"\[['\"]last_heartbeat_at['\"]\]\)\s*>\s*parse\(" + o + r"\[['\"]last_heartbeat_at['\"]\]\)"
            same_id = r"assert\s+(?:sys\.argv\[1\]|expected_id)\s*==\s*" + o + r"\[['\"]id['\"]\]\s*==\s*" + c + r"\[['\"]id['\"]\]"
            capture = r"\bcp\s+" + re.escape(now) + r"\s+" + re.escape(baseline) + r"\s*(?:;|\n)\s*(\w+)=true"
            captured = re.search(capture, block)
            if not captured:
                continue
            flag = re.escape(captured.group(1))
            authenticated = r"fp\s+auth\s+whoami(?:\s*>\s*/dev/null)?(?:\s+2\s*>\s*&\s*1)?\s*&&\s*fp\s+-o\s+json\s+dataplane\s+get\s+dp-eval\s*>\s*" + re.escape(now)
            nonnull = c + r"\[['\"]last_heartbeat_at['\"]\]\s+is\s+not\s+None"
            valid_capture = c + r"\[['\"]id['\"]\]\s*==\s*(?:sys\.argv\[1\]|expected_id)"
            compared = re.search(comparison, block)
            if (compared is not None and has(block, same_id) and has(block, authenticated)
                    and has(block, nonnull) and has(block, valid_capture)
                    and has(block, flag + r"=false") and has(block, r"if\s+\[\s*['\"]?\$" + flag + r"['\"]?\s*=\s*false\s*\]")
                    and block.index(captured.group(0)) < compared.start()
                    and (has(block, r"fromisoformat") and has(block, r"timezone\.utc")
                         or has(block, r"datetime\.strptime") and "%f%z" in block and "%S%z" in block)
                    and has(block, r"expected_id\s*=\s*(?:\$\([^\n]*\.data\.id[^\n]*\)|json\.load[^\n]*|\w+\[['\"]id['\"]\])")
                    and any(has(r, authenticated) and has(r, capture) and bounded_failure(r + "\n" + block[block.rfind("done") + 4:]) for r in regions)):
                return True
    return False


def compose_services(command: str) -> set[str]:
    """Positional service filters for the supported ps/logs diagnostics."""
    try:
        tokens = shlex.split(command)[1:]
    except ValueError:
        return {"__unparsed__"}
    services: set[str] = set()
    skip = False
    for token in tokens:
        if skip:
            skip = False
        elif token in ("--tail", "-n", "--since", "--until", "--format", "--status"):
            skip = True
        elif not token.startswith("-"):
            services.add(token)
    return services


def check_guide(text: str, initial_tutorial: str = "") -> list[Finding]:
    errors: list[Finding] = []

    def require(rule: str, condition: bool, message: str) -> None:
        if not condition:
            errors.append(Finding(rule, message))

    ready = section(text, r"readiness|ready checks|preflight")
    delegated = bool(initial_tutorial) and bool(markdown_links(ready)) and has(prose(ready), r"delegat|initial.*tutorial|tutorial.*initial")
    if delegated:
        # The split install tutorial owns setup, helper, readiness and inventory
        # across child headings. Never import exposure/policy tutorials here.
        if has(prose(initial_tutorial), r"(?m)^# Install and verify Flowplane\s*$"):
            ready = initial_tutorial
        else:
            ready = section(initial_tutorial, r"readiness|ready checks|preflight|install.*infrastructure") or initial_tutorial
    keep = section(text, r"preserv|resum|non.destructive")
    reset = section(text, r"destructive reset|reset.*empty|reset.*evaluation")
    diagnostics = section(text, r"diagnos|troubleshoot")
    ports = section(text, r"ports?|bind conflicts")
    platform = section(text, r"qualification|platform|evidence|limitations") or prose(text)
    context = initial_tutorial if delegated else ""
    code = "\n".join(blocks(text))
    all_code = code + "\n" + "\n".join(blocks(context))
    commands = compose_commands(text)
    keep_commands = compose_commands(keep, text)
    reset_commands = compose_commands(reset, text)

    require("readiness-separate", bool(ready) and has(prose(ready), r"(?:before|without|separate|independent)[^.\n]{0,160}(?:expos|sample|gateway)")
            and has(prose(ready), r"(?:no|not|zero|empty)[^.\n]{0,100}(?:sample|response|API|gateway)")
            and not exposure_or_volume_mutation(ready),
            "Readiness must precede explicit exposure; a fresh install has no sample response.")
    require("fresh-empty", has(ready, r"fresh|new install") and has(ready, r"zero|empty|length\s*==\s*0")
            and all(has(ready, r"\bfp\s+(?:-o\s+json\s+)?" + r + r"\s+list\b") for r in ("listener", "route", "cluster"))
            and (empty_inventory(ready) or (delegated and manual_inventory(ready))),
            "Show asserted data.items or delegated manual fresh empty listener/route/cluster reads with stop guidance.")
    require("auth-bounded", any(bounded_auth(b) for b in blocks(ready))
            or (delegated and manual_initial_auth(ready)),
            "Authentication readiness needs a bounded read-only wait with visible nonzero failure, or delegated manual identity/advancing-heartbeat reads with outcomes and stop/diagnostic guidance.")
    require("heartbeat-newer", any(has(b, r"fp\s+-o\s+json\s+dataplane\s+get\s+dp-eval")
            and "last_heartbeat_at" in b and newer_heartbeat(b)
            and bounded_failure(b) and has(prose(text), r"(?:invoke|call|run)[^.\n]*after[^.\n]*(?:disrupt|resum)")
            and has(prose(text), r"archiv[^.\n]*baseline[^.\n]*(?:advanced|later|current)") for b in blocks(text)),
            "Invoke recovery after disruption: authenticate, capture a same-ID non-null post-disruption baseline, then parse a strictly later same-ID heartbeat in a bounded wait; archive both reads.")
    require("identity-readback", json_field(text, "id") and has(text, r"dataplane.*identity|identity.*dataplane")
            and all(has(code, r"fp\s+-o\s+json\s+" + r + r"\s+get\s+\S+") for r in ("listener", "route", "cluster"))
            and (all(json_field(text, field) for field in ("spec", "revision"))
                 or (has(code, r"\[['\"]data['\"]\]") and any(
                     all(has(t, r"['\"]" + f + r"['\"]") for f in ("id", "revision", "spec"))
                     for t in re.findall(r"for\s+\w+\s+in\s+\(([^)]+)\)", code)))),
            "Use supported CP readbacks for dataplane identity and product id/spec/revision.")
    require("preserve-resume", bool(keep) and any(has(c, r"^down\b") for c in keep_commands)
            and any(has(c, r"^up\b") and has(c, r"(?:^|\s)-d(?:\s|$)") and "--no-build" in c for c in keep_commands)
            and all(v in keep for v in ("pgdata", "shared", "pki-cp", "pki-dp"))
            and has(keep, r"same[^.\n]*directory") and has(keep, r"same[^.\n]*project")
            and has(keep, r"same(?:\s+(?:evaluation|local|candidate|container|published))?\s+image|same[^.\n]*(?:directory|project)[^\n]*[/,]\s*(?:project[/,]\s*)?image") and has(keep, r"user.authored|existing(?: gateway)? resources")
            and has(keep, r"validator|guard") and has(keep, r"without[^.\n]*reseed|no[^.\n]*reseed|not[^.\n]*reseed"),
            "Document down/up --no-build, preserved named volumes, same directory/project/image and guarded non-reseed resume.")
    require("preserve-safe", not any(has(c, r"(?:^|\s)(?:-v|--volumes)(?:\s|$)|volume\s+(?:rm|prune)") for c in keep_commands)
            and not exposure_or_volume_mutation(keep),
            "Preserve/resume examples must not delete volumes or repeat exposure.")
    destructive = [c for c in reset_commands if has(c, r"^down\b") and has(c, r"(?:^|\s)(?:-v|--volumes)(?:\s|$)")]
    require("reset-consent", bool(destructive) and has(prose(reset), r"destruct|permanent")
            and has(prose(reset), r"explicit[^.\n]*(?:consent|confirm|permission)")
            and has(prose(reset), r"(?:own|owned)[^.\n]*(?:evaluat|resources)")
            and all(has(reset, word) for word in (r"identit", r"config", r"PKI", r"shared"))
            and has(reset, r"remov|delet|eras"), "Warn before down -v: explicit consent, owned evaluator only, identity/config/PKI/shared loss.")
    reset_code = "\n".join(blocks(reset))
    deletion = re.search(r"\bdown\s+(?:-v|--volumes)\b", reset_code)
    after = reset_code[deletion.end():] if deletion else ""
    require("reset-empty", bool(after) and has(after, r"\bup\b")
            and all(has(after, r"fp\s+(?:-o\s+json\s+)?" + r + r"\s+list") for r in ("listener", "route", "cluster"))
            and empty_inventory(after) and has(reset, r"zero|empty|length\s*==\s*0")
            or (delegated and bool(after) and (has(after, r"\bup\b")
                or any(has(c, r"^up\b") for c in compose_commands(ready, initial_tutorial)))
                and has(reset, r"initial tutorial") and has(reset, r"fresh empty")
                and (empty_inventory(ready) or manual_inventory(ready))
                and all(has(ready, r"fp\s+(?:-o\s+json\s+)?" + r + r"\s+list") for r in ("listener", "route", "cluster"))),
            "After destructive reset and reinstall, show empty lists; old APIs must not return.")
    diag_commands = compose_commands(diagnostics, text)
    require("supported-diagnostics", any(has(c, r"^ps\b") and not compose_services(c) for c in diag_commands)
            and any(has(c, r"^logs\b") and (not compose_services(c)
                     or {"init", "flowplane-eval", "envoy", "flowplane-agent"} <= compose_services(c)) for c in diag_commands)
            and has(diagnostics, r"Exited\s*\(?\s*0\s*\)?") and has(diagnostics, r"normal|expected|success")
            and has(diagnostics, r"setup") and has(diagnostics, r"readiness|ready") and has(diagnostics, r"port|bind")
            and has(diagnostics, r"fp\s+auth\s+whoami"),
            "Diagnose setup/readiness/port failures with Compose ps/logs and supported CP reads; one-shot Exited(0) is normal.")
    require("no-admin-bypass", not has(all_code, r"config_dump|:9901\b|\bcurl\b[^\n]*(?:--insecure|\s-[a-zA-Z]*k[a-zA-Z]*\b)|(?:TLS|MTLS)[_\w]*\s*=\s*(?:false|0)|\bhttp://[^\s]*:(?:50051|18000)\b"),
            "Executable diagnostics must not use Envoy admin/config_dump or plaintext/TLS bypass.")
    require("no-mutation-retry", not mutation_in_loop(text + "\n" + context),
            "Do not put exposure mutations in automatic retry loops; retry bounded reads only.")
    require("agent-first", (has(text, r"(?:remove|rm)[^.\n]{0,80}flowplane-agent[^.\n]{0,80}(?:before|first)[^.\n]{0,80}(?:envoy|replace)")
            or (has(prose(text), r"remove only the agent before replacing Envoy")
                and any(has(c, r"^rm\s+-sf\s+flowplane-agent(?:\s|$)") for c in commands)))
            and has(text, r"namespace") and has(text, r"recreat[^.\n]*(?:both|envoy[^.\n]*flowplane-agent)"),
            "Explain namespace sharing: remove the agent before replacing Envoy, recreate both.")
    require("ports-distinct", all(v in ports for v in ("FLOWPLANE_EVAL_API_PORT", "FLOWPLANE_EVAL_GATEWAY_PORT"))
            and has(ports, r"host") and has(ports, r"REST") and has(ports, r"container[^.\n]*10000|10000[^.\n]*container")
            and has(ports, r"--port(?:\s+|=)10000") and has(ports, r"dashboard")
            and has(ports, r"8081") and has(ports, r"Host(?:/Origin)? validation")
            and has(ports, r"fixed|cannot|not arbitrarily"),
            "Separate host REST/gateway remaps from exposure container port 10000; dashboard host 8081 is constrained.")
    require("platform-gaps", all(has(platform, p) for p in (r"Mac|macOS", r"ARM|arm64", r"Linux", r"Podman", r"docker(?:\s+compose|-compose)", r"pre.version.bump|source.built[^.\n]*supporting image", r"3\.2\.0", r"Docker\s*Desktop", r"remote\s*CI", r"unfamiliar.*(?:user|developer)"))
            and any(all(has(sentence, term) for term in (r"Mac|macOS", r"ARM|arm64", r"Linux", r"separate(?:.artifact|[^.\n]*release artifacts)"))
                    for sentence in re.split(r"[.\n]", prose(platform))) and has(platform, r"gap|not (?:yet )?qualified|unqualified|unverified|not exercised")
            and not has(platform, r"(?:all platforms|universally|universal)[^.\n]*(?:qualified|supported|resource floor)")
            and not has(platform, r"source.built supporting image,\s*(?:is\s+)?a published immutable"),
            "Retain Mac ARM/Linux separate-artifact gaps; source-built supporting evidence is not immutable published 3.2.0 qualification, and untested environments remain gaps.")
    require("compose-interface", bool(commands), "Use the public compose.eval.yml Compose interface.")
    require("token-inside", not has(all_code, r"TOKEN=\$\([^\n]*(?:compose|exec)[^\n]*(?:cat|token)") and has(all_code, r"fp\s*\(\)\s*\{.*?exec\b[^\n]*flowplane-eval[^\n]*sh\s+-[a-z]*[lc][a-z]*\b")
            and has(text + (initial_tutorial if delegated else ""), r"token[^.\n]*inside|inside[^.\n]*token"),
            "Document fp() reading the token inside the control-plane container, not on the host.")
    unsupported = []
    for b in blocks(text + "\n" + context):
        try:
            shell_tokens(b)
        except ValueError as exc:
            unsupported.append(str(exc))
    require("unsupported-shell", not unsupported,
            "Unsupported shell syntax/dynamic construction needs manual review: " + "; ".join(unsupported))
    outside_reset = text.replace(reset, "") if reset else text
    require("destructive-scope", not any(has(c, r"^down\b") and has(c, r"(?:^|\s)(?:-v|--volumes)(?:\s|$)")
            for c in compose_commands(outside_reset, text)) and not any(
                has(b, r"\b(?:docker|podman)\s+volume\s+(?:rm|prune)\b") for b in blocks(outside_reset)),
            "Volume deletion belongs only in the explicitly consented owned-evaluator reset section.")
    require("paste-safety", all(not has(b, r"(?m)^\s*set\s+-[^\n]*e[^\n]*$") or
            all(has(b[:m.start()], r"\(\s*$") for m in re.finditer(r"(?m)^\s*set\s+-[^\n]*e[^\n]*$", b)) for b in blocks(text + "\n" + context)),
            "Fail-fast shell setup belongs in a local subshell, not the interactive shell.")
    require("paste-clean", not has(all_code, r"(?m)^\s*#|\bstatus\s*="),
            "Pasteable commands must avoid comment lines and zsh's special status variable.")
    return errors


def markdown_links(text: str) -> list[str]:
    """Inline and reference-definition links; code samples do not become links."""
    clean = prose(text)
    return re.findall(r"(?<!!)\[[^\]]*\]\(\s*<?([^\s)>]+)>?(?:\s+['\"][^\n]*?)?\s*\)", clean) + re.findall(r"(?m)^\s*\[[^\]]+\]:\s*<?([^\s>]+)", clean)


def anchors(text: str) -> set[str]:
    result: set[str] = set()
    counts: dict[str, int] = {}
    for title in re.findall(r"(?m)^#{1,6}\s+(.+?)\s*#*\s*$", prose(text)):
        title = re.sub(r"\[([^]]+)\]\([^)]*\)", r"\1", title)
        slug = re.sub(r"[^\w\- ]", "", title.lower()).replace(" ", "-")
        n = counts.get(slug, 0)
        counts[slug] = n + 1
        result.add(slug + (f"-{n}" if n else ""))
    result.update(re.findall(r"(?:id|name)=[\"']([^\"']+)", text))
    return result


def check_links(documents: dict[str, str], exists=None) -> list[Finding]:
    errors: list[Finding] = []
    tutorials = [p for p, text in documents.items() if p != GUIDE and "tutorial" in p.lower()
                 and ("eval" in p.lower() or "compose.eval.yml" in text or "flowplane-eval" in text
                      or any(resolve_link(p, link)[0] == GUIDE for link in markdown_links(text)))]
    for source in ["README.md", *tutorials]:
        if not any(resolve_link(source, link)[0] == GUIDE for link in markdown_links(documents.get(source, ""))):
            errors.append(Finding("navigation", f"{source} must link to {GUIDE}."))
    if not tutorials:
        errors.append(Finding("navigation", "A public evaluation tutorial must link to the guide."))
    if GUIDE not in documents:
        return errors + [Finding("guide-present", f"Missing {GUIDE}.")]
    # Guide links are all in scope; README/tutorial links only if lifecycle-relevant.
    for source, text in documents.items():
        for link in markdown_links(text):
            target, fragment = resolve_link(source, link)
            if target is None:
                continue
            relevant = source == GUIDE or target == GUIDE or has(link, r"readiness|recovery|lifecycle|evaluation|tutorial")
            if not relevant:
                continue
            present = target in documents or (exists is not None and exists(target))
            if not present:
                errors.append(Finding("links", f"{source}: missing local target {link}."))
            elif fragment and target in documents and fragment not in anchors(documents[target]):
                errors.append(Finding("links", f"{source}: missing anchor {link}."))
    return errors


def resolve_link(source: str, link: str) -> tuple[str | None, str]:
    parsed = urlsplit(link)
    if parsed.scheme or parsed.netloc:
        return None, ""  # External links deliberately not network-checked.
    raw = unquote(parsed.path)
    base = PurePosixPath(source).parent if not raw.startswith("/") else PurePosixPath()
    parts: list[str] = []
    for part in (base / raw.lstrip("/")).parts if raw else PurePosixPath(source).parts:
        if part == "..":
            if not parts:
                return "__outside_repository__", unquote(parsed.fragment)
            parts.pop()
        elif part != ".":
            parts.append(part)
    return "/".join(parts), unquote(parsed.fragment)


SYNTHETIC = r'''# Evaluation lifecycle

## Readiness before exposure
Readiness is separate from explicit exposure. A fresh install has zero gateway APIs
and no sample response before exposure. Existing evaluations may contain user-authored resources.
The fp helper reads the token inside the container.
```sh
fp() { docker compose -f compose.eval.yml exec -T flowplane-eval sh -lc 'TOKEN=$(read-token); flowplane "$@"' sh "$@"; }
(
set -eu
ok=0
for attempt in $(seq 1 12); do
  if fp auth whoami; then ok=1; break; fi
  sleep 2
done
if [ "$ok" -ne 1 ]; then printf 'authentication failed\n' >&2; exit 1; fi
)
fp -o json listener list | jq '.data.items | length == 0'
fp -o json route list | jq '.data.items | length == 0'
fp -o json cluster list | jq '.data.items | length == 0'
```

## Preserve and resume
Compose down removes containers/network but keeps pgdata, shared, pki-cp, pki-dp.
Resume with the same directory, same project and same image. User-authored resources
remain. Setup may rerun validators/guards without reseeding.
```sh
docker compose -f compose.eval.yml down
docker compose -f compose.eval.yml up -d --no-build
```

## Recovery freshness
Remove flowplane-agent before replacing Envoy because of namespace sharing; recreate both.
Invoke this recovery helper AFTER disruption/resume. Archive both the post-disruption baseline and advanced read. Prior capture supplies expected identity/config only.
The dataplane identity is data.id. For authored product readbacks inspect data.id,
data.spec and data.revision. Use your own resource names, not assumed fresh resources.
```sh
expected_id=$(jq -r .data.id before-disruption.json)
eval_baseline_captured=false
recovered=0
for attempt in {1..30}; do
  if fp auth whoami && fp -o json dataplane get dp-eval > now.json; then
    if [ "$eval_baseline_captured" = false ]; then
      if python3 - "$expected_id" <<'PY'
import json, sys
b = json.load(open('now.json'))['data']
assert b['id'] == sys.argv[1] and b['last_heartbeat_at'] is not None
PY
      then cp now.json after-disruption-baseline.json; eval_baseline_captured=true; fi
    else
      if python3 - "$expected_id" <<'PY'
import json, sys
from datetime import datetime, timezone
parse = lambda value: datetime.strptime(value, '%Y-%m-%dT%H:%M:%S.%f%z' if '.' in value else '%Y-%m-%dT%H:%M:%S%z')
a = json.load(open('after-disruption-baseline.json'))['data']
b = json.load(open('now.json'))['data']
assert sys.argv[1] == a['id'] == b['id']
assert parse(b['last_heartbeat_at']) > parse(a['last_heartbeat_at'])
PY
      then recovered=1; break; fi
    fi
  fi
  sleep 2
done
if [ "$recovered" -ne 1 ]; then echo 'heartbeat timed out' >&2; exit 1; fi
fp -o json listener get my-listener
fp -o json route get my-route
fp -o json cluster get my-cluster
```

## Destructive reset evaluation
WARNING: destructive reset requires explicit consent for your own evaluator only.
It removes database identities/config and PKI/shared data. The next install has zero APIs.
```sh
docker compose -f compose.eval.yml down -v
docker compose -f compose.eval.yml up -d --no-build
fp -o json listener list | jq '.data.items | length == 0'
fp -o json route list | jq '.data.items | length == 0'
fp -o json cluster list | jq '.data.items | length == 0'
```

## Diagnostics
For setup/readiness/port bind failures use supported Compose/CP diagnostics.
Services: postgres, shared-init, pki, flowplane-eval, demo-upstream, init,
pki-client-verify, envoy, flowplane-agent and optional flowplane-dashboard.
One-shot Exited(0) is normal, not a failure. Do not use Envoy admin or plaintext bypass.
```sh
docker compose -f compose.eval.yml ps -a
docker compose -f compose.eval.yml logs --tail 100 init flowplane-eval envoy flowplane-agent
fp auth whoami
fp -o json dataplane get dp-eval
```

## Host ports
FLOWPLANE_EVAL_API_PORT changes only the host published REST port.
FLOWPLANE_EVAL_GATEWAY_PORT changes only the host published gateway port;
the container port remains 10000 and exposure still uses --port 10000.
The optional dashboard uses fixed host 8081; it cannot be arbitrarily remapped due to Host validation.

## Platform qualification and evidence gaps
Original Mac ARM and Linux separate-artifact qualification remains required.
Supporting evidence: macOS arm64 host / Podman Linux arm64 VM through docker-compose,
pre-version-bump local image only. Gaps, not qualified: published immutable 3.2.0,
Docker Desktop, Linux Docker host, remote CI, unfamiliar user. No universal resource
floor is established without measurements.

[Evaluation tutorial](../tutorials/evaluation.md#evaluation)
[README](../../README.md#flowplane)
'''


def synthetic_documents(guide: str = SYNTHETIC) -> dict[str, str]:
    return {GUIDE: guide,
            "README.md": f"# Flowplane\n[Readiness and recovery]({GUIDE}#readiness-before-exposure)\n",
            "docs/tutorials/evaluation.md": "# Evaluation\n[Readiness](../how-to/evaluation-readiness-and-recovery.md)\n"}


def self_test() -> int:
    cases: list[tuple[str, str | None, list[Finding], bool]] = []

    def positive(name: str, guide: str) -> None:
        cases.append((name, None, check_guide(guide) + check_links(synthetic_documents(guide)), True))

    def negative(name: str, rule: str, old: str, new: str) -> None:
        assert old in SYNTHETIC, f"Synthetic fixture mutation missing: {name}"
        changed = SYNTHETIC.replace(old, new)
        cases.append((name, rule, check_guide(changed), False))

    positive("complete acceptance fixture", SYNTHETIC)
    positive("Compose spelling and option order", SYNTHETIC.replace("docker compose", "docker-compose").replace("up -d --no-build", "up --no-build -d").replace("down -v", "down --volumes"))
    positive("equivalent bounded shell waits", SYNTHETIC.replace("$(seq 1 12)", "{1..12}").replace("{1..30}", "$(seq 30)").replace("printf 'authentication failed\\n'", "echo 'authentication failed'"))
    positive("arithmetic shell loop bounds", SYNTHETIC.replace("for attempt in $(seq 1 12); do", "for ((i=0; i<12; i++)); do").replace("for attempt in {1..30}; do", "for ((n=0; n<30; n++)); do"))
    positive("simple Compose helper", SYNTHETIC.replace("docker compose -f compose.eval.yml", "dc").replace("## Readiness before exposure", "## Readiness before exposure\n```sh\ndc() { docker compose -f compose.eval.yml \"$@\"; }\n```", 1).replace("fp() { dc", "fp() { docker compose -f compose.eval.yml"))
    negative("exposure is not readiness", "readiness-separate", "fp auth whoami; then", "fp expose sample --port 10000; then")
    negative("fresh lists missing", "fresh-empty", "fp -o json route list", "echo route-list-omitted")
    negative("auth wait unbounded", "auth-bounded", "for attempt in $(seq 1 12); do", "while true; do")
    negative("auth failure invisible", "auth-bounded", "printf 'authentication failed\\n' >&2; exit 1", "true")
    negative("heartbeat non-null is stale", "heartbeat-newer", "parse(b['last_heartbeat_at']) > parse(a['last_heartbeat_at'])", "b['last_heartbeat_at'] is not None")
    negative("heartbeat wait unbounded", "heartbeat-newer", "for attempt in {1..30}; do", "while true; do")
    negative("heartbeat failure invisible", "heartbeat-newer", "echo 'heartbeat timed out' >&2; exit 1", "true")
    negative("product identity omitted", "identity-readback", "data.revision", "revision-omitted")
    negative("resume deletes volumes", "preserve-safe", "down\ndocker", "down --volumes\ndocker")
    negative("resume blindly exposes", "preserve-safe", "## Recovery freshness", "```sh\nfp expose sample --port 10000\n```\n\n## Recovery freshness")
    negative("resume not same artifact", "preserve-resume", "same image", "a newly built image")
    negative("reset has no consent", "reset-consent", "explicit consent", "implicit permission")
    negative("reset missing empty readback", "reset-empty", "down -v\ndocker compose -f compose.eval.yml up -d --no-build", "down -v\n```\n\n## Other commands\n```sh\ndocker compose -f compose.eval.yml up -d --no-build")
    negative("unsupported admin diagnostics", "no-admin-bypass", "fp auth whoami\nfp -o json dataplane", "curl http://localhost:9901/config_dump\nfp auth whoami\nfp -o json dataplane")
    negative("TLS bypass", "no-admin-bypass", "fp auth whoami\nfp -o json dataplane", "curl --insecure https://localhost:8080\nfp auth whoami\nfp -o json dataplane")
    negative("plaintext xDS bypass", "no-admin-bypass", "fp auth whoami\nfp -o json dataplane", "curl http://localhost:50051\nfp auth whoami\nfp -o json dataplane")
    negative("automatic exposure retry", "no-mutation-retry", "  sleep 2\ndone\nif [ \"$recovered", "  fp expose sample --port 10000\n  sleep 2\ndone\nif [ \"$recovered")
    negative("diagnostics missing compose logs", "supported-diagnostics", "logs --tail 100", "version --tail 100")
    negative("agent replacement order omitted", "agent-first", "Remove flowplane-agent before replacing Envoy", "Replace Envoy first")
    negative("host port confused with exposure", "ports-distinct", "--port 10000", "--port $FLOWPLANE_EVAL_GATEWAY_PORT")
    negative("dashboard unconstrained", "ports-distinct", "fixed host 8081; it cannot be arbitrarily remapped due to Host validation", "freely remappable host port")
    negative("qualification overclaim", "platform-gaps", "Gaps, not qualified:", "All platforms are qualified:")
    negative("candidate gap missing", "platform-gaps", "published immutable 3.2.0", "candidate image")
    negative("global errexit", "paste-safety", "(\nset -eu", "set -eu")
    negative("zsh status assignment", "paste-clean", "ok=0", "status=0")
    negative("paste comments", "paste-clean", "ok=0", "# pasted comment\nok=0")
    negative("host-side helper", "token-inside", "flowplane-eval sh -lc", "flowplane-eval flowplane")
    for name, rule, change in (
        ("README navigation missing", "navigation", lambda d: d.update({"README.md": "# Flowplane\n"})),
        ("tutorial navigation missing", "navigation", lambda d: d.update({"docs/tutorials/evaluation.md": "# Evaluation\n"})),
        ("broken guide target", "links", lambda d: d.update({GUIDE: d[GUIDE] + "\n[Missing](missing.md)\n"})),
        ("broken guide anchor", "links", lambda d: d.update({GUIDE: d[GUIDE] + "\n[Bad](../../README.md#missing)\n"})),
        ("guide absent", "guide-present", lambda d: d.pop(GUIDE)),
    ):
        docs = synthetic_documents()
        change(docs)
        cases.append((name, rule, check_links(docs), False))
    docs = synthetic_documents()
    docs[GUIDE] += "\n[External](https://example.invalid/not-fetched)\n"
    cases.append(("external links remain offline", None, check_links(docs), True))
    docs = synthetic_documents()
    docs["docs/tutorials/unrelated.md"] = "# Unrelated public tutorial\nNo evaluation instructions here.\n"
    cases.append(("unrelated tutorials need no lifecycle link", None, check_links(docs), True))
    def paired(name: str, guide: str, rule: str, old: str, new: str,
               context: str = "") -> None:
        assert old in guide, name
        cases.append((name, None, check_guide(guide, context), True))
        cases.append((name + " regression", rule,
                      check_guide(guide.replace(old, new), context), False))

    headings = SYNTHETIC.replace("## Preserve and resume", "## Stop and resume without deleting state").replace(
        "## Recovery freshness", "## Capture the state you intend to retain").replace(
        "## Destructive reset evaluation", "## Deliberately reset only your disposable evaluation").replace(
        "## Diagnostics", "## Diagnose before changing state").replace(
        "## Platform qualification and evidence gaps", "## Qualification evidence and gaps")
    for title in ("Diagnose", "Readiness"):
        paired("peer sections below " + title + " H1", headings.replace("# Evaluation lifecycle", "# " + title),
               "preserve-safe", "down\ndocker", "down -v\ndocker")
    bracket = SYNTHETIC.replace("jq '.data.items | length == 0'", "python3 -c 'import json,sys; assert json.load(sys.stdin)[\"data\"][\"items\"] == []'").replace(
        "data.id", "['data']['id']").replace("data.spec", "['data']['spec']").replace("data.revision", "['data']['revision']").replace("jq -r .['data']['id']", "jq -r .data.id")
    paired("JSON bracket access", bracket, "identity-readback", "['data']['revision']", "['data']['omitted']")
    tuple_fields = bracket.replace("['data']['id'],\n['data']['spec'] and ['data']['revision']", "the fields extracted in the loop below").replace(
        "fp -o json listener get my-listener", "python3 - <<'PY'\nimport json\nd = json.load(open('resource.json'))['data']\nfor key in ('id', 'revision', 'spec'):\n    print(d[key])\nPY\nfp -o json listener get my-listener")
    paired("JSON tuple field extraction", tuple_fields, "identity-readback", "('id', 'revision', 'spec')", "('id', 'spec')")
    # Review correction: the old before.json/range fixture neither captured a
    # post-disruption baseline nor refreshed now.json. Keep the named Python
    # comparison/failure controls, with real per-attempt authenticated read text.
    python_heartbeat = SYNTHETIC.replace("{1..30}", "$(seq 1 30)")
    paired("Python newer-heartbeat comparison", python_heartbeat, "heartbeat-newer",
           "parse(b['last_heartbeat_at']) > parse(a['last_heartbeat_at'])", "b['last_heartbeat_at'] is not None")
    paired("Python heartbeat bounded failure", python_heartbeat, "heartbeat-newer", "$(seq 1 30)", "true")
    aliases = SYNTHETIC.replace("postgres,", "Postgres,").replace("Exited(0)", "Exited (0)").replace(
        "Host validation", "Host/Origin validation").replace("docker-compose,", "docker compose,").replace(
        "pre-version-bump local image", "pre-version-bump source image").replace("not qualified:", "not yet qualified:").replace(
        "Mac ARM and Linux separate-artifact", "separate native Mac ARM/Linux release artifacts").replace(
        "same directory, same project and same image", "same directory/project, with the same image").replace(
        "User-authored resources\nremain", "Existing gateway resources\nremain").replace(
        "Remove flowplane-agent before replacing Envoy", "Remove only the agent before replacing Envoy").replace(
        "expected_id=$(jq -r .data.id before-disruption.json)", "docker compose -f compose.eval.yml rm -sf flowplane-agent\nexpected_id=$(jq -r .data.id before-disruption.json)")
    paired("prose aliases and explicit agent removal", aliases, "agent-first", "rm -sf flowplane-agent", "rm -sf envoy")
    paired("unqualified alias", aliases.replace("not yet qualified", "unqualified"), "platform-gaps",
           "published immutable 3.2.0", "candidate image")
    intro = SYNTHETIC.replace("## Platform qualification and evidence gaps\n", "")
    paired("qualification as introductory prose", intro, "platform-gaps", "Gaps, not qualified:", "All platforms are qualified:")
    inventory = SYNTHETIC.replace("Services: postgres, shared-init, pki, flowplane-eval, demo-upstream, init,\npki-client-verify, envoy, flowplane-agent and optional flowplane-dashboard.\n", "Compose ps inventories every public service.\n")
    paired("diagnostic public inventory", inventory, "supported-diagnostics", "logs --tail 100 init flowplane-eval envoy flowplane-agent", "logs --tail 100 demo-upstream")
    error_only = SYNTHETIC.replace("docker compose -f compose.eval.yml down\n", "printf 'do not replay expose\\n'\ndocker compose -f compose.eval.yml down\n", 1)
    paired("exposure warning is not a mutation", error_only, "preserve-safe", "printf 'do not replay expose\\n'", "fp expose sample --port 10000")
    paired("direct volume deletion", error_only, "preserve-safe", "printf 'do not replay expose\\n'", "docker volume rm pgdata")
    paired("volume prune through executable shell", error_only, "preserve-safe", "printf 'do not replay expose\\n'", "docker volume prune -f")
    paired("retained named volume still required", aliases, "preserve-resume", "pki-dp", "unnamed-volume")
    paired("guarded non-reseed still required", aliases, "preserve-resume", "without reseeding", "with reseeding")
    initial = section(SYNTHETIC, r"Readiness before exposure")
    delegated = SYNTHETIC.replace(initial, "## Initial readiness\nReadiness is delegated to the [public initial tutorial](../tutorials/evaluation.md#evaluation).\n\n")
    delegated = delegated.replace("fp -o json listener list | jq '.data.items | length == 0'\nfp -o json route list | jq '.data.items | length == 0'\nfp -o json cluster list | jq '.data.items | length == 0'\n", "\n").replace("## Destructive reset evaluation\n", "## Destructive reset evaluation\nFollow the initial tutorial to reinstall and check fresh empty inventories.\n")
    paired("linked initial readiness and container helper", delegated, "reset-consent", "explicit consent", "implicit permission", initial)
    paired("delegation cannot hide destructive preserve", delegated, "preserve-safe", "down\ndocker", "down -v\ndocker", initial)
    cases.append(("delegated readiness needs context", "fresh-empty", check_guide(delegated), False))
    cases.append(("delegated authentication still bounded", "auth-bounded", check_guide(delegated, initial.replace("$(seq 1 12)", "true")), False))
    cases.append(("delegated inventories still empty", "fresh-empty", check_guide(delegated, initial.replace("length == 0", "length > 0")), False))
    for title in ("Diagnose", "Readiness"):
        exposed = headings.replace("# Evaluation lifecycle", "# " + title).replace(
            "## Stop and resume", "## Explicit optional exposure\n```sh\nfp expose my-api --port 10000\n```\n\n## Stop and resume")
        paired("H1 cannot absorb peer exposure " + title, exposed, "supported-diagnostics",
               "logs --tail 100", "version --tail 100")
    paired("Python heartbeat visible failure", python_heartbeat, "heartbeat-newer", "echo 'heartbeat timed out' >&2; exit 1", "true")
    paired("Python stale self-comparison", python_heartbeat, "heartbeat-newer",
           "parse(b['last_heartbeat_at']) >", "parse(a['last_heartbeat_at']) >")
    paired("JSON empty brackets must be empty", bracket, "fresh-empty", '["items"] == []', '["items"] != []')
    cases.append(("delegated helper remains container-side", "token-inside", check_guide(delegated, initial.replace("flowplane-eval sh -lc", "flowplane-eval flowplane")), False))
    cases.append(("delegated TLS bypass forbidden", "no-admin-bypass", check_guide(delegated, initial + "\n```sh\ncurl -k https://localhost:8080\n```\n"), False))
    delegated_docs = synthetic_documents(delegated)
    delegated_docs["README.md"] = f"# Flowplane\n[Recovery]({GUIDE})\n"
    delegated_docs["docs/tutorials/evaluation.md"] += initial
    cases.append(("linked documents supply initial context", None, check_document_guide(delegated_docs) + check_links(delegated_docs), True))
    delegated_docs[GUIDE] = delegated.replace("[public initial tutorial](../tutorials/evaluation.md#evaluation)", "public initial tutorial")
    cases.append(("unlinked tutorial cannot supply context", "fresh-empty", check_document_guide(delegated_docs), False))
    paired("unscoped logs cover public services", inventory.replace("logs --tail 100 init flowplane-eval envoy flowplane-agent", "logs --tail 100"),
           "supported-diagnostics", "logs --tail 100", "logs --tail 100 postgres")
    paired("public ps inventory is not service-filtered", inventory, "supported-diagnostics", "ps -a", "ps -a envoy")
    paired("executable exposure after warning", error_only, "preserve-safe", "printf 'do not replay expose\\n'", "printf 'do not replay expose\\n'; fp expose sample --port 10000")
    paired("executable volume deletion after warning", error_only, "preserve-safe", "printf 'do not replay expose\\n'", "printf 'do not replay expose\\n'; docker volume rm pgdata")
    initial_start = initial.replace("```sh\n", "```sh\ndocker compose -f compose.eval.yml up -d --no-build\n", 1)
    delegated_start = delegated.replace("down -v\ndocker compose -f compose.eval.yml up -d --no-build", "down -v")
    cases.append(("fresh reinstall commands can be delegated", None, check_guide(delegated_start, initial_start), True))
    cases.append(("delegated reset still needs reinstall", "reset-empty", check_guide(delegated_start, initial), False))
    cases.append(("wrapped exposure remains destructive", "preserve-safe", check_guide(error_only.replace("printf 'do not replay expose\\n'", "sh -c 'fp expose sample --port 10000'")), False))
    cases.append(("Podman volume removal remains destructive", "preserve-safe", check_guide(error_only.replace("printf 'do not replay expose\\n'", "podman volume rm pgdata")), False))
    # Review correction: prefixes now apply to the post-disruption baseline;
    # an evaluation-before.json comparison can no longer be a positive.
    python_prefixed = python_heartbeat.replace("after-disruption-baseline.json", "nested/evaluation-after-disruption-baseline.json").replace("now.json", "nested/evaluation-now.json")
    paired("prefixed baseline/current filenames", python_prefixed, "heartbeat-newer", " > parse(a[", " <= parse(a[")
    paired("exec shell errexit flag", SYNTHETIC.replace("sh -lc", "sh -ec"), "token-inside", "flowplane-eval sh -ec", "flowplane-eval flowplane")
    paired("unfamiliar developer alias", SYNTHETIC.replace("unfamiliar user", "unfamiliar developer"), "platform-gaps", "unfamiliar developer", "unexamined person")
    for title in ("Install infrastructure, not APIs",):
        tutorial_variant = initial.replace("Readiness before exposure", title)
        cases.append(("initial infrastructure heading alias", None, check_guide(delegated, tutorial_variant), True))
        cases.append(("initial infrastructure heading still bounded", "auth-bounded", check_guide(delegated, tutorial_variant.replace("for attempt in $(seq 1 12); do", "while true; do")), False))
    paired("visible did-not-become-ready diagnostic", SYNTHETIC.replace("authentication failed", "authentication did not become ready"), "auth-bounded", "authentication did not become ready", "starting")
    poll_only = SYNTHETIC + "\n## Explicit API exposure and bounded traffic reads\n```sh\nfp expose sample --port 10000\nfor attempt in $(seq 1 12); do\n  curl http://localhost:10000\ndone\n```\n"
    paired("one mutation outside read retry", poll_only, "no-mutation-retry", "  curl http://localhost:10000", "  fp expose sample --port 10000")
    # External-review regressions use synthetic contracts only.
    for name, old, new in (
        ("post-disruption baseline required", "eval_baseline_captured=false", "cp now.json after-disruption-baseline.json\neval_baseline_captured=true"),
        ("baseline capture required", "then cp now.json after-disruption-baseline.json; eval_baseline_captured=true", "then eval_baseline_captured=true"),
        ("same dataplane assertion required", "assert sys.argv[1] == a['id'] == b['id']", "pass"),
        ("authenticated post-disruption read", "fp auth whoami && fp -o json dataplane", "fp -o json dataplane"),
        ("parsed fractional timestamps", "parse(b['last_heartbeat_at']) > parse(a['last_heartbeat_at'])", "b['last_heartbeat_at'] > a['last_heartbeat_at']"),
        ("redirection is not comparison", "assert parse(b['last_heartbeat_at']) > parse(a['last_heartbeat_at'])", 'current=now; baseline=before; echo "$current" > "$baseline"'),
        ("invoke after disruption", "AFTER disruption/resume", "before disruption"),
        ("archive recovery evidence", "Archive both the post-disruption baseline and advanced read.", "Keep no recovery evidence."),
    ):
        paired(name, SYNTHETIC, "heartbeat-newer", old, new)
    for name, safe, unsafe in (
        ("until condition exposure", "until fp auth whoami; do sleep 1; done", "until fp expose sample; do sleep 1; done"),
        ("while condition exposure", "while ! fp auth whoami; do sleep 1; done", "while ! fp expose sample; do sleep 1; done"),
        ("direct flowplane loop", "for n in {1..2}; do flowplane auth whoami; done", "for n in {1..2}; do flowplane expose sample; done"),
        ("compose exec loop", "for n in {1..2}; do docker compose -f compose.eval.yml exec flowplane-eval flowplane auth whoami; done", "for n in {1..2}; do docker compose -f compose.eval.yml exec flowplane-eval flowplane expose sample; done"),
        ("nested loop shell", "for n in {1..2}; do sh -c 'fp auth whoami'; done", "for n in {1..2}; do sh -c 'fp expose sample'; done"),
    ):
        guide = SYNTHETIC + "\n## Optional reads\n```sh\n" + safe + "\n```\n"
        paired(name, guide, "no-mutation-retry", safe, unsafe)
    for name, safe, unsafe in (
        ("combined curl sk", "curl -s https://localhost:8080", "curl -sk https://localhost:8080"),
        ("combined curl fsSk", "curl -fsS https://localhost:8080", "curl -fsSk https://localhost:8080"),
        ("eval xDS plaintext 18000", "curl https://localhost:18000", "curl http://localhost:18000"),
        ("xDS plaintext 50051 paired", "curl https://localhost:50051", "curl http://localhost:50051"),
    ):
        paired(name, SYNTHETIC + "\n```sh\n" + safe + "\n```\n", "no-admin-bypass", safe, unsafe)
    for title in ("Diagnostics", "Host ports"):
        paired("destructive down outside reset " + title, SYNTHETIC, "destructive-scope",
               "## " + title, "## " + title + "\n```sh\ndocker compose -f compose.eval.yml down -v\n```\n")
    for spelling in ("docker compose -p eval -f compose.eval.yml", "docker compose --file compose.eval.yml", "podman compose -f compose.eval.yml"):
        guide = SYNTHETIC.replace("docker compose -f compose.eval.yml", spelling)
        paired("Compose variant " + spelling, guide, "preserve-safe", "down\n", "down -v\n")
    paired("multiline quoted shell", SYNTHETIC + "\n```sh\nprintf 'two\nlines'\n```\n",
           "unsupported-shell", "printf 'two\nlines'", "printf 'unterminated")
    paired("heredoc apostrophe", SYNTHETIC + "\n```sh\ncat <<'TEXT'\nit's literal text\nTEXT\n```\n",
           "unsupported-shell", "TEXT\n```", "MISSING\n```")
    paired("host token extraction", SYNTHETIC, "token-inside", "fp() {",
           "TOKEN=$(docker compose -f compose.eval.yml exec flowplane-eval cat token)\nfp() {")
    paired("auth bound belongs to auth loop", SYNTHETIC, "auth-bounded",
           "for attempt in $(seq 1 12); do", "for n in {1..12}; do true; done\nwhile true; do")
    paired("dynamic shell explicitly unsupported", SYNTHETIC + "\n```sh\nsh -c 'fp auth whoami'\n```\n",
           "unsupported-shell", "sh -c 'fp auth whoami'", 'eval "$command"')
    paired("container token command stays inside", SYNTHETIC.replace("TOKEN=$(read-token)", "TOKEN=$(cat /run/token)"),
           "token-inside", "fp() {", "TOKEN=$(docker compose -f compose.eval.yml exec flowplane-eval cat /run/token)\nfp() {")
    paired("multiline nested shell mutation", SYNTHETIC + "\n```sh\nfor n in {1..2}; do sh -c 'printf \"two\nlines\"'; done\n```\n",
           "no-mutation-retry", 'printf "two\nlines"', 'printf "two\nlines"; fp expose sample')
    paired("heredoc literal expose is inert", SYNTHETIC + "\n```sh\ncat <<'TEXT'\nit's not a command: fp expose sample\nTEXT\n```\n",
           "no-mutation-retry", "TEXT\n```", "TEXT\nfor n in {1..2}; do flowplane expose sample; done\n```")
    paired("nested dynamic shell needs review", SYNTHETIC + "\n```sh\nsh -c 'fp auth whoami'\n```\n",
           "unsupported-shell", "sh -c 'fp auth whoami'", 'sh -c \'eval "$command"\'')
    paired("unknown Compose options need review", SYNTHETIC + "\n```sh\ndocker compose -f compose.eval.yml ps\n```\n",
           "unsupported-shell", "docker compose -f compose.eval.yml ps\n```", "docker compose --mystery eval -f compose.eval.yml down -v\n```")
    paired("variable command needs review", SYNTHETIC + "\n```sh\nfp auth whoami\n```\n",
           "unsupported-shell", "fp auth whoami\n```", '$command auth whoami\n```')
    paired("expected ID comes from retained capture", SYNTHETIC, "heartbeat-newer",
           "expected_id=$(jq -r .data.id before-disruption.json)", "expected_id=dp-eval")
    portable = SYNTHETIC
    paired("portable strptime comparator", portable, "heartbeat-newer",
           "parse(b['last_heartbeat_at']) > parse(a['last_heartbeat_at'])", "b['last_heartbeat_at'] > a['last_heartbeat_at']")
    named = portable.replace("after-disruption-baseline.json", "post-disruption-baseline.json").replace(
        "a = json.load", "baseline = json.load").replace("b = json.load", "current = json.load").replace("a['", "baseline['").replace("b['", "current['")
    paired("baseline/current variable equivalence", named, "heartbeat-newer",
           "parse(current['last_heartbeat_at']) > parse(baseline['last_heartbeat_at'])", "parse(baseline['last_heartbeat_at']) > parse(baseline['last_heartbeat_at'])")
    source_image = SYNTHETIC.replace("pre-version-bump local image only", "source-built supporting image, not a published immutable 3.2.0 artifact")
    paired("source-built supporting image wording", source_image, "platform-gaps", "Gaps, not qualified:", "All platforms are qualified:")
    paired("supporting image cannot become published proof", source_image, "platform-gaps",
           "not a published immutable 3.2.0 artifact", "a published immutable 3.2.0 artifact")
    for label, value in (
        ("Mac ARM", "Mac ARM"), ("Linux", "Linux separate-artifact"),
        ("Docker Desktop", "Docker Desktop"), ("remote CI", "remote CI"),
        ("unfamiliar user", "unfamiliar user"),
    ):
        paired("source-built gaps retain " + label, source_image, "platform-gaps", value, "undisclosed environment")
    variant = SYNTHETIC.replace("fp auth whoami &&", "fp auth whoami >/dev/null 2>&1 &&").replace("; eval_baseline_captured=true", "\neval_baseline_captured=true")
    paired("auth redirections and multiline capture", variant, "heartbeat-newer", "eval_baseline_captured=false", "eval_baseline_captured=true")
    paired("literal Compose overlay", SYNTHETIC + "\n```sh\ndocker compose -f compose.eval.yml -f compose.host.yml ps -a\n```\n", "unsupported-shell", "-f compose.eval.yml -f compose.host.yml", "-f other.yml -f compose.host.yml")
    paired("multiline authenticated read conjunction", SYNTHETIC.replace("whoami && fp", "whoami &&\n fp"), "heartbeat-newer", "whoami &&\n fp", "whoami ||\n fp")
    manual_install = r'''# Install and verify Flowplane
A fresh install has no gateway APIs or sample response before exposure.
The helper reads the token inside the container.
```sh
fp() { docker compose -f compose.eval.yml exec -T flowplane-eval sh -ec 'flowplane "$@"' sh "$@"; }
docker compose -f compose.eval.yml up -d --no-build
```
### Inspect readiness
```sh
fp auth whoami
fp -o json dataplane get dp-eval
```
Expected: authenticated identity and organization information.
Inspect data.last_heartbeat_at: it must be non-null and recent.
Wait and run the same read again to see it advance.
If authentication fails or the timestamp remains missing/stale, stop before exposure.
```sh
docker compose -f compose.eval.yml logs flowplane-eval init flowplane-agent envoy
```
### Inspect the empty gateway
```sh
fp listener list
fp route list
fp cluster list
```
Expected: all three inventories are empty on a fresh installation.
If they are not, stop and inspect.
'''
    cases.append(("delegated split manual install", None, check_guide(delegated, manual_install), True))
    for name, rule, old, new in (
        ("real auth read", "auth-bounded", "fp auth whoami\n", "echo auth whoami\n"),
        ("real heartbeat read", "auth-bounded", "fp -o json dataplane get dp-eval", "echo dataplane get dp-eval"),
        ("auth expected outcome", "auth-bounded", "Expected: authenticated identity", "Identity may be absent"),
        ("recent nonnull heartbeat", "auth-bounded", "non-null and recent", "recorded previously"),
        ("advancing heartbeat inspection", "auth-bounded", "run the same read again to see it advance", "accept the old timestamp"),
        ("auth stop guidance", "auth-bounded", "stop before exposure", "continue regardless"),
        ("diagnostic reads", "auth-bounded", "docker compose -f compose.eval.yml logs", "echo logs"),
        ("real cluster inventory", "fresh-empty", "fp cluster list", "echo cluster list"),
        ("empty expected outcome", "fresh-empty", "all three inventories are empty", "inventories may contain APIs"),
        ("inventory stop guidance", "fresh-empty", "If they are not, stop and inspect", "Ignore nonempty inventories"),
    ):
        assert old in manual_install, name
        cases.append(("manual " + name + " positive", None, check_guide(delegated, manual_install), True))
        cases.append(("manual " + name + " negative", rule,
                      check_guide(delegated, manual_install.replace(old, new)), False))
    cases.append(("manual readiness never replaces recovery freshness", "heartbeat-newer",
                  check_guide(delegated.replace("parse(b['last_heartbeat_at']) > parse(a['last_heartbeat_at'])",
                                               "b['last_heartbeat_at'] is not None"), manual_install), False))
    cases.append(("manual readiness still requires reinstall", "reset-empty",
                  check_guide(delegated_start, manual_install.replace(
                      "docker compose -f compose.eval.yml up -d --no-build", "true")), False))
    # Resolve the direct install link, not an arbitrary aggregate of tutorials.
    split_names = ('eval-install-and-verify', 'eval-expose-first-api',
                   'eval-expose-own-backend', 'eval-local-rate-limit', 'eval-remove-apis')
    split_docs = synthetic_documents(delegated.replace(
        '../tutorials/evaluation.md#evaluation', '../tutorials/eval-install-and-verify.md'))
    split_docs['README.md'] = f'# Flowplane\n[Recovery]({GUIDE})\n'
    for name in split_names:
        split_docs['docs/tutorials/' + name + '.md'] = (
            manual_install if name == split_names[0] else '# Next step\n') + (
                '\n[Recovery](../how-to/evaluation-readiness-and-recovery.md)\n')
    cases.append(("five manual tutorials with direct initial link", None,
                  check_document_guide(split_docs) + check_links(split_docs), True))
    for name in split_names:
        path = 'docs/tutorials/' + name + '.md'
        bad = dict(split_docs)
        bad[path] = bad[path].replace('[Recovery](../how-to/evaluation-readiness-and-recovery.md)', '')
        cases.append((name + " recovery navigation missing", "navigation", check_links(bad), False))
    bad = dict(split_docs)
    bad.pop('docs/tutorials/eval-install-and-verify.md')
    cases.append(("missing direct install link target", "links", check_links(bad), False))
    cases.append(("unrelated tutorial cannot supply manual install", "fresh-empty",
                  check_document_guide(bad), False))
    failures = 0
    for name, rule, findings, expect_clean in cases:
        passed = not findings if expect_clean else any(f.rule == rule for f in findings)
        failures += not passed
        print(f"{PREFIX} {'PASS' if passed else 'FAIL'}: {name}")
        if not passed:
            print(f"{PREFIX} expected={'clean' if expect_clean else rule}; observed={[f.rule for f in findings]}")
    positives = sum(1 for _, _, _, clean in cases if clean)
    print(f"{PREFIX} self-test: {len(cases) - failures}/{len(cases)} passed; {positives} positive, {len(cases) - positives} negative; {failures} failed")
    return 1 if failures else 0


def check_document_guide(documents: dict[str, str]) -> list[Finding]:
    """Use only already-read public tutorials explicitly linked by readiness."""
    text = documents[GUIDE]
    ready = section(text, r"readiness|ready checks|preflight")
    for link in markdown_links(ready):
        target, _ = resolve_link(GUIDE, link)
        if target and target.startswith("docs/") and "tutorial" in target.lower() and target in documents:
            return check_guide(text, documents[target])
    return check_guide(text)


def check_repo(root: Path) -> int:
    root = root.resolve()
    documents: dict[str, str] = {}
    # Discovery is Markdown-only, restricted to public docs and root README.
    paths = [root / "README.md", root / GUIDE]
    docs_dir = root / "docs"
    if docs_dir.is_dir():
        paths.extend(p for p in docs_dir.rglob("*.md") if "tutorial" in p.relative_to(root).as_posix().lower())
    for path in dict.fromkeys(paths):
        if path.is_file() and path.resolve().is_relative_to(root):
            documents[path.relative_to(root).as_posix()] = path.read_text(encoding="utf-8")
    # Load only local Markdown link targets, for heading validation, not code.
    for _ in range(2):
        targets = {resolve_link(source, link)[0] for source, text in list(documents.items()) for link in markdown_links(text)}
        for target in targets:
            if target and target.endswith(".md") and target not in documents:
                path = root / target
                if path.is_file() and path.resolve().is_relative_to(root):
                    documents[target] = path.read_text(encoding="utf-8")

    def exists(target: str) -> bool:
        path = root / target
        return path.resolve().is_relative_to(root) and path.is_file()

    errors = check_links(documents, exists)
    if GUIDE in documents:
        errors.extend(check_document_guide(documents))
    for finding in errors:
        print(f"{PREFIX} REPO TEXT FAIL {finding.rule}: {finding.message}")
    print(f"{PREFIX} REPO TEXT: {len(errors)} findings; static contract lint only, no commands/platform/runtime qualified")
    return 1 if errors else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--self-test", action="store_true", help="in-memory synthetic paired acceptance tests")
    group.add_argument("--repo", type=Path, metavar="ROOT", help="read-only public Markdown contract lint")
    args = parser.parse_args()
    try:
        return self_test() if args.self_test else check_repo(args.repo)
    except (OSError, UnicodeError) as exc:
        print(f"{PREFIX} input failure: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
