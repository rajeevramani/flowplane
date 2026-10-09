# Flowplane documentation

> Audience: end users (operators, platform engineers, API teams) · Status: stable

This directory is **user-facing product documentation only**. Everything here is written for people *using* Flowplane, and every page **stands alone** — you never need to read `spec/` or `internal/` to follow a doc here.

For engineering design records, decisions, progress, and release evidence, see [`../internal/README.md`](../internal/README.md) and `../spec/`.

## Guides and references

Choose a task below, or follow the [evaluation learning path](tutorials/evaluate-no-clone.md) from installation to safe removal.

| You want to… | Start here |
|--------------|------------|
| Try Flowplane locally | [Evaluation learning path](tutorials/evaluate-no-clone.md) |
| Expose your API or share a listener | [Expose an API](how-to/expose-an-api.md) |
| Evaluate a production-shaped platform setup | [Evaluate a production-shaped platform setup](how-to/evaluate-platform.md) |
| Delegate API onboarding to a team | [Onboard an API team](how-to/onboard-api-team.md) |
| Build and run from source | [Build and run from source](tutorials/build-and-run-from-source.md) |
| Protect a route with JWT auth + rate limit | [JWT auth & rate limit](how-to/jwt-auth-rate-limit-route.md) |
| Cap a route globally across all Envoys | [Enable global rate limiting](how-to/global-rate-limit.md) |
| Learn an API spec from live traffic | [Learn & publish an API spec](how-to/learn-and-publish-api-spec.md) |
| Front an LLM with a token budget | [AI gateway route & budget](how-to/ai-gateway-route-budget.md) |
| Inspect a team's gateway in the dashboard | [View your team's gateway dashboard](how-to/view-team-dashboard.md) |
| Secure the data plane with mTLS | [Register a dataplane (mTLS)](how-to/register-dataplane-mtls.md) |
| Understand tenancy, grants, and xDS | [Tenancy, grants & the xDS pipeline](concepts/tenancy-grants-xds.md) |
| Understand global rate limiting | [Global rate limiting](concepts/global-rate-limiting.md) |

Reference: [CLI](reference/cli.md) · [Configuration](reference/configuration.md) · [REST API](reference/rest-api.md) · [Filters](reference/filters.md) · [Errors](reference/errors.md) · [Adoption issue map](reference/adoption-evaluation-issue-map.md)

Contributor build and verification commands live in the [contributor guide](../CONTRIBUTING.md).

## Structure (Diátaxis)

The primary axis is **Diátaxis mode**, not audience. Audience and status are per-document metadata (a header banner), not directories.

| Directory | Mode | What it holds |
|-----------|------|---------------|
| `tutorials/` | tutorial | Learning-oriented. One guided path to a first success. |
| `how-to/`    | how-to   | Task-oriented. One concrete problem solved for someone who knows the basics. |
| `reference/` | reference | Information-oriented. Dry, exhaustive: config/env vars, CLI, API, filters, errors. |
| `concepts/`  | explanation | Understanding-oriented. Why things fit together. (Diátaxis "explanation"; we name the dir `concepts/`.) |

## Cardinal rule

**User docs must stand alone.** The test is *completability*: a reader must be able to finish a tutorial, how-to, or reference page — every step, every required value — **without opening `spec/` or `internal/`**. A page may be *derived* from `spec/`, but the spec must never be required reading to succeed.

What this allows and forbids:

- ✅ Internal docs may link *into* `docs/`.
- ✅ **Optional** "Further reading" / design-reference links into `spec/` are fine, as long as they are clearly marked optional and the task is complete without them. Put them under a `## Further reading` (or "Design references") heading, or label them inline as optional background.
- ✅ `concepts/` (explanation) pages may cite `spec/` freely — by design they are a bridge into the design records (see #112). They still must not pull in `internal/`.
- ❌ No page may make a `spec/` or `internal/` link **required reading** — i.e. a step the reader must follow to complete the task.
- ❌ No page may depend on an `internal/` artifact in a task step (e.g. `source internal/.env...`); inline a self-contained example instead.

### Enforcement (CI)

A CI check lists every `docs/**/*.md` link into `../internal/` or `../spec/` and fails on the ones that are **not** allowed. Allowed:

1. `docs/README.md` (this index) and links to a real bucket index such as `../internal/README.md`.
2. Any link from a `docs/concepts/` page into `../spec/` (explanation bridge).
3. Links into `../spec/` that sit under a `## Further reading` / "Design references" heading (optional background).

Everything else — required-reading spec links in task steps, and **any** link into `../internal/` from a task page — fails the check.

Tracked in #116, #118.

## Per-document banner

Every page starts with one metadata line:

```md
> Audience: operators · Status: stable
```

`Audience` and `Status` values are open-ended conventions, not closed vocabularies. Common examples
are `Audience: operators` / `platform-engineers` / `api-teams` / `newcomers` and
`Status: stable` / `draft`; use other clear values when they describe the page better.

## First exposure

Start with the [evaluation learning path](tutorials/evaluate-no-clone.md). Its focused tutorials cover [installation and verification](tutorials/eval-install-and-verify.md), [your first API](tutorials/eval-expose-first-api.md), [your own backend](tutorials/eval-expose-own-backend.md), [local rate limiting](tutorials/eval-local-rate-limit.md), and [safe removal](tutorials/eval-remove-apis.md). They target the draft 3.2.0 journey and label the platform paths actually exercised. Use [Expose an API](how-to/expose-an-api.md) for exposure modes, additional published ports and conflicts. For startup diagnostics, preserve/resume and deliberate reset, use [evaluation readiness and recovery](how-to/evaluation-readiness-and-recovery.md); supporting local execution is not immutable-release or cross-platform qualification.

For an optional continuation after HTTP success, see [Expose an existing API to MCP](how-to/expose-an-api-to-mcp.md) (draft; runtime qualification pending). It distinguishes an MCP invocation descriptor from a separate backend call.

## Source-of-truth policy

- **Implementation truth** → code + tests.
- **User/operator truth** → the single canonical page for that task (e.g. one bootstrap how-to, one configuration reference). Do not restate it elsewhere.
- **Deployment examples** (AWS, Fly.io, later k8s/systemd) → platform-specific *delivery* only; they **link** to the canonical how-to/reference instead of duplicating it. Current provider pages: [AWS secure deployment](how-to/aws-secure-deployment.md) and [Fly.io with Tailscale dataplanes](how-to/fly-secure-deployment.md) (draft).
- **Design rationale** → `spec/` + issues (linked, not inlined).

When behavior changes: update the one canonical user page plus any deployment example whose exact commands would otherwise be wrong. Do not sprinkle the change across every file.

## Migration status

The Diátaxis directories are populated by epic #100 (sub-issues #101–#112). The existing operator docs have been reclassified (#116):

| Old path | New location | Class |
|----------|--------------|-------|
| `docs/aws-secure-deployment.md` | `docs/how-to/aws-secure-deployment.md` | user |
| `docs/production-readiness.md` | `docs/how-to/production-readiness.md` | user |
| `docs/secret-kek-rotation.md` | `docs/how-to/secret-kek-rotation.md` | user |
| `docs/dev-dataplane.md` | `../internal/dev-dataplane.md` | internal (dev workflow) |
| `docs/release-packaging.md` | `../internal/release/release-packaging.md` | internal |

The boundary is enforced in CI by `scripts/ci/check-docs-boundary.py` (see [Enforcement (CI)](#enforcement-ci)).
