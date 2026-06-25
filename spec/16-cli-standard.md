# 16 — The Flowplane CLI Standard

*An executable best-practice standard for the `flowplane` binary's client surface.*

This document defines what a **brilliant** `flowplane` CLI is, derived from external
authorities first and the existing code second. It is the normative reference; the
companion conformance backlog (`spec/16-cli-conformance.md`) measures the current CLI
against it.

**Governing authorities** (every rule traces to one):

- **CLIG** — [Command Line Interface Guidelines](https://clig.dev). Primary source for
  the human/machine duality, output design, errors, help, discoverability, interactivity.
- **POSIX** — IEEE Std 1003.1 *Utility Conventions* (§12.1/§12.2) + **GNU** long-option
  conventions. Flag shape, `--` terminator, `-` for stdin/stdout.
- **kubectl / gh** — de-facto reference for a resource-oriented control-plane CLI: the
  `-o json|yaml|wide|name` output contract, noun-verb grammar, resource addressing.
- **sysexits** — BSD `sysexits.h` exit-code semantics (reconciled below).
- **Clap v4** — derive idioms, so rules are enforced in the type system where possible.

---

## 1. Status block — relationship to `spec/12` and `spec/07`

`spec/12-cli-design.md` is **auditable, not ground truth**. `spec/07-cli-and-workflows.md`
is a known-issues checklist (it documents *v1* from a separate tree and is used only as a
hint; every finding here is re-verified against current code with `path:line`).

### Adopted from `spec/12` (agrees with the authorities)

| spec/12 rule | Adopted as | Note |
|---|---|---|
| §1.1 noun-verb, closed verb set, no bare top-level verbs | CLI-R-20, CLI-R-21 | Matches kubectl/gh. |
| §1.2 `-o` = format always; `--json` sugar; mutations scriptable | CLI-R-01, CLI-R-07 | With one supersession (below). |
| §1.3 `--dry-run` on every mutation; destructive prompts + `--yes` | CLI-R-32, CLI-R-33 | Extended with `--no-input`. |
| §1.4 one addressing scheme (name, UUID accepted anywhere) | CLI-R-27 | Matches kubectl. |
| §1.5 `--revision N` → If-Match, RMW fallback | CLI-R-35 | Promoted to **Tier 1** (scriptable concurrency). |
| §1.6 / §4 error style `error (code): message` + `→ hint` + `request id` | CLI-R-12, CLI-R-14 | Adopted wholesale; matches CLIG. |
| §1.7 config precedence flag > env > file > default, uniform | CLI-R-17 | Adopted; v1's inversions superseded. |
| §1.8 completions bash/zsh/fish | CLI-R-30 | Matches CLIG/clap_complete. |
| §1.9 examples in `--help`; `help <topic>` | CLI-R-28, CLI-R-29 | Matches CLIG. |
| §1.2 `--out <path>` for file output, distinct from `-o` | CLI-R-10 | Cleans up the garbled prose in §1.2. |

### Extended beyond `spec/12` (silent gaps the authorities require us to fill)

- **CLI-R-04 / CLI-R-06 / CLI-R-09 / CLI-R-11 / CLI-R-34** — `spec/12` is silent on the
  stdout/stderr split, `NO_COLOR`, secrets-not-via-flags, the `--` terminator, and a
  global `--no-input`. CLIG/POSIX mandate all five.
- **CLI-R-13** — `spec/12 §1.6` defers exit-code *semantics* to "spec/10 §8"; this
  standard pins the full taxonomy and reconciles it with `sysexits` (§3 below).
- **CLI-R-18** — token-via-flag is removed entirely (CLIG: never accept secrets through
  flags), which also dissolves v1's flag-vs-env precedence problem.

### Superseded (this standard overrides `spec/12` or v1 — named)

- **Anti-pattern flagged in `spec/12 §1.2`: "default `table` on TTY, `json` when
  piped."** Silent, TTY-triggered *format* switching breaks least-surprise: the same
  command yields different machine output depending on whether a pseudo-TTY is present
  (CI vs local), so scripts are fragile and `-o json` becomes the only reliable contract
  anyway. **Superseded by CLI-R-01**: output *format* is always **explicit** (defaults to
  `table`, never auto-switches); only *decoration* (color, spinners, prompts) is
  auto-suppressed on non-TTY. This is the kubectl/gh-aligned behaviour.
- **v1 token precedence inversion** (`env > --token-file > creds > --token`, flag lowest;
  `spec/07 §1`) — superseded by CLI-R-17 (uniform `flag > env > file > default`) and
  CLI-R-18 (no token flag at all).
- **v1 `-o` overloaded as a file path** in `learn export` / `schema export` /
  `wasm download` (`spec/07 §2`) — superseded by CLI-R-10 (`--out`).
- **v1 prose-only mutations / exit-code monoculture (0/1) / raw HTTP error dumps**
  (`spec/07 §1, §3`) — superseded by CLI-R-07, CLI-R-12..16.

---

## 2. Principles (≤7)

1. **Human-first, machine-ready.** One command serves an interactive human and a script;
   the machine contract is *explicitly requested*, never an accident of TTY detection.
   *(CLIG: "Human-readable output is paramount… make it usable by programs too.")*
2. **The scriptable contract is a versioned API.** `-o json` shape, exit codes, and error
   format change only additively. *(CLIG output/robustness; kubectl `-o json` stability.)*
3. **Noun-verb, uniformly.** Resources are nouns; actions are a closed, guessable verb
   set; an unknown command suggests the nearest valid one. *(kubectl/gh.)*
4. **Errors teach.** Every failure states the fact, the code, and the next runnable
   command; format and exit code are part of the contract. *(CLIG errors.)*
5. **One precedence, everywhere.** `flag > env > config > default` for every value, with
   no per-field exceptions. *(CLIG config; GNU.)*
6. **Least surprise, full control.** Safe defaults, confirm destructive acts, and never
   block a script on a prompt. *(CLIG interactivity/safety.)*
7. **Discoverable and composable.** Real examples in help; `stdin`/`stdout`/`-`/pipes/
   `NO_COLOR` all honoured; output dense but parseable. *(CLIG, POSIX.)*

---

## 3. Exit-code taxonomy (reconciling `sysexits` with `spec/12`'s 0–7 range)

**The conflict, stated plainly.** `spec/12 §1.6` gestures at a small `0–7` range. BSD
`sysexits.h` defines `64–78` (`EX_USAGE 64`, `EX_NOPERM 77`, `EX_CONFIG 78`, …).

**Decision — adopt the small `0–7` range; reject `sysexits` 64–78.** A script author is
better served by small, memorable codes that align with the surrounding ecosystem:

- Clap already exits **2** on a usage error; git, gh, and most tooling use `0/1/2`.
  Overriding clap's `2`→`64` fights the framework for no script-author benefit.
- `sysexits` codes are obscure; almost no shell author branches on `EX_PROTOCOL 76`.
- The only distinctions a script actually branches on are: *did it work* (0 vs non-0),
  *should I retry* (transient vs permanent), *must I re-auth* (401), *is my invocation
  wrong* (usage). A curated 0–7 map captures exactly these.

| Code | Name | Meaning | Maps from | Retry? |
|---|---|---|---|---|
| **0** | OK | Success | 2xx | — |
| **1** | GENERAL | Unclassified / internal client error / server 500 software error | 500, panics | no |
| **2** | USAGE | Bad invocation **and** server-rejected input (400/422) — "fix the input" | clap, 400, 422 | no |
| **3** | AUTH | Authentication required/failed | 401 | no — re-login |
| **4** | PERMISSION | Authenticated but forbidden | 403 | no |
| **5** | CONFLICT | Conflict / precondition failed / resource-in-use | 409, 412 | no |
| **6** | NOT_FOUND | Resource does not exist (no cross-tenant leak) | 404 | no |
| **7** | UNAVAILABLE | Transient: network/timeout/5xx (non-500)/429 | timeout, 503, 429 | **yes** |

`5` aligns with `spec/12 §4`'s worked example (`resource_in_use` → "exit code 5").

**Open fork (surfaced in §7, not silently resolved):** folding server validation (400/422)
into `2` alongside clap usage errors — both mean "don't retry, fix input," but they have
different *origins*. An alternative gives validation its own code. Recorded as DD-3.

---

## 4. Rules (`CLI-R-NN`)

Each rule: **the rule · tier · authority · conforming ✓ · violating ✗ · named check.**
Tier 1 = scriptable contract (violations silently break scripts; ship first).
Tier 2 = ergonomics (a user trips once and adapts; migrate after Tier 1).

The **named checks** reference five enforcement harnesses (stubs to implement — see §6):

- `tests/cli_contract.rs` — introspects the built Clap `Command` tree (reflective, no
  process spawn): flag shape, `-o` presence, examples, verb set, kebab-case.
- `tests/cli_transcripts/` — golden transcripts (`trycmd`/`assert_cmd` + `insta`):
  output, error text, exit codes.
- `tests/cli_exit_codes.rs` — `assert_cmd` asserting `.code(N)` per error class.
- `tests/cli_json_contract.rs` — `insta` snapshot of each command's `-o json` shape;
  diff guard fails on non-additive change.
- `tests/cli_precedence.rs` — unit tests over the value-resolution function.

### Axis A — Output model

#### CLI-R-01 — `-o/--output` on every command; format is explicit, decoration is automatic — **Tier 1** — CLIG, kubectl
Every command (including mutations and deletes) accepts `-o table|json|yaml|wide`.
Default is `table`; the format **never** auto-switches on pipe/TTY. Only *decoration*
(color, spinners, prompts) is suppressed on non-TTY. `--json` is sugar for `-o json`.
- ✓ `flowplane cluster get web -o json | jq .name` yields JSON whether or not stdout is a TTY.
- ✗ A command that emits a table interactively but JSON when piped (the `spec/12 §1.2`
  anti-pattern), so `cmd | cat` differs from `cmd`.
- **Check:** `cli_contract::every_leaf_has_output_flag` + `cli_transcripts/output_format_is_explicit` (run piped and in a pty, assert identical stdout for a fixed `-o`).

#### CLI-R-02 — `-o json` is a stable, snake_case, additive-only contract — **Tier 1** — kubectl, CLIG
JSON keys are `snake_case` and match the REST contract (`spec/01`). Fields are never
renamed or removed without a documented version bump; additions are allowed.
- ✓ `{"name":"web","team":"payments","created_at":"…"}` stable across releases.
- ✗ Renaming `created_at`→`createdAt` between patch releases.
- **Check:** `cli_json_contract::snapshot_*` (insta) — review must approve any key delta.

#### CLI-R-03 — Documented JSON envelopes per verb — **Tier 1** — kubectl, gh
`list` → `{"items":[…],"page":{"next_token":<str|null>}}`; `get/create/update` → the bare
resource object; `delete` → `{"status":"deleted","kind":<k>,"name":<n>}`. Identical shape
across nouns.
- ✓ `flowplane cluster list -o json | jq '.items[].name'` works for every noun.
- ✗ One noun returns a bare array, another returns `{data:[…]}`.
- **Check:** `cli_json_contract::list_envelope_uniform`, `…::delete_envelope_uniform`.

#### CLI-R-04 — stdout carries data only; stderr carries everything else — **Tier 1** — CLIG, POSIX
Primary requested output → **stdout**. Progress, logs, confirmations, prompts, and errors
→ **stderr**. So `cmd -o json > out.json` produces clean JSON even amid progress noise.
- ✓ `flowplane api create … -o json 2>/dev/null` is valid JSON on stdout.
- ✗ A spinner or `✓ created` line written to stdout, corrupting piped JSON.
- **Check:** `cli_transcripts/streams_separated` (capture fds independently).

#### CLI-R-05 — `--quiet/-q` silences chrome, never data or errors — **Tier 1** — CLIG
`-q` suppresses progress and confirmation prose; it does **not** suppress the requested
resource output or error envelopes.
- ✓ `flowplane cluster delete web -q -o json` still prints the delete envelope and still errors loudly on failure.
- ✗ `-q` swallowing a 409 so a script sees exit 0.
- **Check:** `cli_transcripts/quiet_keeps_data_and_errors`.

#### CLI-R-06 — Color off on non-TTY, `NO_COLOR`, `--no-color`, or `TERM=dumb` — **Tier 1** — CLIG ([no-color.org](https://no-color.org))
Any of: stdout not a TTY, `NO_COLOR` set (any value), `--no-color`, or `TERM=dumb`
disables ANSI styling.
- ✓ `NO_COLOR=1 flowplane cluster list` emits no escape sequences.
- ✗ Honoring `--no-color` but ignoring the `NO_COLOR` env var.
- **Check:** `cli_transcripts/no_color_matrix` (asserts no `\x1b[` in any of the four cases).

#### CLI-R-07 — No prose-only commands; mutations/deletes are fully structured — **Tier 1** — CLIG, supersedes `spec/07 §1`
Every command that changes state (`create/update/delete/expose/attach/enable/rotate/…`)
supports `-o json` and emits a machine envelope.
- ✓ `flowplane expose http://… -o json` returns the created resources as JSON.
- ✗ `expose`/`delete`/`mcp enable` printing only human prose (v1 behaviour).
- **Check:** `cli_contract::no_command_is_prose_only`.

### Axis B — Input model

#### CLI-R-08 — Uniform spec input via `-f/--file`, accepting `-` for stdin — **Tier 1** — POSIX, kubectl, supersedes `spec/07` (`secret create`)
Every `create`/`update` accepts a spec via `-f <file>`; `-f -` reads stdin. No command
takes its spec as an ad-hoc inline JSON-string flag.
- ✓ `cat secret.yaml | flowplane secret create -f -`.
- ✗ `flowplane secret create --config '{"k":"v"}'` (inline JSON string, v1 behaviour).
- **Check:** `cli_contract::create_update_accept_file` + `cli_transcripts/dash_means_stdin`.

#### CLI-R-09 — Secrets never via flags — **Tier 1** — CLIG ("don't accept secrets through flags")
Token/credential/secret material is supplied via env, `--*-file`, stdin/`--from-stdin`, or
an interactive prompt — never as a flag value (which leaks into shell history, `ps`, logs).
- ✓ `FLOWPLANE_TOKEN=… flowplane …` or `flowplane secret create … --from-stdin < key.txt`.
- ✗ `flowplane --token sk-123 …` or `secret create --value sk-123`.
- **Check:** `cli_contract::no_secret_valued_flags` (denylist of flag names) + `cli_transcripts/token_flag_absent`.

#### CLI-R-10 — File **output** is `--out <path>`; `-o` is never a file — **Tier 2** — CLIG, supersedes `spec/07 §2`
Writing bytes to a file uses `--out <path>` (`--out -` = stdout). `-o/--output` is
reserved exclusively for *format*.
- ✓ `flowplane api spec export orders --out orders.yaml`; `wasm get f1 --out f1.wasm`.
- ✗ `learn export -o orders.yaml` (where `-o` means a path).
- **Check:** `cli_contract::dash_o_is_format_only` (asserts `-o` value-parser is the format enum everywhere).

#### CLI-R-11 — `--` terminates options — **Tier 2** — POSIX §12.2
Arguments after `--` are treated as literal operands, enabling names that begin with `-`.
- ✓ `flowplane cluster get -- -weird-name`.
- ✗ Treating `-weird-name` after `--` as an unknown flag.
- **Check:** `cli_transcripts/double_dash_terminator` (clap provides this; the test guards against `allow_hyphen_values` regressions).

### Axis C — Errors & exit codes

#### CLI-R-12 — Errors are `error (code): message` + `→ hint` + `request id` — **Tier 1** — CLIG, `spec/12 §4`
Server `{code,message,hint,request_id}` renders as three parts: a lowercase
`error (code): message` line, a `→` line whose hint is a copy-pasteable command, and a
`request id:` line. Raw HTTP/JSON bodies are never dumped on a TTY (full body via
`--verbose` or `-o json`).
- ✓ `error (resource_in_use): cluster "web" is referenced by 2 route configs` / `→ run flowplane cluster get web -o wide …` / `request id: 01JX…`.
- ✗ `HTTP request failed with status 409 Conflict: {"error":…}` (v1 behaviour).
- **Check:** `cli_transcripts/error_format_golden` per error class.

#### CLI-R-13 — Exit codes follow the §3 taxonomy — **Tier 1** — `sysexits` (reconciled), `spec/12 §1.6`
Process exit code is the §3 mapping; nothing is "always 1."
- ✓ A 404 exits `6`; a connection timeout exits `7`.
- ✗ Every failure exiting `1` (v1 monoculture).
- **Check:** `cli_exit_codes::class_to_code_*`.

#### CLI-R-14 — Per-class hint + privacy rules — **Tier 1** — CLIG, `spec/12 §4`
`401`→ hint `flowplane auth login` (exit 3); `403`→ names the missing `(resource, action)`
(exit 4); `404`→ never reveals cross-tenant existence (exit 6); `409/412`→ prints both
revisions (exit 5); transient → states it is retryable (exit 7).
- ✓ `403`: `error (forbidden): missing grant (clusters, create) for team payments`.
- ✗ `404` for a resource in another tenant leaking "exists but not yours."
- **Check:** `cli_transcripts/error_hints_by_class`, `cli_transcripts/no_cross_tenant_leak`.

#### CLI-R-15 — In `-o json`, errors are a JSON envelope on stderr — **Tier 1** — CLIG, kubectl
When `-o json` is active, a failure emits `{"error":{"code","message","hint","request_id"}}`
to **stderr** (stdout stays clean) and exits with the mapped code.
- ✓ `flowplane cluster get nope -o json 2>err.json; echo $?` → `6`, `err.json` parses.
- ✗ Human-formatted error text while `-o json` was requested.
- **Check:** `cli_transcripts/json_error_envelope`.

#### CLI-R-16 — Usage errors exit 2 and never hit the network — **Tier 1** — Clap, POSIX
Invalid flags/args are caught by clap and exit `2` before any request; the message points
at the offending token.
- ✓ `flowplane cluster get` (missing name) → clap usage error, exit `2`.
- ✗ Sending a malformed request to the server to discover the arg was missing.
- **Check:** `cli_exit_codes::usage_is_2`.

### Axis D — Config / auth precedence

#### CLI-R-17 — Uniform precedence `flag > env(FLOWPLANE_*) > config > default` for **every** value — **Tier 1** — CLIG, GNU, supersedes `spec/07 §1`
Token, team, org, server, timeout, context, and every future value resolve by the *same*
order. No per-field exceptions (v1 had four different orders).
- ✓ `--timeout` beats `FLOWPLANE_TIMEOUT` beats `config.toml` beats `30s`, and team/org/server resolve the same way.
- ✗ `timeout` having no env var while `base_url` resolves env-before-flag (v1).
- **Check:** `cli_precedence::uniform_order_for_all_values` (table-driven over every value).

#### CLI-R-18 — Auth via env / file / credential store / context; no token flag — **Tier 1** — CLIG, kubectl
Tokens come from `FLOWPLANE_TOKEN`, `--token-file`, the `~/.flowplane/credentials` store,
or the active `--context`. There is no `--token` value flag (CLI-R-09). This also removes
v1's "flag is lowest precedence" surprise.
- ✓ `flowplane config use-context prod` then `flowplane cluster list`.
- ✗ A `--token <TOKEN>` global flag.
- **Check:** `cli_contract::no_token_flag` + `cli_precedence::token_sources`.

#### CLI-R-19 — `--org`/`--team` resolve identically and travel as headers — **Tier 1** — `spec/12 §1.7`, CLIG
`--org`/`--team` use the CLI-R-17 order and are sent as `X-Flowplane-Org` / team path
scoping; an omitted org is server-inferred only when unambiguous.
- ✓ `FLOWPLANE_ORG=acme flowplane team list` with no `--org`.
- ✗ `--team` honouring flag>config>env while `--base-url` honours env>flag (v1 inversion).
- **Check:** `cli_precedence::org_team_uniform`.

### Axis E — Verb grammar

#### CLI-R-20 — Closed verb set; no bare top-level verbs — **Tier 2** — kubectl/gh, `spec/12 §1.1`
Verbs are `list|get|create|update|delete` plus noun-specific transitions
(`publish`, `rotate`, `attach`, …). The only bare top-level commands are `version`,
`completion`, and `help` (`expose`/`unexpose` are the two blessed shortcuts per `spec/12`).
- ✓ `flowplane api spec publish orders v2`.
- ✗ Top-level `flowplane doctor`, `flowplane status`, `flowplane validate` as bare verbs (v1).
- **Check:** `cli_contract::no_bare_top_level_verbs` (allowlist).

#### CLI-R-21 — Noun-verb, singular nouns, bounded top-level breadth — **Tier 2** — kubectl/gh, `spec/12 §1.1`
Resources are singular nouns; pseudo-noun read-wrappers are folded into a real noun or
`ops`. Target ≲ 12 top-level groups.
- ✓ `flowplane ops topology` instead of top-level `topology`; `route-views` folded into `route list -o wide`.
- ✗ `route-views`, `reports`, `stats`, `xds` as separate top-level pseudo-nouns (v1).
- **Check:** `cli_contract::top_level_group_count` + `…::nouns_are_singular`.

#### CLI-R-22 — `list` is paginated-plural; `get` is single — **Tier 2** — kubectl, gh
`list` returns a collection envelope and auto-paginates server cursors (unless `--limit`);
`get` returns exactly one resource by handle.
- ✓ `flowplane cluster list` walks `next_token`; `flowplane cluster get web` returns one.
- ✗ `list` capping silently at a hardcoded `limit=1000` (v1).
- **Check:** `cli_transcripts/list_autopaginates`.

### Axis F — Flag naming

#### CLI-R-23 — Long flags are kebab-case — **Tier 2** — GNU, POSIX
All long flags use `--kebab-case`.
- ✓ `--route-config`, `--max-duration`, `--from-openapi`.
- ✗ `--routeConfig`, `--max_duration`.
- **Check:** `cli_contract::long_flags_kebab_case` (regex over every flag id).

#### CLI-R-24 — Reserved single-char shorts, consistent meaning — **Tier 2** — CLIG, GNU
Short flags are limited to a reserved set with one meaning everywhere:
`-h`(help) `-o`(output) `-q`(quiet) `-v`(verbose) `-f`(file) `-y`(yes). No bespoke shorts.
- ✓ `-f` is always "input file."
- ✗ `-o` meaning "output file path" on one command and "format" on another (v1).
- **Check:** `cli_contract::short_flag_registry` (every short maps to its canonical long).

#### CLI-R-25 — No arbitrary abbreviation; exact match + typo suggestion — **Tier 2** — CLIG ("don't allow arbitrary abbreviations")
`infer_subcommands` / `infer_long_args` are **off**; an unknown subcommand/flag yields a
"did you mean" suggestion (clap's suggestions feature, on).
- ✓ `flowplane clustr list` → `error: unrecognized subcommand 'clustr'` / `tip: did you mean 'cluster'?`.
- ✗ `flowplane clu list` silently resolving to `cluster`.
- **Check:** `cli_transcripts/typo_suggests_nearest` + `cli_contract::inference_disabled`.

#### CLI-R-26 — Boolean flags default false; default-true exposed via `--no-x` — **Tier 2** — CLIG, Clap
Flags are opt-in booleans defaulting to false; any default-on behaviour is toggled off via
an explicit `--no-<name>` (clap `ArgAction::SetTrue` + paired negation).
- ✓ `--no-color`, `--no-input`.
- ✗ `--color=false` style or a bare flag whose absence means "on."
- **Check:** `cli_contract::bool_flags_default_false`.

### Axis G — Resource addressing

#### CLI-R-27 — One handle per resource; UUID accepted wherever a name is; parent scope via flags — **Tier 2** — kubectl, `spec/12 §1.4`, supersedes `spec/07 §3`
Resources are addressed by team-unique **name**; a UUID is accepted anywhere a name is
(server resolves both). Parent scoping is via flags (`--api`, `--route-config`), never
positional ambiguity. No resource is addressed *only* by an opaque id.
- ✓ `flowplane mcp enable --route web/api-vhost/r1` (name path) **or** a UUID.
- ✗ `mcp enable <ROUTE_ID>` accepting only a numeric/opaque route id (v1).
- **Check:** `cli_contract::addressing_accepts_name_and_uuid` (per resource arg).

### Axis H — Discoverability & help

#### CLI-R-28 — Every command's `--help` carries ≥1 runnable example — **Tier 2** — CLIG ("lead with examples")
Each leaf command sets `after_help`/`after_long_help` with at least one real, runnable
example.
- ✓ `flowplane api create --help` shows `flowplane api create orders --from-openapi orders.yaml --port 10002`.
- ✗ A command whose help has a synopsis and flags but no example.
- **Check:** `cli_contract::every_leaf_has_example`.

#### CLI-R-29 — `flowplane help <topic>` for workflows — **Tier 2** — CLIG, `spec/12 §1.9`
Multi-command workflows (learning, ai, tenancy) have a `help <topic>` page.
- ✓ `flowplane help learning` prints the capture→review→publish flow.
- ✗ Workflow knowledge living only in external docs.
- **Check:** `cli_transcripts/help_topics_exist`.

#### CLI-R-30 — `completion bash|zsh|fish` — **Tier 2** — CLIG, clap_complete
A `completion <shell>` command emits a completion script (dynamic resource-name
completion when authenticated is a stretch goal).
- ✓ `flowplane completion zsh > _flowplane`.
- ✗ No completion subcommand (v1).
- **Check:** `cli_transcripts/completion_emits_script`.

#### CLI-R-31 — Unknown command/flag suggests the nearest valid one — **Tier 2** — CLIG
(Companion to CLI-R-25; also covers misspelled global flags.)
- ✓ `--otput json` → `tip: did you mean '--output'?`.
- **Check:** covered by `cli_transcripts/typo_suggests_nearest`.

### Axis I — Safety, idempotency, feedback

#### CLI-R-32 — `--dry-run` on every mutating command; dry-run == real effect — **Tier 1** — kubectl (`--dry-run=server`), `spec/12 §1.3`
Every mutation accepts `--dry-run` (server-side validation + would-be result, no write).
Dry-run output equals the subsequent real mutation's effect.
- ✓ `flowplane route generate --from-spec billing/v1 --port 10003 --dry-run` prints the plan, writes nothing.
- ✗ A mutation with no dry-run, or a dry-run whose plan diverges from the real result.
- **Check:** `cli_contract::every_mutation_has_dry_run` + `cli_transcripts/dry_run_matches_real`.

#### CLI-R-33 — Destructive ops confirm on TTY; `--yes` bypasses; non-TTY without `--yes` errors — **Tier 2** — CLIG safety
`delete`/`--cascade`/`publish`-of-discovered prompt `[y/N]` on a TTY; `--yes/-y` bypasses;
on a non-TTY *without* `--yes` the op **fails fast** (it neither hangs nor silently proceeds).
- ✓ `flowplane cluster delete web --yes` in CI; interactive prompt locally.
- ✗ A delete that hangs forever waiting on stdin in CI, or proceeds without confirmation.
- **Check:** `cli_transcripts/destructive_requires_confirmation` (pty vs pipe).

#### CLI-R-34 — Global `--no-input`: never prompt, fail instead — **Tier 1** — CLIG ("provide a way to disable prompts")
`--no-input` (and a non-TTY by default) guarantees no command ever blocks on interactive
input; anything that would prompt instead errors with a hint naming the flag to supply.
- ✓ `flowplane cluster delete web --no-input` → `error (input_required): refusing to prompt; pass --yes to confirm` (exit 2).
- ✗ Any prompt reachable under `--no-input`.
- **Check:** `cli_transcripts/no_input_never_prompts` (asserts process never reads stdin).

#### CLI-R-35 — Uniform optimistic concurrency via `--revision N` — **Tier 1** — `spec/12 §1.5`, kubectl (resourceVersion), supersedes `spec/07 §3`
Every `update`/`delete` accepts `--revision N` → `If-Match`; without it the CLI does
read-modify-write and fails on a race with a 409 printing both revisions. Uniform across
all resources (v1 had it only for rate-limit).
- ✓ `flowplane cluster update web -f web.yaml --revision 4`.
- ✗ Last-writer-wins updates on every resource except rate-limit (v1).
- **Check:** `cli_contract::update_delete_accept_revision` + `cli_transcripts/revision_conflict_409`.

#### CLI-R-36 — Idempotent script affordances + progress feedback — **Tier 2** — kubectl (`--ignore-not-found`), CLIG feedback
`delete` accepts `--ignore-not-found` (exit 0 when absent) for idempotent scripts; long
operations show progress on **stderr**, with spinners only on a TTY.
- ✓ `flowplane cluster delete web --ignore-not-found` exits 0 if already gone.
- ✗ A 30s operation with no feedback, or a spinner emitted into piped output.
- **Check:** `cli_transcripts/ignore_not_found_idempotent`, `…/progress_on_stderr_tty_only`.

#### CLI-R-37 — `--timeout` honoured and env-backed; transient failures exit 7 — **Tier 1** — CLIG robustness, `sysexits` (reconciled)
`--timeout` (with `FLOWPLANE_TIMEOUT`, per CLI-R-17) bounds requests; a timeout or
transient transport/5xx/429 failure exits `7` and is labelled retryable.
- ✓ `flowplane --timeout 5 cluster list` against a stalled server → exit 7, "retryable."
- ✗ A timeout exiting 1 indistinguishably from a 404 (v1).
- **Check:** `cli_exit_codes::transient_is_7`.

---

## 5. New-command checklist

Copy-paste and tick before merging any new command or subcommand:

```
Output & streams
[ ] Accepts -o table|json|yaml|wide (CLI-R-01); format never auto-switches on TTY
[ ] -o json is snake_case and matches the REST contract (CLI-R-02)
[ ] Uses the standard list/get/delete envelope (CLI-R-03)
[ ] Data → stdout, progress/errors → stderr (CLI-R-04)
[ ] -q silences chrome only; honours NO_COLOR/--no-color/non-TTY (CLI-R-05, R-06)
[ ] If it mutates state, it is NOT prose-only — has -o json (CLI-R-07)

Input
[ ] create/update take -f/--file with - for stdin (CLI-R-08)
[ ] No secret-valued flags; no --token flag (CLI-R-09, R-18)
[ ] File output is --out, not -o (CLI-R-10)

Errors & exit codes
[ ] Errors render error (code): msg + → hint + request id (CLI-R-12)
[ ] Maps to the §3 exit-code taxonomy; usage = 2 (CLI-R-13, R-16)
[ ] -o json errors emit the JSON envelope to stderr (CLI-R-15)

Config / precedence
[ ] Every new value resolves flag > env(FLOWPLANE_*) > config > default (CLI-R-17)

Grammar / flags / addressing
[ ] Noun-verb; verb is in the closed set or a named transition (CLI-R-20)
[ ] Long flags kebab-case; only reserved shorts (CLI-R-23, R-24)
[ ] Addressed by name; UUID accepted too; parent scope via flags (CLI-R-27)

Safety / discoverability
[ ] Mutations have --dry-run; dry-run == real effect (CLI-R-32)
[ ] Destructive → confirm on TTY, --yes bypass, non-TTY errors (CLI-R-33)
[ ] Honours --no-input (never blocks) (CLI-R-34)
[ ] update/delete accept --revision (CLI-R-35)
[ ] --help carries ≥1 runnable example (CLI-R-28)
[ ] A named check in §4 covers each rule this command touches
```

---

## 6. Enforcement harnesses (stubs to implement)

These are the named checks the rules reference. None exist yet as a unified suite; each is
a contributor-implementable stub.

1. **`tests/cli_contract.rs`** — builds the Clap `Command` (`Cli::command()`), walks it
   recursively, and asserts structural invariants without spawning a process. Powers
   R-01, R-07, R-08, R-09, R-10, R-18, R-20..28, R-32, R-35. *Intent: a violating PR fails
   at `cargo nextest` because the new subcommand breaks a tree invariant.*
2. **`tests/cli_transcripts/`** — `trycmd`/`assert_cmd` golden files plus `insta`
   snapshots for output, error text, and pty-vs-pipe behaviour. Powers R-01, R-04, R-05,
   R-06, R-08, R-11, R-12, R-14, R-15, R-22, R-25, R-29, R-30, R-32, R-33, R-34, R-36.
3. **`tests/cli_exit_codes.rs`** — `assert_cmd` asserting `.code(N)` per error class
   against a stub/mock server. Powers R-13, R-16, R-37.
4. **`tests/cli_json_contract.rs`** — `insta` JSON-shape snapshots; the diff is the
   change-control gate (additive-only). Powers R-02, R-03.
5. **`tests/cli_precedence.rs`** — table-driven unit tests over the single value-resolution
   function. Powers R-17, R-19, R-18.

**Uncheckable-rule honesty:** R-21's "≲ 12 top-level groups" is a *guideline*, not a hard
gate — the count threshold is a judgment call; the check asserts only "no new top-level
group without a noun rationale in the PR description," which is advisory. R-29's "help is
*useful*" can be checked for *existence* but not *quality*; treat the quality bar as review,
not CI.

---

## 7. Open design decisions — see `spec/16-cli-conformance.md` §Open and the inline list

The forks where the authorities are silent or in conflict are enumerated as **DD-1..DD-6**
in the conformance document and surfaced inline by the authoring task. They are *not*
silently resolved into the rules above; each rule that depends on one names it.
</content>
</invoke>
