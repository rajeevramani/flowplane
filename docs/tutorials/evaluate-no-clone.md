# Evaluate Flowplane without cloning the repo

> Audience: newcomers, api-teams · Status: stable

This tutorial takes you from a clean machine to a working Flowplane evaluation using only published artifacts. You will start the evaluation bundle, route a request through Envoy, use the `flowplane` CLI inside the published image, import a small OpenAPI document, publish it, and verify that API tools became visible.

You need Docker Compose or Podman Compose and `curl`. The examples use `docker compose`; if you use Podman Compose, replace that command with your local Podman Compose equivalent. You do not need Rust, a source checkout, `./target/debug/flowplane`, `internal/`, or `spec/`.

The evaluation bundle runs dev mode: an in-process identity issuer, seeded `dev-org` / `default` resources, a dev bearer token, Postgres, a demo upstream, and Envoy. It binds host ports to `127.0.0.1` and is not a production shape.

## 1. Start the published evaluator bundle

Use a published release. This example uses `2.1.0`, whose eval compose file and multi-arch eval image are published:

```bash
VER=2.1.0

curl -fsSLO https://raw.githubusercontent.com/rajeevramani/flowplane/v${VER}/compose.eval.yml

FLOWPLANE_EVAL_IMAGE=ghcr.io/rajeevramani/flowplane:${VER}-eval \
  docker compose -f compose.eval.yml up -d --no-build
```

Wait until the services are healthy, then send a request through Envoy:

```bash
curl http://127.0.0.1:10000/
```

Expected body:

```text
hello from the flowplane eval demo upstream
```

That request reached the demo upstream through Envoy on `127.0.0.1:10000`; the control plane did not proxy the request.

## 2. Confirm CLI authentication

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

You should see the resources created by the evaluator bundle. They are the durable gateway resources that produce Envoy config: a cluster, a route config, a listener, and a dataplane record.

## 3. Import a small OpenAPI document

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

## 4. Publish the spec and verify tools

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

## 5. Decide what you learned

At this point you have proven:

- a published Flowplane eval artifact can start without a source checkout;
- a request can route through Envoy;
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
