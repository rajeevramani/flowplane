# Expose an API through Envoy

> Audience: api-teams, platform-engineers · Status: draft — 3.2.0 release preparation

Use `flowplane expose` to create one upstream/route shortcut, either with a new listener or on an existing listener. Flowplane configures Envoy; it does not proxy requests itself. This guide targets 3.2.0. Do not use its shared-listener commands with an older CLI/server.

**Prerequisites:** an authenticated CLI with server/org/team context, a connected same-team dataplane, an upstream reachable from Envoy, and a published/reachable gateway port. The [no-clone evaluation tutorial](../tutorials/evaluate-no-clone.md) supplies those prerequisites and a short `fp()` helper. Here `flowplane` means that configured CLI; run `fp` in your host shell to execute that CLI inside the eval container.

## Create a new listener

```sh
flowplane expose http://backend:8080 --name orders --path /orders --port 10000 \
  --public-base-url http://127.0.0.1:10000
```

Replace the upstream and public base URL with reachable addresses in your deployment. `--public-base-url` describes the URL callers should use; it does not publish a container port, add DNS, or establish TLS. The evaluation bundle publishes only gateway port `10000` by default. Without `--port`, the shortcut allocates an available listener port, which may not be published by your deployment.

The independent mode atomically creates cluster `orders-upstream`, route config `orders-routes`, listener `orders`, and its internal shortcut association. The result reports `mode: created`. A failed operation must not leave a partially created trio. Management grants are required to Create all three resource kinds; automatic port allocation also requires Listeners/Read to inspect occupied ports.

Wait for xDS delivery, then call the returned reachable gateway URL. `/orders` is a literal Prefix and is forwarded unchanged: the backend must implement `/orders`; the shortcut does not strip it or infer a rewrite from the upstream URL. Prefix `/orders` also matches `/orders-extra`.

## Attach another API to the existing listener

```sh
flowplane expose http://catalog:8080 --name catalog --path /catalog --listener orders
```

The result reports `mode: attached` and the **actual** listener/config names, not fabricated `catalog` listener names. No second listener or gateway port is allocated. Do not combine `--listener` with `--port` or `--public-base-url`. Inspect the listener and route config before choosing a path:

```sh
flowplane listener get orders
flowplane route get orders-routes
```

Supported target: same-team user-owned HTTP listener, one virtual host with domains exactly `["*"]`, and routes with understandable Prefix/Exact matchers and no header/query matchers. Unsupported shapes, duplicate paths, and conflicting bindings/captures reject attachment rather than guess. A narrower Prefix is inserted before the first broader Prefix; existing routes retain their relative order. An earlier Exact rule can still win for its exact path.

**Fanout:** a route-config change affects every listener sharing the named route config. Table output names this consequence; JSON/YAML preserve the server response and do not add a separate fanout field.

Shared mode requires Clusters/Create, Listeners/Read, and RouteConfigs/Read+Update. It consumes cluster quota, not another listener/config quota. These grants are not automatically inherited from whoever originally created the listener.

## Publish an additional evaluation port

Prefer `--listener demo` to share the existing port. If you deliberately need an independent listener, save this additional file as `compose.extra-port.yml` beside `compose.eval.yml`:

```yaml
services:
  envoy:
    ports:
      - "127.0.0.1:10001:10001"
```

Compose merges this port mapping with the original `10000` mapping. Remove the agent container first because it shares Envoy's network namespace; otherwise replacing Envoy can fail on a dependent-container guard. Then recreate Envoy and the agent with the extra mapping and explicitly select the matching **container** listener port. This can briefly interrupt existing evaluation traffic; the database and shared volumes are retained:

```sh
(
set -eu
docker compose -f compose.eval.yml rm -sf flowplane-agent
docker compose -f compose.eval.yml -f compose.extra-port.yml up -d --no-build envoy flowplane-agent
fp expose http://demo-upstream:5678 --name extra --path / --port 10001 \
  --public-base-url http://127.0.0.1:10001
extra_ready=false
for attempt in $(seq 1 30); do
  body=$(curl --max-time 2 -fsS http://127.0.0.1:10001/ 2>/dev/null) || body=
  if [ "$body" = 'hello from the flowplane eval demo upstream' ]; then
    extra_ready=true
    break
  fi
  sleep 1
done
[ "$extra_ready" = true ] || { printf '%s\n' 'Additional-port traffic did not converge; inspect port mapping, Envoy and agent logs.' >&2; exit 1; }
printf '%s\n' "$body"
)
```

Use a free host port and wait for xDS convergence before expecting the body. Changing only the host-port side of a mapping does not change the listener's container port. Keep the override file in subsequent full-stack Compose invocations while using this additional mapping. Remove only this exposure with `fp --yes unexpose extra` when finished.

## Choose an upstream Envoy can reach

- A backend on the Compose network uses service DNS and its container port, such as `http://orders:8080`.
- A host backend on Docker Desktop/macOS can use `http://host.docker.internal:3001` if it accepts connections from Docker.
- Linux Docker may need an explicit `host.docker.internal:host-gateway` mapping on the **Envoy** service; see the complete override in the tutorial.
- Podman's `host.containers.internal` is runtime-dependent; verify it rather than assuming Docker instructions qualify Podman.
- Host `127.0.0.1` is not the host from inside an Envoy container.

The management bearer token authenticates CLI/REST changes, not traffic. Do not forward the dev token to your API. Install a traffic JWT filter and requirement when needed; see [JWT auth and rate limiting](jwt-auth-rate-limit-route.md). A local rate-limit filter is per-Envoy; see [global rate limiting](global-rate-limit.md) for shared fleet quotas.

## Remove the exposure without deleting somebody else's infrastructure

Inspect the removal result, not just its exit status:

```sh
flowplane unexpose catalog
flowplane unexpose orders
```

On a non-interactive terminal, add the global `--yes` flag deliberately, for example `flowplane --yes unexpose catalog`. The evaluation helper runs non-interactively.

| Situation | Successful shortcut removal |
|---|---|
| Other genuine routes/exposures remain | Remove this exposure's route and upstream; retain the shared listener/config. Verify surviving traffic. |
| Last exposure on shortcut-managed scaffold, no blocking references | Delete the upstream and final managed listener/config. Later policy edits on that scaffold are also deleted. |
| Listener/config were manually created or legacy | Never infer cleanup ownership from matching names. Keep borrowed infrastructure; removal that would leave an invalid last-route config fails atomically. Add a genuine replacement route through ordinary revision-checked authoring before retrying that removal. |
| Live API bindings, capture history, or other blocking references | Reject unsafe removal; resolve dependencies through supported resource lifecycles. Stopping a capture does not remove its historical foreign-key references. |

Either order works for the managed pair: remove the original before its attachment, or the attachment before the original. Cleanup provenance remains tied to the exact listener/config identities, so the final attached exposure can clean up the original managed scaffold when grants and reference guards permit it. Another manual listener sharing the config is a different borrowed pair, not automatic adoption.

The response reports `cluster_disposition`, `route_config_disposition`, and `listener_disposition` as `deleted` or `retained`. Cleanup still requires the corresponding Delete grants. Dataplane registration and infrastructure processes are not deleted by `unexpose`.

## Conflicts, retries, and upgrade boundaries

- For a stale shortcut route/binding, restore its exact route name, direct upstream and listener/config binding through ordinary revision-checked updates before retrying `unexpose`. Matching names or a newly created resource with another identity do not restore ownership.
- Inspect a 409 before acting. Re-read current resources/revisions for a concurrency conflict; an unsupported listener shape or unresolved dependency is not repaired by blind retry.
- The CLI does not automatically replay a failed mutation. Read-only traffic convergence checks may retry within a bounded deadline.
- An explicit port is never silently substituted. Auto-allocation retries only after the entire failed attempt rolls back.
- Pre-3.2 matching-name trios have no shortcut association. Upgrade does not adopt, wipe, or backfill them; `unexpose <legacy-name>` returns 404 with manual-cleanup guidance. Inspect dependencies and use ordinary revision-checked resource commands instead.

For the full request/output fields, use the [CLI reference](../reference/cli.md#expose) and [REST reference](../reference/rest-api.md). For the complete sample/own-backend/policy/removal sequence, follow the [no-clone tutorial](../tutorials/evaluate-no-clone.md).
