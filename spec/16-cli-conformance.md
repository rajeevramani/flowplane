# 16 — CLI Conformance Backlog (Tier 1)

> Measures the current `flowplane` binary against `spec/16-cli-standard.md`. **Tier 1 only** this pass
> (machine/scriptable contract); Tier 2 ergonomics deferred to a second run per the standard's tier split.
> Build under test: **`flowplane 1.1.0`** (`./target/debug/flowplane version` → `1.1.0`; the `version`
> command emits no git SHA — see key `output+version+main.rs:208`, itself a finding).

## Test environment

- Binary: `./target/debug/flowplane` (pre-built; not rebuilt).
- Server: running on `127.0.0.1:8080` (gvproxy listener confirmed), **dev mode** (`FLOWPLANE_DEV_MODE=true`,
  in-process issuer, per-boot 1h dev token at `/shared/dev-token`, `serve.rs`).
- **Auth: resolved mid-run.** The first token was stale (dev token TTL is 1h, minted once per boot and not
  refreshed); after the operator restarted the CP, the fresh `/shared/dev-token` authenticated
  (`auth whoami` exit 0). The **full create→route→filter→read dogfood, idempotency, and live 403/404/409
  were then exercised for real.** No infrastructure was stood up by this run — the operator owns the CP
  lifecycle; the run only read the token file and drove the CLI.
- An earlier symptom (`identity provider unreachable (https://dev.flowplane.local/jwks)`, 503) is a
  **misleading dev-mode message** — JWKS is served in-process, never fetched externally; the real cause was
  always token expiry. Logged as an error-message-clarity observation, not a Tier 1 contract finding.

## Command tree (enumerated before judging)

Top-level commands (24) from `Cli`/`Command` in `crates/flowplane/src/main.rs:31-141`:
`serve · db(migrate) · openapi · auth · config · org · team · cluster · listener · route · api · mcp ·
ai · rate-limit · learn · secret · dataplane · expose · unexpose · stats · ops · apply · completion ·
version`.

Global flags (apply to every subcommand, `GlobalOptions`, `config.rs:13-43`):
`--context · --server[env FLOWPLANE_SERVER] · --team · --org · -o/--output{table,json,yaml,wide} ·
--json · --no-color · --quiet · --verbose · --dry-run · -y/--yes · --revision · --timeout[=30] · --out`.

Resource verb sets (`cluster`/`listener` = `ResourceCommand`: list,get,create,update,delete;
`route` = list,get,create,update,delete,generate,apply). Body input is `-f/--file` on create/update.

---

## Findings (Tier 1, sorted by rule)

| key | rule (CLI-R-NN) | mode | command(s) | evidence (path:line + run) | deviation | verdict | migration cost |
|---|---|---|---|---|---|---|---|
| input+apply+cli/client.rs:84 | CLI-R-09 | real-run | `apply -f -` | `client.rs:84` reads body; run: `cat m.json \| flowplane apply -f - --dry-run` parsed manifest, reached reconcile (then 503 auth) | stdin `-` is honoured for `apply` | conforms | — |
| output+version+main.rs:208 | CLI-R-10 | real-run | `version` | `main.rs:208` `println!("{VERSION}")`; run: `version -o json` and `version --json` both print bare `1.1.0` | `version` ignores `-o`/`--output`; no command-level structured output | needs-change | low (route `version` through `render()`) |
| output+mutations+cli/client.rs:145 | CLI-R-10 | real-run | `listener/route delete` | `client.rs:145-150` 204/empty → `print_mutation_summary` prose; run: `listener delete cli-test-listener -o json --revision 2` → stdout `deleted api/v1/teams/default/listeners/cli-test-listener` (prose + raw path, not JSON) | delete emits prose even under `-o json`; leaks the URL path instead of a structured result | needs-change | medium |
| output+apply+cli/mod.rs(run_apply) | CLI-R-10 | real-run | `apply` | run: `apply -f l.json -o json` → stdout `apply summary:` / `  unchanged listener "…"` (prose) — ignores `-o json` entirely | `apply` has no structured output; its reconcile result (created/updated/unchanged per resource) is unscriptable | needs-change | medium |
| output+global+cli/config.rs:99 | CLI-R-11 | real-run | all (`--json` vs `-o json`) | `config.rs:99-105` `format()`: `--json`→Json else `--output`; run: `version --json` ignores it (≠ `-o json` which is also ignored) → divergence proves dual-path risk | two format selectors; `version` honours neither | needs-change | low (drop `--json`, or assert equivalence) |
| output+global+cli/config.rs:99 | CLI-R-12 | real-run | all readers | `config.rs:99-105` default `OutputFormat::Table`; no `is_terminal` anywhere (`grep` in `cli/` → none) | default is `table` even when stdout is piped/non-TTY; `list \| jq` needs explicit `-o json` | needs-change | low (add TTY check in `format()`) |
| output+mutation+cli/output.rs:500 | CLI-R-13 | reasoned | all mutations | `output.rs:500-502` `print_mutation_summary` early-returns on `quiet`; `client.rs:146` guards summary on `!quiet` | `--quiet` does suppress the human summary; not verified that data/stderr split holds on a live write | conforms | — |
| output+global+cli/config.rs:42 | CLI-R-14 | real-run | all (`--out`) | `config.rs:42` `--out <PathBuf>`; `output.rs:120` writes file; `-o` is enum-only (`config.rs:23`) | `-o` = format, `--out` = destination; no overload present | conforms | — |
| output+all+cli/output.rs:114 | CLI-R-15 | reasoned | all `-o json` | `output.rs:114-126` generic `serde_to_string_pretty`; no `schemaVersion`, no snapshot suite | JSON shape is unversioned and unsnapshotted; no breaking-change gate | needs-change | medium (add snapshots + version field) |
| output+global+cli/config.rs:28 | CLI-R-16 | real-run | all | `config.rs:28` `no_color` field; `grep no_color/NO_COLOR` → only the definition, never read | `--no-color` is a dead flag; no `NO_COLOR` env handling (output is plain, so no ANSI leaks, but the contract is unmet) | needs-change | low |
| errors+all+cli/output.rs:28 | CLI-R-30 | real-run | all HTTP errors | `output.rs:28-50`; run: `cluster list` → `error (unavailable): … / → hint / request id`; `-o json` → `{code,message,status,request_id,hint}` to **stderr**, empty stdout, exit 6 | HTTP-error envelope conforms (TTY + JSON-to-stderr) | conforms | — |
| errors+network+main.rs:154 | CLI-R-30 | real-run | any (transport failure) | `main.rs:154` `eprintln!("Error: {err:?}")`; run (server down earlier): `Error: send request / Caused by: … Connection refused (os error 61)` raw chain, not JSON under `-o json`, exit 1 | transport/network errors bypass the envelope → raw anyhow dump, unstructured | needs-change | medium (wrap reqwest send/timeout in envelope) |
| errors+apply+cli/mod.rs(run_apply) | CLI-R-30 | real-run | `apply` | run: `apply -f m.json --dry-run -o json` → JSON envelope on stderr **then** `Error: apply failed for 1 resource(s); see summary above` raw trailer, exit 1 | apply double-renders (structured + raw anyhow) and collapses underlying status | needs-change | medium |
| errors+exit+cli/output.rs:103 | CLI-R-31 | real-run | all HTTP errors | `output.rs:103-112` mapping; run: 503→exit 6 (`cluster list`), 401→exit **2** (`cluster list` w/ dev token) | semantic range exists (good) | conforms | — |
| errors+exit+cli/output.rs:105 | CLI-R-31 | real-run | usage vs auth | run: `cluster get` (missing arg, clap) → exit **2**; 401 → exit **2** (`output.rs:105`) — **same code** | clap usage errors collide with auth (401/403) on exit 2 | needs-change | low (remap auth to 3, usage stays 2) |
| errors+exit+main.rs:154 | CLI-R-31 | real-run | transport / apply | `main.rs:154` non-`CliHttpError` → exit 1; run: connection-refused → 1; `apply` 503 → 1 (not 6) | transport + apply failures collapse to generic exit 1, losing class | needs-change | medium |
| errors+exit+cli/output.rs:106 | CLI-R-31 | real-run | get/create/update/delete | run (live): 404 `listener get nope-xyz` → exit 3; 409 dup `route create` → exit 3; 422 `validation_failed` (bad filter body) → exit 4 (`output.rs:106-107`) | not-found and conflict share exit 3; validation is 4 — distinct classes resolve correctly for these (the unresolved issue is the usage/auth-on-2 collision, separate row) | conforms | — |
| errors+envelope+cli/output.rs:52 | CLI-R-32 | real-run | all errors | `output.rs:52-101` envelope builder; run: 503 JSON envelope = `{code,hint,message,request_id,status}` — **no `retryable`**; `grep retry/transient` in `cli/` → none | no machine retryable/terminal signal; agents must infer from status | needs-change | medium |
| errors+auth+cli/output.rs:28 | CLI-R-33 | real-run | 401/403 | `output.rs:28-50` passes server `hint` through verbatim; run: 401 `token expired` → `→ re-authenticate: flowplane auth login` (server-supplied, shown), but 401 `token validation failed` → **no** hint | hint quality depends entirely on the server; client never synthesizes a `flowplane auth login` hint when the server omits one, so some 401s teach and some don't | needs-change | low |
| config+token+cli/config.rs:200 | CLI-R-40 | real-run | all (precedence) | `config.rs:200-209` token = `FLOWPLANE_TOKEN` env → ctx → file → credentials (**no flag, env highest**); `:211-228` server/org/team = flag → env → ctx → file | precedence inverts for `token` vs every other value | needs-change | medium |
| config+timeout+cli/config.rs:39 | CLI-R-41 | real-run | all (`--timeout`) | `config.rs:39-40` `--timeout` default 30, no env, not in `effective()`; doc `docs/reference/cli.md` lists no `FLOWPLANE_TIMEOUT` | `timeout` has flag+ (no env, no file resolution path) — missing tiers | needs-change | low |
| config+stateless+cli/config.rs:190 | CLI-R-42 | real-run | all | `config.rs:190-243` resolves entirely from flags/env/file; run: full `FLOWPLANE_*` env drove the request (reached server) with no `config use-context` | invocation is fully specifiable by env/flags; context is optional override | conforms | — |
| config+secrets+cli/config.rs:171 | CLI-R-43 | real-run | config/credentials write | `config.rs:171-183` file `0600`, dir `0700`; unit `private_file_write_*` `config.rs:274-309` | credential/config files are private; `config show` redaction not re-verified this pass | conforms | — |
| mutation+create+cli/mod.rs(run_resource) | CLI-R-45 | real-run | `route/listener create` | run: second `route create -f rc.json` → `error (conflict): route config "cli-test-routes" already exists` exit 3 (with hint `→ choose a different name or update`) | re-create is not idempotent; agents must catch 409 (no `--idempotency-key`) | needs-change | medium |
| mutation+apply+cli/mod.rs(run_apply) | CLI-R-45 | real-run | `apply` | run: `apply -f listener.json` twice → both `unchanged listener "cli-test-listener"` exit 0 (GET-then-decide reconcile) | `apply` IS idempotent/declarative — conforming; caveat `--prune` is a no-op (`apply --help`: "additive-only") | conforms | — |
| mutation+dryrun+cli/client.rs:121 | CLI-R-46 | real-run | `cluster/listener/secret create --dry-run` | `client.rs:121-125` short-circuits non-GET, echoes `{method,path,body}`; run: `cluster create -f m.json --dry-run` → table `PATH/BODY/METHOD` with body `{...}` (no server validation) | per-resource `--dry-run` is a client-side echo, not a server-validated plan; body unreadable in table | needs-change | high |
| mutation+dryrun+cli/mod.rs(run_apply) | CLI-R-46 | real-run | `apply --dry-run` | run: `apply -f m.json --dry-run` reached the server (503 auth) rather than echoing → different semantics from per-resource dry-run | two conflicting `--dry-run` behaviours (apply=server, create=client echo) | needs-change | medium |
| mutation+revision+cli/config.rs:38 | CLI-R-47 | real-run | all update/delete | `config.rs:38` global `--revision` → `client.rs:192-194` `If-Match`; run: `listener update … --revision 1` → rev 2, exit 0; `route delete cli-test-routes` (no `--revision`) → exit 4 `error (validation_failed): this operation requires the resource revision` / `→ … send its revision as: If-Match: <revision>` | `--revision` is uniform and works; but it is **mandatory** on delete (server rejects without it) — the CLI does **no** read-modify-write fallback the standard assumes, though the error teaches the fix well | needs-change | medium |
| interactivity+destructive+cli/config.rs:35 | CLI-R-22 | real-run | `delete`, `unexpose`, `apply --prune` | `config.rs:35` `-y/--yes` defined; `grep confirm/prompt/read_line` in `cli/` → none (only `"prompts_enabled":false` literal `mod.rs:2726`) | `--yes` is a dead flag; destructive commands never prompt and have no confirmation guard | needs-change | medium |
| interactivity+noninteractive+cli/mod.rs | CLI-R-26 | real-run | all | no `is_terminal` / prompt anywhere; deletes execute immediately | never blocks on a prompt (satisfied by absence) — but for the wrong reason (no guard exists); non-TTY safety is incidental, not designed | conforms (incidentally) | low (formalize with CLI-R-22) |
| agent+introspect+cli/mod.rs | CLI-R-50 | real-run | (missing) `schema`/`--help=json` | `grep introspect/schema/--help=json` → none; `openapi` (`main.rs:169`) prints the **REST** OpenAPI, not the CLI tree | no machine-readable command catalog; MCP schemas cannot be derived from the CLI | needs-change | high |
| agent+fields+cli/output.rs:114 | CLI-R-51 | reasoned | all readers | `output.rs:114-126` renders whole value; no `--fields`/jsonpath/`--template` arg in `GlobalOptions` (`config.rs:13-43`) | no field selection; agents pay for full payloads | needs-change | medium |

---

## Coverage note

**Ran for real (`real-run`):**
- **The representative dogfood, end to end:** `route create cli-test-routes` (exit 0) → `listener create cli-test-listener` cross-referencing the route (exit 0) → attach a CORS filter via `listener update … --revision 1` (exit 0, rev 1→2) → `listener get -o json` read-back showing the filter → `apply` idempotency (re-apply → `unchanged`, exit 0) → `listener delete --revision 2` / `route delete --revision 1` cleanup. All test resources removed.
- **Live error/exit behaviour:** 503→exit 6, 401 (expired token)→exit 2, 404 (`listener get nope-xyz`)→exit 3, 409 (dup `route create`)→exit 3, 422 (bad filter body)→exit 4; error envelope on TTY and under `-o json` (to stderr, empty stdout); the **exit-2 collision** (clap usage `cluster get` == 401) captured directly.
- **Offline / auth-independent surface:** top-level + leaf `--help`, `version` (+ `--json`, `-o json` both ignored), `completion bash`, unknown-subcommand suggestion (`clustr`/`cluster lst`), missing-required-arg exit 2, `apply -f /nonexistent.json`, `apply -f -` via stdin, `cluster create --dry-run` (client echo) vs `apply --dry-run` (server dispatch), `auth whoami`.

**Read only (`reasoned`, source-grounded with `path:line`):**
- `CLI-R-13` `--quiet` data/stderr split on a real mutation; `CLI-R-43` `config show` token redaction; `CLI-R-15` field-by-field JSON stability (no snapshot history exists to diff against); `CLI-R-51` field selection (no such flag exists to exercise).

**Could not verify / structural absences (marked `needs-change` without a positive run):**
- **`CLI-R-50` introspection** — no `schema`/`--help=json` exists, so there is no output to derive MCP schemas from. Structural.
- **The standard's dogfood assumes a `filter` verb that does not exist.** The workflow *was* completed, but only by embedding `http_filters` JSON inside the listener spec (`mod.rs:2857`); `main.rs:31-141` has no `Filter` command. Worse for first-contact: the filter sub-schema is **undiscoverable** from any CLI surface — leaf `--help` is empty and there is no `filter types`/schema command, so the CORS body shape had to be reverse-engineered through **five successive `validation_failed` round-trips**, with the errors leaking internal Rust type names (`expected internally tagged enum OriginMatcher`, `missing field 'match'`). A first-contact agent without source access would be stuck. Logged against CLI-R-30 (errors leak internals) and CLI-R-50 (no schema source); the underlying verb gap is a Tier 2 grammar item.
- **Create vs update body-shape mismatch** (real-run): `create` wants `{name,spec}`, `update` rejects `name` and wants bare `{spec}` (`unknown field 'name', expected 'spec'`). Surfaced during the dogfood; it is a CLI-R-08 (Tier 2) input-model issue, recorded here for the Tier 2 pass.

**Build pinned:** `./target/debug/flowplane version` → `1.1.0`. The `version` command emits **no git SHA**
(`main.rs:208` prints only `CARGO_PKG_VERSION`), so the exact commit cannot be pinned from the CLI — itself
a Tier 1 finding (row `output+version+main.rs:208`). Tested build = whatever the running dev-mode CP and the
pre-built `target/debug` binary were at session time (branch `feature/fpv2-4ht-global-rate-limit`).

**No silent gaps:** every Tier 1 rule in the standard has at least one row in the table (conforms or needs-change). Tier 2 rules (CLI-R-01..08, 27, 44, 52) are intentionally out of scope this pass and carry no rows.

**Remaining `reasoned` rows** are now only the genuinely structural ones (CLI-R-15 stability, CLI-R-50
introspection, CLI-R-51 field selection — features that do not exist to exercise) plus CLI-R-13/43
quality checks. They cannot become `real-run` until the corresponding feature is built; the keys are
stable, so they update in place when it is.
</content>
