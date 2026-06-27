# The CLI as a typed contract

> Audience: cli-users, api-teams, platform-engineers · Status: stable

This page explains *why* the `flowplane` CLI behaves the way it does — the versioned output
envelope, the structured errors, the scriptable exit codes, optimistic concurrency, and the
`schema` command. It is understanding-oriented. For the step-by-step recipe see
[Script Flowplane from a shell or agent](../how-to/script-the-cli.md); for exact fields and flags
see [`../reference/cli.md`](../reference/cli.md).

## One surface, two consumers

Flowplane's command surface has two readers: a **human** at a terminal, and an **agent/script**
consuming the same binary. A CLI that only reads well to a human forces machines to scrape prose —
brittle, and the first thing that breaks on a cosmetic change. So the CLI is designed as a *typed
contract*: a human-friendly default that becomes a stable, machine-parseable shape the moment it is
consumed by a program. The same command serves both; neither pays for the other.

That single idea explains every behavior below.

## Output adapts to its consumer

Reader commands print a table on an interactive terminal and switch to JSON automatically when
stdout is not a terminal. The terminal is the signal: a human gets readability, a pipe gets data.
An explicit `-o/--output` always wins, so a script never has to depend on that heuristic.

## Why a versioned envelope, not a bare object

Every JSON success payload is wrapped in `{schemaVersion, kind, data}` rather than printed raw.
Three reasons:

- **Forward compatibility.** `schemaVersion` is an integer contract version. Consumers can detect a
  shape change instead of silently mis-parsing one. The shape is frozen by a snapshot test suite, so
  a later release cannot quietly re-shape it.
- **Self-description.** `kind` (`cluster`, `clusterList`, `mutationResult`, …) tells a consumer what
  `data` is without inferring it from the command it ran.
- **Uniformity.** Reads, mutations, and `schema` all use the same outer shape, so one parser handles
  every command.

The cost — a tiny wrapper around the resource — buys a stable integration point.

## Errors are data, on the right stream

Failures are written as a structured object to **stderr**, leaving stdout empty. A pipeline that
reads stdout therefore never confuses an error for a result. The error object is deliberately *not*
wrapped in the success envelope — it is a distinct shape with `code`, `message`, `retryable`, and
(when present) `status`, `request_id`, and `hint`.

`retryable` is the key affordance: it is `true` for transient failures (`429`, any `5xx`, transport
and timeout errors) and `false` for terminal `4xx`. A client can implement backoff by reading one
boolean instead of hard-coding a status list.

## Exit codes carry the failure class

The process exits `0` on success and a **specific code by failure class** otherwise — usage `2`,
auth `3`, not-found/conflict `4`, validation `5`, rate-limited `6`, server/transport `7` — rather
than a generic non-zero. Usage `2` covers invalid flags/arguments and local preflight usage checks.
A script can branch on `$?` alone, before parsing any JSON, and a CI job can distinguish "your input
was wrong" (`2`/`5`) from "the server is down" (`7`). The codes mirror the error classes, so the
exit code and the `code`/`retryable` fields always agree.

## Optimistic concurrency instead of last-write-wins

`update` and `delete` are revision-checked. With an explicit `--revision`, the CLI sends it as an
`If-Match` precondition; with none, it reads the resource's current revision first and sends that
(read-modify-write). Either way a concurrent edit surfaces as a `409` (exit `4`) that names both the
attempted and the server's current revision — a detected conflict, never a silent overwrite. This
keeps two operators (or two agents) from clobbering each other.

## `schema`: the CLI describes itself

`flowplane schema` emits the entire command tree as JSON — every command, flag, value type, and
default — with no network call. This exists so agents do not have to scrape `--help` text. It is
also the **derivation seam** between the human CLI and the agent/MCP layer: the machine-readable
catalog is the single source the MCP tool definitions are derived from, so the two surfaces cannot
drift apart. (See decision FP-DEC-0003.)

## Safety is explicit, never a hang

Destructive commands confirm. On a terminal they prompt `[y/N]`; on a non-interactive terminal they
**fail fast** with a usage error rather than block on input that will never arrive — the classic
`--full-auto < /dev/null` deadlock is impossible. `--yes` is the explicit, greppable opt-out for
automation. The principle: never silently destroy, and never silently hang.

## Further reading

- [Script Flowplane from a shell or agent](../how-to/script-the-cli.md) — the hands-on recipe.
- [`../reference/cli.md`](../reference/cli.md) — the exhaustive command and flag reference.
- [`spec/16-cli-standard.md`](../../spec/16-cli-standard.md) — the full CLI standard and the external authorities it derives from.
