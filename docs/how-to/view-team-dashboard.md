# View your team's gateway dashboard

> Audience: cli-users · Status: stable

This guide opens a live, read-only web view of your team's gateway — dataplane liveness, config verification, request/error totals — using the credentials the CLI already stores. It assumes the CLI is authenticated and scoped to a team (see [Authenticate the CLI](cli-auth-and-contexts.md)).

## Open the dashboard

```bash
flowplane dashboard
```

Your browser opens an Overview page for the resolved team. The page refreshes every 10 seconds. Stop the dashboard with Ctrl-C; the page stops working the moment the command exits.

If no team is resolvable from your context, the command fails with the CLI's standard `team is required` error — pass `--team <name>` or configure a context.

On a headless machine (or when no browser opener exists), the command prints the URL and keeps serving:

```text
Dashboard running at http://127.0.0.1:52345/1f0c…9ab2/ (Ctrl-C to stop)
```

Open that URL in any local browser. To always skip the browser launch, set `FLOWPLANE_DASHBOARD_NO_BROWSER=1`.

## What the page shows

- **Team totals** — dataplane counts (total / live / stale) and request, error, and warming-failure totals. These are cumulative stored counters, not a time window.
- **Gateway** — the health string, recent NACK count (last 15 minutes, team-wide), and a per-dataplane table: liveness, last heartbeat age, config state ("ever verified" / "never verified"), and per-dataplane counters.

Very large teams: the per-dataplane table lists at most 500 dataplanes. When your team has more, a banner says "Showing first 500 of N" — the numeric team totals above it are always complete, but the table (and the health inputs derived from it) covers only the listed dataplanes, while NACK status stays team-wide.

## Security model

The dashboard is a local presentation layer, not a new API surface:

- The server binds `127.0.0.1` only. There is no flag or configuration to bind any other address in this release.
- Every URL contains a random per-launch path secret (a 128-bit nonce). Requests without it get a 404, so other local pages or tabs cannot guess the URL. Requests with a foreign `Host` or `Origin` header are rejected.
- Only GET routes exist. The local server cannot mutate anything, and it forwards only two fixed control-plane reads (`/stats/overview` and `/xds/status`) for your resolved team.
- Your bearer token never reaches the browser: it stays in the CLI process's memory and appears in no HTML, header, or log. The browser only ever talks to the loopback server.
- What you can see is decided by the control plane: the dashboard adds no permissions, and a panel you lack the `stats: read` grant for shows "Not authorized" instead of data.

The dashboard process is exactly as trusted as the CLI itself — it reads the same stored credentials under `~/.flowplane`. It does not defend against other processes running as your own user (they already share your files and can observe the launch URL); the nonce and loopback bind defend against browser-origin attacks such as drive-by pages probing localhost.

## Troubleshooting

- **"Session expired" banner naming `flowplane auth login`** — your stored token was rejected by the control plane (expired or revoked). The page stops refreshing. Run `flowplane auth login`, then restart `flowplane dashboard`.
- **A panel says "Not authorized"** — your principal lacks the `stats: read` grant for the team. Ask a team or org admin to grant it; the rest of the page keeps rendering.
- **A panel says "unavailable"** — the control plane read failed (server error or connection problem). The page keeps polling; the panel recovers when the read does.
- **`team is required` on start** — no team in your context. Run with `--team <name>`, set `FLOWPLANE_TEAM`, or configure a context.
- **`invalid team name for dashboard`** — the configured team value contains characters that can't form a team name (team names are letters, digits, and hyphens). Fix the `--team`/`FLOWPLANE_TEAM` value.
- **Browser didn't open** — the command prints the URL either way; open it manually. `FLOWPLANE_DASHBOARD_NO_BROWSER=1` makes this the default behavior.
- **The page 404s after a restart** — the nonce URL changes on every launch by design. Use the freshly printed URL, not a bookmark.
