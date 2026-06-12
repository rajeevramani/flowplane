# Flowplane v2 — Architectural Decisions

Every divergence from v1, every borrowed idea from prior art, every cut or reshape. Format per
entry: **Context** → **Decision** → **Why it's better than the v1 approach** (or why borrowed /
rejected). Decisions made without founder response to a question in `QUESTIONS.md` are marked
**provisional** until approved or vetoed.

---

## D-001: Spec-first rewrite process

- **Context:** v2 is a ground-up rewrite; v1 is read-only reference.
- **Decision:** Extract behavioral specs (Phase 0) before any v2 code; implement only from specs;
  return to v1 source only when a spec is ambiguous, and fix the spec when that happens.
- **Why:** Prevents verbatim porting of v1's coupling problems; makes the contract reviewable by
  the founder before implementation cost is sunk.

## D-002: Every v1 UI workflow gets a CLI/MCP path or a recorded cut

- **Context:** v2 drops the SvelteKit UI. spec/07 §4–5 inventories all 62 UI pages (~38
  workflows): 21 already CLI-covered, 10 partial, 7 with no CLI path at all.
- **Decision:** All 7 zero-coverage workflow families become v2 CLI commands (team/org
  membership + grants, scoped filter configuration, single-route edit + bulk MCP ops, MCP tool
  update/apply-learned, MCP connections, secret update/references, org update + member roles),
  and the 10 partial gaps are closed in their owning slices. The spec/07 §5 fates table is the
  binding inventory; the four visualization-only workflows are proposed cuts (Q-002).
- **Why better than v1:** v1's CLI was a subset of the UI; with no UI, CLI parity is the
  product floor, not a nice-to-have.

## D-003 (approved by founder, Q-002): Cut dashboard rendering, keep the data

- **Context:** Four v1 UI workflows are visualization-only: stats dashboard (30 s polling
  charts), platform-admin KPI dashboard, per-org governance drill-in widgets, profile/password
  page (an IdP deep-link).
- **Decision (provisional):** Keep every underlying data endpoint (`stats *`, `admin
  resources/audit`, new `admin health` for the xDS rollup) with `--json` + `--watch`; cut the
  chart/dashboard rendering — point operators at Prometheus/Grafana for visualization;
  `auth whoami` prints the IdP account-console URL replacing the password page.
- **Why:** A control plane without a UI shouldn't own dashboard rendering; operators already
  run metric stacks. Recorded as removing real (cosmetic) user value → founder veto in Q-002.

## D-004: Environment-agnostic deployment (founder non-negotiable, 2026-06-12)

- **Context:** Founder directive: control plane and data plane must be deployable in any
  environment — bare metal, VMs, plain containers, or Kubernetes — never *designed for*
  Kubernetes.
- **Decision:** No Kubernetes API dependency anywhere: PostgreSQL (not CRDs) is the source of
  truth; identity via OIDC + mTLS certs (not ServiceAccounts); deployment artifacts are a
  static binary, an OCI image, a compose bundle, and systemd guidance — K8s manifests may be
  *offered* as one packaging among equals, never required. Prior-art borrowings (Envoy
  Gateway's IR pipeline, AI Gateway's metering) are adopted as mechanisms, stripped of their
  CRD/controller substrate (spec/09 rejects already aligned with this).
- **Why better than v1:** v1 was already environment-agnostic (compose-first); this locks the
  property in as a reviewed constraint so no v2 design step regresses it.

## D-005: CLI precedence (Q-001 approved): server env-first, CLI flag-first

- **Decision:** Server config: env > config file > defaults. CLI client: explicit flag > env >
  config file > defaults — uniformly for every value (token, team, org, base-url, timeout).
- **Why better than v1:** v1 had three contradictory precedence orders across values; explicit
  flags silently losing to ambient env vars violates least surprise (gh/kubectl convention).
