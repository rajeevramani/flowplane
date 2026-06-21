# Tenancy, grants, and the deterministic xDS pipeline

> Audience: operators, platform-engineers · Status: stable

This page explains the three ideas you need to hold in your head to operate Flowplane confidently: how tenants are isolated, how access is decided, and why the Envoy configuration Flowplane produces is predictable. It is understanding-oriented — it argues *why* the system is shaped this way and links
out for the exhaustive detail. For the authoritative decision tables and threat
model see [spec/08a — Security & Tenancy](../../spec/08a-security-and-tenancy.md)
and [spec/05 — Auth](../../spec/05-auth.md); for the xDS subsystem see
[spec/04 — xDS](../../spec/04-xds.md). Error shapes referenced below are catalogued
in [reference/errors.md](../reference/errors.md).

## 1. Multi-tenancy: every query names whose data it touches

Flowplane is multi-tenant by construction. Tenancy has two levels:
**organizations** own **teams**, and teams own the gateway resources (clusters,
routes, listeners, secrets, and the rest). A team is the unit of isolation — it
is what a dataplane's configuration is built from, and what almost every access
check is scoped to.

The load-bearing idea is that **a tenant query always says which team's data you
mean.** Many repository methods take a team id directly; where a query could span
tenants it instead takes a `TeamScope`, a type with exactly two shapes:
`TeamScope::Team(team_id)`, which pushes the team predicate into the SQL itself,
and `TeamScope::PlatformAdmin { reason }`, an explicit, greppable, audit-carrying
admission that a query deliberately crosses tenants. There is no "just give me the
rows regardless of tenant" variant. This matters because
the classic multi-tenant failure mode is a handler that forgets to add the tenant
filter; here that handler simply cannot be written — an unscoped tenant query is
not representable. The cross-tenant escape hatch carries a human-readable reason
precisely because "because admin" is not a good enough explanation at 2am.

A consequence worth internalising: **cross-tenant existence is hidden.** When a
principal in org A asks about a team that belongs to org B, Flowplane does not
say "you are not allowed to see this" — it behaves as though the team does not
exist. The authorization engine settles the org boundary *before* it ever
consults grants, and the API layer renders that denial as `404 not_found` rather
than `403 forbidden`. The distinction is deliberate anti-enumeration: a
`forbidden` answer confirms the resource is real, which leaks one tenant's
inventory to another. So `not_found` here means "does not exist *within your
visibility*," which is indistinguishable from genuine absence (see the
`not_found` vs `forbidden` rows in [reference/errors.md](../reference/errors.md)).
`forbidden` is reserved for the case where you *can* see the resource but lack the
specific grant.

One refinement to keep in mind: a human can belong to more than one org. When the
caller has a single non-platform membership, Flowplane uses it implicitly — no
selector needed. But when the caller has several memberships and sent no selector,
it fails closed by asking for one (`org_selector_required`) rather than silently
guessing an org or leaking a `not_found`. So the active org is inferred only when
it is unambiguous; as soon as there is a choice to make, the caller must make it
explicitly (via `X-Flowplane-Org` / `--org`).

## 2. Grants and authorization: one pure decision over a grant set

Access in Flowplane is **grant-based**. A grant is a row keyed by the triple
`(resource, action, team)` — for example "read clusters in team T." `Resource`
and `Action` are a small, closed vocabulary: every surface (REST route, MCP tool,
CLI command) declares the `(resource, action)` pair it requires, and the grant
table stores the same pairs, so there is one shared vocabulary and no phantom
permissions that nothing can ever satisfy.

Two things make this tractable to reason about:

**Authorization is a pure function.** A single gate decides every access on every
surface. Its inputs are a snapshot of *who is asking* (the principal context,
loaded once per request from the database) and *what is being attempted* (the
resource, action, and optionally the target team). It performs no IO, reads no
clock, and touches no globals — the answer depends only on its arguments. That is
why the engine can be exhaustively property-tested, and why you can predict a
decision from the principal's grant set without tracing through handler code.
Every decision also returns a *reason* (a stable string like `grant_match`,
`cross_org`, or `platform_governance`) so that audit records can say not just
that access was denied but *why*.

**Resources split into governance and tenant.** A handful of resources —
organizations, users, teams, audit, platform — are *governance* resources: they
describe the platform itself, are never team-scoped, and writing them is reserved
for the platform admin. Everything else is a *tenant* resource, owned by a team.
This split drives the two big invariants you should hold:

- The **platform admin** is a governance role, not a superuser. Platform-admin
  rights apply to governance resources only; a pure platform-admin context is
  *denied* tenant resources outright — it cannot read another tenant's clusters
  by virtue of being admin. Cross-tenant access goes through the explicit
  `TeamScope::PlatformAdmin` hatch described above, not through the authorization
  bypass.
- Inside an org, access is the union of explicit grants and a few sensible
  defaults: an exact grant matches; an org admin gets implicit access to teams in
  *its own* org; an "any-team" grant lets list endpoints through (the rows are
  then filtered down to the caller's teams by `TeamScope`); and org-scoped callers
  can read governance resources but not write them.

**Principals come in two kinds, and they are not symmetric.** A `User` is a human
with org memberships and a grant set. An `Agent` is a machine identity, and agents
are *structurally* constrained before grants are even consulted — the kind of
agent fixes what it can ever touch. A gateway tool agent, for instance, can only
ever reach MCP tools and is denied every other resource by structure; an
API-consumer agent is denied everything at this layer; a control-plane tool agent
is grants-only, with no governance arm and no org-admin shortcut. The point is
that an agent's blast radius is bounded by *what it is*, independent of what
grants happen to exist. The exhaustive decision table lives in
[spec/05 §3.1](../../spec/05-auth.md) and the threat model in
[spec/08a](../../spec/08a-security-and-tenancy.md); this page is only the mental
model.

## 3. The deterministic xDS pipeline: same inputs, same Envoy bytes

Flowplane translates each team's stored gateway resources into the protobuf
Envoy speaks (CDS, RDS, LDS, EDS, SDS) and serves it over an ADS stream. The
property that makes this safe to operate is **determinism**: the same database
state always produces the same configuration bytes.

**Stable encoding.** Resources are sorted by name and assembled through stable,
ordered structures, so translation has no run-to-run variation — no map iteration
order leaking into the wire, no spurious diffs. This is what lets the next two
properties exist.

**Per-type versions that bump only on real change.** Each team's snapshot carries
an independent version *per resource type* (clusters, routes, listeners,
endpoints, secrets). A version is incremented only when that type's *encoded bytes
actually change*. A rebuild that produces identical bytes does not bump anything,
so Envoy is never told to re-apply configuration it already has. The split by type
is what keeps unrelated churn from rippling: endpoint changes for an EDS cluster
bump only the endpoints version, leaving the cluster and route versions —
and therefore Envoy's view of them — untouched. Rebuilds are driven by outbox
events from writes, not by polling, and the serving snapshots are held in memory,
so reconnecting dataplanes are answered from cache rather than re-querying the
database per request.

**NACK quarantine that serves last-good.** Envoy can reject a configuration it
considers invalid (a NACK). Flowplane's response is surgical: it quarantines only
the *specific resources that changed* since the last accepted generation and
falls back to their last-known-good bytes, rather than rolling back or blanking
the whole resource type. A single bad cluster does not take a team's working
listeners offline; the rest of the snapshot keeps serving. The quarantine clears
itself when the offending bytes change again — that is, when an operator pushes a
fix — and quarantined resources are surfaced as "degraded" so the failure is
visible rather than silent. The same wait-for-fix posture applies to resources
that fail translation inside the control plane before Envoy ever sees them: they
are skipped and reported, never allowed to poison the snapshot.

Two operational corollaries follow from this design. First, a control-plane
restart is safe: the cache is primed from the database at startup so a freshly
started process serves a database-built snapshot instead of an empty one — it
never hands a reconnecting dataplane an empty snapshot that would wipe its config.
(The in-memory quarantine/last-good state is not carried across a restart; the
snapshot is simply rebuilt from persisted resources.) Second,
because identical inputs yield identical bytes and versions only move on genuine
change, "nothing happened" is a meaningful, observable state. For the wire-level
contract — stream loops, mTLS and SPIFFE identity, per-dataplane scoping, and NACK
persistence — see [spec/04 — xDS](../../spec/04-xds.md).
