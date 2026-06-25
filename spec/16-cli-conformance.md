# 16 — CLI Conformance Backlog

Measures the **current** `flowplane` client against `spec/16-cli-standard.md`. Every
finding cites `path:line` in the actual source (`crates/flowplane/src/main.rs`,
`crates/flowplane/src/cli/{mod,commands,config,output,client}.rs`). Findings with no
`path:line` were dropped.

`key = axis + command + path:line`. `verdict ∈ conforms | needs-change`.
Sorted by **tier**, then **rule**.

> **Grounding note.** This is a different, much smaller tree than the v1 documented in
> `spec/07` (5 CLI files, ~4.8k lines; ~22 top-level commands). Several `spec/07` smells
> are **already fixed here** and are recorded below as `conforms` so a re-run does not
> re-flag them: uniform `-f/--file` input (incl. `secret create`), `--out` for file output
> (no `-o`-as-path overload), a real differentiated exit-code map, a structured
> `error (code): message → hint` renderer, JSON error envelopes, and largely-uniform
> config precedence.

## Command tree (enumerated before judging)

```
flowplane [GLOBALS] <command>
GLOBALS (config.rs:13-43, all global=true): --context --server(env FLOWPLANE_SERVER)
  --team --org -o/--output{table,json,yaml,wide} --json --no-color --quiet --verbose
  --dry-run -y/--yes --revision --timeout(=30) --out

serve | db migrate | openapi | version | completion <shell>            (main.rs:31-136)
auth      whoami | token | login{--token,--token-stdin,--device,--pkce,…} | logout
config    path | show | set-context <name> --server … | use-context <name> | get-contexts
org       list | get <org> | create <name> | delete <org> | member{list|add|remove}
team      list | create <name> | delete --team | member{list|add|remove} | grant{list|add|remove}
cluster   list | get | create -f | update -f | delete            (ResourceCommand)
listener  list | get | create -f | update -f | delete            (ResourceCommand)
route     list | get | create -f | update -f | delete | generate --from-spec | apply <plan_id>
api       list | get | status | spec{reject|publish} | create [--from-openapi] | delete
mcp       status | connections | enable --api | disable --api
ai        providers/routes/budgets (ResourceCommand) | usage
learn     discover{start|list|status|stop|generate-spec} | start | list | get | stop | generate-spec | cancel
secret    list | get | create -f | rotate --revision -f
dataplane list | get | create | telemetry -f | bootstrap(alias envoy-config) | cert{list|register|issue|revoke}
expose <upstream> --name … | unexpose <name>
stats     overview
ops       xds{status|nacks} | trace
apply     -f <file> [--diff] [--prune]
```

## Findings

| key | rule (CLI-R-NN) | tier | command(s) | evidence (path:line) | deviation | verdict | migration cost |
|---|---|---|---|---|---|---|---|
| output+global+config.rs:99 | CLI-R-01 | 1 | all | config.rs:99-105 | `format()` defaults `Table`, never auto-switches on TTY/pipe; `--json` is sugar. Exactly the standard's explicit-format model. | conforms | — |
| output+wide+output.rs:118 | CLI-R-01 | 1 | all (`-o wide`) | output.rs:118 | `Wide` maps to the same `table()` as `Table` — `-o wide` is identical to `-o table`, adds no columns. | needs-change | S (drop or implement wide column set) |
| json+global+output.rs:114 | CLI-R-02 | 1 | all `-o json` | output.rs:114-126 | JSON is `serde_json::to_string_pretty` passthrough of the server body — snake_case inherited from REST contract; no CLI re-shaping. Stability rides on the server contract. | conforms | — |
| json+yaml+output.rs:464 | CLI-R-02 | 1 | all `-o yaml` | output.rs:464-492 | `yaml_like` is a hand-rolled emitter with no string quoting/escaping; values containing `:`/newlines and some scalars are not guaranteed valid YAML → not reliably `yq`-round-trippable. | needs-change | M (serialize via `serde_yaml`) |
| output+delete+client.rs:145 | CLI-R-03 | 1 | `* delete`, empty-body mutations | client.rs:145-150; output.rs:494-523 | 204/empty responses bypass `render()` and always print the prose `print_mutation_summary`, even under `-o json` → no machine envelope for deletes. | needs-change | M (emit `{status:"deleted",…}` honoring format) |
| output+global+output.rs:521 | CLI-R-04 | 1 | all mutations | output.rs:521 vs 37-47 | Errors go to stderr (good), but the human mutation summary `created "x"` is `println!` to **stdout**; mixes chrome into the data stream when `-o json` is not set. | needs-change | S (route summary to stderr) |
| output+global+output.rs:37 | CLI-R-05 | 1 | all | output.rs:37-47,500; client.rs:146 | `--quiet` suppresses the mutation summary only; rendered data (`render`) and errors (`render_error`) ignore quiet. | conforms | — |
| output+global+config.rs:28 | CLI-R-06 | 1 | all | config.rs:28; (no readers in cli/) | No ANSI is emitted anywhere, so non-TTY/NO_COLOR are trivially satisfied — **but** `--no-color` is a declared-yet-unread dead flag and `NO_COLOR`/`TERM=dumb`/`is_terminal` are never consulted (latent risk the moment color is added). | needs-change | S (wire or remove the flag; add `NO_COLOR`) |
| output+delete+client.rs:145b | CLI-R-07 | 1 | `* delete`, `mcp enable/disable` | client.rs:145-150 | Mutations returning a body **do** render JSON under `-o json` (client.rs:152-163) — good; but empty-response mutations (delete, enable/disable when 204) are prose-only and unscriptable. | needs-change | M (same fix as CLI-R-03) |
| input+create+commands.rs:170 | CLI-R-08 | 1 | `cluster/listener/route/secret/dataplane … create/update/rotate/telemetry`, `cert register`, `apply` | commands.rs:170-171,201-202,474-475,483-484,510-511,588-589,649; mod.rs:33-43 | All spec input is `-f/--file`; `body_from_file` accepts `-` for stdin and parses JSON **or** YAML. Fully matches the standard (and fixes the v1 `secret --config` smell). | conforms | — |
| input+auth-login+commands.rs:9 | CLI-R-09 | 1 | `auth login`, `config set-context` | commands.rs:9-10,41-42 | A `--token <TOKEN>` value flag exists on both (leaks into shell history / `ps`). A `--token-stdin` alternative is offered, so the flag is avoidable but still present. | needs-change | S (hide/deprecate `--token`; keep `--token-stdin`) |
| input+global+config.rs:42 | CLI-R-10 | 2 | all (file output) | config.rs:42; output.rs:120-124; client.rs:104-108 | File output is the global `--out` (`-o` is format-only everywhere). The v1 `-o`-as-path overload does not exist in this tree. | conforms | — |
| errors+global+output.rs:41 | CLI-R-12 | 1 | all | output.rs:28-50 | Non-JSON errors render `error (code): message` + `  -> hint` + `  request id:`; raw bodies are not dumped (full body only via `-o json`). Matches `spec/12 §4`. | conforms | — |
| exit+global+output.rs:103 | CLI-R-13 | 1 | all | output.rs:103-112; main.rs:144-151 | A real differentiated map exists, but the numbering diverges from the §3 taxonomy **and** from `spec/12 §4`'s own example (`resource_in_use`/409 → "exit 5" in spec; code maps 409→**3**). Current: 401/403→2, 404/409/412→3, 400/422→4, 429→5, 5xx→6, else→1. | needs-change | M (renumber per §3 / DD-2) |
| errors+global+output.rs:70 | CLI-R-14 | 1 | all | output.rs:42,70-74 | Hints are shown only when the **server** supplies a `hint` field; the CLI injects no client-side per-class hint (401→`auth login`, 403→missing `(resource,action)`). No-cross-tenant-leak for 404 is not enforced client-side (relies on server). | needs-change | M (add per-status hint table) |
| errors+global+output.rs:35 | CLI-R-15 | 1 | all `-o json` | output.rs:35-39 | Under `-o json`, failures emit the JSON `{code,message,status,request_id}` envelope to **stderr** and still return `CliHttpError`. Matches the standard. | conforms | — |
| exit+global+output.rs:105 | CLI-R-16 | 1 | all | output.rs:105 | Clap usage errors exit `2` (good), but 401/403 are **also** mapped to `2` → a script cannot distinguish "bad invocation" from "auth/forbidden." | needs-change | S (move 401/403 off 2 per §3) |
| config+timeout+config.rs:39 | CLI-R-17 | 1 | all | config.rs:39-40,200-242; client.rs:18 | server/org/team/token resolve uniformly `flag > env > context > file > default` (a real improvement over v1) — **but** `--timeout` has no env var and is read from neither context nor config (flag-or-default only), breaking "one precedence for every value." | needs-change | S (add `FLOWPLANE_TIMEOUT` + config/context) |
| config+global+config.rs:200 | CLI-R-18 | 1 | all | config.rs:200-209 | No global `--token` flag; token resolves from `FLOWPLANE_TOKEN` → context → config → credentials file. Contexts (`--context`) work like kubeconfig. | conforms | — |
| config+global+client.rs:189 | CLI-R-19 | 1 | all | client.rs:189-191; config.rs:217-228 | `--org` travels as `X-Flowplane-Org`; team via path scoping; both resolve by the uniform order. | conforms | — |
| safety+global+client.rs:121 | CLI-R-32 | 1 | all mutations | client.rs:84-89,121-125 | `--dry-run` prints a **local** `{method,path,body}` echo and returns without calling the server — it performs no server-side validation and the "plan" is the request, not the would-be result, so dry-run ≠ real effect. | needs-change | L (server `?dry_run` round-trip) |
| safety+global+config.rs:13 | CLI-R-34 | 1 | all | config.rs:13-43 | No `--no-input` flag. In practice nothing prompts (so scripts never block), but the guarantee is implicit, not contractual, and there is no TTY detection to fall back from. | needs-change | S (add `--no-input`, tie to DD-6) |
| concurrency+global+client.rs:192 | CLI-R-35 | 1 | `* update/delete`, `secret rotate` | client.rs:192-194; config.rs:37-38; commands.rs:481-482 | Global `--revision` → `If-Match` exists, but: (a) no read-modify-write fallback when omitted; (b) `secret rotate` declares its **own** required `--revision` (commands.rs:481), which overlaps the global flag of the same name. | needs-change | M (RMW fallback; resolve the duplicate flag) |
| exit+global+main.rs:149 | CLI-R-37 | 1 | all | main.rs:144-151; output.rs:109-112; config.rs:39 | Transient transport failures (timeout, connection refused) are **not** `CliHttpError`, so they fall through to `eprintln!("Error: …"); exit(1)` — indistinguishable from a generic failure; 5xx maps to 6, not a retryable code; no env-backed timeout. No "retryable" class. | needs-change | M (classify transient → retryable code) |
| grammar+top-level+main.rs:31 | CLI-R-20 | 2 | root | main.rs:31-136 | Verbs are noun-scoped; the only bare top-level verbs are the blessed `expose`/`unexpose` plus `version`/`completion`/`openapi`/`serve`/`db`. Matches the standard's allowlist. | conforms | — |
| grammar+top-level+main.rs:31b | CLI-R-21 | 2 | root | main.rs:31-136; commands.rs:607-613 | ~22 top-level groups (within the ≲12-noun *spirit* once server/db/openapi/serve are excluded as non-client). `stats` wraps a single `overview` verb and is a thin pseudo-noun. | conforms | — |
| grammar+list+commands.rs:158 | CLI-R-22 | 2 | `cluster/listener/route/api/secret/dataplane/org/team list` | commands.rs:158-161,261-264,462-465,490-493 | These `list` commands expose no `--limit/--page-token` and do not auto-paginate; only `learn`/`ai usage` have manual `--limit/--offset`. No cursor walking anywhere. | needs-change | M (server-cursor auto-pagination) |
| flags+global+commands.rs | CLI-R-23 | 2 | all | commands.rs (e.g. 288 `from_openapi`→`--from-openapi`, 222 `listener_port`→`--listener-port`) | Clap derive auto-kebabs every long flag from the snake_case field; no `camelCase`/`snake_case` long flags. | conforms | — |
| flags+global+config.rs:23 | CLI-R-24 | 2 | all | config.rs:23,35; commands.rs:170,201 | Single-char shorts are limited to `-o` (output), `-y` (yes), `-f` (file) — all in the reserved set, each with one meaning. | conforms | — |
| disco+root+main.rs:11 | CLI-R-25 | 2 | all | main.rs:11-23 | `infer_subcommands`/`infer_long_args` are not enabled (clap default off) → no arbitrary abbreviation; clap suggestions remain on. | conforms | — |
| flags+global+config.rs:25 | CLI-R-26 | 2 | all | config.rs:25-36; commands.rs:423 | Every boolean (`--json,--no-color,--quiet,--verbose,--dry-run,--yes,--diff,--prune,--upstream-tls,--device,--pkce,--token-stdin`) defaults false; default-on toggles are expressed as `--no-*`. | conforms | — |
| addr+team+commands.rs:102 | CLI-R-27 | 2 | `team`, `mcp`, `route generate`, `secret rotate` | commands.rs:55-64,102-105,220,336,481; (mcp.rs `--api` id) | Mixed handles: `org` is positional `<org>`, `team` is the `--team` flag; `mcp enable` keys on `--api <name|id>`; `route generate --from-spec` and `route apply` take a plan UUID; rotate/cert/grant use serial/grant_id. No single "name, UUID accepted anywhere" rule. | needs-change | M (normalize to name+UUID, parent via flags) |
| disco+leaves+mod.rs | CLI-R-28 | 2 | every leaf | main.rs:16-22; (no `after_help` in mod.rs/commands.rs) | Only the **root** carries examples; no subcommand sets `after_help`/`after_long_help`, so `cluster create --help` etc. have no runnable example. | needs-change | M (per-leaf examples) |
| disco+root+main.rs:31 | CLI-R-29 | 2 | root | main.rs:31-136 | No `flowplane help <topic>` for workflows (learning/ai/tenancy). | needs-change | M (add topic pages) |
| disco+completion+main.rs:132 | CLI-R-30 | 2 | `completion` | main.rs:132-133,195-199 | `completion <shell>` generates via `clap_complete` for bash/zsh/fish/etc. | conforms | — |
| disco+root+clap | CLI-R-31 | 2 | all | (clap default) | Unknown subcommand/flag yields clap's built-in "did you mean" suggestion. | conforms | — |
| safety+delete+config.rs:35 | CLI-R-33 | 2 | `* delete`, `unexpose`, destructive ops | config.rs:35-36; (no confirm/`is_terminal`/`read_line` in cli/) | `-y/--yes` is a declared-yet-unread dead flag: no command prompts for confirmation and none checks for a TTY, so destructive ops run unconditionally and `--yes` changes nothing. | needs-change | M (confirm-on-TTY + honor `--yes`/non-TTY) |
| safety+delete+commands.rs:180 | CLI-R-36 | 2 | `* delete` | commands.rs:180-185 | No `--ignore-not-found`; deletes of absent resources surface a 404 (exit 3). No progress/spinner for long ops. | needs-change | S (`--ignore-not-found`) |

## Coverage note

- **Inspected fully:** `main.rs` (entrypoint, exit dispatch, root help, transcript tests),
  `config.rs` (`GlobalOptions`, precedence in `effective()`), `client.rs` (request/render,
  auth headers, dry-run, `--out`), `output.rs` (error envelope, exit-code map, table/yaml/
  json render, mutation summary), `commands.rs` (the entire Clap command/flag tree).
- **Sampled, not fully read:** the `run_*` dispatch handlers inside `cli/mod.rs` (2,690
  lines) were grepped for prompting, TTY detection, color, examples, and file/stdin
  reading, and `body_from_file` was read directly — but per-handler rendering quirks (e.g.
  bespoke flatteners a given `run_api`/`run_learn` path may apply) were not exhaustively
  traced. Findings keyed to `mod.rs` rely on the grep + targeted reads, not a line-by-line
  pass.
- **Could not verify from CLI source alone (server-coupled):** the *stability* of `-o json`
  shapes (CLI-R-02) and the uniformity of the `list` envelope (CLI-R-03) are determined by
  the REST contract (`spec/01`), since the CLI passes server JSON through; the
  no-cross-tenant-leak privacy rule (CLI-R-14) is enforced server-side. These are marked
  accordingly rather than asserted.
- **Out of current scope (absent commands, not deviations):** `spec/12`'s `stack`/compose
  lifecycle, `filter`, `rate-limit`, `agent`, standalone `secret update`, and `ai
  budget set` either do not exist in this tree or appear in reduced form; their absence is
  noted here but not scored as per-rule violations (nothing to measure).

---

## Open design decisions (forks the authorities leave to a human)

These are surfaced, **not** silently resolved into the standard. Each rule that depends on
one names it.

**DD-1 — Auto-JSON-on-pipe vs explicit format.** *(blocks CLI-R-01)* The authorities
conflict: `gh` switches to machine output when stdout is not a TTY; `kubectl` never does
(always table unless `-o` given); CLIG suggests-but-doesn't-mandate auto-switching.
Current code is explicit (config.rs:99-105).
- (a) **Explicit format only; auto-suppress decoration** *(recommended — matches current
  code, robust under CI pseudo-TTYs)*; (b) auto-emit JSON when piped (gh-style);
  (c) hybrid: keep table when piped but strip color/alignment.
- *Rec: (a).* It's already true and is the least-surprising for scripts.

**DD-2 — Exit-code numbering.** *(blocks CLI-R-13, CLI-R-16, CLI-R-37)* Three live
variants: current `output.rs:103-112` (401/403→2, 404/409/412→3, 400/422→4, 429→5, 5xx→6),
`spec/12 §4`'s example (`resource_in_use`/409→5), and this standard's §3 (401→3,403→4,
404→6,409/412→5,400/422→2,429/5xx/timeout→7).
- (a) **Adopt §3** *(recommended — frees `2` for usage-only, gives 401 vs 403 distinct
  codes, adds a retryable class)*; (b) keep current numbering and just document it;
  (c) `sysexits` 64–78 (rejected in §3).
- *Rec: (a),* accepting a one-time breaking renumber (exit codes are a contract — bundle it
  with the first `spec/16` migration and note it in `DECISIONS`).

**DD-3 — Where server validation (400/422) lands.** *(blocks CLI-R-13)* Both clap usage
errors and server validation mean "fix the input, don't retry," but they differ in origin.
- (a) **Fold 400/422 into `2`** *(recommended — same script action, fewer codes)*;
  (b) give validation its own code (e.g. `8`/`65`) to distinguish client-arg vs
  server-body rejection.
- *Rec: (a)* unless a consumer genuinely branches on "my YAML was rejected" vs "my flags
  were wrong."

**DD-4 — `list` JSON envelope shape.** *(blocks CLI-R-03)* The CLI currently passes the
server body through (output.rs:114-126), so the shape is whatever REST returns.
- (a) **`{items:[…],page:{next_token}}`** *(recommended — kubectl-like, pairs with
  cursor auto-pagination in CLI-R-22, stable `.items[]` jq path)*; (b) bare top-level array
  (gh-like, simplest `jq '.[]'`); (c) keep server-passthrough (status quo, but then "uniform
  envelope" is a server contract, not a CLI one).
- *Rec: (a),* and make the CLI normalize to it so the envelope is a CLI guarantee.

**DD-5 — `--token` flag on `auth login` / `config set-context`.** *(blocks CLI-R-09)*
CLIG says never accept secrets via flags; the flags exist (commands.rs:9-10,41-42) alongside
`--token-stdin`.
- (a) **Remove/hide `--token`, keep `--token-stdin` + `--token-file`** *(recommended)*;
  (b) keep `--token` for ergonomics but warn it is logged; (c) keep as-is.
- *Rec: (a);* the stdin path already exists, so this is a deprecation, not a feature loss.

**DD-6 — Destructive-action model.** *(blocks CLI-R-33, CLI-R-34)* Today nothing prompts and
`--yes` is inert (config.rs:35-36) — fine for a script-first CLI, surprising for a human who
fat-fingers `cluster delete`.
- (a) **Confirm on TTY, `--yes` bypasses, non-TTY-without-`--yes` errors** *(recommended —
  CLIG default; needs TTY detection added)*; (b) never prompt, require `--yes` on
  destructive verbs always (uniform, script-friendly, no TTY logic); (c) keep current
  no-guard behavior.
- *Rec: (a)* for `delete`/`--cascade`/`publish`-of-discovered; pair with the `--no-input`
  flag (CLI-R-34) so CI is explicit.
</content>
</invoke>
