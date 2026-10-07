# Evaluate Flowplane without cloning the repo

> Audience: newcomers, api-teams · Status: stable

This tutorial takes you from a clean machine to a working Flowplane evaluation using only published artifacts. You will start the evaluation bundle, route a request through Envoy, open the read-only dashboard, use the `flowplane` CLI inside the published image, import a small OpenAPI document, publish it, and verify that API tools became visible.

You need Docker Compose or Podman Compose and `curl`. The examples use `docker compose`; if you use Podman Compose, replace that command with your local Podman Compose equivalent. You do not need Rust, a source checkout, `./target/debug/flowplane`, `internal/`, or `spec/`.

The evaluation bundle runs dev mode: an in-process identity issuer, seeded `dev-org` / `default` identities, a dev bearer token, Postgres, a demo upstream, and Envoy. It installs no exposed APIs or gateway clusters, route configs or listeners. It binds host ports to `127.0.0.1` and is not a production shape.

## 1. Start the published evaluator bundle

This example describes the `3.2.0` empty-install bundle; use it only after `v3.2.0` and its eval image are published. Older `3.1.x` bundles automatically expose the demo and do not have this empty-install behavior.

```bash
VER=3.2.0

curl -fsSLO https://raw.githubusercontent.com/rajeevramani/flowplane/v${VER}/compose.eval.yml

FLOWPLANE_EVAL_IMAGE=ghcr.io/rajeevramani/flowplane:${VER}-eval \
  docker compose -f compose.eval.yml up -d --no-build
```

Wait until the services are healthy. Confirm authenticated control-plane readiness and the automatic dataplane registration independently of gateway traffic:

```bash
docker compose -f compose.eval.yml exec flowplane-eval \
  sh -c 'FLOWPLANE_TOKEN=$(cat /shared/dev-token) flowplane auth whoami'

docker compose -f compose.eval.yml exec flowplane-eval \
  sh -c 'FLOWPLANE_TOKEN=$(cat /shared/dev-token) FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default flowplane dataplane get dp-eval'
```

The dataplane's `last_heartbeat_at` should become non-null. At this point the gateway has no listener, so `curl http://127.0.0.1:10000/` should fail, not return the demo body. Explicitly expose the supplied backend, then call it:

```bash
docker compose -f compose.eval.yml exec flowplane-eval \
  sh -c 'FLOWPLANE_TOKEN=$(cat /shared/dev-token) FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default flowplane expose http://demo-upstream:5678 --name demo --path / --port 10000 --public-base-url http://127.0.0.1:10000'
```

Allow xDS delivery to converge, then send a request through Envoy:

```bash
curl http://127.0.0.1:10000/
```

Expected body:

```text
hello from the flowplane eval demo upstream
```

That request reached the demo upstream through Envoy on `127.0.0.1:10000`; the control plane did not proxy the request.

## 2. Open the dashboard

This step is optional; gateway traffic does not depend on the dashboard.

The bundle also serves a read-only dashboard for the seeded team. Its URL contains a per-launch security nonce, and the `shared` volume is a named volume the host cannot read directly, so read the URL through the container:

```bash
docker compose -f compose.eval.yml exec flowplane-dashboard cat /shared/dashboard-url
```

Open the printed URL (`http://127.0.0.1:8081/<nonce>/`) in your browser. The dashboard opens on Overview and also provides Resources, APIs, Learning, AI, MCP, and Operations screens for the seeded `default` team. Overview lists the eval dataplane `dp-eval` and team totals. If the file does not exist yet, the dashboard is still waiting for the control plane — watch its progress with:

```bash
docker compose -f compose.eval.yml logs flowplane-dashboard
```

The dashboard is published on host loopback only (`127.0.0.1:8081`), and every route requires the nonce path — a request without it is rejected. Each restart of the dashboard container generates a fresh nonce, so re-read the file after a restart. The bundle runs `flowplane-agent` beside Envoy over xDS mTLS, so `dp-eval` should become live and its telemetry should advance after requests. If it remains stale, inspect `docker compose -f compose.eval.yml logs flowplane-agent`.

## 3. Confirm CLI authentication

The eval image includes the `flowplane` CLI. The control-plane container writes a dev token to `/shared/dev-token`; use it only for this local evaluation stack:

```bash
docker compose -f compose.eval.yml exec flowplane-eval \
  sh -c 'FLOWPLANE_TOKEN=$(cat /shared/dev-token) flowplane auth whoami'
```

For the remaining commands, run the CLI inside the eval container and set the seeded org/team context:

```bash
docker compose -f compose.eval.yml exec flowplane-eval \
  sh -c 'FLOWPLANE_TOKEN=$(cat /shared/dev-token) FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default flowplane cluster list'

docker compose -f compose.eval.yml exec flowplane-eval \
  sh -c 'FLOWPLANE_TOKEN=$(cat /shared/dev-token) FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default flowplane listener list'

docker compose -f compose.eval.yml exec flowplane-eval \
  sh -c 'FLOWPLANE_TOKEN=$(cat /shared/dev-token) FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default flowplane route list'
```

You should now see the gateway resources created by your explicit exposure: cluster `demo-upstream`, route config `demo-routes`, and listener `demo`. The bundle registered dataplane `dp-eval` automatically before that exposure. These durable gateway resources produce Envoy config. Before exposure, the three gateway lists are empty. To inspect or create those resources directly, use the [gateway resource request body examples](../reference/rest-api.md#gateway-resource-request-bodies).

## 4. Import a small OpenAPI document

Create the sample document inside the eval container and import it as an API definition:

```bash
docker compose -f compose.eval.yml exec -T flowplane-eval sh <<'EOF'
cat >/tmp/catalog-openapi.json <<'JSON'
{
  "openapi": "3.0.3",
  "info": {
    "title": "Catalog",
    "version": "1.0.0"
  },
  "paths": {
    "/items/{id}": {
      "get": {
        "operationId": "getItem",
        "parameters": [
          {
            "name": "id",
            "in": "path",
            "required": true,
            "schema": { "type": "string" }
          }
        ],
        "responses": {
          "200": {
            "description": "Item found"
          }
        }
      }
    }
  }
}
JSON

FLOWPLANE_TOKEN=$(cat /shared/dev-token) \
FLOWPLANE_ORG=dev-org \
FLOWPLANE_TEAM=default \
flowplane api create catalog --from-openapi /tmp/catalog-openapi.json --team default
EOF
```

Importing creates the API definition, an imported spec version, and generated tool rows. Those generated artifacts are inert until you publish the spec version. In this fresh evaluation stack, the first import creates spec version `1`; `flowplane api status catalog --team default` shows the version state after publish.

## 5. Publish the spec and verify tools

Publish imported spec version `1`:

```bash
docker compose -f compose.eval.yml exec flowplane-eval \
  sh -c 'FLOWPLANE_TOKEN=$(cat /shared/dev-token) FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default flowplane api spec publish catalog 1 --team default --reason "eval import"'
```

Verify the API status:

```bash
docker compose -f compose.eval.yml exec flowplane-eval \
  sh -c 'FLOWPLANE_TOKEN=$(cat /shared/dev-token) FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default flowplane api status catalog --team default'
```

Confirm the output shows a published spec and a non-zero tool count. Then check the MCP tool summary:

```bash
docker compose -f compose.eval.yml exec flowplane-eval \
  sh -c 'FLOWPLANE_TOKEN=$(cat /shared/dev-token) FLOWPLANE_ORG=dev-org FLOWPLANE_TEAM=default flowplane mcp status --team default'
```

The published OpenAPI operation is now represented as a generated API tool for the `default` team. Tool execution requires a listener route binding; importing without one is still useful for evaluating the API lifecycle and tool generation gate. To bind an API to a listener route at import time, see [Import and publish an OpenAPI spec](../how-to/import-and-publish-openapi-spec.md).

## 6. Decide what you learned

At this point you have proven:

- a published Flowplane eval artifact can start without a source checkout;
- installation has no exposed APIs, and a request routes through Envoy after your explicit exposure;
- the read-only dashboard shows the team's dataplane and totals at a nonce-protected loopback URL;
- the CLI can authenticate and inspect gateway resources;
- an OpenAPI document can become an API definition;
- generated API tools remain inert until the spec is published;
- `api status` and `mcp status` show what became served for the team.

For deeper evaluation:

- [Import and publish an OpenAPI spec](../how-to/import-and-publish-openapi-spec.md) covers route bindings and generated tool callability.
- [Learn and publish an API spec version](../how-to/learn-and-publish-api-spec.md) covers learning from captured traffic.
- [Authenticate the CLI and point it at the right server/org/team](../how-to/cli-auth-and-contexts.md) covers non-eval CLI contexts.
- [Register a dataplane and connect its agent over mTLS](../how-to/register-dataplane-mtls.md) shows the production dataplane identity path.

## Tear down

```bash
docker compose -f compose.eval.yml down -v
```
