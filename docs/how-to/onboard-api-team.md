# How-to: onboard an API team

> Audience: api-teams, platform-engineers · Status: stable

Use this guide when a platform team has already deployed Flowplane and wants an API team to self-serve one API without platform-admin credentials. The path starts from the handoff values the platform team provides and ends with a published API spec and verified MCP tools.

This page is the handoff spine. It links to the canonical task pages for CLI auth, OpenAPI import, learning, gateway resource bodies, and dataplane mTLS instead of duplicating them.

## Prerequisites

You need these values from the platform team:

- control-plane URL, for example `https://cp.example`;
- tenant org name, for example `edgeco`;
- team name, for example `payments`;
- OIDC CLI login values: issuer URL, CLI client id, scopes, and whether device-code or browser login is enabled;
- confirmation that your identity has been added to the org and team;
- grants for the resources you need in this team.

For a simple API onboarding evaluation, ask for these team grants:

```bash
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource api-definitions --action create
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource api-definitions --action read
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource api-definitions --action update
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource mcp-tools --action read
flowplane team grant add api-dev@example.com --org edgeco --team payments --resource mcp-tools --action update
```

If the API team will create gateway resources or route-generation plans, also ask for the relevant `clusters`, `route-configs`, and `listeners` `create/read/update` grants. If they will learn from traffic, ask for `learning-sessions` `create/read/execute/delete` as needed. If they only publish against a platform-provided listener binding, they do not need broad dataplane or platform-admin rights.

Flowplane deliberately separates these roles:

- platform teams own the control plane, OIDC, bootstrap, tenant creation, dataplane registration policy, certificate policy, and production xDS paths;
- tenant org owners/admins own org membership, team membership, and team grants;
- API teams own API import or learning, publish gates, and tool verification inside their team scope.

Platform-admin status is not a tenant bypass. The platform org cannot host tenant teams, dataplanes, or gateway resources.

## 1. Log in and select the tenant team

Use the login method your platform team enabled. Device-code login is common for CLI-only evaluation:

```bash
flowplane auth login --device-code \
  --issuer https://issuer.example \
  --client-id flowplane-cli
```

Create a context that includes the control-plane URL, tenant org, and team:

```bash
flowplane config set-context edgeco-payments \
  --server https://cp.example \
  --org edgeco \
  --team payments

flowplane config use-context edgeco-payments
flowplane auth whoami
```

`auth whoami` should show the authenticated user, the active tenant org, and the grants available to this identity. If it reports that an org selector is required, keep `--org edgeco` in the context or pass it on commands.

For the full auth and context behavior, see [CLI auth and contexts](cli-auth-and-contexts.md).

## 2. Verify team access

Confirm that the tenant org and team are visible:

```bash
flowplane team list --org edgeco
flowplane team member list --team payments
flowplane team grant list --team payments
```

Then check whether gateway resources already exist:

```bash
flowplane cluster list --team payments
flowplane route list --team payments
flowplane listener list --team payments
```

If these commands return `403`, use the error's `(resource, action)` hint to request the smallest missing grant. If the team is not found, confirm the org/team names and that your identity is a member of the tenant org.

## 3. Choose the API onboarding path

Use one of these paths:

- If you already have an OpenAPI document, follow [Import and publish an OpenAPI spec](import-and-publish-openapi-spec.md).
- If you need Flowplane to infer the spec from traffic, follow [Learn and publish an API spec version](learn-and-publish-api-spec.md).

If tools must be callable, the API needs a listener binding. That can be a platform-provided route/listener pair, resources created by the API team, or a route generated from the API spec. The public gateway body examples are in [REST API reference: gateway resource request bodies](../reference/rest-api.md#gateway-resource-request-bodies).

Publishing an API spec makes MCP tools listed and authorized. Actual upstream calls still go through Envoy; the control plane returns invocation descriptors and does not proxy request traffic.

## 4. Publish and verify tools

After import or learning, publish the reviewed spec version:

```bash
flowplane api spec publish catalog 1 --team payments --reason "API team evaluation"
```

Verify what became callable:

```bash
flowplane api status catalog --team payments
flowplane mcp status --team payments
```

Success looks like:

- the API status shows a published spec version;
- `tool_count` is greater than zero;
- `mcp status` shows the team's dynamic enabled tool count increased;
- tool calls have a listener/dataplane route if they need to invoke the upstream through Envoy.

If publishing succeeds but a tool call fails with no listener/dataplane route, ask the platform team for the listener binding details or the grants needed to create/bind a listener.

## 5. Verify the dataplane path when needed

For a production-shaped evaluation, the platform team should already have registered and connected a dataplane over mTLS. Ask for the listener address clients can reach and the `stats:read` grant if you are expected to run team-scoped diagnostics:

```bash
flowplane stats overview --team payments
flowplane ops xds status --team payments
flowplane ops xds nacks --team payments
```

Those checks prove the control plane is serving xDS state and Envoy is accepting or rejecting it. The canonical dataplane setup task is [Register a dataplane and connect its agent over mTLS](register-dataplane-mtls.md).

## Handoff back to platform

Send the platform team these results:

- the API name and published spec version;
- the `api status` and `mcp status` output;
- any missing `(resource, action)` grant from `403` errors;
- whether a listener binding exists and which dataplane/listener address was used for smoke traffic.

Do not share bearer tokens, provider API keys, bootstrap tokens, private keys, or certificate PEM bodies in the handoff.
