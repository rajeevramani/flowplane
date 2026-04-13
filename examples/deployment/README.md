# Flowplane Deployment Examples

Reference deployment templates for running Envoy alongside `flowplane-agent`
in production. The agent is **optional but strongly recommended** — without
it, listener warming failures (a common class of Envoy config rejection that
surfaces only via the admin API) are invisible to the control plane.

| Target | File | When to use |
|---|---|---|
| Kubernetes / Helm | [`helm/envoy-with-flowplane-agent.yaml`](helm/envoy-with-flowplane-agent.yaml) | Most prod deployments. Agent runs as a sidecar in the same pod as Envoy. |
| Docker Compose | [`docker-compose/docker-compose-agent.yml`](docker-compose/docker-compose-agent.yml) | Single-host evaluation, CI, or simple prod-like topologies. |
| systemd / VM | [`systemd/flowplane-agent.service`](systemd/flowplane-agent.service) | VM-based Envoy deployments. |

## What does the agent do?

The agent runs alongside each Envoy instance, polls the local Envoy admin API
for `/config_dump`, walks the dynamic listener / cluster / route config error
states, and streams those reports to the Flowplane control plane over an
authenticated outbound gRPC connection.

This catches a class of failure that stream-level NACKs miss: Envoy ACKs the
xDS DiscoveryResponse inline, then internally tries to warm the listener,
warming fails, the new listener is discarded, and the error is written only
to admin `/config_dump`. Without the agent, the control plane has no idea
this happened — `flowplane xds nacks` returns empty even when Envoy is
actively rejecting every update.

See the `flowplane-ops` skill (section 6 "Diagnostics Agent
(flowplane-agent)") for the full architectural rationale and the
"xDS Delivery Failure" troubleshooting playbook.

## Verifying it is working

After deploying the agent, within ~15 seconds:

```
$ flowplane xds status
NAME              CONNECTED   LAST_CONFIG_VERIFY     STATE
prod-edge-1       yes         2026-04-13 12:34:56    OK
```

A dataplane shown as `NOT MONITORED` (no `last_config_verify` ever recorded)
means no agent has reported for it. The deployment is in degraded mode —
warming failures will not be surfaced.

## Degraded mode (no agent)

Stream-level NACKs still work without the agent. `flowplane xds nacks` will
still capture rejections that surface inline on the xDS stream. What you
lose:

- Listener warming failures (common with OAuth2, JWT auth, ext_authz
  misconfigurations)
- Cluster warming failures (DNS resolution, EDS bootstrap)
- Route config validation failures that surface post-ACK

The control plane logs an INFO once on dataplane registration when no agent
has reported, pointing you at this directory.

## Security model

Read this before deploying.

- **The admin API is privileged.** Envoy admin (`:9901` by default) exposes
  `/quitquitquit`, `/drain_listeners`, `/runtime_modify`, and other
  endpoints that can stop the proxy or alter its runtime behavior. **Never
  expose the admin port across a network.** All examples in this directory
  bind admin to `127.0.0.1` only.
- **The agent is the only admin client.** It reads `/config_dump` over
  loopback. It never calls any other admin endpoint, never POSTs, never
  performs runtime ops.
- **Same network namespace.** Helm uses a single pod (shared loopback);
  Compose uses `network_mode: "service:envoy"`; systemd assumes Envoy on
  the same host. No example crosses a network boundary to reach admin.
- **SPIFFE identity.** The agent authenticates to the control plane with an
  mTLS cert whose SAN encodes its `dataplane_id`. The control plane rejects
  reports for any other dataplane. Reuse whatever cert delivery pipeline
  you already use for Envoy.
- **Sandboxed user.** All examples run the agent as a non-root,
  non-privileged user with read-only filesystem access where the runtime
  supports it.
- **No write path to config.** The diagnostics protocol is one-way: agent →
  CP, error reports only. The agent cannot modify gateway configuration.

If you find yourself wanting to expose `:9901` to make this work, stop —
the architecture is wrong. Use one of the same-namespace patterns above.
