# Flowplane v2 — Questions for the Founder

Standing format (per founder instruction): every question states the decision needed, 2–3 viable
options with one-line trade-offs, the recommended option, and why. If unanswered, work proceeds
with the recommendation, recorded in `DECISIONS.md` as provisional.

Status legend: **OPEN** / **ANSWERED** / **PROCEEDED-PROVISIONAL**

---

## Q-001: CLI config precedence — does "env > config file > CLI args" apply to the CLI client? — OPEN

- **Decision needed:** The code standards state config precedence `env vars > config file > CLI
  args > defaults`. For the *server* this is normal 12-factor practice. Applied to the *CLI
  client*, it means an explicit `--team foo` on the command line is silently overridden by a
  forgotten `FLOWPLANE_TEAM` in the shell — the exact behavior spec/07 flags as v1's
  least-surprise violation (v1 is even internally inconsistent: token resolves env>file>flag,
  team/org resolve flag-first).
- **Options:**
  1. **Server: env > file > defaults; CLI client: flag > env > file > defaults (recommended).**
     Matches gh/kubectl/aws and user expectation that typing a flag always wins; keeps 12-factor
     for the server.
  2. Apply env-first uniformly to both — consistent with the written standard, but explicit flags
     silently lose; foot-gun for operators and confusing in CI.
  3. Flag-first uniformly including the server — unconventional for service deployment.
- **Recommendation:** Option 1. The standard's intent reads as "no hardcoding, env-configurable
  deployments", not "flags lose to env". Will proceed with Option 1 in spec/12 unless vetoed.

## Q-002: Cut UI-only visualization workflows? — OPEN (D-003 provisional)

- **Decision needed:** Four v1 UI workflows are pure visualization (stats charts, admin KPI
  dashboard, per-org drill-in widgets, password deep-link page). With no v2 UI, do we rebuild
  their rendering in the CLI or cut it?
- **Options:**
  1. **Cut rendering, keep all data endpoints + CLI `--json`/`--watch`, add `admin health`
     (recommended).** Operators visualize in Grafana; zero data loss, large effort savings.
  2. Build rich TUI dashboards (`flowplane stats --dashboard`) — high effort, duplicates
     Grafana, delays core slices.
  3. Keep a minimal web status page — contradicts the no-UI scope decision.
- **Recommendation:** Option 1; v1.0 ships Prometheus-consumable metrics anyway (production
  readiness), so charts belong there. Proceeding per D-003 unless vetoed.
