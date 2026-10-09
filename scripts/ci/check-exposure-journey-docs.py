#!/usr/bin/env python3
"""Independent S4 static guard. Never runs tutorial commands or proves runtime success.

Only --self-test is exercised by the independent author. --repo reads the four
public documentation targets after the producer has finished; human confirmation
and real sample/own-backend/429/recovery execution remain separate gates.
"""
import argparse
import os
from pathlib import Path
import re
import subprocess
import tempfile
from urllib.parse import unquote, urlsplit

FILES = ('README.md', 'docs/tutorials/evaluate-no-clone.md',
         'docs/how-to/expose-an-api.md', 'docs/reference/cli.md')
FLAGS = re.M | re.I


def markdown(text):
    """Return prose and (language, body) fences; code is excluded from links."""
    blocks, prose, active, body = [], [], None, []
    for line in text.splitlines():
        fence = re.match(r'^\s{0,3}(`{3,}|~{3,})([^\s]*)\s*$', line)
        if active is None and fence:
            active = (fence[1][0], len(fence[1]), fence[2].lower())
            body = []
        elif active and re.match(r'^\s{0,3}' + re.escape(active[0])
                                 + '{' + str(active[1]) + r',}\s*$', line):
            blocks.append((active[2], '\n'.join(body)))
            active = None
        elif active:
            body.append(line)
        else:
            prose.append(line)
    if active:
        raise ValueError('unterminated Markdown fence')
    return re.sub(r'(`+).*?\1', '', '\n'.join(prose)), blocks


def link_errors(root, relative, prose):
    refs = dict(re.findall(r'^\s*\[([^]]+)\]:\s*(\S+)', prose, re.M))
    destinations = re.findall(r'!?\[[^]]*\]\(\s*(<[^>]*>|[^\s)]+)', prose)
    destinations += [refs[name] for name in
                     re.findall(r'\[[^]]*\]\[([^]]+)\]', prose) if name in refs]
    errors = []
    for destination in destinations:
        target = urlsplit(destination.strip('<>'))
        if target.scheme or target.netloc or not target.path or target.path.startswith('/'):
            continue  # External/root URLs and local anchors are not file links.
        path = (root / relative).parent / unquote(target.path)
        if not path.is_file():
            errors.append(f'{relative}: missing linked file {target.path}')
    return errors


def journey(text, blocks):
    """Contract heuristics, deliberately independent of headings/resource names."""
    shell = '\n'.join(body for lang, body in blocks if lang in ('sh', 'bash'))
    shell = re.sub(r'\\\n\s*', ' ', shell)
    code = '\n'.join(body for _, body in blocks)
    prose = text.lower()
    errors = []

    def require(key, condition):
        if not condition:
            errors.append('tutorial: ' + key)

    def match(pattern, value=shell):
        return re.search(pattern, value, FLAGS)

    helper = re.search(r'fp\s*\(\s*\)\s*\{(.*?)^\}', shell, re.M | re.S)
    h = helper[1] if helper else ''
    require('posix-helper', helper and h.count('"$@"') >= 2
            and not match(r'\[\[|function\s+fp', h))
    inner = re.search(r"\bexec\s+-T\b.*?\bsh\s+-(?:ec|c)\s+'([^']+)'", h, re.S)
    context = inner[1] if inner else ''
    require('container-token', inner and all(
            match(r'--' + key + r'\s+|FLOWPLANE_' + key.upper() + r'\s*=', context)
            for key in ('server', 'org', 'team', 'token'))
            and match(r'(?:--token\s+|FLOWPLANE_TOKEN\s*=)"\$\(\s*cat\s+[^)]+\)"', context))
    expose = match(r'^\s*fp\b[^\n]*\bexpose\s+(?:http://)?demo-upstream:5678\b[^\n]*--port\s+10000\b')
    require('explicit-sample', expose)
    first = expose.start() if expose else 0
    before = shell[:first]
    require('ready-before-expose', match(r'\bauth\s+whoami\b', before)
            and match(r'\bdataplane\s+get\b', before) and 'last_heartbeat_at' in before)
    require('empty-before-expose', all(match(r'\b' + resource + r'\s+list\b', before)
            for resource in ('listener', 'route', 'cluster'))
            and bool(match(r'(length\s*==\s*0|==\s*\[\]|-eq\s+0)', before)))
    traffic = list(re.finditer(r'\bcurl\b[^\n]*', shell))
    sample = [m for m in traffic if re.search(r'(127\.0\.0\.1|localhost):10000', m[0])]
    refusals = list(re.finditer(r'\bif\s+curl\b[^;\n]+;\s*then\b'
                    r'(?:(?!\b(?:else|fi)\b)[\s\S])*?(?:^|;)\s*false\s*(?:;|\n)\s*fi\b', shell, re.M))
    def positive(request):
        return re.search(r'(?:\s-[A-Za-z]*f[A-Za-z]*\b|--fail\b)', request[0])
    require('first-positive-after-expose', sample and expose
            and all(m.start() > first or not positive(m)
                    or any(g.start() <= m.start() < g.end() for g in refusals) for m in sample)
            and any(m.start() > first and positive(m) for m in sample))
    own = match(r'^\s*fp\b[^\n]*\bexpose\s+(?!http://demo-upstream|demo-upstream)(\S+)[^\n]*--path\s+(/[^\s]+)[^\n]*--listener\s+\S+')
    require('own-backend-attachment', own and not re.search(r'[<>]', own[1])
            and any(own[2] in m[0] for m in traffic))
    require('network-caveat', 'host.docker.internal' in prose and 'linux' in prose
            and 'host-gateway' in prose and '127.0.0.1' in prose
            and bool(re.search(r'(reach|network|container)', prose)))
    update = match(r'\bfp\b[^\n]*--revision\s+\S+[^\n]*\b(?:listener|route)\s+update\b[^\n]*(?:--file|-f)\s+\S+')
    if not update:
        update = match(r'\bfp\b[^\n]*\b(?:listener|route)\s+update\b[^\n]*--revision\s+\S+[^\n]*(?:--file|-f)\s+\S+')
    require('revision-local-policy', update and 'local_rate_limit' in code
            and bool(match(r'\b(?:listener|route)\s+get\b')) and 'revision' in shell)
    # Printed failures are valid in local, interactive-safe subshells too.
    printed_failure = r'(?:printf|echo)\s+[^{}\n]*>&2;\s*exit\s+1'
    failure_guard = r'(?:false|\{\s*' + printed_failure + r';\s*\})'

    def status_assertions(status):
        assertions = []
        pattern = r'(?:^|;)\s*(?:test|\[)\s+"\$([A-Za-z_]\w*)"\s+=\s+' + status + r'\b[ \t]*(?:\][ \t]*)?(?:\|\|[ \t]*' + failure_guard + r'[ \t]*)?(?:;|$)'
        for assertion in re.finditer(pattern, shell, re.M):
            if match(r'(?:^|;)\s*' + assertion[1] + r'=\$\(curl\b[^\n]*%\{http_code\}', shell[:assertion.start()]):
                assertions.append(assertion)
        return assertions
    denied = status_assertions('429')
    cases = re.finditer(r'\bcase\s+"\$([A-Za-z_]\w*)"\s+in\s+200\)\s*;;\s*'
                       r'429\)\s*(\w+)=true;\s*break\s*;;\s*\*\)\s*(?:false;\s*break|'
                       + printed_failure + r')\s*;?\s*;;\s*esac', shell)
    for case in cases:
        if (match(r'(?:^|;)\s*' + case[1] + r'=\$\(curl\b[^\n]*%\{http_code\}', shell[:case.start()])
                and match(r'^\s*(?:done\s+)?\[\s+"\$' + case[2] + r'"\s+=\s+true\s*\]\s*\|\|\s*' + failure_guard + r'[ \t]*(?:\n|$)', shell[case.end():])):
            denied.append(case)
    def refill_body_asserted(assertion):
        requests = list(re.finditer(r'(?:^|;)\s*' + assertion[1]
                        + r'=\$\(curl\b[^\n]*', shell[:assertion.start()], re.M))
        request = requests[-1][0] if requests else ''
        after = shell[assertion.end():]
        # Literal paths must be the same curl output and cat input. Limit this
        # static grammar to inert shell words, not expansions or cat options.
        filename = r'[A-Za-z_./][A-Za-z0-9_./-]*'
        literal = re.search(r'\s-o\s+(?:(' + filename + r')|\'(' + filename
                            + r')\'|"(' + filename + r')")(?=\s)', request)
        path = next((part for part in literal.groups() if part), '') if literal else ''
        if path and path != '/dev/null':
            same_path = re.escape(path)
            cat_path = r'(?:' + same_path + r'|\'' + same_path + r'\'|"' + same_path + r'")'
            return bool(re.search(r'^\s*\[\s+"\$\(\s*cat\s+' + cat_path
                        + r'\s*\)"\s+=\s+\'[^\'\n]+\'\s+\]\s*\|\|\s*'
                        + failure_guard + r'[ \t]*(?:\n|$)', after, re.M))
        if not re.search(printed_failure, assertion[0]):
            return True  # Preserve the existing direct status-only forms.
        output = re.search(r'\s-o\s+"\$([A-Za-z_]\w*)"', request)
        return bool(output and match(r'^\s*grep\s+-q\s+[\'\"][^\'\"\n]+[\'\"]\s+"\$'
                    + output[1] + r'"\s*\|\|\s*' + failure_guard + r'[ \t]*(?:\n|$)', after))

    recovery = any(match(r'(?:^|;)\s*sleep\s+\S+\s*(?:;|\n)\s*' + a[1]
                         + r'=\$\(curl\b[^\n]*%\{http_code\}', shell[d.end():a.start()])
                   and refill_body_asserted(a)
                   for d in denied for a in status_assertions('200') if a.start() > d.end())
    require('assert-429-recovery', recovery)
    require('safe-unexpose', match(r'^\s*fp\b(?=[^\n]*\bunexpose\s+\S+)(?=[^\n]*\s(?:--yes|-y)(?:\s|$))')
            and all(word in prose for word in ('retained', 'policy', 'managed'))
            and bool(re.search(r'(delete|cleanup)', prose)))
    # Traffic curl may carry its own credential, never a management bearer.
    token_vars = set(re.findall(r'\b([A-Z_][A-Z_0-9]*)\s*=\s*.*(?:cat|auth token)', shell))
    token_vars.update(('FLOWPLANE_TOKEN', 'MANAGEMENT_TOKEN', 'MGMT_TOKEN'))
    unsafe = any(re.search(r'authorization\s*:\s*bearer', m[0], re.I)
                 and any(re.search(r'\$\{?' + re.escape(v) + r'\b', m[0]) for v in token_vars)
                 for m in sample)
    require('separate-auth', not unsafe and 'management' in prose
            and bool(re.search(r'(traffic|data.plane)', prose))
            and bool(re.search(r'(separate|not .*token|not .*bearer)', prose)))
    require('optional-continuations', all(re.search(r'optional[^.\n]*' + word
            + r'|' + word + r'[^.\n]*optional', prose) for word in ('dashboard', 'mcp')))
    # Tie publication caution to the versioned images in the same sentence;
    # an unpublished *port*, or a caution about another release, is unrelated.
    release_sentences = re.split(r'[.!?](?:\s|$)', prose)
    require('unpublished-release', any(
            re.search(r'(?<![\w.])v?3\.2\.0(?![\w.])', sentence)
            and re.search(r'\b(?:images?\s+(?:(?:is|are|remain)\s+)?'
                          r'(?:unpublished|not (?:yet )?published)'
                          r'|images?\s+(?:has|have)\s+not\s+(?:yet\s+)?been\s+published'
                          r'|unpublished\s+(?:container\s+)?images?)\b', sentence)
            for sentence in release_sentences))
    return errors


def check(root):
    errors = []
    for relative in FILES:
        try:
            text = (root / relative).read_text(encoding='utf-8')
            prose, blocks = markdown(text)
        except (OSError, ValueError) as exc:
            errors.append(f'{relative}: {exc}')
            continue
        errors.extend(link_errors(root, relative, prose))
        for index, (lang, body) in enumerate(blocks, 1):
            if lang in ('sh', 'bash'):
                result = subprocess.run(['sh', '-n'], input=body, text=True,
                                        capture_output=True, check=False)
                if result.returncode:
                    errors.append(f'{relative}: shell block {index}: {result.stderr.strip()}')
        if relative == FILES[1]:
            errors.extend(journey(text, blocks))
    return errors


FIXTURE = '''Matching v3.2.0 container images are not yet published. Dashboard optional. MCP optional.
Management authentication is separate from data-plane traffic authentication.
Containers cannot reach host 127.0.0.1; use host.docker.internal on macOS;
Linux needs host-gateway mapping. Final managed cleanup deletes policy edits;
shared infrastructure is retained.
```sh
fp() {
  docker compose exec -T cp sh -c 'flowplane --server http://cp:8080 --org org --team team --token "$(cat /run/token)" "$@"' sh "$@"
}
fp auth whoami
fp -o json dataplane get dp-eval | jq -e '.data.last_heartbeat_at != null'
fp listener list | jq -e '.data | length == 0'
fp route list | jq -e '.data | length == 0'
fp cluster list | jq -e '.data | length == 0'
fp expose demo-upstream:5678 --name sample --port 10000
curl -fsS http://127.0.0.1:10000/
fp expose http://host.docker.internal:8081 --name own --path /health --listener sample
curl -fsS http://127.0.0.1:10000/health
revision=$(fp listener get sample | jq -r '.data.revision')
cat > policy.json <<'JSON'
{"spec":{"filters":[{"type":"local_rate_limit"}]}}
JSON
fp --revision "$revision" listener update sample --file policy.json
code=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:10000/)
test "$code" = 429
sleep 2
code=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:10000/)
test "$code" = 200
fp unexpose own --yes
```
'''


def self_test():
    mutations = [
        ('posix-helper', '"$@"', '$@'),
        ('container-token', 'exec -T', 'exec'),
        ('explicit-sample', '--port 10000', '--port 10001'),
        ('ready-before-expose', 'fp auth whoami', 'true'),
        ('empty-before-expose', 'fp cluster list', 'fp cluster get other'),
        ('first-positive-after-expose', 'fp auth whoami', 'curl -fsS http://127.0.0.1:10000/\nfp auth whoami'),
        ('own-backend-attachment', '--listener sample', '--port 10001'),
        ('own-backend-attachment', '/health', '/missing', 1),
        ('network-caveat', 'host-gateway', 'mapping'),
        ('revision-local-policy', '--revision "$revision"', ''),
        ('revision-local-policy', 'local_rate_limit', 'jwt'),
        ('assert-429-recovery', 'test "$code" = 429', 'echo 429'),
        ('assert-429-recovery', 'test "$code" = 200', 'echo 200'),
        ('safe-unexpose', 'unexpose own --yes', 'unexpose own'),
        ('separate-auth', 'curl -fsS http://127.0.0.1:10000/', 'curl -fsS -H "Authorization: Bearer $MANAGEMENT_TOKEN" http://127.0.0.1:10000/'),
        ('optional-continuations', 'MCP optional', 'MCP required'),
        ('unpublished-release', 'not yet published', 'published'),
        ('unpublished-release', 'Matching v3.2.0 container images are not yet published.', 'v3.2.0 images are available with an unpublished container port.'),
        ('unpublished-release', 'Matching v3.2.0 container images are not yet published.', 'v3.2.0 uses an unpublished container port.'),
        ('unpublished-release', 'Matching v3.2.0 container images are not yet published.', 'v3.2.0 is available. Matching v3.1.0 container images are not yet published.'),
        ('unpublished-release', 'Matching v3.2.0 container images are not yet published.', 'v3.2.0 is available. An unrelated image is unpublished.'),
    ]
    env_helper = '''docker compose exec -T cp sh -ec '
    export FLOWPLANE_SERVER=http://cp:8080 FLOWPLANE_ORG=org FLOWPLANE_TEAM=team
    export FLOWPLANE_TOKEN="$(cat /run/token)"
    flowplane "$@"' sh "$@"'''
    flag_helper = FIXTURE.splitlines()[7].strip()
    refusal = '''if curl --max-time 3 -fsS http://127.0.0.1:10000/; then
  echo unexpected-success-error >&2
  false
fi
fp auth whoami'''
    status_case = '''saw_429=false
for attempt in 1 2 3; do
  status=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:10000/)
  case "$status" in
    200) ;;
    429) saw_429=true; break ;;
    *) false; break ;;
  esac
done
[ "$saw_429" = true ] || false'''
    interactive_ready = '''(
set -eu
ready=false
for attempt in 1 2 3; do
  if fp auth whoami && fp -o json dataplane get dp-eval | jq -e '.data.last_heartbeat_at != null'; then
    ready=true
    break
  fi
  sleep 1
done
[ "$ready" = true ] || { printf '%s\\n' 'authenticated heartbeat readiness timed out' >&2; exit 1; }
)'''
    interactive_rate = '''(
set -eu
refill_body=$(mktemp)
trap 'rm -f "$refill_body"' EXIT
observed429=false
for attempt in 1 2 3; do
  status=$(curl --max-time 3 -sS -o /dev/null -w '%{http_code}' http://127.0.0.1:10000/)
  case "$status" in
    200) ;;
    429) observed429=true; break ;;
    *) printf '%s\\n' 'unexpected traffic status' >&2; exit 1 ;;
  esac
done
[ "$observed429" = true ] || { printf '%s\\n' 'no 429 observed' >&2; exit 1; }
sleep 2
refill_status=$(curl --max-time 3 -sS -o "$refill_body" -w '%{http_code}' http://127.0.0.1:10000/)
[ "$refill_status" = 200 ] || { printf '%s\\n' 'refill did not recover' >&2; exit 1; }
grep -q 'demo' "$refill_body" || { printf '%s\\n' 'missing refill response body' >&2; exit 1; }
)'''
    ready_lines = "fp auth whoami\nfp -o json dataplane get dp-eval | jq -e '.data.last_heartbeat_at != null'"
    rate_lines = FIXTURE[FIXTURE.index('code=$(curl'):FIXTURE.index('fp unexpose')].rstrip()
    interactive = FIXTURE.replace(ready_lines, interactive_ready).replace(rate_lines, interactive_rate)
    literal_rate = interactive_rate.replace('refill_body=$(mktemp)\ntrap \'rm -f "$refill_body"\' EXIT\n', '').replace(
        '-o "$refill_body"', '-o recovery-body.txt').replace(
        'grep -q \'demo\' "$refill_body"', '[ "$(cat recovery-body.txt)" = \'expected-body\' ]')
    literal = FIXTURE.replace(rate_lines, literal_rate)
    literal_body = '[ "$(cat recovery-body.txt)" = \'expected-body\' ]'
    literal_status = '[ "$refill_status" = 200 ]'
    literal_pairs = [
        ('literal-body-missing', literal_body, "echo 'expected-body'"),
        ('literal-body-wrong-file', 'cat recovery-body.txt', 'cat unrelated-body.txt'),
        ('literal-body-file-case', 'cat recovery-body.txt', 'cat Recovery-body.txt'),
        ('literal-status-constant', literal_status, '[ "200" = 200 ]'),
        ('literal-status-echo', literal_status, 'echo 200'),
        ('literal-status-fabricated', 'refill_status=$(curl --max-time 3 -sS -o recovery-body.txt -w \'%{http_code}\' http://127.0.0.1:10000/)', 'refill_status=$(echo 200)'),
        ('literal-body-suppressed', "'missing refill response body' >&2; exit 1", "'missing refill response body' >&2; exit 0"),
        ('literal-status-suppressed', "'refill did not recover' >&2; exit 1", "'refill did not recover' >&2; exit 0"),
        ('literal-body-or-true', "'missing refill response body' >&2; exit 1; }", "'missing refill response body' >&2; exit 1; } || true"),
        ('literal-status-or-true', "'refill did not recover' >&2; exit 1; }", "'refill did not recover' >&2; exit 1; } || true"),
        ('literal-unrelated-sleep', 'sleep 2\nrefill_status=', 'refill_status='),
    ]
    variants = [
        ('interactive-ready-auth', interactive, 'ready-before-expose', 'fp auth whoami &&', 'true &&'),
        ('interactive-ready-heartbeat', interactive, 'ready-before-expose', 'last_heartbeat_at', 'irrelevant_field'),
        ('interactive-observed429', interactive, 'assert-429-recovery', '[ "$observed429" = true ]', 'echo 429'),
        ('interactive-refill-status', interactive, 'assert-429-recovery', '[ "$refill_status" = 200 ]', 'echo 200'),
        ('interactive-refill-body', interactive, 'assert-429-recovery', "grep -q 'demo' \"$refill_body\"", "echo 'demo'"),
        ('interactive-case-failure', interactive, 'assert-429-recovery', "'unexpected traffic status' >&2; exit 1", "'unexpected traffic status' >&2; exit 0"),
        ('image-unpublished', FIXTURE.replace('not yet published', 'unpublished'), 'unpublished-release', 'container images', 'container port'),
        ('image-not-published', FIXTURE.replace('not yet published', 'not published'), 'unpublished-release', 'v3.2.0', 'v3.1.0'),
        ('ec-flags', FIXTURE.replace('sh -c', 'sh -ec'), 'container-token', 'exec -T', 'exec'),
        ('env-context', FIXTURE.replace(flag_helper, env_helper), 'container-token', 'FLOWPLANE_TEAM=team', ''),
        ('env-token', FIXTURE.replace(flag_helper, env_helper), 'container-token', '$(cat /run/token)', '$HOST_TOKEN'),
        ('env-args', FIXTURE.replace(flag_helper, env_helper), 'posix-helper', '"$@"', '$@'),
        ('guarded-refusal', FIXTURE.replace('fp auth whoami', refusal), 'first-positive-after-expose', '  false', '  true'),
        ('prefix-yes', FIXTURE.replace('fp unexpose own --yes', 'fp --yes unexpose own'), 'safe-unexpose', '--yes unexpose', 'unexpose'),
        ('suffix-y', FIXTURE.replace('unexpose own --yes', 'unexpose own -y'), 'safe-unexpose', 'own -y', 'own'),
        ('multiline-case', FIXTURE.replace('test "$code" = 429', status_case), 'assert-429-recovery', '[ "$saw_429" = true ] || false', 'echo 429'),
        ('semicolon-recovery', FIXTURE.replace('fp auth whoami', 'sleep 1\nfp auth whoami').replace('test "$code" = 429', status_case).replace('sleep 2\ncode=$(curl', 'sleep 6; status=$(curl').replace('test "$code" = 200', '[ "$status" = 200 ]'), 'assert-429-recovery', '[ "$status" = 200 ]', 'echo 200'),
        ('case-default', FIXTURE.replace('test "$code" = 429', status_case), 'assert-429-recovery', '*) false;', '*) true;'),
        ('guard-unguarded', FIXTURE.replace('fp auth whoami', refusal), 'first-positive-after-expose', refusal, 'curl --max-time 3 -fsS http://127.0.0.1:10000/\nfp auth whoami'),
        ('earlier-sleep', FIXTURE.replace('fp auth whoami', 'sleep 1\nfp auth whoami').replace('sleep 2', 'sleep 6').replace('code=$(curl', 'status=$(curl').replace('"$code"', '"$status"'), 'assert-429-recovery', 'test "$status" = 200', 'echo 200'),
    ]
    # Keep unrelated readiness sleep present when deleting the refill sleep.
    literal = literal.replace('fp auth whoami', 'sleep 1\nfp auth whoami')
    variants += [(name, literal, 'assert-429-recovery', old, new)
                 for name, old, new in literal_pairs]
    for quote in ("'", '"'):
        quoted = literal.replace('-o recovery-body.txt', '-o ' + quote + 'recovery-body.txt' + quote).replace(
            'cat recovery-body.txt', 'cat ' + quote + 'recovery-body.txt' + quote)
        variants.append(('literal-quoted-' + repr(quote), quoted, 'assert-429-recovery',
                         'cat ' + quote + 'recovery-body.txt' + quote,
                         'cat ' + quote + 'unrelated-body.txt' + quote))
    mutations += [
        ('assert-429-recovery', 'test "$code" = 429', 'test "429" = 429'),
        ('assert-429-recovery', 'test "$code" = 429', '# test "$code" = 429'),
        ('assert-429-recovery', 'test "$code" = 429', 'echo \'test "$code" = 429\''),
        ('assert-429-recovery', 'test "$code" = 200', 'test "200" = 200'),
        ('assert-429-recovery', 'test "$code" = 429', 'test "$code" = 429 || true'),
        ('assert-429-recovery', 'sleep 2', 'echo sleep 2'),
    ]
    with tempfile.TemporaryDirectory(prefix='exposure-docs-', dir=os.environ.get('TMPDIR')) as directory:
        # None invokes stdlib selection (including TEMP); explicit TMPDIR wins.
        assert Path(directory).parent == Path(os.environ.get('TMPDIR') or tempfile.gettempdir())
        root = Path(directory)
        for relative in FILES:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(FIXTURE if relative == FILES[1] else '# Contract\n')
        tutorial = root / FILES[1]
        assert not check(root), check(root)
        print('PASS synthetic positive contract')
        for name, fixture, key, old, new in variants:
            tutorial.write_text(fixture)
            assert not check(root), (name, check(root))
            assert old in fixture, name
            tutorial.write_text(fixture.replace(old, new))
            assert 'tutorial: ' + key in check(root), (name, check(root))
        print(f'PASS {len(variants)} paired syntax variants')
        for key, old, new, *count in mutations:
            assert old in FIXTURE
            tutorial.write_text(FIXTURE.replace(old, new, count[0] if count else -1))
            assert 'tutorial: ' + key in check(root), (key, check(root))
        tutorial.write_text(FIXTURE)
        readme = root / FILES[0]
        readme.write_text('[ok](docs/reference/cli.md#anchor) ` [ignored](absent) `\n'
                          '```sh\n# [ignored](absent)\n```\n[web](https://example.invalid)\n')
        assert not check(root), check(root)
        readme.write_text('[broken](absent.md)\n[ref][bad]\n[bad]: missing.md\n')
        assert len([e for e in check(root) if 'missing linked file' in e]) == 2
        readme.write_text('```bash\nif then\n```\n')
        assert any('shell block' in e for e in check(root))
        readme.write_text('```sh\ntrue\n')
        assert any('unterminated' in e for e in check(root))
        print(f'PASS {len(mutations)} semantic negative mutations')
        print('PASS link/code exclusions, 2 broken links, shell syntax, unterminated fence')
        print('Static/synthetic only; no tutorial commands executed or runtime claims made.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, help='repository root to inspect')
    parser.add_argument('--self-test', action='store_true', help='synthetic fixtures only')
    args = parser.parse_args()
    if args.self_test:
        self_test()
    elif args.repo:
        failures = check(args.repo.resolve())
        print('\n'.join(failures) if failures else 'PASS static documentation contracts (not runtime proof)')
        raise SystemExit(bool(failures))
    else:
        parser.error('choose --repo ROOT or --self-test')
