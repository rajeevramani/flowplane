# Import and publish an OpenAPI spec

> Audience: api-teams · Status: stable

**Task:** Import a known OpenAPI document, then publish it so its generated MCP tools are served.

This guide assumes you already have a context configured (`flowplane config set-context` / `use-context`) and a team you can write to. Use this path when you **already have** an OpenAPI document (contrast with [learn-and-publish-api-spec](learn-and-publish-api-spec.md), which *derives* a spec from captured traffic).

## Import does not serve tools by itself

Importing creates the API definition, an **imported** spec version, and the generated API tool rows (`api_tools`) — but those tools are **inert**: they are data, not live config, until you explicitly publish the spec version. This is deliberate (no generated artifact becomes servable without an explicit gate). So importing is a two-step workflow: **import**, then **publish**.

## 1. Import the OpenAPI document

**Endpoint**

```
POST /api/v1/teams/{team}/api-definitions
```

**Body** — `name` plus the `openapi` document. Optionally bind the API to an existing gateway route so the tools are callable later (see step 4):

```json
{
  "name": "catalog",
  "openapi": { "openapi": "3.0.3", "info": { "title": "Catalog", "version": "1" }, "paths": { "/items/{id}": { "get": { "operationId": "getItem" } } } },
  "route_binding": { "route_config_id": "…", "listener_id": "…", "virtual_host": "…", "route": "…" }
}
```

Returns `201` with the API definition, the imported spec `version` (always `1` — there is no re-import path that produces an imported v2), and a `tool_count`. (A later *learning* session attached to this API can still append learned spec versions.)

**CLI**

```bash
flowplane api create catalog --from-openapi openapi.json --team my-team \
  --route-config-id <id> --listener-id <id> --virtual-host <host> --route <route>
```

After this the CLI prints a reminder that the tools are generated **but not served yet**. Confirm: an MCP `tools/list` for the team does **not** include the new `api_catalog-*` tools, and `mcp status`'s `dynamic_enabled_tool_count` does not rise for this API.

## 2. Publish the imported spec version

Publishing flips the API's published pointer to this spec version, regenerates its MCP tools, and makes them servable. Imported specs have no review loop — they publish directly through this gate.

**Endpoint**

```
POST /api/v1/teams/{team}/api-definitions/{name}/specs/{version}/publish
```

`{version}` is `1` for an imported API. A JSON body (`SpecReviewBody`) is required; its only field — `reason` — is optional, so send `{}` or include a reason.

**CLI**

```bash
flowplane api spec publish catalog 1 --team my-team --reason "operator reviewed"
```

Returns `200` with a `PublishSpecView` (`spec` summary + `tool_count`).

> Imported specs cannot be **rejected** — `flowplane api spec reject` is for the learned review loop only and returns *"only learned spec versions can be rejected"*.

## 3. Verify the tools are served

```bash
flowplane api status catalog --team my-team   # api.published_spec_version_id is now set, tool_count > 0
flowplane mcp status --team my-team           # dynamic_enabled_tool_count rises by the published tool count
```

The generated tools now appear in the agent-facing MCP **`tools/list`** response — scoped to the owning team only; they do not appear for other teams. (`mcp status` reports aggregate counts, not individual tool names.) Tool names are derived as `<api>-<operationId>`, lowercased and normalized — so `operationId: "getItem"` on API `catalog` becomes the stored tool `catalog-getitem`, exposed over MCP as `api_catalog-getitem`.

## 4. Call a tool (listener binding required)

Publishing makes a tool **listed** and authorized, but `tools/call` needs one more thing: the API must be bound to a **listener** route. At call time the first route binding that carries a `listener_id` is used to build the upstream descriptor. If the API has no listener binding, the call **fails closed**:

```
api tool "catalog-getitem" has no listener/dataplane route
```

The route binding is set when the API is created, so include the listener binding **at import time** (step 1: `--route-config-id … --listener-id … --virtual-host … --route …`). With a listener binding in place, `tools/call` returns a `gateway_invocation` descriptor (the Envoy-facing URL, headers, body, auth mode, and correlation id) that the caller uses to invoke the upstream **through Envoy** — identical to a published learned tool. The control plane returns the descriptor; it does not proxy the request itself.

To inspect or toggle an individual generated tool:

```bash
flowplane mcp enable  --api api_catalog-getitem --team my-team
flowplane mcp disable --api api_catalog-getitem --team my-team
```

## Further reading

- [learn-and-publish-api-spec](learn-and-publish-api-spec.md) — the same publish gate, but for specs derived from captured traffic.
