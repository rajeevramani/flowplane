# Flowplane v2 — Questions for the Founder

Standing format (per founder instruction): every question states the decision needed, 2–3 viable
options with one-line trade-offs, the recommended option, and why. If unanswered, work proceeds
with the recommendation, recorded in `DECISIONS.md` as provisional.

Status legend: **OPEN** / **ANSWERED** / **PROCEEDED-PROVISIONAL**

---

## Q-001: CLI config precedence — ANSWERED (founder approved Option 1, 2026-06-12)

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

## Q-002: Cut UI-only visualization workflows? — ANSWERED (founder approved Option 1, 2026-06-12)

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

## Q-003: Design-partner profile — ANSWERED (founder: go with recommendation defaults, 2026-06-12)

- **Decision needed:** Target environment, LLM providers in use, and rough scale (teams,
  dataplanes, routes, traffic) of the design partner.
- **Why it matters:** Sets packaging polish order (D-004 list), AI translator priority, and the
  numeric load/hardening targets for S12.
- **Recommendation / default if unanswered:** packaging = compose/VM → ECS → K8s; translators =
  Anthropic + OpenAI + openai-compatible, Bedrock last; load targets = 10 teams, 20 dataplanes,
  1k routes, 5k rps observed traffic, 100 rps LLM.

## Q-004: Identity-provider coupling — ANSWERED (founder approved Option 1, 2026-06-12)

- **Decision needed:** Must customers run Zitadel, or any OIDC IdP?
- **Options:** (1) **Provider-agnostic OIDC core; Zitadel-specific provisioning behind a trait,
  shipped as the batteries-included default (recommended)** — design partners keep their
  Okta/Entra/Keycloak; (2) Zitadel-required as v1 — simpler now, operational burden + sales
  friction later.
- **Recommendation:** Option 1; proceeding with it in S2 unless vetoed.

## Q-005: v1 → v2 data migration — ANSWERED (founder approved Option 1: greenfield, 2026-06-12)

- **Decision needed:** Does any existing v1 deployment (demo environments, early users) need its
  data migrated to v2?
- **Options:** (1) **Greenfield, no migration tool (recommended; zero known v1 production
  installs)**; (2) v1-import command added to S12 (real effort, only if a migrating install
  exists).
- **Recommendation:** Option 1 unless you name an install that must migrate.

## Q-006: v2 repository license — ANSWERED (founder approved Option 1, 2026-06-12)

- **Decision needed:** v1 is MIT; v2 has no LICENSE file. Business decision.
- **Options:** (1) **No license file = all rights reserved; decide before any public release
  (recommended)**; (2) MIT like v1 — adoption-friendly, gives the rewrite away; (3) BSL/fair-
  source — middle ground, needs legal review.
- **Proceeding with (1)**; flagged in production-readiness as a pre-release checklist item.

## Q-007: Implementation review workflow — ANSWERED (founder approved Option 1, 2026-06-12)

- **Decision needed:** How the founder reviews Phase 2 work.
- **Options:** (1) **Single branch, commit-per-checkpoint, notify-only milestones after S4
  (OpenAPI diff), S7 (CLI demo), S9 (loop E2E) (recommended)**; (2) PR per slice — formal but
  costs founder time; (3) silent until done — risky.
- **Proceeding with (1).**
