# 16 — Flowplane CLI Standard

> Audience: CLI contributors, reviewers, and the agents that consume `flowplane`.
> Status: **authored** (rules: all tiers · conformance scan: Tier 1 only — see `spec/16-cli-conformance.md`).
> The `flowplane` binary is one deterministic command surface with two consumers — the human
> CLI and the agent/MCP layer derived from it. This document defines what a *brilliant* version
> of that surface is, derived from external authorities first, then reconciled with the code.

## How this standard was built (anti-anchoring note)

The rules below were derived from external authorities (CLIG.dev, POSIX/GNU, kubectl/gh,
BSD `sysexits.h`, Clap v4 idioms) **before** auditing the existing commands, so the status quo
did not define "good". `spec/12-cli-design.md` and `spec/07-cli-and-workflows.md` were then read
as *auditable input*, not ground truth. The conformance scan (Deliverable 2) measures the current
binary (`flowplane 1.1.0`) against these rules.

---

## 0. Status block — relationship to `spec/12` and `spec/07`

`spec/12-cli-design.md` is the project's prior CLI design note. `spec/07-cli-and-workflows.md`
inventories the **v1** CLI (`/tmp/flowplane-v1`) and is a known-issues checklist. Their fate here:

### Adopted from `spec/12` (agrees with authorities)
- **§1.1 noun-verb grammar, ≤12 nouns, no bare top-level verbs** → adopted as **CLI-R-01/02**
  (kubectl/gh). (Current binary has 24 top-level commands and bare verbs `expose`/`unexpose` — a
  conformance gap, not a standard change.)
- **§1.2 `-o table|json|yaml|wide`, `--out` for file output, `-o` always means format** → adopted as
  **CLI-R-10/14** (kubectl/gh).
- **§1.3 `--dry-run` on mutations; `--yes/-y` for scripts** → adopted as **CLI-R-22/26**.
- **§1.6 error style `error (code): message` + `→ hint` + request id; semantic exit codes** → adopted
  as **CLI-R-30/31** (CLIG). The code already implements this style (`output.rs`).
- **§1.7 config precedence `flag > env > file > default`, uniform per value** → adopted as
  **CLI-R-40** (D-005). The *principle* is adopted; the code violates it for `token` (see conformance).
- **§1.8 completions** → adopted as **CLI-R-52**.
- **§1.9 help with examples + related commands** → adopted as **CLI-R-05/06**.

### Extended beyond `spec/12`
- `spec/12` has no rule for **machine-readable CLI introspection** (`schema`/`--help=json`). Added as
  **CLI-R-50** — the single most important agent affordance and the seam MCP tool schemas derive from.
- `spec/12` has no **retryable-vs-terminal** signal. Added as **CLI-R-32**.
- `spec/12` does not require **TTY/`NO_COLOR` detection** or a non-TTY default. Added as **CLI-R-12/16**.
- `spec/12` does not commit the `-o json` shape to a **stability/versioning** discipline. Added as
  **CLI-R-15**.

### Superseded / flagged as anti-pattern
- **`spec/12` §1.2 keeps a `--json` flag as "sugar for `-o json`".** Flagged as a **mild
  anti-pattern** and superseded by **CLI-R-11**: a second flag that aliases an enum value is redundant
  surface, invites the `version --json` divergence seen in the code (where `--json` is silently
  ignored), and gives agents two code-paths to test. Keep `-o/--output` as the *only* format selector;
  if `--json` is retained, it MUST be exactly `--output json` with zero command-specific exceptions.
- **`spec/12` §6 "exit codes 0/1/2/3/4/5/6/7 per spec/10 §8"** conflicts with BSD `sysexits.h`
  (64–78). **CLI-R-31 reconciles this in favour of the compact 0–7 semantic range** (see the rule for
  why a script author is better served by it) — but fixes the **2-collision** the current code has
  (clap usage errors and `401/403` both exit `2`).

`spec/07` is treated purely as a hint list; every item was re-verified against the current binary,
which has already fixed several v1 problems it describes (structured error envelope, semantic exit
codes, `apply`, shell completion all now exist).

---

## 1. Principles (≤7, each traceable to an authority)

1. **One surface, two consumers.** The machine contract (`-o json`, exit codes, errors, introspection)
   is the source of truth; the human view is a rendering of it, and the MCP layer is *derived* from it,
   never maintained in parallel. *(CLIG.dev — human/machine duality.)*
2. **The machine contract is the MVP.** Anything that silently breaks a script or an agent is a Tier 1
   defect and outranks every ergonomic concern. *(CLIG.dev — "design for machines too".)*
3. **Errors teach and branch.** Every failure states the fact, a stable `code`, a machine-actionable
   `hint`, and whether it is worth retrying — in English on a TTY, in JSON to an agent. *(CLIG.dev.)*
4. **Guessable grammar.** Resource-oriented noun-verb (`<noun> <verb>`) with a fixed verb vocabulary and
   kebab-case flags, so a user/agent can predict a command they have never run. *(kubectl/gh, POSIX/GNU.)*
5. **Least surprise, no hidden state.** One config-precedence rule for every value; every invocation is
   fully specifiable by flags/env; implicit context is an override, never a requirement. *(D-005, CLIG.)*
6. **Never block, always offer an out.** Non-TTY is detected and never prompts; every destructive action
   has `--yes`; long operations report progress; `--quiet` truly silences chrome. *(CLIG.dev.)*
7. **The output shape is a versioned API.** `-o json` is contract: additive changes are safe; renames,
   removals, retypes are breaking and gated. *(gh/kubectl output contract.)*

---

## 2. Tiers

- **Tier 1 — scriptable contract.** `-o json` shape & stability, exit-code semantics, error format &
  structure, stdin/pipe/quiet behaviour, config/auth precedence, and the agent affordances
  (introspection, structured branchable errors, retryable signal, non-interactive paths). **Violations
  silently break scripts and agents.** This tier is shippable on its own and is the only tier scanned in
  this conformance pass.
- **Tier 2 — ergonomics.** Verb grammar, flag naming, help quality, resource addressing. A human trips
  once and adapts. Standardised after Tier 1.

Every rule below is tagged `[T1]` or `[T2]`.

---

## 3. Rules

Each rule: **statement · tier · authority · conforming example · violating example · named check.**
Checks named `chk:<slug>` are stubs a contributor implements; "transcript" = snapshot test under
`crates/flowplane/tests/`, "clap-lint" = a compile-time assertion in `Cli::command().debug_assert()`
or a unit test walking the built `clap::Command`.

### Axis A — Verb grammar & command shape

**CLI-R-01 · Noun-verb grammar [T2]** *(kubectl/gh)*
Every mutating/reading command is `flowplane <noun> <verb> [args]`. The verb vocabulary is fixed:
`list | get | create | update | delete` plus a small set of noun-specific transitions
(`publish`, `attach`, `rotate`, …). No bare top-level verbs except `version`, `completion`, `apply`,
`serve`.
- ✅ `flowplane cluster delete payments-db`
- ❌ `flowplane expose http://…` / `flowplane unexpose demo` (bare top-level verbs)
- **Check** `chk:noun-verb-grammar` — clap-lint: walk top-level subcommands; assert each is either in the
  allow-list (`version`, `completion`, `apply`, `serve`, `db`) or has only sub-subcommands drawn from the
  fixed verb set.

**CLI-R-02 · Bounded top-level surface [T2]** *(kubectl/gh sweet spot)*
≤ 12 top-level nouns. Diagnostics live under one noun (`ops`), not as scattered top-level verbs.
- ✅ `flowplane ops doctor`, `flowplane ops xds status`
- ❌ 24 top-level commands (current `main.rs`)
- **Check** `chk:top-level-count` — clap-lint: assert `Cli::command().get_subcommands().count() <= 12`
  (drives consolidation; tune the cap when the noun set is finalised).

**CLI-R-03 · Unknown command suggests nearest valid [T2]** *(gh, CLIG discoverability)*
A mistyped command exits `2` and prints "did you mean" with the closest valid command.
- ✅ `flowplane clustr list` → `tip: some similar subcommands exist: 'cluster'`
- ❌ bare "unrecognized subcommand" with no suggestion
- **Check** `chk:suggest-nearest` — transcript: `clustr list` stderr contains `cluster`, exit `2`.
  *(Clap provides this for free when built with the `suggestions` feature.)*

### Axis B — Flag naming & input model

**CLI-R-04 · POSIX/GNU flag shape [T2]** *(POSIX, GNU, Clap v4)*
Long flags are `--kebab-case`; single-dash shorts are one character; `--` terminates option parsing;
`-` means stdin/stdout where a path is expected. No `--snake_case`, no multi-char single-dash flags.
- ✅ `--from-openapi`, `-o json`, `apply -f -`
- ❌ `--from_openapi`, `-output`
- **Check** `chk:flag-kebab` — clap-lint: walk every arg of every command, assert long name matches
  `^[a-z][a-z0-9-]*$` and every short is exactly one char.

**CLI-R-05 · Every command has a one-line summary [T2]** *(CLIG help quality)*
Each command and sub-command renders a non-empty `about` in its parent's listing and its own `--help`.
- ✅ `apply  Apply a declarative JSON resource manifest`
- ❌ `create` (blank summary — current `cluster`/`listener`/`route` leaf verbs)
- **Check** `chk:nonempty-about` — clap-lint: walk every `clap::Command`, assert `get_about().is_some()`
  and non-empty. Fails the build on any leaf with no `///` doc comment.

**CLI-R-06 · Help carries a runnable example [T1 for top-level workflows, else T2]** *(CLIG)*
Top-level and every "spine" workflow command's `--help` includes at least one copy-pasteable example
that parses. Examples are verified to parse, not just present.
- ✅ root `after_help` examples, asserted by `cli_help_contains_workflow_examples` (`main.rs:372`)
- ❌ `cluster create --help` with no example of the `-f` file shape
- **Check** `chk:help-examples-parse` — transcript: extract `flowplane …` lines from each `--help`,
  feed each to `Cli::try_parse_from`, assert all parse. *(Extends the existing test in `main.rs:226`.)*

**CLI-R-07 · Every arg has help text [T2]** *(CLIG)*
Every flag/positional renders a non-empty description in `--help`.
- ✅ `--prune  Apply is additive-only until server batch support` *(text needs a rewrite, but present)*
- ❌ `--file <FILE>` / `--team <TEAM>` rendered with blank meaning (current leaf help)
- **Check** `chk:nonempty-arg-help` — clap-lint: assert every `Arg.get_help().is_some()`.

**CLI-R-08 · Uniform create input [T2]** *(least surprise; kubectl `-f`)*
All `create`/`update` take the resource body via `-f/--file <path|->`. Inline flag-soup bodies and
per-noun special-casing (`secret create --config '<json>'`) are disallowed.
- ✅ `flowplane secret create -f secret.json`
- ❌ a `create` that takes `--config '<JSON string>'` instead of `-f`
- **Check** `chk:create-takes-file` — clap-lint: every command named `create`/`update` has an arg
  with long name `file` and short `f`.

**CLI-R-09 · `-` is stdin/stdout [T1]** *(POSIX, CLIG composability)*
`-f -` reads the manifest/body from stdin; `--out -` writes to stdout. Reading stdin never blocks when
stdin is a closed/redirected non-TTY.
- ✅ `cat gateway.json | flowplane apply -f -`
- ❌ `apply -f -` trying to `open("-")` as a literal path
- **Check** `chk:stdin-dash` — transcript: `printf '{...valid...}' | flowplane apply -f - --dry-run`
  succeeds and round-trips the manifest.

### Axis C — Output model

**CLI-R-10 · `-o table|json|yaml|wide` on every command [T1]** *(kubectl/gh)*
Every command — including mutations (`create/update/delete`), `expose`, `version`, and ops verbs —
honours `-o`. There is no plain-prose-only command.
- ✅ `flowplane cluster delete x -o json` emits a JSON result object
- ❌ `flowplane version -o json` printing the bare string `1.1.0` (current behaviour, `main.rs:208`)
- **Check** `chk:output-flag-universal` — transcript matrix: for a representative command per group, run
  with each `-o` value and assert the format (valid JSON / YAML / table header). Includes `version`.

**CLI-R-11 · One format selector [T1]** *(CLIG; supersedes spec/12 §1.2 `--json` sugar)*
`-o/--output` is the only output-format control. If `--json` exists it is *exactly* `--output json`
with no command-specific exception.
- ✅ `--json` and `-o json` produce byte-identical output for every command
- ❌ `version --json` ignoring the flag and printing `1.1.0`
- **Check** `chk:json-flag-equiv` — transcript: for every command in a sampled set, assert
  `cmd --json` output == `cmd -o json` output.

**CLI-R-12 · Format defaults to the consumer [T1]** *(CLIG human/machine duality)*
On a TTY the default is `table`; when stdout is **not** a TTY (piped/redirected) the default is `json`,
so `flowplane cluster list | jq` works with no flag. An explicit `-o` always wins.
- ✅ `flowplane cluster list | jq '.[].name'` (auto-json when piped)
- ❌ default `table` regardless of TTY (current `GlobalOptions::format()`, `config.rs:99`)
- **Check** `chk:tty-default-format` — transcript: run with stdout piped (non-TTY) and assert the output
  parses as JSON without `-o`.

**CLI-R-13 · `--quiet` suppresses all chrome [T1]** *(CLIG)*
`--quiet` silences progress, confirmations, and human summaries, leaving only the requested data (or
nothing for a pure mutation) on stdout, and errors on stderr.
- ✅ `flowplane cluster create -f c.json --quiet` prints nothing on success, exit 0
- ❌ `--quiet` still printing `created "x"`
- **Check** `chk:quiet-silences` — transcript: `--quiet` success path has empty stdout; error path still
  writes the error envelope to stderr.

**CLI-R-14 · `-o` is format, `--out` is destination [T1]** *(kubectl; spec/12 §1.2)*
`-o/--output` selects format only. File destination is `--out <path>`. `-o` is never overloaded to mean
a file path (the v1 `learn export -o <FILE>` collision is prohibited).
- ✅ `flowplane dataplane bootstrap dp -o yaml --out envoy.yaml`
- ❌ `learn export -o ./out.json` meaning "write to file"
- **Check** `chk:o-is-format-only` — clap-lint: assert no command redefines `-o`/`--output` as a path;
  the global `OutputFormat` enum arg is the only `-o`.

**CLI-R-15 · `-o json` is a versioned contract [T1]** *(gh/kubectl)*
The JSON shape is an API: additive fields are safe; renaming/removing/retyping a field is breaking and
requires a deliberate gate. Top-level machine output carries a `schemaVersion` (or the response is a
typed envelope) so agents can branch on shape.
- ✅ adding `created_at` to a list row
- ❌ renaming `name`→`resourceName` in a point release with no version bump
- **Check** `chk:json-snapshot` — snapshot tests of `-o json` for every command's happy path; a diff in
  an existing field fails CI and forces an explicit `schemaVersion` bump in the snapshot.

**CLI-R-16 · `NO_COLOR` and non-TTY disable color; `--no-color` works [T1]** *(CLIG, no-color.org)*
Color/styling is emitted only to a TTY and only when `NO_COLOR` is unset and `--no-color` is absent.
A declared `--no-color` flag must actually be consulted (no dead flags).
- ✅ `NO_COLOR=1 flowplane cluster list` and `… --no-color` both emit no ANSI
- ❌ `--no-color` defined but never read (current: `config.rs:28`, unused)
- **Check** `chk:no-color` — transcript: assert no ANSI escape bytes in output under `NO_COLOR=1`,
  under `--no-color`, and under non-TTY stdout; plus a clap-lint that the `no_color` field is referenced.

### Axis D — Errors & exit codes

**CLI-R-30 · Errors are structured and teach [T1]** *(CLIG; spec/12 §4)*
Every error carries: a stable `code` (enum, not free text), a human `message`, an optional
machine-actionable `hint` (ideally the corrected command), the offending field where applicable, and
`request_id`. On a TTY: `error (code): message` / `  → hint` / `  request id: …`. Under `-o json`: the
JSON envelope to **stderr**, nothing on stdout. Raw HTTP/JSON bodies are never dumped on a TTY.
- ✅ `error (resource_in_use): cluster "payments-db" is referenced by 2 route configs` / `→ …`
  (the path the code takes for HTTP errors, `output.rs:28`)
- ❌ `Error: send request / Caused by: … Connection refused (os error 61)` — raw anyhow chain, no code,
  no hint, not JSON even under `-o json` (current network-error path, `main.rs:154`)
- **Check** `chk:error-envelope` — transcript: force a 404 and a network failure; assert TTY format has
  `error (` + `→`, and `-o json` emits a JSON object with `{code,message,status,request_id}` on stderr,
  empty stdout. **Network/transport errors must go through the same envelope, not `eprintln!("{err:?}")`.**

**CLI-R-31 · Semantic exit codes, no collisions [T1]** *(BSD `sysexits.h`, reconciled)*
Exit codes are a compact semantic range, distinct per failure class, **with usage errors separated from
auth errors**:
`0` success · `1` generic/unexpected · `2` **usage/parse** (clap) · `3` auth (401/403) ·
`4` not-found/conflict/precondition (404/409/412) · `5` validation (400/422) · `6` rate-limited (429) ·
`7` server/transport (5xx, connection, timeout).
*Reconciliation note:* BSD `sysexits.h` (64–78) is rejected in favour of this 0–7 range because a
control-plane script branches on **outcome class** (retry? re-auth? fix input?), which a dense
semantically-grouped range expresses more legibly than `EX_NOPERM=77`; the cost is non-standard
numbers, accepted. The current code (`output.rs:103`) already uses a 0–6 range but **collides clap usage
errors and auth on `2`** and routes transport errors to `1` — both fixed here.
- ✅ `flowplane cluster get missing` (404) exits `4`; `… get` (missing arg) exits `2`
- ❌ `401` and a clap usage error both exiting `2` (current collision)
- **Check** `chk:exit-codes` — transcript/unit: table-test status→exit mapping (extend
  `http_statuses_map_to_scriptable_exit_codes`, `output.rs:552`) **plus** assert a clap usage error and a
  401 produce *different* codes, and a connection failure exits the transport class, not `1`.

**CLI-R-32 · Retryable vs terminal is machine-detectable [T1]** *(agent affordance; CLIG resilience)*
An agent can decide to retry without parsing English: transient failures (429, 5xx, connection/timeout)
are a distinct exit class (≥6 per CLI-R-31) **and** the JSON error envelope carries
`"retryable": true|false`. Validation/auth/not-found are terminal (`retryable:false`).
- ✅ `429` → exit 6, envelope `{"code":"rate_limited","retryable":true}`
- ❌ connection-refused and a 400 both exiting `1` with no retryable signal (current)
- **Check** `chk:retryable-signal` — transcript: assert `retryable` present and correct for a 429, a 503,
  a connection failure, and a 400; assert the exit class matches.

**CLI-R-33 · Auth/permission errors name the fix [T1]** *(CLIG errors-that-teach)*
`401` always hints `flowplane auth login`; `403` names the missing `(resource, action)` (and team/org);
`404` never reveals cross-tenant existence.
- ✅ `error (unauthorized): not authenticated` / `→ run \`flowplane auth login\``
- ❌ `401` rendered with only the server message and no login hint (current: hint only if server sends it)
- **Check** `chk:auth-hint` — transcript: stub a 401/403 response; assert the client *injects* the login
  hint / the `(resource,action)` even when the server omits `hint`.

### Axis E — Config / auth precedence

**CLI-R-40 · One precedence rule for every value [T1]** *(D-005, least surprise)*
For **every** configurable value (server, org, team, token, timeout, output, context) precedence is
identical: `flag > env (FLOWPLANE_*) > active context > config file > built-in default`. No value
inverts the order.
- ✅ `--server` beats `FLOWPLANE_SERVER` beats context beats file
- ❌ `token` resolved `env > context > file` with **no flag and env highest**, while `server/org/team`
  are flag-highest (current split, `config.rs:200` vs `:211`)
- **Check** `chk:precedence-uniform` — unit: a table-driven test sets flag/env/context/file for each
  value and asserts the same winner order for all; fails if any value diverges.

**CLI-R-41 · Every value has flag + env + file [T1]** *(consistency)*
Each value is settable by all three of: a global flag, a `FLOWPLANE_*` env var, and the config file.
No value is missing a tier (e.g. `timeout` currently has flag+file but no env).
- ✅ `--timeout` / `FLOWPLANE_TIMEOUT` / `timeout =` in config
- ❌ `timeout` with no `FLOWPLANE_TIMEOUT` (current)
- **Check** `chk:value-tiers` — unit: for each value, assert a flag, an env binding, and a config field
  all exist.

**CLI-R-42 · No required hidden state [T1]** *(agent affordance — no implicit context)*
Any invocation is fully specifiable by flags/env alone; a previously-set context is an *override*, never
a *requirement*. Parallel invocations and fresh shells behave identically given the same flags/env.
- ✅ `FLOWPLANE_SERVER=… FLOWPLANE_TEAM=… FLOWPLANE_TOKEN=… flowplane cluster list` with no config file
- ❌ a command that fails unless `config use-context` was run first
- **Check** `chk:stateless-invocation` — transcript: run a representative command with `HOME` pointed at
  an empty dir and everything supplied via env; assert success.

**CLI-R-43 · Secrets never leak to disk or logs world-readably [T1]** *(security; CLIG)*
Credential/config files are `0600`, their dirs `0700`; tokens are redacted in `config show` and never
printed by `--verbose`.
- ✅ `~/.flowplane/credentials` written `0600` (current `config.rs:171`)
- ❌ `--verbose` echoing the bearer token
- **Check** `chk:secret-perms` — unit (exists: `private_file_write_*`, `config.rs:274`) extended to assert
  `config show` redaction and that no log path prints the token.

### Axis F — Resource addressing & mutation semantics

**CLI-R-44 · One addressing scheme [T2]** *(kubectl name-vs-id)*
Resources are addressed by team-unique **name**; a UUID is accepted anywhere a name is (server resolves
both). No command requires a different handle (the v1 `mcp enable <routeID>` mix is prohibited).
- ✅ `flowplane mcp enable --api orders`
- ❌ one command keyed by numeric/UUID while siblings use names
- **Check** `chk:name-addressing` — transcript: addressing a resource by name and by id yields the same
  result for a sampled command.

**CLI-R-45 · Mutations are idempotent or declarative [T1]** *(agent affordance; kubectl apply)*
Prefer declarative `apply` semantics; `create` of an identical existing resource is a no-op success (or
supports `--idempotency-key`), so an agent that retries does not double-create or hard-fail.
- ✅ `flowplane apply -f gateway.json` twice → second run reports "unchanged"
- ❌ second `create` of the same name → opaque `409` with no idempotency path
- **Check** `chk:idempotent-apply` — transcript (server-backed): apply the same manifest twice; assert
  the second is a success with a no-change result.

**CLI-R-46 · `--dry-run` returns the real would-be effect [T1]** *(agent approval seam; CLIG)*
`--dry-run` performs server-side validation and returns the structured diff/plan that the real mutation
would produce — not a trivial echo of the request — so it is a usable approval gate.
- ✅ `apply --dry-run` returns the server's computed create/update/no-op plan per resource
- ❌ `--dry-run` echoing `{method, path, body}` client-side without contacting the server
  (current, `client.rs:121`)
- **Check** `chk:dryrun-fidelity` — transcript (server-backed): assert `--dry-run` output structurally
  equals the subsequent real mutation's effect.

**CLI-R-47 · Optimistic concurrency everywhere or nowhere [T1]** *(consistency; spec/07 #2)*
`update`/`delete` accept `--revision N` (→ `If-Match`) uniformly across resources; without it the CLI
does read-modify-write and fails a race with a clear `409` showing both revisions.
- ✅ `flowplane cluster update web -f c.json --revision 7`
- ❌ revision honoured only for `rate-limit` (v1 behaviour)
- **Check** `chk:revision-uniform` — clap-lint: every `update`/`delete` exposes `--revision`; transcript:
  a stale `--revision` yields a `409` envelope naming both revisions.

### Axis G — Interactivity & feedback

**CLI-R-22 · Destructive actions confirm on TTY, `--yes` skips [T1]** *(CLIG; spec/12 §1.3)*
`delete`, `--prune`, `unexpose`, and publish-of-discovered prompt `[y/N]` on a TTY and proceed only on
confirmation; `--yes/-y` bypasses. A declared `--yes` must gate a real confirmation (no dead flag).
- ✅ `flowplane cluster delete db` → `delete cluster "db"? [y/N]`; `… --yes` skips
- ❌ `--yes` defined but no confirmation exists anywhere (current: `-y` is a no-op)
- **Check** `chk:confirm-destructive` — transcript: destructive command on a pty prompts; with `--yes`
  does not; clap-lint asserts the `yes` field is referenced by destructive handlers.

**CLI-R-26 · Never prompt on non-TTY [T1]** *(agent affordance — the `--full-auto < /dev/null` deadlock)*
When stdin is not a TTY, no command ever blocks on a prompt. A destructive action on non-TTY without
`--yes` fails fast with exit `2` and a hint to pass `--yes`, rather than hanging.
- ✅ `flowplane cluster delete db < /dev/null` → `error (confirmation_required): … pass --yes` exit 2
- ❌ a `read_line` that blocks forever when stdin is closed
- **Check** `chk:no-prompt-noninteractive` — transcript: destructive command with stdin=`/dev/null` and
  no `--yes` exits non-zero promptly (timeout-bounded) and never reads stdin.

**CLI-R-27 · Progress for long operations [T2]** *(CLIG feedback)*
Operations that take >2s (xDS convergence, learn sessions, bootstrap) emit progress to **stderr** so
stdout stays a clean data stream; `--quiet` suppresses it.
- ✅ spinner/status lines on stderr during `dataplane bootstrap`
- ❌ a long command that prints nothing until it finishes
- **Check** `chk:progress-stderr` — transcript: assert progress bytes land on stderr, not stdout.

### Axis H — Agent affordances

**CLI-R-50 · Machine-readable command catalog [T1]** *(agent affordance — "the CLI's OpenAPI")*
`flowplane schema` (or `--help=json`) emits the full command tree as JSON: commands, subcommands, every
flag/positional with type, enum values, required/optional, defaults, and help text. MCP tool schemas are
**derived** from this output, not hand-maintained. (Distinct from `flowplane openapi`, which is the
*server's* REST contract, not the CLI's.)
- ✅ `flowplane schema | jq '.commands[] | select(.name=="cluster")'`
- ❌ no introspection command; agents must scrape `--help` text (current state)
- **Check** `chk:cli-schema` — transcript: `flowplane schema` is valid JSON, contains every command in
  `Cli::command()`, and a generator test asserts the MCP tool list is reproducible from it.

**CLI-R-51 · Context economy — concise by default, field selection on demand [T1]** *(agent token cost)*
Default output is the minimal useful set; verbosity is opt-in (`--verbose`/`wide`). Agents can request
exactly the bytes they need via `--fields a,b,c` (or a jsonpath/`--template`) and `--quiet` to drop all
chrome.
- ✅ `flowplane cluster list -o json --fields name,revision`
- ❌ list always returning every field, forcing the agent to pay for and `jq` them away
- **Check** `chk:field-selection` — transcript: `--fields name` returns objects with only `name`.

**CLI-R-52 · Shell completion for humans, schema for machines [T2]** *(spec/12 §1.8)*
`flowplane completion bash|zsh|fish` is generated from the clap tree (already present), and stays in
sync with CLI-R-50 (same source of truth).
- ✅ `flowplane completion zsh`
- ❌ hand-written completion drifting from the command tree
- **Check** `chk:completion-generates` — transcript: each shell completion generates non-empty output and
  references a sampled subcommand.

---

## 4. New-command checklist (tick before merging any new command)

```
[ ] Grammar: it is `<noun> <verb>` with a verb from the fixed set, or justified in review (CLI-R-01).
[ ] about: the command and EVERY arg has a non-empty `///` doc summary (CLI-R-05, CLI-R-07).
[ ] Example: `--help` shows ≥1 runnable example that parses in a test (CLI-R-06).
[ ] Input: body comes via `-f/--file` (path or `-` for stdin); no inline JSON-string soup (CLI-R-08/09).
[ ] Output: honours `-o table|json|yaml|wide`; mutations emit a JSON result under `-o json` (CLI-R-10).
[ ] Default format flips to json on non-TTY stdout (CLI-R-12); `--quiet` silences chrome (CLI-R-13).
[ ] File destination is `--out`, never `-o` (CLI-R-14); color gated on TTY + NO_COLOR (CLI-R-16).
[ ] `-o json` shape added to the snapshot set with `schemaVersion` (CLI-R-15).
[ ] Errors go through the shared envelope (code/message/hint/field/request_id/retryable), to stderr,
    with the right exit class — including transport errors (CLI-R-30/31/32/33).
[ ] Every configurable value uses the one precedence rule and has flag+env+file (CLI-R-40/41).
[ ] Mutations: idempotent/declarative where possible (CLI-R-45); `--dry-run` returns the real plan
    (CLI-R-46); `--revision` for update/delete (CLI-R-47).
[ ] Destructive: prompts on TTY, `--yes` skips, never prompts on non-TTY (CLI-R-22/26).
[ ] Introspection: the new command and flags appear in `flowplane schema` output (CLI-R-50).
[ ] A `chk:*` test (transcript or clap-lint) accompanies the command — no prose-only rules.
```

---

*Conformance for Tier 1 → `spec/16-cli-conformance.md`. Open design decisions → printed with the run
that authored this file (Deliverable 3).*
</content>
</invoke>
