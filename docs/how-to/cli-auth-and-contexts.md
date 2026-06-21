# Authenticate the CLI and point it at the right server/org/team

> Audience: cli-users · Status: stable

This guide gets the `flowplane` CLI talking to a control plane: set a token, point at the right server, optionally scope to an org/team, and verify. It assumes you have the `flowplane` binary on your `PATH` and a control-plane URL to talk to.

For the full command surface see [`../reference/cli.md`](../reference/cli.md), and for the complete list of environment variables and config keys see [`../reference/configuration.md`](../reference/configuration.md).

## Fastest path: token + server, then verify

If you already have a bearer token, the quickest way to get going is two environment variables and a `whoami`:

```bash
export FLOWPLANE_TOKEN="<your-token>"
export FLOWPLANE_SERVER="https://fp.example"

flowplane auth whoami
```

If `whoami` returns your identity, you are authenticated. (`FLOWPLANE_SERVER` is also a documented flag — `--server` — and falls back to `http://127.0.0.1:8080` when unset.)

Prefer to persist the token to disk instead of carrying an env var? Save it with `auth login`:

```bash
# Pass the token directly...
flowplane auth login --token "<your-token>"

# ...or pipe it in to keep it out of your shell history:
printf '%s' "<your-token>" | flowplane auth login --token-stdin
```

`auth login` writes the token to the credentials file next to your config (`~/.flowplane/credentials`, mode `0600`). After that, `flowplane auth whoami` works without `FLOWPLANE_TOKEN` set. To remove it later, run `flowplane auth logout` (this deletes the credentials file). To print the token the CLI would currently use, run `flowplane auth token`.

## OIDC login (PKCE)

If your control plane uses an OIDC provider, log in interactively with PKCE instead of pasting a raw token. This opens a browser authorize URL and waits for the provider to redirect back to a loopback callback:

```bash
flowplane auth login --pkce \
  --issuer https://issuer.example \
  --client-id flowplane-cli
```

Defaults you usually do not need to set:

- `--scope` defaults to `openid email profile`.
- `--callback-url` defaults to `http://127.0.0.1:8976/callback`. The callback must be an `http://` loopback on `127.0.0.1` or `localhost` with an explicit port.

The resulting token is saved to the credentials file, just like `auth login --token`. `--issuer` and `--client-id` can also come from config (`oidc_issuer` / `oidc_client_id`) or the matching env vars — if both are configured, plain `flowplane auth login` will use PKCE automatically. For headless machines, use `--device` (alias `--device-code`) instead of `--pkce` to run the device-code flow.

## Contexts: stop repeating flags

A context bundles a server (and optionally org, team, token) under a name so you do not have to pass `--server`/`--org`/`--team` on every command. Create one with `config set-context` (the `--server` value is required; `name` is positional):

```bash
flowplane config set-context prod \
  --server https://fp.example \
  --org acme \
  --team payments
```

Creating a context with `set-context` also makes it the current context if none is set yet. List what you have (the current one is marked with `*`):

```bash
flowplane config get-contexts
```

Switch the active context:

```bash
flowplane config use-context prod
```

To set or change the org/team for a context, re-run `set-context` with the same name — it replaces the existing entry. You can also override per-invocation with `--org` / `--team`, or select a different context for one command with `--context <name>`.

To store a token inside the context itself (instead of the shared credentials file), pass `--token` or `--token-stdin` to `set-context`. Note a context token takes precedence over the saved credentials file, so `auth login` will warn you if the current context already carries its own token.

See `flowplane config show` to print the resolved config file and `flowplane config path` to print its location (`~/.flowplane/config.toml` by default).

## Precedence: how a value is resolved

For each setting, the CLI walks these sources in order and uses the first one present:

- **Server**: `--server` flag / `FLOWPLANE_SERVER` env → current context → config file `base_url` → default `http://127.0.0.1:8080`.
- **Org**: `--org` flag → `FLOWPLANE_ORG` env → current context → config file `org`.
- **Team**: `--team` flag → `FLOWPLANE_TEAM` env → current context → config file `team`.
- **Token**: `FLOWPLANE_TOKEN` env → current context token → config file `token` → credentials file (`~/.flowplane/credentials`).

The rule of thumb: **flag > env > config file**. The active context is whatever `--context` names, otherwise the `current_context` saved by `use-context`.

## Verify

Whichever path you took, confirm it end to end:

```bash
flowplane auth whoami
```

A successful response means the CLI resolved a server and token correctly and the control plane accepted your identity. If it fails, check `flowplane auth token` (is a token resolved at all?) and `flowplane config get-contexts` (is the right context active and pointing at the right server?).
