# Global rate limiting

> Audience: platform-engineers, operators · Status: stable

This page explains how Flowplane's first-party global rate limiting fits together and why it is
shaped this way — the separate service, the tenant-namespaced counters, the push-based policy sync,
and the deliberate topology limits. It is understanding-oriented; for the step-by-step setup see
[Enable global rate limiting](../how-to/global-rate-limit.md), and for exact fields see the
[`global_rate_limit` filter](../reference/filters.md) and
[configuration](../reference/configuration.md) references.

## The pieces

Global rate limiting spans three processes:

- **Envoy** runs the `global_rate_limit` HTTP filter. On each matching request it builds the
  descriptors its route config declares and calls a rate-limit service (RLS) over gRPC.
- **`flowplane-rls`** is the first-party RLS — a standalone process (not the control plane). It
  answers Envoy's `ShouldRateLimit`, holds the policy set in RAM, and counts hits in an in-memory
  fixed-window store.
- **The control plane** owns the policy data (team-scoped `rate-limit-domains` / `policies` /
  `overrides`), injects the built-in `rate_limit_cluster` into every Envoy's CDS, and pushes the
  policy set to the RLS.

The request hot path never touches the control plane — only Envoy ↔ RLS. The control plane is on
the *config* path only. That separation is deliberate: per-request work must not flow through the
control plane.

## Why a separate process, not the control plane

The RLS sees every rate-limited request. Folding it into the control plane would put request-path
latency and load on the component that also serves xDS and the management API, and would couple the
two deployments. Keeping `flowplane-rls` standalone lets it scale and fail independently, and keeps
the control plane off the hot path.

## Tenancy: the namespaced domain is the trust boundary

Envoy's `ShouldRateLimit` carries a `domain` string and a caller-shaped descriptor list — and
descriptor keys/values can be influenced by the request. **Neither can be trusted as the tenant
boundary.** A header-derived descriptor is caller-controlled; a free-text domain is just a config
string.

So the control plane binds tenancy *by construction*. It owns the `domain` value the filter is
configured with: on the built-in path it composes the emitted Envoy domain as
`{orgUUID}|{teamUUID}|{policyDomain}`, where the org/team UUIDs are deterministic SHA-256-derived
values. The same composition is used when the control plane **pushes** the policy to the RLS, so the
namespace Envoy sends and the namespace the policy lives under agree byte-for-byte. The RLS keys its
counter on `{orgUUID}|{teamUUID}|{policyDomain}` + the canonical descriptor set + the time window.

Two consequences fall out:

- Two teams that both name a policy `checkout` never share a counter — their namespace prefixes
  differ.
- A request whose composed domain resolves to no known policy is simply not counted — it is not
  limited, and it cannot reach another tenant's counter.

This is why you only ever type the short policy domain (`checkout`); the long namespaced value you
see in Envoy's config dump is the control plane's doing. It is also why there are *two* distinct
"domain" notions: the **policy domain** an operator names (1–253 chars, on the row) and the
**composed Envoy filter domain** the control plane emits (which is why the filter's domain length
ceiling is raised to admit it).

## Out-of-the-box wiring, fail-closed when unconfigured

When `FLOWPLANE_RLS_GRPC_URL` is set, the control plane synthesizes the built-in
`rate_limit_cluster` and injects it into CDS, and `global_rate_limit` filters default their
`service_cluster` to it. You don't define that cluster — that's the "works out of the box" claim.

The name `rate_limit_cluster` (and the `rate_limit_` prefix) is **reserved**: you cannot create a
user cluster with it, so the built-in injection never collides with a team's clusters.

The flip side is fail-closed validation at **config time**. Clusters are only validated to exist
when a listener is written, so the reference check lives in the listener service: a
`global_rate_limit` filter must resolve to either the built-in cluster (only when
`FLOWPLANE_RLS_GRPC_URL` is set) or an existing same-team cluster — otherwise the create/update is
rejected (`400` for the unconfigured built-in path, `404` for an unknown/cross-team cluster) before
anything is persisted. Envoy is never handed a filter that points at a cluster that does not exist.

## Policy sync: push + 60 s reconcile

The RLS does not read the product database (that would give a second process tenant-wide DB access
and couple deployments). Instead the control plane **pushes** the full, namespaced policy set to the
RLS admin endpoint — on every change and on a 60 s reconcile timer. In production that channel
is HTTPS with a bearer credential (`FLOWPLANE_RLS_ADMIN_TLS_*` + `FLOWPLANE_RLS_ADMIN_TOKEN` on
the RLS, the matching token on the CP); plaintext HTTP exists only on a loopback bind behind
explicit `yes-this-is-local-only` escape hatches — see
[Enable global rate limiting](../how-to/global-rate-limit.md).

The reconcile is **level-triggered**: each push is the complete set, so a missed change self-heals
within the window — there is no per-event delivery guarantee to get right. A deleted policy
therefore stops being enforced within ≤ 60 s. `FLOWPLANE_RLS_RECONCILE_SECS` can lower that window
(clamped to ≤ 60 s, so it can only make convergence faster, never slower than the documented
backstop); `POST /api/v1/admin/rls/force-repush` (platform-admin) forces an immediate reconcile as a
fast path.

## Failure modes

The RLS adds a gRPC round-trip per rate-limited request (`timeout_ms`, default 20 ms). When the RLS
is slow or unreachable, each filter's `failure_mode_deny` decides: `false` fails **open** (the
request proceeds), `true` fails **closed** (the request is rejected, default `500`). Pick per
listener based on whether availability or the limit matters more for that traffic.

## Deliberate limits (1.1.0 topology)

The counter store is in-memory fixed-window. That keeps the deployment dependency-free — **no Redis
required** — but has stated semantics:

- **Restart resets counters.** A window in progress is lost when the RLS restarts.
- **Horizontal scaling over-admits.** Each RLS instance keeps its own counters, so N instances admit
  up to ~N× the limit.

The committed 1.1.0 topology is therefore a **single** RLS instance per control plane. A
Redis-backed counter store for multi-instance correctness and durable counters is a named follow-on,
not part of 1.1.0. The algorithm is fixed-window (not sliding).

## Further reading

- [Enable global rate limiting](../how-to/global-rate-limit.md) — the hands-on recipe.
- [`global_rate_limit` filter reference](../reference/filters.md) and [rate-limit REST](../reference/rest-api.md#rate-limiting) / [CLI](../reference/cli.md#rate-limit).
- [Tenancy, grants, and the deterministic xDS pipeline](tenancy-grants-xds.md) — the broader isolation model this builds on.
