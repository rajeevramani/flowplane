# 12 — v2 CLI Design

The CLI is the human's native surface (benchmark: `gh`/`kubectl` feel). This design fixes every
item in spec/07 §3/§6 and follows D-005 precedence. Binary: `flowplane` (client subcommands of
the same binary that runs the server).

## 1. Design rules (binding)

1. **Noun-verb grammar**, ≤ 12 top-level nouns. Verbs are uniform: `list|get|create|update|
   delete` plus noun-specific transitions. No bare top-level verbs except `version` and
   `completion`.
2. **Every command supports `-o/--output table|json|yaml|wide`** (default `table` on TTY,
   `json` when piped). `-o` ALWAYS means output format; file output is ALWAYS `--file/-f` is
   input and `--out <path>` is file output. `--json` is sugar for `-o json`. Mutations print a
   one-line confirmation on TTY and the full resource as JSON with `-o json` — everything is
   scriptable.
3. **`--dry-run` on every mutating command** (server-side validation + would-be result, no
   write). Destructive commands (`delete`, `--cascade`, `publish` of discovered specs) prompt
   on TTY; `--yes/-y` for scripts.
4. **One addressing scheme**: resources addressed by name (per-team unique); UUID accepted
   anywhere a name is (`@id:` prefix not needed — server resolves both). Parent scoping via
   flags (`--api`, `--route-config`), never positional ambiguity.
5. **Revisions**: `update`/`delete` accept `--revision N` (maps to If-Match); without it, the
   CLI does read-modify-write and fails with a clear 409 message on race.
6. **Errors** (§4): server's `{code, message, hint, request_id}` rendered as
   `error (CODE): message` + `→ hint` + `request id`; exit codes 0/1/2/3/4/5/6/7 per spec/10 §8.
7. **Config precedence (D-005)**: flag > env (`FLOWPLANE_*`) > `~/.flowplane/config.toml` >
   default — uniformly for every value. `flowplane config` manages the file; contexts
   (`--context`, named server+org+team tuples) like kubeconfig. `--org` is the human-facing
   active-org selector; the CLI sends it to REST/MCP as `X-Flowplane-Org`.
8. **Completions**: `flowplane completion bash|zsh|fish` (clap_complete), including dynamic
   resource-name completion via the API when authenticated.
9. **Help**: every command has `--help` with one-line summary, examples, and related commands;
   `flowplane help <topic>` for workflows (learning, ai, tenancy).

## 2. Command tree

```
flowplane
├── auth        login | logout | whoami | token
├── config      init | show | set | unset | path | use-context | get-contexts
├── org         list | get | create | update | delete
│               member  list | add | remove | set-role | invite
├── team        list | get | create | update | delete
│               member  list | add | remove
│               quota   show | set                      (platform admin)
├── agent       list | get | create | delete
│               grant   list | add | remove
├── api         list | get | create | delete | status   ← ApiDefinition (the loop's spine)
│               spec    list | get | diff | review | publish | unpublish | discard | export
│               tools   list | get | update | enable | disable | refresh
├── cluster     list | get | create | update | delete | scaffold
├── route       list | get | create | update | delete | scaffold | generate (--from-spec)
│               (route = RouteConfig; nested rules addressed as NAME/VHOST/RULE)
├── listener    list | get | create | update | delete | scaffold
├── filter      list | get | create | update | delete | types | scaffold
│               attach | detach | configure              (--listener | --route-config [--vhost --rule])
├── secret      list | get | create | update | delete | rotate   (create --from-ref for external)
├── dataplane   list | get | create | update | delete | bootstrap | status
│               cert    list | get | issue | revoke
├── learn       start | stop | list | get | health       (capture sessions)
│               discover start|stop|status               (traffic-first; requires --upstream
│                                                         or --to-host/--to-cidr allowlist)
├── ai          provider  list | get | create | update | delete | models
│               route     list | get | create | update | delete
│               budget    list | get | set | delete
│               usage     show (--by model|provider|team) [--watch]
├── rate-limit  domain … | policy …                      (as v1, name-addressed)
├── mcp         status | connections | enable | disable (--api | --route … | --all)
├── ops         status | doctor | trace | topology | validate | xds (status|nacks) | audit
├── stats       overview | clusters [--watch]
├── admin       org … | team … | health | resources | audit | apps | scopes |
│               rate-limit (overrides|reap) | rls repush | filter-schemas reload
├── stack       up | down | logs | status                (local dev stack; compose)
├── db          migrate | status                         (server-side ops, direct DB)
├── expose <url> / unexpose <name>                       (the two blessed top-level shortcuts)
├── apply -f <file|dir> [--prune --dry-run --diff]       (declarative; ALL resource kinds)
├── completion <shell> | version
```

Renames vs v1 recorded for DECISIONS: `import openapi` → `api create --from-openapi`;
`schema *` → `api spec *`; `wasm *` → `filter create --wasm …`; `init/down/logs` → `stack *`;
`learn export` → `api spec export --out`; `cert *` → `dataplane cert *`; route-views/reports
fold into `route list -o wide` / `ops topology`.

## 3. Global flags

`--context`, `--server`, `--team`, `--org`, `-o/--output`, `--json`, `--no-color`, `--quiet`,
`--verbose`, `--dry-run`, `--yes`, `--revision`, `--timeout`, `--out <path>` (where file output
exists). Paging: `--limit/--page-token` (server-driven cursors; the CLI auto-paginates `list`
unless `--limit` given).

## 4. Error-message style guide

```
$ flowplane cluster delete payments-db
error (resource_in_use): cluster "payments-db" is referenced by 2 route configs
  → run `flowplane cluster get payments-db -o wide` to see dependents,
    then delete those routes or re-run with --cascade
  request id: 01JXYZ…   (exit code 5)
```

Rules: lowercase `error (code):`; message states the fact; `→` line says the next action with
a copy-pasteable command; never dump raw HTTP/JSON on TTY (full body available with
`--verbose` or `-o json`); 401 always hints `flowplane auth login`; 403 names the missing
`(resource, action)`; 404 never reveals cross-tenant existence; 409 prints both revisions.

## 5. Worked transcripts

### 5.1 First contact (dev)

```
$ flowplane stack up
✓ postgres ready   ✓ control plane ready (http://localhost:8080)   ✓ envoy ready   ✓ agent ready
$ flowplane expose http://host.docker.internal:3000 --name demo
✓ created cluster demo, route config demo, listener demo (port 10001 from range 10000-10100)
  try: curl http://localhost:10001/
```

Local ports shown in transcripts are defaults, not fixed contracts. `stack up`, `dataplane up`,
and `expose` resolve ports from flags/env/config (`--api-port`, `--xds-port`, `--postgres-port`,
`--admin-port`, `--gateway-port-range`) and write the resolved values into generated bootstrap
and compose/systemd/K8s artifacts. If a requested/default port is occupied, the CLI fails before
mutating CP state and prints the exact override to use.

### 5.2 Config-first loop: create → observe → review → publish → tools

```
$ flowplane api create orders --from-openapi orders.yaml --port 10002
✓ api "orders" created (origin: imported, spec v1 published)
  routes: 12   cluster: orders   listener: :10002   mcp tools: 12 generated
$ flowplane learn start --api orders --sample-count 5000
✓ capture session lrn-7f3a started (state: observing)
$ flowplane learn get lrn-7f3a
SESSION   API      STATE      SAMPLES   ENDPOINTS   CONFIDENCE
lrn-7f3a  orders   observing  3 214     14          0.87
$ flowplane api spec list --api orders
VERSION  STATE      SOURCE     ENDPOINTS  CONFIDENCE  CREATED
v2       candidate  learned    14         0.87        2m ago     ← from lrn-7f3a; 2 endpoints not in v1
v1       published  imported   12         —           1h ago
$ flowplane api spec diff orders v1 v2
+ GET /orders/{orderId}/refunds          (new, 412 samples, confidence 0.91)
+ POST /orders/{orderId}/refunds         (new, 98 samples, confidence 0.84)
~ GET /orders: response field "discount" now optional (was required)
$ flowplane api spec review orders v2 --approve
✓ spec v2 → reviewed
$ flowplane api spec publish orders v2
✓ spec v2 → published; current pointer updated; 14 MCP tools regenerated (2 new); routes unchanged
$ flowplane api status orders
API     STATE   SPEC  ROUTES SERVED  TOOLS  DATAPLANE ACK
orders  served  v2    14/14          14     dp-main ✓ (2s ago)
```

### 5.3 Traffic-first loop: discover → generate → approve → serve

```
$ flowplane learn discover start --port 10099 --to-cidr 10.2.0.0/16 --max-duration 24h
✓ discovery listener active on :10099 for team payments (state: observing)
   forwarding: host-routed (dynamic forward proxy), allowed destinations: 10.2.0.0/16
   note: observed traffic is data only — nothing becomes config without approval
   direct traffic at :10099 (preserve Host headers) to begin observation
$ flowplane learn discover status
DISCOVERY  STATE      SAMPLES  CANDIDATE APIS
active     observing  18 402   2  (billing-internal: 9 endpoints · partners: 4 endpoints)
$ flowplane api list --origin discovered
NAME              STATE    SPEC  ENDPOINTS  CONFIDENCE
billing-internal  candidate  v1    9          0.83
$ flowplane api spec review billing-internal v1 --approve
✓ spec v1 → reviewed
$ flowplane route generate --from-spec billing-internal/v1 --port 10003 --dry-run
plan: would create
  cluster   billing-internal     (upstream 10.2.3.4:8443, TLS, from observed Host/SNI)
  route     billing-internal     (9 rules from spec v1)
  listener  billing-internal:10003
nothing created (--dry-run)
$ flowplane route generate --from-spec billing-internal/v1 --port 10003
about to create 1 cluster, 1 route config (9 rules), 1 listener — proceed? [y/N] y
✓ created; spec v1 → published; 9 MCP tools generated
$ flowplane api status billing-internal
billing-internal  served  v1  9/9 routes  9 tools  dp-main ✓
```

### 5.4 AI gateway: provider → route → budget → failover

```
$ flowplane secret create openai-key --type api-key --from-stdin < key.txt
✓ secret openai-key created (values are write-only)
$ flowplane ai provider create openai-prod --kind openai-compatible --credential openai-key
✓ provider openai-prod created
$ flowplane ai route create llm-main --port 10010 \
    --backend openai-prod:gpt-5:priority=0 \
    --backend openai-fallback:gpt-5:priority=1
✓ ai route llm-main created — unified endpoint :10010/v1/chat/completions
$ flowplane ai budget set --team payments --provider openai-prod \
    --tokens 5_000_000/day --shadow
✓ budget set (shadow mode: metering only, not enforcing)
$ flowplane ai usage show --by model
MODEL              REQS   IN TOK     OUT TOK   BUDGET USED
gpt-5              1 204  8 214 991  922 410   16 % (shadow)
$ flowplane ai budget set --team payments --provider openai-prod --tokens 5_000_000/day
✓ budget enforcing (429 on exhaustion; overdraft-on-last-request)
```

### 5.5 Tenancy administration

```
$ flowplane team create payments --org acme
$ flowplane team member add payments alice@acme.com
$ flowplane agent create ci-bot --org acme
$ flowplane agent grant add ci-bot --resource clusters --action create --team payments
$ flowplane team quota show payments
RESOURCE            LIMIT   USED
clusters            50      12
learning sessions   5       1
capture MB/day      500     38
```

### 5.6 Day-2 diagnostics

```
$ flowplane ops doctor
✓ control plane reachable   ✓ db healthy (outbox lag 0)   ✓ 2 dataplanes connected
✗ dataplane dp-edge: listener billing-internal warming for 45s
  → flowplane ops xds nacks --dataplane dp-edge
$ flowplane ops xds nacks --dataplane dp-edge
TIME   TYPE  RESOURCE           ERROR
12:01  LDS   billing-internal   cert secret "edge-tls" not found
  → flowplane secret create edge-tls … ; resource is quarantined (last good config still serving)
```

(Transcripts 5.7–5.9 — `apply --diff` GitOps flow, `mcp enable --api`, upstream deletion with
cascade preview — follow the same conventions; elided for brevity, covered in slice tests.)

## 6. Auth & config handling

`flowplane auth login` (PKCE loopback; `--device-code` for headless) → tokens in
`~/.flowplane/credentials` (0600, auto-refresh). `FLOWPLANE_TOKEN` env supported for CI.
Contexts: `flowplane config set-context prod --server https://fp.acme.com --org acme --team payments`,
`flowplane config use-context prod`. `--org` on a command overrides the active context and is sent
as `X-Flowplane-Org`; when omitted, the server only infers an org for users with exactly one active
non-platform membership. `stack *` and `db migrate` are the only commands that don't go through the
REST API (local compose / direct DB) and are clearly labeled as such in help.

## 7. Acceptance criteria (tested in the CLI slice and every feature slice after)

Every API capability has a CLI path in the same slice; `-o json` round-trips through `apply`;
`--dry-run` output equals the subsequent real mutation's effect; all errors carry code + hint;
completions install on bash/zsh/fish; `--help` examples are doctest-style verified against a
live server in E2E.
