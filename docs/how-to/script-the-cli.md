# Script Flowplane from a shell or agent

> Audience: cli-users, api-teams · Status: stable

This guide shows how to drive the `flowplane` CLI **non-interactively** — from a shell script,
a CI job, or an LLM agent. You will read structured JSON output, branch on exit codes, project
just the fields you need, discover the command surface programmatically, and make changes safely
with confirmation and optimistic concurrency.

It assumes you already have the CLI talking to a control plane. If not, do
[Authenticate the CLI and point it at the right server/org/team](cli-auth-and-contexts.md) first.
For the exhaustive flag/command list see [`../reference/cli.md`](../reference/cli.md); for *why* the
CLI behaves as a typed contract see [The CLI as a typed contract](../concepts/cli-contract.md).

Every command below is copy-pasteable. Examples use the team `payments`; substitute your own.

## 1. Get machine-readable output

Reader commands print a **table** on an interactive terminal but switch to **JSON automatically
when stdout is not a terminal** — so a pipe just works, no flag needed:

```bash
flowplane cluster list --team payments | jq '.data'
```

To be explicit (recommended in scripts so behavior never depends on the terminal), pass
`-o json` (or the identical `--json`):

```bash
flowplane cluster list --team payments -o json
```

Every success payload is wrapped in a stable, versioned envelope:

```json
{
  "schemaVersion": 1,
  "kind": "clusterList",
  "data": [
    { "name": "alpha", "revision": 1, "service_name": "alpha-svc" },
    { "name": "beta",  "revision": 2, "service_name": "beta-svc" }
  ]
}
```

- `schemaVersion` — integer contract version of the envelope; branch on it if you parse defensively.
- `kind` — what `data` holds (`cluster` for one object, `clusterList` for a list, `mutationResult`
  for a delete, etc.).
- `data` — the resource (object) or resources (array).

Pull a single value out with `jq`:

```bash
flowplane cluster get alpha --team payments -o json | jq -r '.data.revision'
# 1
```

## 2. Branch on exit codes

The CLI exits with a **scriptable code by failure class** (not a generic `1`). On failure the
error is a JSON object on **stderr** (stdout stays empty), so your pipeline never parses an error
as data:

```bash
if out=$(flowplane cluster get missing --team payments -o json 2>err.json); then
  echo "found: $(jq -r '.data.name' <<<"$out")"
else
  code=$?
  echo "failed with exit $code: $(jq -r '.code + ": " + .message' err.json)"
fi
```

The full map (see [`../reference/cli.md#exit-codes`](../reference/cli.md#exit-codes)):

| Code | Meaning | Trigger |
|------|---------|---------|
| `0` | Success | — |
| `1` | Generic / internal CLI error | Unclassified local failure |
| `2` | Usage error | Invalid flags/arguments |
| `3` | Auth | HTTP `401`, `403` |
| `4` | Not found / conflict / precondition | HTTP `404`, `409`, `412` |
| `5` | Validation | HTTP `400`, `422` |
| `6` | Rate limited | HTTP `429` |
| `7` | Server / transport | HTTP `5xx`, connection refused, timeout |

The error envelope carries a `retryable` boolean — `true` for `429`/`5xx`/transport, `false` for
terminal `4xx`. A retry loop can key off it:

```bash
for attempt in 1 2 3; do
  flowplane cluster get alpha --team payments -o json >out.json 2>err.json && break
  if [ "$(jq -r '.retryable' err.json)" != "true" ]; then
    echo "giving up: $(jq -r '.message' err.json)"; exit 1
  fi
  sleep $((attempt * 2))
done
```

## 3. Project only the fields you need

`--fields` trims reader output to a comma-separated key set, applied **inside** `data` (per item
for lists). `schemaVersion` and `kind` always survive; absent keys are omitted:

```bash
flowplane cluster list --team payments -o json --fields name,revision
```

```json
{
  "schemaVersion": 1,
  "kind": "clusterList",
  "data": [
    { "name": "alpha", "revision": 1 },
    { "name": "beta",  "revision": 2 }
  ]
}
```

## 4. Discover the command surface programmatically

Instead of scraping `--help`, ask for the whole CLI as JSON — every command, flag, value type,
and default. `schema` makes **no network call**, so an agent can introspect offline:

```bash
flowplane schema -o json | jq -r '.data.command.subcommands[].name'
# serve
# db
# auth
# ...
```

Inspect one command's flags:

```bash
flowplane schema -o json \
  | jq '.data.command.subcommands[] | select(.name=="cluster")
        | .subcommands[] | select(.name=="create") | .args'
```

The output is the same `{schemaVersion, kind:"cliSchema", data}` envelope; `data.command` is the
recursive command tree.

## 5. Make changes safely (non-interactive)

Destructive commands (`delete`, `unexpose`) **confirm before acting**. On a terminal they prompt
`[y/N]`; with no terminal they **fail fast** rather than hang. In a script you must pass `--yes`:

```bash
# Without --yes on a pipe/CI runner: exits 2, never blocks.
flowplane cluster delete alpha --team payments </dev/null
# error (confirmation_required): refusing to delete … without --yes on a non-interactive terminal

# Correct in automation:
flowplane cluster delete alpha --team payments --yes
```

Updates and deletes are **optimistically concurrent**. Pass the revision you read; if the server
moved on, you get a `409` (exit `4`) naming both revisions instead of clobbering a concurrent edit:

```bash
rev=$(flowplane cluster get alpha --team payments -o json | jq -r '.data.revision')
flowplane cluster update alpha --team payments -f cluster.json --revision "$rev" --yes
```

Omitting `--revision` makes the CLI read-modify-write (it reads the current revision and sends it
for you) — convenient interactively, but in a script prefer passing the `--revision` you already
read, so a concurrent change is reported rather than silently overwritten.

## 6. Pin everything explicitly in CI

For reproducible runs, set the connection and scope via environment instead of relying on a config
file or context, and silence chrome with `--quiet`:

```bash
export FLOWPLANE_SERVER=https://fp.example.com
export FLOWPLANE_TOKEN=…           # highest-priority token source
export FLOWPLANE_TEAM=payments
export FLOWPLANE_TIMEOUT=15        # seconds

flowplane cluster list -o json --quiet | jq '.data[].name'
```

Precedence for every value (including the token) is `flag > env > context > file > default` — see
[`../reference/configuration.md`](../reference/configuration.md). `--quiet` removes progress and
human-readable summaries, leaving only the requested data envelope on stdout.

## Verify

A quick end-to-end check that your scripting setup works:

```bash
flowplane cluster list --team payments -o json | jq -e '.schemaVersion == 1' >/dev/null \
  && echo "OK: structured output flowing"
```

If that prints `OK`, JSON output, your token, and your server resolution are all working.

## Further reading

- [`../reference/cli.md`](../reference/cli.md) — every command, flag, the envelope, and the exit-code table.
- [The CLI as a typed contract](../concepts/cli-contract.md) — why the envelope, exit codes, and `schema` are shaped this way.
