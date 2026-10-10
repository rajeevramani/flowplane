# Expose and call your first API

> Audience: newcomers, api-teams · Status: draft — 3.2.0 release preparation

**Outcome:** Explicitly expose the supplied backend and verify its response through Envoy.

**Prerequisites and starting state:** Complete [Install and verify Flowplane](eval-install-and-verify.md). Start with authenticated `fp`, an advancing dataplane heartbeat, and empty gateway inventories. Use the same terminal and evaluation directory.

Run commands individually and inspect each result. **Stop on failures or unexpected results; do not continue blindly.** These are manual steps, not an onboarding script.

**Release boundary:** matching v3.2.0 container images are not yet published. This draft is not packaged-candidate or cross-platform qualification. See the [evaluation learning path](evaluate-no-clone.md) for the shared release and platform limitations.

The bundle publishes gateway port `10000`; select it explicitly. Automatic port allocation may choose an unpublished container port.

```sh
fp expose http://demo-upstream:5678 --name demo --path / --port 10000 \
  --public-base-url http://127.0.0.1:10000
```

Expected: a successful exposure result. If it fails, inspect the error and existing resources; do not replay the mutation blindly.

Inspect each created resource:

```sh
fp listener get demo
```

```sh
fp route get demo-routes
```

```sh
fp cluster get demo-upstream
```

Expected: listener `demo` on port `10000`, route config `demo-routes`, and cluster `demo-upstream` targeting the supplied backend. Flowplane sends configuration to Envoy; it does not proxy the API request itself.

```sh
curl --max-time 2 -fsS http://127.0.0.1:10000/
```

Expected body:

```text
hello from the flowplane eval demo upstream
```

If the first call fails, wait a few seconds for xDS delivery and rerun **only the curl command**. Check the actual body, not merely HTTP 200. If it still fails or returns different content, stop and inspect logs/routes instead of rerunning `expose`.

**Two separate security planes:** `fp` uses the management bearer token to configure Flowplane. The sample gateway request does not need that token. Dev-mode OIDC authentication of the CLI does **not** automatically protect exposed traffic. Never send `/shared/dev-token` to your backend. To protect traffic, configure the listener's JWT filter and route requirement with your own issuer, audience, and JWKS; see [JWT authentication and rate limiting](../how-to/jwt-auth-rate-limit-route.md).

## Continue

[Next: Expose your own backend](eval-expose-own-backend.md). For failures, use [evaluation readiness and recovery](../how-to/evaluation-readiness-and-recovery.md); do not retry mutations as readiness checks.
