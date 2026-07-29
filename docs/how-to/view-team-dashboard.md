# View your team's gateway dashboard

> Audience: cli-users · Status: stable

This guide opens a live, read-only web view of your team's gateway and API platform using the credentials the CLI already stores. Seven screens cover Overview, Resources, APIs, Learning, AI, MCP, and Operations. It assumes the CLI is authenticated and scoped to a team (see [Authenticate the CLI](cli-auth-and-contexts.md)).

## Open the dashboard

```bash
flowplane dashboard
```

Your browser opens the Overview screen for the resolved team. Stop the dashboard with Ctrl-C; the page stops working the moment the command exits.

If no team is resolvable from your context, the command fails with the CLI's standard `team is required` error — pass `--team <name>` or configure a context.

On a headless machine, use `--no-open`. The command prints the URL and keeps serving:

```text
Dashboard running at http://127.0.0.1:52345/1f0c…9ab2/ (Ctrl-C to stop)
```

Open that URL in a local browser. `FLOWPLANE_DASHBOARD_NO_BROWSER=1` remains supported, but `--no-open` is the preferred explicit interface.

For a fixed bind and a machine-readable readiness handoff:

```bash
flowplane dashboard \
  --listen 127.0.0.1:8081 \
  --no-open \
  --url-file /tmp/flowplane-dashboard-url
```

`--url-file` is written atomically only after the server is bound and its first upstream fetch succeeds. A stale file is removed before binding, and an unwritable path is fatal.

An off-loopback bind is supported as a container or tunnel transport endpoint:

```bash
flowplane dashboard \
  --listen 0.0.0.0:8081 \
  --no-open \
  --url-file /shared/dashboard-url
```

This prints a prominent exposure warning. The dashboard still emits a `http://127.0.0.1:8081/<nonce>/` URL and accepts only `127.0.0.1`/`localhost` Host and Origin values. Publish the container port on host loopback with the same port number (for example `127.0.0.1:8081:8081`), or carry it through a same-port SSH local forward such as `ssh -L 8081:127.0.0.1:8081 host`; then open the emitted loopback URL locally. Direct `http://remote-host:8081/...` browser access is intentionally rejected. The nonce remains mandatory, but it is a bearer-like URL secret rather than a substitute for host-loopback publication or another deployment network boundary.

## What the page shows

- **Overview** — team totals, dataplane liveness, config verification, request/error counters, warming failures, and recent NACK state.
- **Resources** — cluster/listener/route topology, filters, secrets, rate-limit domains and policies, and orphaned-resource diagnostics with filtering and paging.
- **APIs** — API lifecycle summaries and an accessible master/detail view of versions, decisions, bindings, generated tools, and spec content.
- **Learning** — capture and discovery sessions, progress counters, provenance, and generated-spec inspection.
- **AI** — providers, route chains, budget meters, time-windowed usage, and cursor-paged request traces.
- **MCP** — node-local status/connections, the executable tool catalogue, agents, and agent grants.
- **Operations** — xDS status, withdrawn resources, and filtered/cursor-paged NACK history.

Panels acquire data lazily and degrade independently. A denied or unavailable read does not prevent the rest of the screen from rendering. Large collections expose their own paging/truncation status; for example, the Overview dataplane table and MCP agents panel cap their fetched rows and show when more exist.

## Security model

The dashboard is a local presentation layer, not a new API surface:

- The server binds `127.0.0.1` by default. `--listen` may bind another address as a container/tunnel endpoint; off-loopback use warns, the browser-facing URL remains loopback with the same port, and direct remote Host/Origin values are rejected.
- Every URL contains a random per-launch path secret (a 128-bit nonce). Requests without it get a 404, so other local pages or tabs cannot guess the URL. Requests with a foreign `Host` or `Origin` header are rejected.
- Only fixed GET routes exist. The local server cannot mutate anything and has no generic proxy route; each panel maps to an explicit allowlisted control-plane read for the resolved team.
- Your bearer token never reaches the browser: it stays in the CLI process's memory and appears in no HTML, header, or log. The browser talks only to the dashboard presentation server, never directly to the control-plane API.
- htmx owns network acquisition. Dashboard JavaScript is DOM-only: no `fetch`, `eval`, `localStorage`, cookie ownership, bearer-token ownership, HTML-string insertion, or inline event handlers/styles.
- What you can see is decided by the control plane: the dashboard adds no permissions, and a panel whose read is denied shows "Not authorized" instead of data.

The dashboard process is exactly as trusted as the CLI itself — it reads the same stored credentials under `~/.flowplane`. It does not defend against other processes running as your own user (they already share your files and can observe the launch URL); the nonce, host/origin checks, and default loopback bind defend against browser-origin attacks such as drive-by pages probing localhost.

## Troubleshooting

- **"Session expired" banner naming `flowplane auth login`** — your stored token was rejected by the control plane (expired or revoked). The page stops refreshing. Run `flowplane auth login`, then restart `flowplane dashboard`.
- **A panel says "Not authorized"** — your principal lacks the read grant required by that panel. Ask a team or org admin to grant it; the rest of the page keeps rendering.
- **A panel says "unavailable"** — the control plane read failed (server error or connection problem). Overview retries every 10 seconds. Other panels load on demand; reload or revisit the screen (or reopen a disclosure) after the control-plane read recovers.
- **`team is required` on start** — no team in your context. Run with `--team <name>`, set `FLOWPLANE_TEAM`, or configure a context.
- **`invalid team name for dashboard`** — the configured team value contains characters that can't form a team name (team names are letters, digits, and hyphens). Fix the `--team`/`FLOWPLANE_TEAM` value.
- **Browser didn't open** — the command prints the URL either way; open it manually. Use `--no-open` when browser launch is intentionally disabled.
- **`--url-file` never appears** — readiness requires the bind and first successful upstream fetch. Check the control-plane URL, token, org/team context, and the dashboard process logs. An unwritable target causes the command to exit.
- **Off-loopback warning** — expected whenever `--listen` is not loopback. Publish or forward the same port to local `127.0.0.1`, then use the emitted loopback URL. A direct remote-host URL returns `403` by design.
- **The page 404s after a restart** — the nonce URL changes on every launch by design. Use the freshly printed URL, not a bookmark.
