# Evaluate Flowplane without cloning the repo

> Audience: newcomers, api-teams · Status: draft — 3.2.0 release preparation

Follow these short tutorials in order. Installation starts infrastructure with **no APIs exposed**; you explicitly expose the first API. No checkout or Rust toolchain is required for the published-artifact path. Run commands manually and inspect their expected results.

## Learning path

| Step | Tutorial | Outcome |
|---|---|---|
| 1 | [Install and verify Flowplane](eval-install-and-verify.md) | Start the stack, define `fp`, check readiness and empty inventories. |
| 2 | [Expose and call your first API](eval-expose-first-api.md) | Expose the sample backend and verify its response. |
| 3 | [Expose your own backend](eval-expose-own-backend.md) | Attach a host backend to the same listener and verify both routes. |
| 4 | [Add a local rate limit](eval-local-rate-limit.md) | Edit the policy, observe `429`, then verify HTTP 200 and the response body recover. |
| 5 | [Remove APIs safely](eval-remove-apis.md) | Remove shared exposures in both orders and verify survivor/final state. |

Each tutorial states its prerequisites and starting state. Later tutorials continue the same evaluation; they do not reinstall or reset it. Stop on errors or unexpected results. Keep your evaluation terminal open because the `fp` helper is session-local.

## Release and platform boundaries

Matching v3.2.0 container images are not yet published. Use the published-artifact installation only after `v3.2.0` and its matching eval image are published. Older `3.1.x` bundles automatically expose the demo and do not implement the empty-install/shared-exposure journey. Packaged 3.2.0 candidate qualification is pending.

The explicitly labelled local variant has source-bound supporting execution evidence using Docker Compose connected to a Linux-arm64 Podman VM. Docker Desktop/macOS and Linux Docker host-gateway alternatives are unverified in this draft. Runtime and host-networking differences need their own verification; Docker and Podman qualification are not interchangeable.

This bundle is **local-only dev mode**, not a production base: an in-process identity issuer, seeded identities, a management bearer token on disk, PostgreSQL, the demo upstream, Envoy and a dataplane agent. Host ports bind to `127.0.0.1`; xDS uses mTLS. Protect tokens and dashboard nonce URLs when sharing diagnostics. Management authentication does not automatically protect API traffic.

## Task guides and optional continuations

- **Readiness, diagnostics, preserve/resume and deliberate reset:** [Evaluation readiness and recovery](../how-to/evaluation-readiness-and-recovery.md). Test retained exposure/policy recovery before removing configuration; `unexpose` is not a stop/resume operation.
- **Additional ports, exposure modes and conflicts:** [Expose an API](../how-to/expose-an-api.md). The bundle publishes gateway port `10000` by default; automatic allocation does not publish additional container ports.
- **Optional dashboard:** [View your team's gateway dashboard](../how-to/view-team-dashboard.md).
- **Traffic authentication:** [JWT authentication and rate limiting](../how-to/jwt-auth-rate-limit-route.md). Never use the dev management token as a backend credential.
- **Optional API-to-MCP after HTTP success:** [Expose an existing API to MCP](../how-to/expose-an-api-to-mcp.md). Generated tools/status alone do not prove a backend invocation.
- **Production-shaped evaluation:** [Evaluate a production-shaped platform setup](../how-to/evaluate-platform.md).
