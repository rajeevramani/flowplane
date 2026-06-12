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
  environment — bare metal, VMs, plain containers, managed container platforms (ECS/Fargate,
  Nomad, Cloud Run and similar), or Kubernetes — never *designed for* Kubernetes or any other
  specific orchestrator (founder clarification 2026-06-12: ECS-class environments are explicitly
  in scope).
- **Decision:** No Kubernetes API dependency anywhere: PostgreSQL (not CRDs) is the source of
  truth; identity via OIDC + mTLS certs (not ServiceAccounts); deployment artifacts are a
  static binary, an OCI image, a compose bundle, and systemd guidance — plus deployment notes
  per environment class (ECS task definitions, K8s manifests) offered as packagings among
  equals, never required. Concretely this forbids: orchestrator-specific service discovery,
  sidecar injection assumptions, orchestrator secret stores as the only secrets path, and any
  health/identity mechanism that doesn't work over plain TCP/HTTP. Prior-art borrowings (Envoy
  Gateway's IR pipeline, AI Gateway's metering) are adopted as mechanisms, stripped of their
  CRD/controller substrate (spec/09 rejects already aligned with this).
- **Why better than v1:** v1 was already environment-agnostic (compose-first); this locks the
  property in as a reviewed constraint so no v2 design step regresses it.

## D-005: CLI precedence (Q-001 approved): server env-first, CLI flag-first

- **Decision:** Server config: env > config file > defaults. CLI client: explicit flag > env >
  config file > defaults — uniformly for every value (token, team, org, base-url, timeout).
- **Why better than v1:** v1 had three contradictory precedence orders across values; explicit
  flags silently losing to ambient env vars violates least surprise (gh/kubectl convention).

## D-006: Discovery listener is a forwarding proxy with mandatory destination constraints

- **Context:** Founder design review of the traffic-first transcript asked what the discovery
  listener listens to and whether routes must exist first.
- **Decision:** No user routes required. `learn discover start` materializes a Flowplane-owned
  Envoy listener whose chain is capture + catch-all forwarding (explicit `--upstream`, or
  host-routed dynamic forward proxy). Forwarding is mandatory (responses must be observed to
  learn schemas/status/auth). Because a host-routed forwarder is an open-proxy/SSRF surface,
  discovery refuses to start without `--upstream` or a destination allowlist; CP, loopback,
  link-local, and cloud-metadata ranges are always denied; sessions have TTL + capture quotas.
- **Why better than v1:** v1 could only black-hole unmatched traffic (request-only, useless
  responses, spec/06 §9); v2 observes full exchanges safely and feeds upstream provenance into
  route generation.

## D-007: Dataplane analytics via agent telemetry relay (not CP admin-scrape)

- **Context:** v1 serves `stats *` by scraping each Envoy's admin API over the network
  (`stats_data_source.rs`) — requires CP→dataplane inbound reachability and off-box admin-port
  exposure; breaks NAT'd/ECS environments (D-004) and is a security liability. The v1 agent
  proto already reserves field ranges for heartbeats (20–29) and telemetry relay (40–49).
- **Decision:** fp-agent scrapes Envoy admin on loopback only and streams a curated metric set
  (request/response counters, response-code classes, latency histograms, connection gauges,
  listener/cluster health) + liveness heartbeats to the CP over the existing outbound mTLS
  diagnostics stream, using the reserved proto ranges. CP aggregates per team → powers
  `flowplane stats`, `dataplane status`, `ops doctor`, and learning-loop usage insights.
  Envoy's native /stats/prometheus stays available for customer monitoring stacks
  (complementary). Envoy admin ports are never exposed off-box in v2.
- **Why better than v1:** outbound-only connectivity works on bare metal/VM/ECS/K8s alike;
  removes an entire attack surface; heartbeats give real liveness (v1 inferred it from xDS
  stream state); founder-raised gap (2026-06-12).

## D-008: Native TLS on the API listener; bearer-only auth (no cookie/CSRF surface)

- **Context:** v1 served REST/MCP on plaintext :8080 assuming LB termination, and carried a
  BFF/cookie/CSRF stack for the SvelteKit UI. D-004 deployments (bare metal, ECS) may have no
  fronting LB; v2 has no UI.
- **Decision:** The v2 API listener supports native TLS (cert/key via config or secrets
  subsystem); plaintext requires explicit opt-in that warns at startup. All auth is bearer
  tokens on every surface — v1's BFF/cookie/CSRF machinery is deleted, not ported (MCP Origin
  allowlist retained). Consolidated security architecture recorded as spec/10 §4a
  (founder-raised, 2026-06-12).
- **Why better than v1:** removes an entire browser-attack surface class; works without an LB
  in any environment; smaller auth codebase with one path to test.
