# 16 — Flowplane CLI Standard (enforceable)

A single, enforceable standard governing every current and future `flowplane` command. Where
spec/12 stated intent, this document turns that intent into pass/fail rules a reviewer can apply to
a PR, names the check for each, and records the audit deviations it governs. Companion audit:
Deliverable 1 of the CLI design review (finding ids `AX-NN` are referenced throughout).

---

## 1. Status — relationship to spec/12 and spec/07

This standard **adopts** the binding rules of `spec/12-cli-design.md` as the design baseline and
**extends** them into checkable rules. It treats `spec/07-cli-and-workflows.md` §3/§6 as the v1
defect list and verifies each defect against current code (some are fixed; see §5).

Rule-level relationship to spec/12 §1 (binding rules) and §2/§3:

| spec/12 reference | this standard | note |
|---|---|---|
| §1.1 noun-verb, uniform verbs, no bare top-level verbs | **adopts + extends** → CLI-R-01, CLI-R-02 | adds the explicit blessed-exception list and the "full CRUD or documented omission" test |
| §1.2 `-o/--output`, `--json` sugar, `--out` file, scriptable mutations | **adopts + extends** → CLI-R-05, CLI-R-07, CLI-R-08 | adds TTY/pipe default and the `wide` distinctness requirement |
| §1.3 `--dry-run` everywhere, destructive prompts, `--yes` | **adopts** → CLI-R-13, CLI-R-15 | code defines `--yes` but never honors it (SAFE-01) |
| §1.4 one addressing scheme, name+UUID, no positional ambiguity | **adopts + extends** → CLI-R-10 | adds mandatory percent-encoding of every path segment (ADDR-01) |
| §1.5 revisions `--revision`→If-Match, RMW fallback | **adopts** → CLI-R-14 | RMW only implemented in `apply` today (SAFE-02) |
| §1.6 error format + exit codes 0–7 | **adopts + extends** → CLI-R-11, CLI-R-12 | adds: client-side errors use the same format; code 7 reserved for transport |
| §1.7 config precedence `flag > env > file > default`, contexts | **adopts + extends** → CLI-R-09 | tightens to one chain incl. token & timeout (CFG-01/02/03) |
| §1.8 completions | **adopts** → CLI-R-01 (exceptions) | static completion exists; dynamic name completion remains future work |
| §3 global flags, `--limit/--page-token`, auto-paginate | **adopts** → CLI-R-16 | most lists have no paging today (PAGE-01) |

**Supersedes (named):** this standard **supersedes** the following spec/12 §2 command-tree lines
where current code has deliberately diverged and the divergence is the better design — they are
restated here as the canonical form, not silently contradicted:
- spec/12 §2 `ai  provider … | route … | budget … | usage show` — pluralization/verbs are an open
  reconciliation item; **CLI-R-01** fixes the canonical form and **VERB-02** must be resolved to it.
- spec/12 §2 `config  init | show | set | unset | path | use-context | get-contexts` — superseded by
  the context-centric form `path | show | set-context | use-context | get-contexts` (**VERB-06**).
- spec/12 §2 `admin … rls repush` — there is no `admin` namespace; **CLI-R-04** requires choosing one
  home for platform-admin verbs (**VERB-04**).

Everything else in spec/12 stands. No rule below contradicts an un-named spec/12 rule.

---

## 2. Principles (≤7)

1. **One grammar:** every command is `flowplane <noun> [<sub-noun>] <verb> [handle] [flags]`; the
   verb vocabulary is closed and uniform.
2. **Scriptable by default:** any command's output is machine-readable with `-o json`, and piping
   selects JSON automatically; no command is prose-only.
3. **One precedence rule:** every configurable value resolves `flag > env > context > file >
   default`, with no exceptions and no per-value reordering.
4. **One handle:** a resource is addressed by its name (UUID accepted wherever a name is), and every
   value interpolated into a URL path is percent-encoded.
5. **Safe mutations:** destructive actions confirm on a TTY and honor `--yes`; concurrent edits use
   revisions; `--dry-run` previews every mutation.
6. **Errors are uniform and typed:** one `error (code): message → hint` format for every failure
   (HTTP or local), with exit codes spanning the 0–7 taxonomy.
7. **No dead controls:** a flag that exists is honored; a format that is offered does something
   distinct.

---

## 3. Rules (`CLI-R-NN`)

### CLI-R-01 — Noun-verb grammar and closed verb vocabulary
Every command is a noun (or noun → sub-noun) followed by a verb from the closed set
`list | get | create | update | delete` plus declared noun-specific transitions
(`status`, `rotate`, `generate`, `apply`, `bootstrap`, `enable`, `disable`, `publish`, `reject`,
`stop`, `cancel`, `generate-spec`, `issue`, `revoke`, `member`, `grant`, `usage show`, …). Sub-nouns
are **singular**. The only blessed bare top-level commands are `version`, `completion`, `expose`,
`unexpose`, `apply`, `serve`, `db`. Any other bare top-level verb is a violation.
- **Conforming:** `flowplane ai provider create openai-prod …`; `flowplane cluster list`.
- **Violating:** `flowplane openapi` (bare verb — VERB-01); `flowplane ai providers list` (plural
  sub-noun — VERB-02); `flowplane rate-limit force-repush` (bare verb under a noun — VERB-04).
- **Enforced by:** a transcript test enumerating the full command tree and asserting each top-level
  name is in the blessed set or has subcommands; `Cli::command().debug_assert()`
  (`main.rs:221`) for structural validity.

### CLI-R-02 — Full CRUD or a documented omission
Each resource noun ships the complete `list | get | create | update | delete` set. A missing verb is
allowed **only** when the REST API cannot support it, and then the omission is documented in the
command's `--help` long-text with the API reason.
- **Conforming:** `cluster` exposes all five (`commands.rs:156-185`).
- **Violating:** `team` has no `get`/`update`; `secret` has no `update`/`delete`; `dataplane` has no
  `update`/`delete`/`status` with no documented reason (VERB-03).
- **Enforced by:** a test that, for every noun in a canonical list, asserts each CRUD verb is either
  present or annotated with a `# cli-omit: <reason>` marker the test recognizes.

### CLI-R-03 — kebab-case flags; never re-declare a global flag locally
All long flags are kebab-case (clap derive handles this from snake_case fields). The documented name
is the canonical `#[arg(long)]`; alternates are `alias`. No subcommand may declare a flag whose long
name collides with a `global = true` flag.
- **Conforming:** `--from-openapi`, `--display-name`; `#[arg(long, alias = "device-code")]` only if
  `--device` is the documented name.
- **Violating:** `secret rotate` re-declaring `--revision` as a required local arg over the global
  optional `--revision` (FLAG-01); documenting `--device-code` while `--device` is canonical
  (FLAG-03).
- **Enforced by:** `Cli::command().debug_assert()` catches duplicate arg ids; a lint/test asserting
  no leaf subcommand defines an arg id already marked `global = true` in `GlobalOptions`.

### CLI-R-04 — One home for platform-admin verbs
Platform-admin operations (force-repush, schema reload, tombstone reap, app toggles) live under a
single declared namespace (`ops` or a new `admin`), not scattered as bare verbs under feature nouns.
- **Conforming:** `flowplane ops rls repush` (or `admin rls repush`), one namespace.
- **Violating:** `flowplane rate-limit force-repush` (VERB-04).
- **Enforced by:** the CLI-R-01 command-tree test plus a review checklist item.

### CLI-R-05 — `-o` is format, `--out` is a file, `-f` is input
`-o/--output` selects `table|json|yaml|wide` only. `--out <path>` writes output to a file. `-f/--file`
reads an input body (`-` = stdin). `--json` is sugar for `-o json`. None of these three letters is
ever overloaded for another meaning.
- **Conforming:** `apply -f gateway.json`; `dataplane bootstrap … --out envoy.yaml`;
  `cluster get api -o json` (`config.rs:23,42`, `commands.rs:170`).
- **Violating:** any command where `-o` means a file path (the v1 `learn export -o` / `schema export
  -o` collision called out in spec/07 §3 — verified **fixed**, no longer present).
- **Enforced by:** transcript tests already assert `apply -f` and `--out` forms (`main.rs:247,304`);
  add one asserting `-o json` parses on a representative leaf.

### CLI-R-06 — Two create shapes, applied consistently
A `create`/`update` takes input in exactly one of two forms, chosen by resource class and used
uniformly: **(a) spec resources** (cluster, listener, route, secret, ai\*, rate-limit, cert) take
`-f/--file` with the name inside the body; **(b) scalar resources** (org, team, api, dataplane) take
the primary identity as a **positional** and attributes as typed flags. The same logical input never
appears as a positional in one command and a flag in another.
- **Conforming:** `cluster create -f c.json`; `team create payments --display-name "Payments"`.
- **Violating:** `org member add … --email` (flag) vs `team member add <email>` (positional) for the
  same input (IN-01); inconsistent create shapes across nouns (IN-02).
- **Enforced by:** a review-checklist item plus a test asserting member/grant "email" is positional
  everywhere (or a flag everywhere) across the member/grant command set.

### CLI-R-07 — Every command supports `-o`; default table on TTY, JSON when piped; `wide` is distinct
Every command that emits a resource or status supports `-o table|json|yaml|wide`. On a TTY with no
explicit `-o`/`--json`, the default is `table`; when stdout is **not** a TTY, the default is `json`.
`wide` must render strictly more columns than `table`, or it must not be offered. `--no-color` and
`--quiet` are honored wherever they are meaningful.
- **Conforming:** `cluster list | jq .` yields JSON without `--json`; `cluster list -o wide` shows
  extra columns.
- **Violating:** `format()` defaults to `Table` unconditionally with no `is_terminal()` check
  (OUT-02); `wide` maps to the same renderer as `table` (`output.rs:118`, OUT-01); `--no-color`
  defined but never read (`config.rs:28`, FLAG-02); `config show`/`get-contexts` ignore `-o`
  (`mod.rs:550,588-642`, OUT-04).
- **Enforced by:** a unit test that `format()` returns `Json` when `stdout` is not a terminal and no
  flag is set; a test that `wide` output column count > `table` for a multi-column resource.

### CLI-R-08 — Mutations: one-line confirm on TTY, full resource under `-o json`
A successful mutation prints a single human confirmation line on a TTY (`created "x" (revision N)`)
and the full resource body when `-o json`/`-o yaml` is requested or when piped. Mutation commands do
not dump the full body on a bare TTY.
- **Conforming:** `print_mutation_summary` path for POST/PATCH/DELETE (`output.rs:494-523`,
  `client.rs:161`).
- **Violating:** `expose`/`unexpose`/`route generate`/`route apply` always render the full body via
  `request_and_render` even on a TTY (OUT-03).
- **Enforced by:** a test asserting a mutating command's default TTY stdout is the one-line form, and
  that `-o json` returns parseable JSON of the resource.

### CLI-R-09 — One config precedence chain for every value
Every configurable value (server, org, team, token, timeout, output, …) resolves in the order
`flag > env (FLOWPLANE_*) > context > file > default`, implemented by one shared resolver. Token has
a `--token`/`--token-file` flag at the top of its chain; timeout reads `FLOWPLANE_TIMEOUT`.
- **Conforming:** `org` resolution `flag > FLOWPLANE_ORG > context > file` (`config.rs:217-222`).
- **Violating:** token chain is env-first with no flag (CFG-01); `--timeout` is flag/default only
  (CFG-02); `--server` env via clap `env=` vs org/team env via manual `std::env::var` (CFG-03).
- **Enforced by:** a table-driven test feeding (flag, env, context, file) for each value and
  asserting the resolved precedence is identical across all values.

### CLI-R-10 — One handle; every path segment percent-encoded
A resource is addressed by name; a UUID is accepted wherever a name is and resolved server-side.
Parent scoping is via flags (`--domain`, `--route-config`), never positional ambiguity. **Every**
value interpolated into a request path is percent-encoded with the shared encoder.
- **Conforming:** rate-limit/learn/api-spec/mcp use `query_component` on every segment
  (`mod.rs:1047-1051,1310,1439`).
- **Violating:** `run_resource` interpolates `{name}` raw for clusters/listeners/route/secret/api-get
  (`mod.rs:865,885,895,1283,1703`), so a name with `/` or space breaks (ADDR-01); five distinct
  handle types across the tree (ADDR-02).
- **Enforced by:** extend the existing `query_component_encodes_path_unsafe_chars` test
  (`mod.rs:2545`) into a per-noun test that a name containing `/` round-trips for every CRUD path.

### CLI-R-11 — One error format for every failure, with injected hints
Every failure — HTTP or client-side — renders as `error (code): message`, then an optional
`  → hint` line, then `  request id: <id>` when present. With `-o json` the machine envelope goes to
stderr. Canonical client-side hints are injected by status when the server omits one (401 →
`flowplane auth login`; 403 → the missing `(resource, action)`).
- **Conforming:** HTTP path in `render_error` (`output.rs:41-47`).
- **Violating:** client-side `anyhow` errors print `Error: {err:?}` and bypass the format
  (`main.rs:154`, ERR-01); no client-side 401 hint injection (ERR-04); hint uses ASCII `->` not `→`
  (`output.rs:43`, ERR-03).
- **Enforced by:** a test that a simulated client-side error (e.g. missing team) renders in
  `error (code): message` form; a test asserting a 401 with no server hint still prints the login
  hint.

### CLI-R-12 — Exit-code taxonomy 0–7
Exit codes: `0` success; `2` auth (401/403); `3` not-found/conflict/precondition (404/409/412); `4`
bad-request/validation (400/422); `5` rate-limited (429); `6` server error (5xx); `7`
network/transport/timeout; `1` reserved for unexpected/internal CLI errors only.
- **Conforming:** HTTP status mapping (`output.rs:103-112`) for codes 2–6.
- **Violating:** code `7` is never emitted — transport/timeout errors collapse to `1`
  (`main.rs:154-156`, ERR-02).
- **Enforced by:** extend `http_statuses_map_to_scriptable_exit_codes` (`output.rs:552`) to assert a
  transport error maps to `7`.

### CLI-R-13 — Destructive commands confirm; `--yes` is honored
`delete`, cascade, and `publish` of discovered specs prompt for confirmation on a TTY. `--yes/-y`
skips the prompt; in a non-TTY the command fails unless `--yes` is given. `--yes` must be read.
- **Conforming:** `cluster delete payments-db` prompts `proceed? [y/N]`; `cluster delete payments-db
  --yes` does not.
- **Violating:** `--yes/-y` is defined `global = true` but never read; no command prompts (SAFE-01,
  `config.rs:35`).
- **Enforced by:** a test that a destructive command in a non-TTY without `--yes` exits non-zero
  without calling the API, and with `--yes` proceeds.

### CLI-R-14 — Revisions and read-modify-write
`update`/`delete`/`rotate` accept `--revision N`, sent as `If-Match`. When `--revision` is omitted,
the CLI does read-modify-write (GET current revision, resend it) and surfaces a 409 printing both
revisions on a race.
- **Conforming:** `apply` reads `revision` from the existing resource and sends it
  (`mod.rs:2067`).
- **Violating:** interactive `update`/`delete` send no `If-Match` without `--revision`
  (last-writer-wins) (SAFE-02, `client.rs:33`).
- **Enforced by:** a test that `update` without `--revision` issues a GET then a PATCH carrying the
  fetched revision.

### CLI-R-15 — `--dry-run` on every mutating command
Every mutating command supports `--dry-run`: server-side validation and the would-be result, no
write. `--dry-run` output matches the subsequent real mutation's effect.
- **Conforming:** `request_inner` short-circuits non-GET under `dry_run` and prints a plan
  (`client.rs:121-125`); `apply --dry-run`/`--diff` (`mod.rs:2014`).
- **Violating:** none currently — this rule guards future commands that bypass `request_inner`.
- **Enforced by:** a test that each mutating leaf, run with `--dry-run`, performs no network write.

### CLI-R-16 — Uniform pagination
Every `list` accepts `--limit` and `--page-token` (server cursor) and auto-paginates to completion
unless `--limit` is given. Offset paging and hardcoded caps are prohibited.
- **Conforming (target):** `cluster list --page-token <t>`; bare `cluster list` returns all pages.
- **Violating:** `ResourceCommand::List` has no paging flags (`commands.rs:157-161`); `learn
  list`/`ai usage` use `--limit/--offset` (`commands.rs:252-255,488-491`); none auto-paginate
  (PAGE-01).
- **Enforced by:** a test that every `list` leaf exposes `--limit`/`--page-token` and that a bare
  `list` follows `next_page_token` until exhausted.

---

## 4. New-command checklist (copy-paste before adding any command)

```
[ ] CLI-R-01  Noun (singular sub-noun) + verb from the closed set; no new bare top-level verb.
[ ] CLI-R-02  Ships list|get|create|update|delete, or documents the omission with the API reason.
[ ] CLI-R-03  Flags kebab-case; documented name is canonical; no local re-decl of a global flag.
[ ] CLI-R-05  -o = format, --out = file, -f = input; nothing overloaded.
[ ] CLI-R-06  Input is shape (a) -f spec OR (b) positional name + typed flags — matching its class.
[ ] CLI-R-07  Supports -o table|json|yaml|wide; TTY→table, pipe→json; wide adds columns.
[ ] CLI-R-08  Mutation prints one-line confirm on TTY, full resource under -o json.
[ ] CLI-R-09  Every new config value flows through the shared flag>env>context>file>default resolver.
[ ] CLI-R-10  Name-addressed (UUID accepted); EVERY path segment percent-encoded.
[ ] CLI-R-11  Failures render error (code): message → hint; request id; client-side too.
[ ] CLI-R-12  Maps its failures onto the 0–7 exit-code taxonomy (7 = transport).
[ ] CLI-R-13  Destructive? prompts on TTY and honors --yes; fails closed in non-TTY without --yes.
[ ] CLI-R-14  Mutating? accepts --revision (If-Match) and does RMW when omitted.
[ ] CLI-R-15  Mutating? supports --dry-run with no write.
[ ] CLI-R-16  list? exposes --limit/--page-token and auto-paginates.
[ ] Tests: a transcript form in main.rs; a render/exit-code assertion; help text with an example.
```

---

## 5. Migration appendix — audit findings → governing rule

`needs-change` = current code violates the rule; `conforms` = verified compliant (kept for the diff).

| audit id | governing rule | status |
|---|---|---|
| ADDR-01 | CLI-R-10 | needs-change |
| ADDR-02 | CLI-R-10 | needs-change |
| CFG-01 | CLI-R-09 | needs-change |
| CFG-02 | CLI-R-09 | needs-change |
| CFG-03 | CLI-R-09 | needs-change |
| ERR-01 | CLI-R-11 | needs-change |
| ERR-02 | CLI-R-12 | needs-change |
| ERR-03 | CLI-R-11 | needs-change |
| ERR-04 | CLI-R-11 | needs-change |
| FLAG-01 | CLI-R-03 | needs-change |
| FLAG-02 | CLI-R-07 | needs-change |
| FLAG-03 | CLI-R-03 | needs-change |
| IN-01 | CLI-R-06 | needs-change |
| IN-02 | CLI-R-06 | needs-change |
| IN-03 | CLI-R-06 | needs-change |
| OUT-01 | CLI-R-07 | needs-change |
| OUT-02 | CLI-R-07 | needs-change |
| OUT-03 | CLI-R-08 | needs-change |
| OUT-04 | CLI-R-07 | needs-change |
| OUT-05 | CLI-R-07 | needs-change |
| PAGE-01 | CLI-R-16 | needs-change |
| SAFE-01 | CLI-R-13 | needs-change |
| SAFE-02 | CLI-R-14 | needs-change |
| VERB-01 | CLI-R-01 | needs-change |
| VERB-02 | CLI-R-01 | needs-change |
| VERB-03 | CLI-R-02 | needs-change |
| VERB-04 | CLI-R-04 | needs-change |
| VERB-05 | CLI-R-01 | needs-change |
| VERB-06 | CLI-R-01 | needs-change (superseding spec/12 §2 config line) |
| (conform) kebab-case derivation | CLI-R-03 | conforms |
| (conform) -o/--out/-f separation | CLI-R-05 | conforms |
| (conform) secret create uses -f | CLI-R-06 | conforms |
| (conform) rate-limit/learn segment encoding | CLI-R-10 | conforms |
| (conform) --dry-run via request_inner | CLI-R-15 | conforms |
| (conform) error envelope JSON on stderr | CLI-R-11 | conforms |

---

## 6. Open verification items (carried from the audit)

1. **Constitution alignment unverified** — `../flowplane-private-vault/constitution.md` was not
   accessible when this standard was authored. No rule here proposes a code change (audit-and-document
   only), but before any rule is implemented it must be checked against the constitution per the
   `/aidf:design-review` gate.
2. **FLAG-01 build behavior** — confirm whether `secret rotate`'s local `--revision` duplicates the
   global arg id at build (`cargo test command_tree_builds`) or shadows it at runtime; either way the
   semantics are non-uniform and CLI-R-03 governs the fix.
